use std::collections::{HashMap, VecDeque};

pub const SAMPLE_RATE: u32 = 48_000;
pub const FRAME: usize = 960;
pub const MAX_BUFFER: usize = FRAME * 5;

/// Streaming conversion keeps fractional phase between device callbacks.
pub struct Resampler {
    input_rate: u32,
    output_rate: u32,
    phase: u64,
    previous: f32,
    initialized: bool,
}
impl Resampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        assert!(input_rate > 0 && output_rate > 0);
        Self {
            input_rate,
            output_rate,
            phase: 0,
            previous: 0.0,
            initialized: false,
        }
    }
    pub fn push(&mut self, sample: f32, mut output: impl FnMut(f32)) {
        if !self.initialized {
            self.previous = sample;
            self.initialized = true;
        }
        while self.phase < self.output_rate as u64 {
            let fraction = self.phase as f32 / self.output_rate as f32;
            output(self.previous + (sample - self.previous) * fraction);
            self.phase += self.input_rate as u64;
        }
        self.phase -= self.output_rate as u64;
        self.previous = sample;
    }
}

/// Queues are bounded so a stalled device or network cannot accumulate latency.
#[derive(Default)]
pub struct Mixer {
    tracks: HashMap<String, VecDeque<f32>>,
}

#[derive(Default)]
pub struct StereoMixer {
    tracks: HashMap<String, VecDeque<[f32; 2]>>,
}
impl StereoMixer {
    pub fn push(&mut self, track: String, samples: &[f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let queue = self.tracks.entry(track).or_default();
        queue.extend(
            samples
                .chunks_exact(channels)
                .map(|frame| [frame[0], if channels > 1 { frame[1] } else { frame[0] }]),
        );
        if queue.len() > MAX_BUFFER {
            queue.drain(..queue.len() - MAX_BUFFER);
        }
    }
    pub fn remove(&mut self, track: &str) {
        self.tracks.remove(track);
    }
    pub fn clear(&mut self) {
        self.tracks.clear();
    }
    pub fn render(&mut self) -> [[f32; 2]; FRAME] {
        let mut mixed = [[0.0f32; 2]; FRAME];
        for queue in self.tracks.values_mut() {
            for output in &mut mixed {
                let frame = queue.pop_front().unwrap_or([0.0; 2]);
                output[0] += frame[0];
                output[1] += frame[1];
            }
        }
        for output in &mut mixed {
            output[0] = output[0].clamp(-1.0, 1.0);
            output[1] = output[1].clamp(-1.0, 1.0);
        }
        mixed
    }
}

impl Mixer {
    pub fn push(&mut self, track: String, samples: &[f32]) {
        let queue = self.tracks.entry(track).or_default();
        queue.extend(samples);
        if queue.len() > MAX_BUFFER {
            queue.drain(..queue.len() - MAX_BUFFER);
        }
    }

    pub fn remove(&mut self, track: &str) {
        self.tracks.remove(track);
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
    }

    pub fn render(&mut self) -> [f32; FRAME] {
        let mut mixed = [0.0_f32; FRAME];
        for samples in self.tracks.values_mut() {
            for out in &mut mixed {
                *out += samples.pop_front().unwrap_or(0.0);
            }
        }
        for sample in &mut mixed {
            *sample = sample.clamp(-1.0, 1.0);
        }
        mixed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn screen_stereo_survives_mixing_with_mono_voice() {
        let mut mixer = StereoMixer::default();
        mixer.push("voice".into(), &[0.2; FRAME], 1);
        mixer.push("screen".into(), &[0.3, -0.4].repeat(FRAME), 2);
        assert_eq!(mixer.render()[0], [0.5, -0.2]);
    }
    #[test]
    fn device_resampling_keeps_fractional_phase_between_callbacks() {
        let mut resampler = Resampler::new(44100, 48000);
        let mut count = 0;
        for _ in 0..44100 {
            resampler.push(0.25, |sample| {
                assert_eq!(sample, 0.25);
                count += 1;
            });
        }
        assert_eq!(count, 48000);
    }

    #[test]
    fn mixes_simultaneous_voices_and_discards_stale_audio() {
        let mut mixer = Mixer::default();
        mixer.push("a".into(), &vec![0.6; MAX_BUFFER * 2]);
        mixer.push("b".into(), &[0.6; FRAME]);
        assert_eq!(mixer.render(), [1.0; FRAME]);
        assert_eq!(mixer.tracks["a"].len(), MAX_BUFFER - FRAME);
        mixer.clear();
        assert_eq!(mixer.render(), [0.0; FRAME]);
    }
}
