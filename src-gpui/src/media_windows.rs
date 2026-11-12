use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use libwebrtc::{
    MediaType,
    audio_frame::AudioFrame,
    audio_source::{AudioSourceOptions, native::NativeAudioSource},
    audio_stream::native::NativeAudioStream,
    desktop_capturer::{
        CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions,
    },
    ice_candidate::IceCandidate,
    media_stream_track::MediaStreamTrack,
    native::{apm::AudioProcessingModule, yuv_helper},
    peer_connection::{
        AnswerOptions, OfferOptions, PeerConnection, PeerConnectionState, TrackEvent,
    },
    peer_connection_factory::{
        IceServer, PeerConnectionFactory, RtcConfiguration, native::PeerConnectionFactoryExt,
    },
    rtp_parameters::{DegradationPreference, RtpEncodingParameters},
    rtp_sender::RtpSender,
    rtp_transceiver::{RtpTransceiverDirection, RtpTransceiverInit},
    session_description::{SdpType, SessionDescription},
    video_frame::{I420Buffer, VideoBuffer, VideoFrame, VideoRotation},
    video_source::{VideoResolution, native::NativeVideoSource},
    video_stream::native::NativeVideoStream,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{CaptureChoice, VideoImage};
use crate::{
    audio::{FRAME, MAX_BUFFER, Mixer},
    denoise::Denoiser,
    device::{Devices, Loopback},
    protocol::{Peer, ServerConfig, envelope},
    settings::{NoiseEngine, ScreenSettings, Settings},
};

enum Event {
    Signal(String),
    Track(String, TrackEvent),
    State(String, PeerConnectionState),
    Audio(String, Vec<f32>),
    CaptureEnded,
}
struct Connection {
    pc: PeerConnection,
    remote_ready: bool,
    candidates: Vec<Value>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.pc.close();
    }
}
struct Publishing {
    connection: Connection,
    sender: RtpSender,
    capture_stop: Arc<AtomicBool>,
    _capture: std::thread::JoinHandle<()>,
    loopback: Option<Loopback>,
    audio: Option<NativeAudioSource>,
    settings: Arc<Mutex<ScreenSettings>>,
}
impl Drop for Publishing {
    fn drop(&mut self) {
        self.capture_stop.store(true, Ordering::Relaxed);
    }
}

pub struct Media {
    factory: PeerConnectionFactory,
    config: RtcConfiguration,
    audio: Connection,
    mic: NativeAudioSource,
    devices: Devices,
    muted: Arc<AtomicBool>,
    deafened: Arc<AtomicBool>,
    apm: AudioProcessingModule,
    denoiser: Denoiser,
    settings: Settings,
    peers: BTreeMap<String, Peer>,
    slots: BTreeMap<String, Option<String>>,
    mixer: Mixer,
    tx: mpsc::Sender<Event>,
    events: mpsc::Receiver<Event>,
    frames: mpsc::Sender<VideoImage>,
    tasks: BTreeMap<String, Vec<tokio::task::JoinHandle<()>>>,
    publisher: Option<Publishing>,
    subscribers: BTreeMap<String, Connection>,
    pub resume_token: Option<String>,
    audio_failed: bool,
    last_devices: Instant,
}
impl Drop for Media {
    fn drop(&mut self) {
        self.muted.store(true, Ordering::Relaxed);
        for tasks in self.tasks.values() {
            for task in tasks {
                task.abort();
            }
        }
    }
}

