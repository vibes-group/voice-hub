use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use tokio::sync::mpsc;

use crate::audio::{FRAME, MAX_BUFFER, SAMPLE_RATE};

pub type Playback = Arc<Mutex<VecDeque<f32>>>;

pub struct Devices {
    _input: cpal::Stream,
    _output: cpal::Stream,
    pub captured: mpsc::Receiver<[f32; FRAME]>,
    pub playback: Playback,
    pub failed: Arc<AtomicBool>,
}

impl Devices {
    pub fn open(muted: Arc<AtomicBool>, deafened: Arc<AtomicBool>) -> Result<Self, String> {
        Self::open_selected(muted, deafened, "", "")
    }

    pub fn open_selected(
        muted: Arc<AtomicBool>,
        deafened: Arc<AtomicBool>,
        mic: &str,
        speaker: &str,
    ) -> Result<Self, String> {
        let host = cpal::default_host();
        let input = selected(&host, mic, true).ok_or("Микрофон не найден")?;
        let output =
            selected(&host, speaker, false).ok_or("Устройство воспроизведения не найдено")?;
        let input_config = input
            .default_input_config()
            .map_err(|_| "Не удалось прочитать параметры микрофона")?;
        let output_config = output
            .default_output_config()
            .map_err(|_| "Не удалось прочитать параметры вывода")?;
        let (tx, captured) = mpsc::channel(3);
        let playback = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_BUFFER)));
        let failed = Arc::new(AtomicBool::new(false));
        macro_rules! input_stream {
            ($ty:ty) => {
                capture::<$ty>(&input, input_config.config(), tx, muted, failed.clone())
            };
        }
        let input = match input_config.sample_format() {
            SampleFormat::F32 => input_stream!(f32),
            SampleFormat::I16 => input_stream!(i16),
            SampleFormat::U16 => input_stream!(u16),
            SampleFormat::I8 => input_stream!(i8),
            SampleFormat::I32 => input_stream!(i32),
            SampleFormat::F64 => input_stream!(f64),
            SampleFormat::U8 => input_stream!(u8),
            SampleFormat::U32 => input_stream!(u32),
            _ => return Err("Формат микрофона не поддерживается".into()),
        }?;
        macro_rules! output_stream {
            ($ty:ty) => {
                play::<$ty>(
                    &output,
                    output_config.config(),
                    playback.clone(),
                    deafened,
                    failed.clone(),
                )
            };
        }
        let output = match output_config.sample_format() {
            SampleFormat::F32 => output_stream!(f32),
            SampleFormat::I16 => output_stream!(i16),
            SampleFormat::U16 => output_stream!(u16),
            SampleFormat::I8 => output_stream!(i8),
            SampleFormat::I32 => output_stream!(i32),
            SampleFormat::F64 => output_stream!(f64),
            SampleFormat::U8 => output_stream!(u8),
            SampleFormat::U32 => output_stream!(u32),
            _ => return Err("Формат устройства вывода не поддерживается".into()),
        }?;
        output
            .play()
            .map_err(|_| "Не удалось запустить вывод звука")?;
        input.play().map_err(|_| "Не удалось запустить микрофон")?;
        Ok(Self {
            _input: input,
            _output: output,
            captured,
            playback,
            failed,
        })
    }
}

fn capture<T>(
    device: &cpal::Device,
    config: StreamConfig,
    tx: mpsc::Sender<[f32; FRAME]>,
    muted: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let mut frame = [0.0; FRAME];
    let mut offset = 0;
    let mut resampler = crate::audio::Resampler::new(config.sample_rate, SAMPLE_RATE);
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                for sample in data.chunks_exact(channels) {
                    let sample = if muted.load(Ordering::Relaxed) {
                        0.0
                    } else {
                        sample.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32
                    };
                    resampler.push(sample, |sample| {
                        frame[offset] = sample;
                        offset += 1;
                        if offset == FRAME {
                            let _ = tx.try_send(frame);
                            offset = 0;
                        }
                    });
                }
            },
            move |_| {
                failed.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|_| "Не удалось открыть микрофон".into())
}

