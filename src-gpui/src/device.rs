use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
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
        let host = cpal::default_host();
        let input = host.default_input_device().ok_or("Микрофон не найден")?;
        let output = host
            .default_output_device()
            .ok_or("Устройство воспроизведения не найдено")?;
        let select = |configs: Vec<cpal::SupportedStreamConfigRange>| {
            configs
                .into_iter()
                .filter(|c| {
                    matches!(
                        c.sample_format(),
                        SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
                    )
                })
                .filter(|c| c.channels() > 0 && c.channels() <= 2)
                .find_map(|c| c.try_with_sample_rate(SAMPLE_RATE))
                .ok_or("Для beta требуется устройство с поддержкой 48 кГц".to_string())
        };
        let input_config = select(
            input
                .supported_input_configs()
                .map_err(|_| "Не удалось прочитать параметры микрофона")?
                .collect(),
        )?;
        let output_config = select(
            output
                .supported_output_configs()
                .map_err(|_| "Не удалось прочитать параметры вывода")?
                .collect(),
        )?;
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
            _ => unreachable!(),
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
            _ => unreachable!(),
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
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                for sample in data.chunks_exact(channels) {
                    frame[offset] = if muted.load(Ordering::Relaxed) {
                        0.0
                    } else {
                        sample.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32
                    };
                    offset += 1;
                    if offset == FRAME {
                        // Never block the audio callback on the network or encoder.
                        let _ = tx.try_send(frame);
                        offset = 0;
                    }
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
                        let sample = T::from_sample(queue.pop_front().unwrap_or(0.0));
                        frame.fill(sample);
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
