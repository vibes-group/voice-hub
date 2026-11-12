use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(any(feature = "desktop", test))]
use std::time::Duration;
use std::time::Instant;

#[cfg(any(feature = "desktop", test))]
use futures_util::SinkExt;
#[cfg(feature = "desktop")]
use futures_util::StreamExt;
#[cfg(any(feature = "desktop", test))]
use serde_json::json;
use str0m::change::SdpOffer;
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid};
#[cfg(feature = "desktop")]
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Rtc};
#[cfg(feature = "desktop")]
use str0m::{Input, Output};
#[cfg(feature = "desktop")]
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
#[cfg(any(feature = "desktop", test))]
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
#[cfg(any(feature = "desktop", test))]
use tokio_tungstenite::tungstenite::{Message as WsMessage, protocol::WebSocketConfig};

use crate::audio::{FRAME, Mixer, SAMPLE_RATE};
#[cfg(feature = "desktop")]
use crate::protocol::Message;
use crate::protocol::{Peer, microphone_mid};
#[cfg(any(feature = "desktop", test))]
use crate::protocol::{envelope, server_url, ws_url};

pub struct Connect {
    pub server: String,
    pub password: String,
    pub name: String,
    pub room: String,
}

pub enum Command {
    State { muted: bool, deafened: bool },
    Leave,
}

pub enum Update {
    Status(String),
    Peers(Vec<Peer>),
    Error(String),
    Closed,
}

pub struct Session {
    pub commands: mpsc::Sender<Command>,
    pub updates: Option<mpsc::Receiver<Update>>,
    pub muted: Arc<AtomicBool>,
    pub deafened: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.muted.store(true, Ordering::Relaxed);
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.commands.try_send(Command::Leave);
    }
}

impl Session {
    #[cfg(feature = "desktop")]
    pub fn start(options: Connect) -> Self {
        let (commands, rx) = mpsc::channel(16);
        let (tx, updates) = mpsc::channel(64);
        let muted = Arc::new(AtomicBool::new(true));
        let deafened = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let session = Self {
            commands,
            updates: Some(updates),
            muted: muted.clone(),
            deafened: deafened.clone(),
            stop: stop.clone(),
        };
        std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Не удалось запустить сетевой поток".to_string())
                .and_then(|runtime| {
                    runtime.block_on(async {
                        let (socket, server) =
                            tokio::time::timeout(Duration::from_secs(15), authenticate(options))
                                .await
                                .map_err(|_| "Сервер не ответил за 15 секунд".to_string())??;
                        if stop.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        let devices =
                            crate::device::Devices::open(muted.clone(), deafened.clone())?;
                        run(socket, &server, rx, &tx, (muted, deafened), stop, devices).await
                    })
                });
            if let Err(error) = result {
                let _ = tx.blocking_send(Update::Error(error));
            }
            let _ = tx.blocking_send(Update::Closed);
        });
        session
    }
}

#[cfg(any(feature = "desktop", test))]
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[cfg(any(feature = "desktop", test))]
async fn authenticate(mut options: Connect) -> Result<(Socket, url::Url), String> {
    let server = server_url(&options.server)?;
    let ws = ws_url(&server, &options.room)?;
    let name = options.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err("Имя должно содержать от 1 до 64 символов".into());
    }
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "Не удалось настроить HTTPS")?;
    let response = http
        .post(server.join("api/login").map_err(|_| "Некорректный адрес")?)
        .form(&[("password", options.password.as_str())])
        .send()
        .await;
    options.password.clear();
    let response = response
        .map_err(|_| "Не удалось подключиться к серверу. Проверьте адрес и сертификат HTTPS")?;
    if response.status() != reqwest::StatusCode::NO_CONTENT {
        return Err(match response.status().as_u16() {
            401 => "Неверный пароль",
            429 => "Слишком много попыток входа. Повторите позже",
            _ => "Сервер отклонил вход",
        }
        .into());
    }
    let cookie = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| v.split(';').next().filter(|v| v.starts_with("vh_session=")))
        .ok_or("Сервер не выдал сессию")?;
    let mut request = ws
        .as_str()
        .into_client_request()
        .map_err(|_| "Некорректный адрес WebSocket")?;
    request.headers_mut().insert(
        "cookie",
        cookie.parse().map_err(|_| "Некорректная сессия сервера")?,
    );
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(256 * 1024);
    config.max_frame_size = Some(256 * 1024);
    let (mut socket, _) =
        tokio_tungstenite::connect_async_with_config(request, Some(config), false)
            .await
            .map_err(|_| "Не удалось открыть соединение с комнатой")?;
    socket
        .send(WsMessage::Text(
            envelope("hello", json!({ "displayName": name, "clientId": "" })).into(),
        ))
        .await
        .map_err(|_| "Соединение с комнатой прервано")?;
    Ok((socket, server))
}