impl Media {
    pub fn take_audio_failed(&mut self) -> bool {
        std::mem::take(&mut self.audio_failed)
    }
    pub fn voice_connected(&self) -> bool {
        self.audio.pc.connection_state() == PeerConnectionState::Connected
    }
    pub fn screen_needs_ice_restart(&self) -> bool {
        self.publisher
            .as_ref()
            .is_some_and(|p| p.connection.pc.connection_state() != PeerConnectionState::Connected)
    }
    pub fn reset_audio(&mut self, config: &ServerConfig) -> Result<(), String> {
        self.config.ice_servers = config
            .ice_servers
            .iter()
            .map(|s| IceServer {
                urls: s.urls.clone(),
                username: s.username.clone(),
                password: s.credential.clone(),
            })
            .collect();
        let audio = connection(&self.factory, &self.config, "audio", &self.tx)?;
        audio
            .pc
            .add_track(
                self.factory
                    .create_audio_track("microphone", self.mic.clone())
                    .into(),
                &["microphone"],
            )
            .map_err(|_| "Не удалось восстановить микрофон")?;
        self.audio = audio;
        self.audio_failed = false;
        if let Some(tasks) = self.tasks.remove("audio") {
            for task in tasks {
                task.abort();
            }
        }
        self.slots.clear();
        self.mixer.clear();
        self.subscribers.clear();
        let keys = self
            .tasks
            .keys()
            .filter(|key| key.starts_with("screen-sub:"))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(tasks) = self.tasks.remove(&key) {
                for task in tasks {
                    task.abort();
                }
            }
        }
        Ok(())
    }
    pub fn new(
        config: &ServerConfig,
        settings: Settings,
        frames: mpsc::Sender<VideoImage>,
    ) -> Result<Self, String> {
        let factory = PeerConnectionFactory::default();
        let mut rtc = RtcConfiguration::default();
        rtc.ice_servers = config
            .ice_servers
            .iter()
            .map(|s| IceServer {
                urls: s.urls.clone(),
                username: s.username.clone(),
                password: s.credential.clone(),
            })
            .collect();
        let (tx, events) = mpsc::channel(64);
        let muted = Arc::new(AtomicBool::new(settings.muted));
        let deafened = Arc::new(AtomicBool::new(settings.deafened));
        let devices = Devices::open_selected(
            muted.clone(),
            deafened.clone(),
            &settings.microphone,
            &settings.speaker,
        )?;
        let audio = connection(&factory, &rtc, "audio", &tx)?;
        // APM is applied before the selected denoiser and send gain, once only.
        let mic = NativeAudioSource::new(AudioSourceOptions::default(), 48000, 1, 0);
        audio
            .pc
            .add_track(
                factory.create_audio_track("microphone", mic.clone()).into(),
                &["microphone"],
            )
            .map_err(|_| "Не удалось добавить микрофон")?;
        let denoiser = Denoiser::new(settings.noise)?;
        let apm =
            AudioProcessingModule::new(true, true, true, settings.noise == NoiseEngine::WebRtc);
        Ok(Self {
            factory,
            config: rtc,
            audio,
            mic,
            devices,
            muted,
            deafened,
            apm,
            denoiser,
            settings,
            peers: BTreeMap::new(),
            slots: BTreeMap::new(),
            mixer: Mixer::default(),
            tx,
            events,
            frames,
            tasks: BTreeMap::new(),
            publisher: None,
            subscribers: BTreeMap::new(),
            resume_token: None,
            audio_failed: false,
            last_devices: Instant::now(),
        })
    }
    pub fn captures() -> Vec<CaptureChoice> {
        let mut choices = Vec::new();
        for (kind, window) in [
            (DesktopCaptureSourceType::Screen, false),
            (DesktopCaptureSourceType::Window, true),
        ] {
            if let Some(capturer) = DesktopCapturer::new(DesktopCapturerOptions::new(kind)) {
                choices.extend(capturer.get_source_list().iter().map(|s| CaptureChoice {
                    id: s.id(),
                    title: s.title(),
                    window,
                }));
            }
        }
        choices
    }
    pub fn set_peers(&mut self, peers: Vec<Peer>) {
        self.peers = peers.into_iter().map(|p| (p.id.clone(), p)).collect();
    }
    pub fn configure(&mut self, settings: Settings) -> Result<(), String> {
        self.muted
            .store(settings.muted || settings.deafened, Ordering::Relaxed);
        self.deafened.store(settings.deafened, Ordering::Relaxed);
        if settings.microphone != self.settings.microphone
            || settings.speaker != self.settings.speaker
        {
            self.devices = Devices::open_selected(
                self.muted.clone(),
                self.deafened.clone(),
                &settings.microphone,
                &settings.speaker,
            )?;
        }
        if settings.noise != self.settings.noise {
            self.denoiser = Denoiser::new(settings.noise)?;
            self.apm =
                AudioProcessingModule::new(true, true, true, settings.noise == NoiseEngine::WebRtc);
        }
        if let Some(publisher) = &mut self.publisher {
            *publisher
                .settings
                .lock()
                .map_err(|_| "Ошибка настроек захвата")? = settings.screen.clone();
            set_video_parameters(&publisher.sender, &settings.screen, true)?;
        }
        self.settings = settings;
        Ok(())
    }
    pub fn is_publishing(&self) -> bool {
        self.publisher.is_some()
    }
    pub async fn signal(&mut self, event: &str, data: &Value) -> Result<Vec<String>, String> {
        match event {
            "audio-slots" => {
                if let Some(slots) = data["slots"].as_array() {
                    for slot in slots {
                        if let Some(id) = slot["slot"].as_str() {
                            self.mixer.remove(id);
                            self.slots
                                .insert(id.into(), slot["peerId"].as_str().map(str::to_owned));
                        }
                    }
                }
                Ok(vec![])
            }
            "offer" => {
                let key = pc_key(data)?;
                if key == "screen-pub" {
                    return Err("Неожиданное предложение screen-pub".into());
                }
                let conn = if key == "audio" {
                    &mut self.audio
                } else {
                    self.subscribers
                        .get_mut(&key)
                        .ok_or("Подписка уже закрыта")?
                };
                let sdp = data["sdp"].as_str().ok_or("Нет SDP")?;
                conn.pc
                    .set_remote_description(
                        SessionDescription::parse(sdp, SdpType::Offer)
                            .map_err(|_| "Некорректный SDP сервера")?,
                    )
                    .await
                    .map_err(|_| "Не удалось согласовать медиапоток")?;
                conn.remote_ready = true;
                flush_candidates(conn).await;
                let answer = conn
                    .pc
                    .create_answer(AnswerOptions::default())
                    .await
                    .map_err(|_| "Не удалось создать ответ WebRTC")?;
                let sdp = answer.to_string();
                conn.pc
                    .set_local_description(answer)
                    .await
                    .map_err(|_| "Не удалось применить ответ WebRTC")?;
                Ok(vec![envelope(
                    "answer",
                    description_data(data, "answer", sdp),
                )])
            }
            "answer" => {
                if data["pc"] == "screen-pub"
                    && let Some(p) = &mut self.publisher
                {
                    let sdp = data["sdp"].as_str().ok_or("Нет SDP")?;
                    p.connection
                        .pc
                        .set_remote_description(
                            SessionDescription::parse(sdp, SdpType::Answer)
                                .map_err(|_| "Некорректный SDP")?,
                        )
                        .await
                        .map_err(|_| "Не удалось подключить демонстрацию")?;
                    p.connection.remote_ready = true;
                    flush_candidates(&mut p.connection).await;
                    set_video_parameters(&p.sender, &self.settings.screen, true)?;
                }
                Ok(vec![])
            }
            "candidate" => {
                let key = pc_key(data)?;
                let conn = if key == "audio" {
                    Some(&mut self.audio)
                } else if key == "screen-pub" {
                    self.publisher.as_mut().map(|p| &mut p.connection)
                } else {
                    self.subscribers.get_mut(&key)
                };
                if let Some(conn) = conn {
                    if conn.remote_ready {
                        add_candidate(&conn.pc, data).await;
                    } else if conn.candidates.len() < 256 {
                        conn.candidates.push(data.clone());
                    }
                }
                Ok(vec![])
            }
            "screen-share-started" => {
                self.resume_token = data["sessionToken"].as_str().map(str::to_owned);
                Ok(vec![])
            }
            "screen-share-ended" => {
                if let Some(id) = data["publisherId"].as_str() {
                    self.unsubscribe(id);
                }
                Ok(vec![])
            }
            "screen-share-error" => {
                self.stop_screen();
                Err("Сервер отклонил демонстрацию. Запустите её снова".into())
            }
            "screen-share-encode-pause" | "screen-share-encode-resume" => {
                if let Some(p) = &self.publisher {
                    set_video_parameters(
                        &p.sender,
                        &self.settings.screen,
                        event.ends_with("resume"),
                    )?;
                }
                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }
    pub fn subscribe(&mut self, id: &str, layer: u8) -> Result<String, String> {
        let key = format!("screen-sub:{id}");
        if !self.subscribers.contains_key(&key) {
            let conn = connection(&self.factory, &self.config, &key, &self.tx)?;
            self.subscribers.insert(key, conn);
        }
        Ok(envelope(
            "screen-share-subscribe",
            json!({"publisherId":id,"preferredTemporalLayer":layer.min(2)}),
        ))
    }
    pub fn unsubscribe(&mut self, id: &str) {
        let key = format!("screen-sub:{id}");
        self.subscribers.remove(&key);
        if let Some(tasks) = self.tasks.remove(&key) {
            for task in tasks {
                task.abort();
            }
        }
        self.mixer.remove(&format!("screen:{id}"));
    }
    pub async fn start_screen(&mut self, choice: CaptureChoice) -> Result<String, String> {
        if self.publisher.is_some() {
            return Err("Демонстрация уже запущена".into());
        }
        let conn = connection(&self.factory, &self.config, "screen-pub", &self.tx)?;
        let (width, height) = self.settings.screen.dimensions();
        let source = NativeVideoSource::new(VideoResolution { width, height }, true);
        let track = self.factory.create_video_track("screen", source.clone());
        let transceiver = conn
            .pc
            .add_transceiver(
                track.into(),
                RtpTransceiverInit {
                    direction: RtpTransceiverDirection::SendOnly,
                    stream_ids: vec!["screen".into()],
                    send_encodings: vec![RtpEncodingParameters {
                        scalability_mode: Some("L1T3".into()),
                        max_bitrate: Some(self.settings.screen.max_bitrate()),
                        max_framerate: Some(self.settings.screen.fps as f64),
                        ..Default::default()
                    }],
                },
            )
            .map_err(|_| "Не удалось добавить видеопоток")?;
        let codec = format!("video/{}", self.settings.screen.codec);
        let codecs = self
            .factory
            .get_rtp_sender_capabilities(MediaType::Video)
            .codecs
            .into_iter()
            .filter(|c| c.mime_type.eq_ignore_ascii_case(&codec))
            .collect::<Vec<_>>();
        if codecs.is_empty() {
            return Err("Выбранный видеокодек недоступен".into());
        }
        transceiver
            .set_codec_preferences(codecs)
            .map_err(|_| "Не удалось выбрать видеокодек")?;
        let sender = transceiver.sender();
        let loopback = if self.settings.screen.system_audio {
            Some(Loopback::open(&self.settings.speaker)?)
        } else {
            None
        };
        let audio = loopback
            .as_ref()
            .map(|_| NativeAudioSource::new(AudioSourceOptions::default(), 48000, 1, 0));
        if let Some(audio) = &audio {
            conn.pc
                .add_track(
                    self.factory
                        .create_audio_track("system-audio", audio.clone())
                        .into(),
                    &["screen"],
                )
                .map_err(|_| "Не удалось добавить системный звук")?;
        }
        let offer = conn
            .pc
            .create_offer(OfferOptions {
                offer_to_receive_audio: None,
                offer_to_receive_video: None,
                ..Default::default()
            })
            .await
            .map_err(|_| "Не удалось создать демонстрацию")?;
        let sdp = offer.to_string();
        if !sdp.contains("dependency-descriptor") {
            return Err("Видеокодек не согласовал SVC dependency descriptor".into());
        }
        conn.pc
            .set_local_description(offer)
            .await
            .map_err(|_| "Не удалось применить настройки демонстрации")?;
        let stop = Arc::new(AtomicBool::new(false));
        let settings = Arc::new(Mutex::new(self.settings.screen.clone()));
        let capture = start_capture(
            choice,
            source.clone(),
            stop.clone(),
            settings.clone(),
            self.tx.clone(),
            self.frames.clone(),
        )?;
        self.publisher = Some(Publishing {
            connection: conn,
            sender,
            capture_stop: stop,
            _capture: capture,
            loopback,
            audio,
            settings,
        });
        Ok(envelope(
            "screen-share-start",
            json!({"sdp":sdp,"hasSystemAudio":self.settings.screen.system_audio,"mode":self.settings.screen.mode}),
        ))
    }
    pub fn stop_screen(&mut self) {
        self.publisher = None;
        self.resume_token = None;
    }
    pub async fn restart_screen_ice(&mut self) -> Result<Option<String>, String> {
        if let Some(p) = &mut self.publisher {
            let offer = p
                .connection
                .pc
                .create_offer(OfferOptions {
                    ice_restart: true,
                    offer_to_receive_audio: None,
                    offer_to_receive_video: None,
                })
                .await
                .map_err(|_| "Не удалось восстановить демонстрацию")?;
            let sdp = offer.to_string();
            p.connection
                .pc
                .set_local_description(offer)
                .await
                .map_err(|_| "Не удалось восстановить ICE")?;
            return Ok(Some(envelope(
                "offer",
                json!({"pc":"screen-pub","type":"offer","sdp":sdp}),
            )));
        }
        Ok(None)
    }
    pub async fn tick(&mut self) -> Result<(Vec<String>, Vec<(String, bool)>), String> {
        let mut signals = Vec::new();
        let mut levels = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Signal(signal) => signals.push(signal),
                Event::State(key, state) => {
                    if key == "audio" && state == PeerConnectionState::Failed {
                        self.audio_failed = true;
                    }
                    if key == "screen-pub"
                        && state == PeerConnectionState::Failed
                        && let Some(offer) = self.restart_screen_ice().await?
                    {
                        signals.push(offer);
                    }
                }
                Event::CaptureEnded => {
                    self.stop_screen();
                    signals.push(envelope("screen-share-stop", json!({})));
                }
                Event::Track(key, event) => self.receive_track(key, event),
                Event::Audio(key, mut samples) => {
                    let (peer, screen) = if let Some(id) = key.strip_prefix("screen:") {
                        (Some(id), true)
                    } else {
                        (self.slots.get(&key).and_then(|p| p.as_deref()), false)
                    };
                    let pref = peer
                        .and_then(|id| self.peers.get(id))
                        .and_then(|p| self.settings.peers.get(p.preference_key()));
                    let gain = pref.map_or(1.0, |p| {
                        if p.muted {
                            0.0
                        } else {
                            (if screen { p.screen_volume } else { p.volume }) as f32 / 100.0
                        }
                    });
                    if let Some(id) = peer {
                        let rms = (samples.iter().map(|s| s * s).sum::<f32>()
                            / samples.len().max(1) as f32)
                            .sqrt();
                        levels.push((id.into(), rms > 0.015));
                    }
                    for s in &mut samples {
                        *s *= gain;
                    }
                    if peer.is_some() {
                        self.mixer.push(key, &samples);
                    }
                }
            }
        }
        while let Ok(mut captured) = self.devices.captured.try_recv() {
            let mut pcm = [0i16; FRAME];
            for (to, from) in pcm.iter_mut().zip(captured) {
                *to = (from.clamp(-1.0, 1.0) * 32767.0) as i16;
            }
            self.apm
                .set_stream_delay_ms(40)
                .map_err(|_| "Ошибка эхоподавления")?;
            self.apm
                .process_stream(&mut pcm, 48000, 1)
                .map_err(|_| "Ошибка обработки микрофона")?;
            for (to, from) in captured.iter_mut().zip(pcm) {
                *to = from as f32 / 32768.0;
            }
            self.denoiser.process(&mut captured)?;
            let rms = (captured.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt();
            levels.push((
                "self".into(),
                !self.settings.muted && !self.settings.deafened && rms > 0.015,
            ));
            for (to, from) in pcm.iter_mut().zip(captured) {
                *to = if self.settings.muted || self.settings.deafened {
                    0
                } else {
                    (from * self.settings.input_volume as f32 / 100.0 * 32767.0)
                        .clamp(-32768.0, 32767.0) as i16
                };
            }
            for chunk in pcm.chunks(480) {
                self.mic
                    .capture_frame(&AudioFrame {
                        data: Cow::Borrowed(chunk),
                        sample_rate: 48000,
                        num_channels: 1,
                        samples_per_channel: 480,
                    })
                    .await
                    .map_err(|_| "Не удалось отправить звук микрофона")?;
            }
        }
        if let Some(p) = &mut self.publisher
            && let (Some(loopback), Some(audio)) = (&mut p.loopback, &p.audio)
        {
            while let Ok(samples) = loopback.captured.try_recv() {
                let pcm: Vec<i16> = samples
                    .iter()
                    .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                    .collect();
                for chunk in pcm.chunks(480) {
                    audio
                        .capture_frame(&AudioFrame {
                            data: Cow::Borrowed(chunk),
                            sample_rate: 48000,
                            num_channels: 1,
                            samples_per_channel: 480,
                        })
                        .await
                        .map_err(|_| "Ошибка передачи системного звука")?;
                }
            }
        }
        if self
            .devices
            .playback
            .lock()
            .map_err(|_| "Ошибка аудиовыхода")?
            .len()
            < FRAME * 2
        {
            let mut mixed = self.mixer.render();
            let gain = if self.settings.deafened {
                0.0
            } else {
                self.settings.output_volume as f32 / 100.0
            };
            for s in &mut mixed {
                *s = (*s * gain).clamp(-1.0, 1.0);
            }
            let mut reverse = [0i16; FRAME];
            for (to, from) in reverse.iter_mut().zip(mixed) {
                *to = (from * 32767.0) as i16;
            }
            self.apm
                .process_reverse_stream(&mut reverse, 48000, 1)
                .map_err(|_| "Ошибка обработки эха")?;
            if let Ok(mut queue) = self.devices.playback.lock() {
                queue.extend(mixed);
                if queue.len() > MAX_BUFFER {
                    let count = queue.len() - MAX_BUFFER;
                    queue.drain(..count);
                }
            }
        }
        if self.last_devices.elapsed() > Duration::from_secs(2) {
            self.last_devices = Instant::now();
            if self.devices.failed.load(Ordering::Relaxed)
                || crate::device::selected_ids(&self.settings.microphone, &self.settings.speaker)
                    .is_some_and(|ids| ids != self.devices.ids)
            {
                self.devices = Devices::open_selected(
                    self.muted.clone(),
                    self.deafened.clone(),
                    &self.settings.microphone,
                    &self.settings.speaker,
                )?;
            }
        }
        Ok((signals, levels))
    }
    fn receive_track(&mut self, key: String, event: TrackEvent) {
        let handle = tokio::runtime::Handle::current();
        match event.track {
            MediaStreamTrack::Audio(track) => {
                let name = if let Some(id) = key.strip_prefix("screen-sub:") {
                    format!("screen:{id}")
                } else {
                    event
                        .streams
                        .first()
                        .map(|s| s.id())
                        .unwrap_or_else(|| track.id())
                };
                let tx = self.tx.clone();
                let mut stream = NativeAudioStream::new(track, 48000, 1);
                let task = handle.spawn(async move {
                    while let Some(frame) = stream.next().await {
                        let samples = frame.data.iter().map(|s| *s as f32 / 32768.0).collect();
                        if tx.send(Event::Audio(name.clone(), samples)).await.is_err() {
                            break;
                        }
                    }
                });
                self.tasks.entry(key).or_default().push(task);
            }
            MediaStreamTrack::Video(track) => {
                let Some(id) = key.strip_prefix("screen-sub:").map(str::to_owned) else {
                    return;
                };
                let frames = self.frames.clone();
                let mut stream = NativeVideoStream::new(track);
                let task = handle.spawn(async move {
                    while let Some(frame) = stream.next().await {
                        let buffer = frame.buffer.to_i420();
                        let (width, height) = (buffer.width(), buffer.height());
                        if width == 0 || height == 0 || width > 8192 || height > 8192 {
                            continue;
                        }
                        let mut bgra = vec![0; width as usize * height as usize * 4];
                        let (y, u, v) = buffer.data();
                        let (sy, su, sv) = buffer.strides();
                        yuv_helper::i420_to_argb(
                            y,
                            sy,
                            u,
                            su,
                            v,
                            sv,
                            &mut bgra,
                            width * 4,
                            width as i32,
                            height as i32,
                        );
                        let _ = frames.try_send(VideoImage {
                            publisher: id.clone(),
                            width,
                            height,
                            bgra: Arc::new(bgra),
                        });
                    }
                });
                self.tasks.entry(key).or_default().push(task);
            }
        }
    }
}
fn connection(
    factory: &PeerConnectionFactory,
    config: &RtcConfiguration,
    key: &str,
    tx: &mpsc::Sender<Event>,
) -> Result<Connection, String> {
    let pc = factory
        .create_peer_connection(config.clone())
        .map_err(|_| "Не удалось создать WebRTC")?;
    let sender = tx.clone();
    let candidate_key = key.to_owned();
    pc.on_ice_candidate(Some(Box::new(move|candidate|{let mut data=json!({"pc":candidate_key.split(':').next().unwrap_or("audio"),"candidate":candidate.candidate(),"sdpMid":candidate.sdp_mid(),"sdpMLineIndex":candidate.sdp_mline_index()});if let Some(id)=candidate_key.strip_prefix("screen-sub:"){data["publisherId"]=id.into();}let _=sender.try_send(Event::Signal(envelope("candidate",data)));})));
    let sender = tx.clone();
    let track_key = key.to_owned();
    pc.on_track(Some(Box::new(move |event| {
        let _ = sender.try_send(Event::Track(track_key.clone(), event));
    })));
    let sender = tx.clone();
    let state_key = key.to_owned();
    pc.on_connection_state_change(Some(Box::new(move |state| {
        let _ = sender.try_send(Event::State(state_key.clone(), state));
    })));
    Ok(Connection {
        pc,
        remote_ready: false,
        candidates: vec![],
    })
}
fn pc_key(data: &Value) -> Result<String, String> {
    match data["pc"].as_str() {
        Some("audio") => Ok("audio".into()),
        Some("screen-pub") => Ok("screen-pub".into()),
        Some("screen-sub") => Ok(format!(
            "screen-sub:{}",
            data["publisherId"].as_str().ok_or("Нет publisherId")?
        )),
        _ => Err("Неизвестный тип WebRTC соединения".into()),
    }
}
fn description_data(data: &Value, kind: &str, sdp: String) -> Value {
    let mut value = json!({"pc":data["pc"],"type":kind,"sdp":sdp});
    if data["pc"] == "screen-sub" {
        value["publisherId"] = data["publisherId"].clone();
    }
    value
}
async fn add_candidate(pc: &PeerConnection, data: &Value) {
    if let Some(candidate) = data["candidate"].as_str() {
        if candidate.is_empty() {
            return;
        }
        if let Ok(candidate) = IceCandidate::parse(
            data["sdpMid"].as_str().unwrap_or("0"),
            data["sdpMLineIndex"].as_i64().unwrap_or(0) as i32,
            candidate,
        ) {
            let _ = pc.add_ice_candidate(candidate).await;
        }
    }
}
async fn flush_candidates(conn: &mut Connection) {
    for candidate in std::mem::take(&mut conn.candidates) {
        add_candidate(&conn.pc, &candidate).await;
    }
}
fn set_video_parameters(
    sender: &RtpSender,
    settings: &ScreenSettings,
    active: bool,
) -> Result<(), String> {
    let mut params = sender.parameters();
    for encoding in &mut params.encodings {
        encoding.active = active;
        encoding.max_bitrate = Some(settings.max_bitrate());
        encoding.max_framerate = Some(settings.fps as f64);
        encoding.scalability_mode = Some("L1T3".into());
    }
    params.set_degradation_preference(if settings.mode == "sharp" {
        DegradationPreference::MaintainResolution
    } else {
        DegradationPreference::MaintainFramerate
    });
    sender
        .set_parameters(params)
        .map_err(|_| "Не удалось применить параметры видео".into())
}
fn start_capture(
    choice: CaptureChoice,
    source: NativeVideoSource,
    stop: Arc<AtomicBool>,
    settings: Arc<Mutex<ScreenSettings>>,
    events: mpsc::Sender<Event>,
    frames: mpsc::Sender<VideoImage>,
) -> Result<std::thread::JoinHandle<()>, String> {
    // Desktop capture has thread affinity. Create, select, capture and destroy on one thread.
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
    let thread = std::thread::spawn(move || {
        let mut options = DesktopCapturerOptions::new(if choice.window {
            DesktopCaptureSourceType::Window
        } else {
            DesktopCaptureSourceType::Screen
        });
        options.set_include_cursor(true);
        let Some(mut capturer) = DesktopCapturer::new(options) else {
            let _ = ready_tx.send(Err("Не удалось открыть захват экрана".into()));
            return;
        };
        let Some(capture) = capturer
            .get_source_list()
            .into_iter()
            .find(|s| s.id() == choice.id)
        else {
            let _ = ready_tx.send(Err("Окно или экран уже закрыты".into()));
            return;
        };
        let callback_settings = settings.clone();
        let callback_stop = stop.clone();
        let start = Instant::now();
        let mut preview = Instant::now() - Duration::from_secs(1);
        capturer.start_capture(Some(capture), move |result| match result {
            Ok(frame) => {
                let (width, height) = (frame.width(), frame.height());
                if width <= 0 || height <= 0 || width > 16384 || height > 16384 {
                    return;
                }
                let mut buffer = I420Buffer::new(width as u32, height as u32);
                let (sy, su, sv) = buffer.strides();
                let (y, u, v) = buffer.data_mut();
                yuv_helper::argb_to_i420(
                    frame.data(),
                    frame.stride(),
                    y,
                    sy,
                    u,
                    su,
                    v,
                    sv,
                    width,
                    height,
                );
                let Ok(settings) = callback_settings.lock() else {
                    return;
                };
                let (maxw, maxh) = settings.dimensions();
                let scale = (maxw as f64 / width as f64)
                    .min(maxh as f64 / height as f64)
                    .min(1.0);
                let buffer = if scale < 1.0 {
                    buffer.scale(
                        ((width as f64 * scale) as i32 / 2 * 2).max(2),
                        ((height as f64 * scale) as i32 / 2 * 2).max(2),
                    )
                } else {
                    buffer
                };
                let mut video = VideoFrame::new(VideoRotation::VideoRotation0, buffer);
                video.timestamp_us = start.elapsed().as_micros() as i64;
                source.capture_frame(&video);
                if preview.elapsed() > Duration::from_millis(100) {
                    preview = Instant::now();
                    let width = video.buffer.width();
                    let height = video.buffer.height();
                    let mut bgra = vec![0; width as usize * height as usize * 4];
                    let (y, u, v) = video.buffer.data();
                    let (sy, su, sv) = video.buffer.strides();
                    yuv_helper::i420_to_argb(
                        y,
                        sy,
                        u,
                        su,
                        v,
                        sv,
                        &mut bgra,
                        width * 4,
                        width as i32,
                        height as i32,
                    );
                    let _ = frames.try_send(VideoImage {
                        publisher: "self".into(),
                        width,
                        height,
                        bgra: Arc::new(bgra),
                    });
                }
            }
            Err(CaptureError::Permanent) => {
                callback_stop.store(true, Ordering::Relaxed);
                let _ = events.try_send(Event::CaptureEnded);
            }
            Err(CaptureError::Temporary) => {}
        });
        let _ = ready_tx.send(Ok(()));
        while !stop.load(Ordering::Relaxed) {
            let at = Instant::now();
            capturer.capture_frame();
            let fps = settings.lock().map_or(30, |s| s.fps.max(1));
            std::thread::sleep(
                Duration::from_secs_f64(1.0 / fps as f64).saturating_sub(at.elapsed()),
            );
        }
    });
    ready_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "Захват экрана не ответил")??;
    Ok(thread)
}