fn play<T>(
    device: &cpal::Device,
    config: StreamConfig,
    playback: Playback,
    deafened: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let ratio = SAMPLE_RATE as f64 / config.sample_rate as f64;
    let mut phase = 0.0;
    let mut previous = 0.0_f32;
    let mut next = 0.0_f32;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                data.fill(T::EQUILIBRIUM);
                if let Ok(mut queue) = playback.try_lock() {
                    if deafened.load(Ordering::Relaxed) {
                        queue.clear();
                        return;
                    }
                    for frame in data.chunks_exact_mut(channels) {
                        while phase >= 1.0 {
                            previous = next;
                            next = queue.pop_front().unwrap_or(0.0);
                            phase -= 1.0;
                        }
                        let sample = previous + (next - previous) * phase as f32;
                        frame.fill(T::from_sample(sample));
                        phase += ratio;
                    }
                }
            },
            move |_| {
                failed.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|_| "Не удалось открыть устройство воспроизведения".into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

pub fn enumerate(input: bool) -> Vec<AudioDevice> {
    let host = cpal::default_host();
    let mut devices = vec![AudioDevice {
        id: String::new(),
        name: "По умолчанию".into(),
    }];
    if let Ok(found) = if input {
        host.input_devices()
    } else {
        host.output_devices()
    } {
        devices.extend(found.filter_map(|device| {
            Some(AudioDevice {
                id: device.id().ok()?.to_string(),
                name: device.to_string(),
            })
        }));
    }
    devices
}
fn selected(host: &cpal::Host, id: &str, input: bool) -> Option<cpal::Device> {
    id.parse::<cpal::DeviceId>()
        .ok()
        .and_then(|id| host.device_by_id(&id))
        .filter(|d| {
            if input {
                d.supports_input()
            } else {
                d.supports_output()
            }
        })
        .or_else(|| {
            if input {
                host.default_input_device()
            } else {
                host.default_output_device()
            }
        })
}

#[cfg(target_os = "windows")]
pub struct Loopback {
    _stream: cpal::Stream,
    pub captured: mpsc::Receiver<[f32; FRAME]>,
}
#[cfg(target_os = "windows")]
impl Loopback {
    pub fn open(speaker: &str) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = selected(&host, speaker, false).ok_or("Не найден системный аудиовыход")?;
        let config = device
            .default_output_config()
            .map_err(|_| "Не удалось прочитать параметры системного аудио")?;
        let (tx, captured) = mpsc::channel(3);
        let muted = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        macro_rules! stream {
            ($ty:ty) => {
                capture::<$ty>(&device, config.config(), tx, muted, failed)
            };
        }
        let stream = match config.sample_format() {
            SampleFormat::F32 => stream!(f32),
            SampleFormat::I16 => stream!(i16),
            SampleFormat::I32 => stream!(i32),
            SampleFormat::U16 => stream!(u16),
            _ => return Err("Неподдерживаемый формат системного аудио".into()),
        }?;
        stream
            .play()
            .map_err(|_| "Не удалось запустить захват системного аудио")?;
        Ok(Self {
            _stream: stream,
            captured,
        })
    }
}

#[derive(Clone, Copy)]
pub enum Feedback {
    Mute,
    Unmute,
    Ping,
}
pub fn feedback(speaker: String, kind: Feedback) {
    std::thread::spawn(move || {
        let host = cpal::default_host();
        let Some(device) = selected(&host, &speaker, false) else {
            return;
        };
        let Ok(config) = device.default_output_config() else {
            return;
        };
        let rate = config.sample_rate() as f32;
        let duration = if matches!(kind, Feedback::Ping) {
            0.44
        } else {
            0.14
        };
        let samples = (rate * duration) as usize;
        let channels = config.channels() as usize;
        let mut position = 0usize;
        let mut phase = 0.0_f32;
        macro_rules! stream {
            ($ty:ty) => {
                device.build_output_stream(
                    config.config(),
                    move |data: &mut [$ty], _| {
                        for frame in data.chunks_exact_mut(channels) {
                            let t = position as f32 / rate;
                            let sample = if position >= samples {
                                0.0
                            } else {
                                match kind {
                                    Feedback::Ping => {
                                        let env = if t < 0.008 {
                                            t / 0.008
                                        } else {
                                            (-20.0 * (t - 0.008)).exp()
                                        };
                                        env * (0.28 * (t * 587.0 * std::f32::consts::TAU).sin()
                                            + 0.18 * (t * 880.0 * std::f32::consts::TAU).sin())
                                    }
                                    Feedback::Mute | Feedback::Unmute => {
                                        let x = (t / 0.12).min(1.0);
                                        let (from, to, peak) = if matches!(kind, Feedback::Mute) {
                                            (520.0_f32, 300.0_f32, 0.2)
                                        } else {
                                            (392.0, 880.0, 0.16)
                                        };
                                        let frequency = from * (to / from).powf(x);
                                        phase += frequency / rate;
                                        let wave = if matches!(kind, Feedback::Mute) {
                                            4.0 * (phase - phase.floor() - 0.5).abs() - 1.0
                                        } else {
                                            (phase * std::f32::consts::TAU).sin()
                                        };
                                        let env = if t < 0.005 {
                                            t / 0.005
                                        } else {
                                            (1.0 - x).max(0.0)
                                        };
                                        wave * env * peak
                                    }
                                }
                            };
                            frame.fill(<$ty>::from_sample(sample));
                            position += 1;
                        }
                    },
                    |_| {},
                    None,
                )
            };
        }
        let stream = match config.sample_format() {
            SampleFormat::F32 => stream!(f32),
            SampleFormat::I16 => stream!(i16),
            SampleFormat::U16 => stream!(u16),
            SampleFormat::I32 => stream!(i32),
            _ => return,
        };
        if let Ok(stream) = stream
            && stream.play().is_ok()
        {
            std::thread::sleep(std::time::Duration::from_secs_f32(duration + 0.05));
        }
    });
}