/// Shared by the hardware session and the headless interoperability test.
pub struct Voice {
    pub rtc: Rtc,
    mid: Option<Mid>,
    encoder: opus::Encoder,
    decoders: BTreeMap<String, opus::Decoder>,
    slot_mids: BTreeMap<String, String>,
    slot_owners: BTreeMap<String, Option<String>>,
    pub mixer: Mixer,
    timestamp: u64,
}

impl Voice {
    pub fn new(local: std::net::SocketAddr) -> Result<Self, String> {
        let mut rtc = Rtc::builder().build(Instant::now());
        rtc.add_local_candidate(
            Candidate::host(local, "udp").map_err(|_| "Некорректный адрес UDP")?,
        );
        let mut encoder =
            opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)
                .map_err(|_| "Не удалось запустить Opus")?;
        encoder
            .set_bitrate(opus::Bitrate::Bits(32_000))
            .map_err(|_| "Не удалось настроить Opus")?;
        encoder
            .set_inband_fec(true)
            .map_err(|_| "Не удалось настроить Opus FEC")?;
        encoder
            .set_packet_loss_perc(10)
            .map_err(|_| "Не удалось настроить Opus FEC")?;
        encoder
            .set_dtx(true)
            .map_err(|_| "Не удалось настроить Opus DTX")?;
        Ok(Self {
            rtc,
            mid: None,
            encoder,
            decoders: BTreeMap::new(),
            slot_mids: BTreeMap::new(),
            slot_owners: BTreeMap::new(),
            mixer: Mixer::default(),
            timestamp: 0,
        })
    }

    pub fn answer(&mut self, sdp: &str) -> Result<String, String> {
        let offer =
            SdpOffer::from_sdp_string(sdp).map_err(|_| "Сервер прислал некорректный SDP")?;
        let answer = self
            .rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|_| "Не удалось согласовать голос")?
            .to_sdp_string();
        self.mid = microphone_mid(&answer).map(|m| Mid::from(m.as_str()));
        for section in sdp.split("m=").skip(1).filter(|s| s.starts_with("audio ")) {
            let mid = section
                .lines()
                .find_map(|l| l.trim().strip_prefix("a=mid:"))
                .map(str::to_owned);
            let stream = section
                .lines()
                .find_map(|l| l.trim().strip_prefix("a=msid:"))
                .and_then(|s| s.split_whitespace().next());
            if let (Some(mid), Some(stream)) = (mid, stream) {
                self.slot_mids.insert(stream.into(), mid);
            }
        }
        Ok(answer)
    }

    pub fn assign_slot(&mut self, slot: &str, owner: Option<&str>) {
        let owner = owner.map(str::to_owned);
        if self.slot_owners.get(slot) != Some(&owner) {
            if let Some(mid) = self.slot_mids.get(slot) {
                self.mixer.remove(mid);
                self.decoders.remove(mid);
            }
            self.slot_owners.insert(slot.to_owned(), owner);
        }
    }

    pub fn encode(&mut self, samples: &[f32; FRAME]) -> Result<(), String> {
        let timestamp = self.timestamp;
        self.timestamp += FRAME as u64;
        let Some(mid) = self.mid else { return Ok(()) };
        let Some(writer) = self.rtc.writer(mid) else {
            return Ok(());
        };
        let Some(pt) = writer
            .payload_params()
            .find(|p| p.spec().codec == str0m::format::Codec::Opus)
            .map(|p| p.pt())
        else {
            return Ok(());
        };
        let mut buffer = [0; 4000];
        let size = self
            .encoder
            .encode_float(samples, &mut buffer)
            .map_err(|_| "Ошибка кодирования Opus")?;
        writer
            .write(
                pt,
                Instant::now(),
                MediaTime::new(timestamp, Frequency::FORTY_EIGHT_KHZ),
                buffer[..size].to_vec(),
            )
            .map_err(|_| "Ошибка отправки звука".to_string())
    }

    pub fn event(&mut self, event: Event, deafened: bool) -> Result<Option<&'static str>, String> {
        match event {
            Event::Connected => return Ok(Some("Голос подключён")),
            Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                return Err(
                    "Голосовое соединение потеряно. Проверьте доступность UDP и подключитесь снова"
                        .into(),
                );
            }
            Event::MediaAdded(media)
                if media.kind == MediaKind::Audio
                    && matches!(media.direction, Direction::RecvOnly | Direction::SendRecv) =>
            {
                self.decoders.insert(
                    media.mid.to_string(),
                    opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono)
                        .map_err(|_| "Ошибка запуска декодера Opus")?,
                );
            }
            Event::MediaData(media) if media.params.spec().codec == str0m::format::Codec::Opus => {
                let key = media.mid.to_string();
                if !self.decoders.contains_key(&key) {
                    self.decoders.insert(
                        key.clone(),
                        opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono)
                            .map_err(|_| "Ошибка запуска декодера Opus")?,
                    );
                }
                let decoder = self.decoders.get_mut(&key).expect("decoder inserted");
                let mut samples = [0.0; 5760];
                // Packets may contain up to 120 ms. Invalid packets are dropped.
                if let Ok(count) = decoder.decode_float(&media.data, &mut samples, false)
                    && !deafened
                {
                    self.mixer.push(key, &samples[..count]);
                }
            }
            _ => {}
        }
        Ok(None)
    }
}

#[cfg(feature = "desktop")]
async fn run(
    mut socket: Socket,
    server: &url::Url,
    mut commands: mpsc::Receiver<Command>,
    updates: &mpsc::Sender<Update>,
    audio_flags: (Arc<AtomicBool>, Arc<AtomicBool>),
    stop: Arc<AtomicBool>,
    mut devices: crate::device::Devices,
) -> Result<(), String> {
    use crate::audio::MAX_BUFFER;
    let (muted, deafened) = audio_flags;
    let address = tokio::net::lookup_host((
        server.host_str().ok_or("Нет адреса сервера")?,
        server.port_or_known_default().ok_or("Нет порта сервера")?,
    ))
    .await
    .map_err(|_| "Не удалось определить адрес сервера")?
    .find(|a| a.is_ipv4())
    .ok_or("Beta требует IPv4-адрес сервера")?;
    // Route lookup only: no data is sent to the HTTP port over UDP.
    let route = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|_| "Не удалось открыть UDP")?;
    route
        .connect(address)
        .map_err(|_| "Не удалось найти маршрут до сервера")?;
    let ip = route
        .local_addr()
        .map_err(|_| "Не удалось определить локальный IP")?
        .ip();
    let udp = UdpSocket::bind((ip, 0))
        .await
        .map_err(|_| "Не удалось открыть голосовой UDP-порт")?;
    let local = udp
        .local_addr()
        .map_err(|_| "Не удалось определить UDP-порт")?;
    let mut voice = Voice::new(local)?;
    let mut peers = BTreeMap::<String, Peer>::new();
    let mut own_id = String::new();
    let mut audio_connected = false;
    let started = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut packet = [0; 2048];
    let _ = updates.try_send(Update::Status("Подключение голоса…".into()));
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let idle_at = loop {
            match voice.rtc.poll_output().map_err(|_| "Ошибка WebRTC")? {
                Output::Timeout(at) => break at,
                Output::Transmit(tx) => {
                    udp.send_to(&tx.contents, tx.destination)
                        .await
                        .map_err(|_| "Ошибка отправки UDP")?;
                }
                Output::Event(event) => {
                    if let Some(status) = voice.event(event, deafened.load(Ordering::Relaxed))? {
                        audio_connected = true;
                        let _ = updates.try_send(Update::Status(status.into()));
                    }
                }
            }
        };
        tokio::select! {
            command = commands.recv() => match command {
                None | Some(Command::Leave) => break,
                Some(Command::State { muted: mic, deafened: deaf }) => {
                    muted.store(mic || deaf, Ordering::Relaxed);
                    deafened.store(deaf, Ordering::Relaxed);
                    if deaf {
                        voice.mixer.clear();
                        if let Ok(mut playback) = devices.playback.lock() { playback.clear(); }
                    }
                    send(&mut socket, "set-state", json!({ "selfMuted": mic || deaf, "deafened": deaf })).await?;
                }
            },
            message = socket.next() => {
                match message {
                    Some(Ok(WsMessage::Ping(data))) => socket.send(WsMessage::Pong(data)).await.map_err(|_| "Соединение потеряно")?,
                    Some(Ok(WsMessage::Text(text))) => {
                        let message: Message = serde_json::from_str(&text).map_err(|_| "Некорректное сообщение сервера")?;
                        let data = message.data;
                        let mut roster_changed = false;
                        match message.event.as_str() {
                            "welcome" => {
                                own_id = data["id"].as_str().ok_or("Нет идентификатора участника")?.to_owned();
                                peers = serde_json::from_value::<Vec<Peer>>(data["peers"].clone()).map_err(|_| "Некорректный список участников")?.into_iter().map(|p| (p.id.clone(), p)).collect();
                                send(&mut socket, "set-state", json!({ "selfMuted": muted.load(Ordering::Relaxed), "deafened": deafened.load(Ordering::Relaxed) })).await?;
                                roster_changed = true;
                            }
                            "offer" if data["pc"] == "audio" => {
                                let answer = voice.answer(data["sdp"].as_str().ok_or("Нет SDP в предложении сервера")?)?;
                                send(&mut socket, "answer", json!({"pc":"audio", "type":"answer", "sdp":answer})).await?;
                            }
                            "candidate" if data["pc"] == "audio" => {
                                if let Some(candidate) = data["candidate"].as_str().filter(|s| !s.is_empty()) {
                                    let candidate = Candidate::from_sdp_string(candidate.trim_start_matches("a=")).map_err(|_| "Некорректный ICE-кандидат")?;
                                    voice.rtc.add_remote_candidate(candidate);
                                }
                            }
                            "peer-joined" | "peer-info" => {
                                let peer: Peer = serde_json::from_value(data).map_err(|_| "Некорректный участник")?;
                                peers.insert(peer.id.clone(), peer);
                                roster_changed = true;
                            }
                            "peer-left" => { if let Some(id) = data["id"].as_str() { peers.remove(id); roster_changed = true; } }
                            "peer-state" => {
                                if let Some(peer) = data["id"].as_str().and_then(|id| peers.get_mut(id)) {
                                    peer.self_muted = data["selfMuted"].as_bool().unwrap_or(false);
                                    peer.deafened = data["deafened"].as_bool().unwrap_or(false);
                                    roster_changed = true;
                                }
                            }
                            "audio-slots" => {
                                for slot in data["slots"].as_array().into_iter().flatten() {
                                    if let Some(name) = slot["slot"].as_str() { voice.assign_slot(name, slot["peerId"].as_str()); }
                                }
                            }
                            _ => {}
                        }
                        if roster_changed { let _ = updates.try_send(Update::Peers(peers.values().filter(|p| p.id != own_id).cloned().collect())); }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => return Err("Соединение с комнатой закрыто. Подключитесь снова".into()),
                    Some(Err(_)) => return Err("Соединение с сервером потеряно. Подключитесь снова".into()),
                    _ => {}
                }
            },
            result = udp.recv_from(&mut packet) => {
                let (len, source) = result.map_err(|_| "Ошибка приёма UDP")?;
                if let Ok(contents) = packet[..len].try_into() {
                    // Unrelated UDP must not tear down a valid connection.
                    let _ = voice.rtc.handle_input(Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination: local, contents }));
                }
            },
            Some(mut samples) = devices.captured.recv() => {
                if muted.load(Ordering::Relaxed) { samples.fill(0.0); }
                if audio_connected { voice.encode(&samples)?; }
            },
            _ = tokio::time::sleep_until(idle_at.into()) => {
                voice.rtc.handle_input(Input::Timeout(Instant::now())).map_err(|_| "Ошибка таймера WebRTC")?;
            },
            _ = tick.tick() => {
                if devices.failed.load(Ordering::Relaxed) { return Err("Аудиоустройство отключено. Проверьте устройства и подключитесь снова".into()); }
                if !audio_connected && started.elapsed() > Duration::from_secs(20) { return Err("Голос не подключился за 20 секунд. В beta требуется прямой доступ по UDP; TURN не поддерживается".into()); }
                let mixed = voice.mixer.render();
                if let Ok(mut queue) = devices.playback.lock() {
                    if deafened.load(Ordering::Relaxed) { queue.clear(); }
                    else {
                        queue.extend(mixed);
                        if queue.len() > MAX_BUFFER { let count = queue.len() - MAX_BUFFER; queue.drain(..count); }
                    }
                }
            }
        }
    }
    voice.rtc.disconnect();
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
    Ok(())
}

#[cfg(feature = "desktop")]
async fn send(socket: &mut Socket, event: &str, data: serde_json::Value) -> Result<(), String> {
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(WsMessage::Text(envelope(event, data).into())),
    )
    .await
    .map_err(|_| "Сервер перестал отвечать")?
    .map_err(|_| "Соединение потеряно".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn request(stream: &mut TcpStream) -> String {
        let mut buffer = Vec::new();
        loop {
            let byte = stream.read_u8().await.unwrap();
            buffer.push(byte);
            if buffer.ends_with(b"\r\n\r\n") {
                break;
            }
            assert!(buffer.len() < 8192);
        }
        let headers = String::from_utf8(buffer).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .map(str::to_owned)
            })
            .unwrap()
            .parse()
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        assert_eq!(String::from_utf8(body).unwrap(), "password=test%2B%26%3D");
        headers
    }

    fn options(port: u16) -> Connect {
        Connect {
            server: format!("http://127.0.0.1:{port}"),
            password: "test+&=".into(),
            name: "Гость".into(),
            room: "room1".into(),
        }
    }

    #[tokio::test]
    #[allow(
        clippy::result_large_err,
        reason = "tungstenite's handshake callback requires its HTTP response error type"
    )]
    async fn authenticates_and_passes_only_session_cookie_to_websocket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert!(
                request(&mut stream)
                    .await
                    .starts_with("POST /api/login HTTP/1.1")
            );
            stream.write_all(b"HTTP/1.1 204 No Content\r\nset-cookie: vh_session=test-session; HttpOnly; SameSite=Strict\r\nconnection: close\r\n\r\n").await.unwrap();
            drop(stream);
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_hdr_async(
                stream,
                |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                    assert_eq!(request.uri().path(), "/ws/room1");
                    assert_eq!(request.headers()["cookie"], "vh_session=test-session");
                    Ok(response)
                },
            )
            .await
            .unwrap();
            let hello = ws.next().await.unwrap().unwrap().into_text().unwrap();
            let message: crate::protocol::Message = serde_json::from_str(&hello).unwrap();
            assert_eq!(message.event, "hello");
            assert_eq!(message.data["displayName"], "Гость");
            assert!(!hello.contains("test+&="));
        });
        let result = tokio::time::timeout(Duration::from_secs(3), authenticate(options(port)))
            .await
            .unwrap();
        assert!(result.is_ok());
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn rejects_auth_failures_without_opening_websocket_or_following_redirects() {
        for (status, expected) in [
            ("401 Unauthorized", "Неверный пароль"),
            ("429 Too Many Requests", "Слишком много попыток"),
            ("307 Temporary Redirect", "Сервер отклонил вход"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                request(&mut stream).await;
                stream.write_all(format!("HTTP/1.1 {status}\r\nlocation: http://127.0.0.1:{port}/stolen\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").as_bytes()).await.unwrap();
                drop(stream);
                assert!(
                    tokio::time::timeout(Duration::from_millis(200), listener.accept())
                        .await
                        .is_err()
                );
            });
            let error = tokio::time::timeout(Duration::from_secs(3), authenticate(options(port)))
                .await
                .unwrap()
                .err()
                .unwrap();
            assert!(error.contains(expected), "{error}");
            server.await.unwrap();
        }
    }
}
