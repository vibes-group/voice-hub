use std::collections::{HashMap, VecDeque};

pub const SAMPLE_RATE: u32 = 48_000;
pub const FRAME: usize = 960;
pub const MAX_BUFFER: usize = FRAME * 5;

/// Queues are bounded so a stalled device or network cannot accumulate latency.
#[derive(Default)]
pub struct Mixer {
    tracks: HashMap<String, VecDeque<f32>>,
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
    fn mixes_simultaneous_voices_and_discards_stale_audio() {
        let mut mixer = Mixer::default();
        mixer.push("a".into(), &vec![0.6; MAX_BUFFER * 2]);
        mixer.push("b".into(), &[0.6; FRAME]);
        assert_eq!(mixer.render(), [1.0; FRAME]);
        assert_eq!(mixer.tracks["a"].len(), MAX_BUFFER - FRAME);
        mixer.clear();
        assert_eq!(mixer.render(), [0.0; FRAME]);
    }

    #[test]
    fn native_opus_round_trip() {
        let mut encoder =
            opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let mut decoder = opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono).unwrap();
        let input: Vec<_> = (0..FRAME)
            .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU) / SAMPLE_RATE as f32).sin() * 0.1)
            .collect();
        let mut packet = [0; 4000];
        let len = encoder.encode_float(&input, &mut packet).unwrap();
        let mut output = [0.0; FRAME];
        assert_eq!(
            decoder
                .decode_float(&packet[..len], &mut output, false)
                .unwrap(),
            FRAME
        );
        assert!(output.iter().any(|s| s.abs() > 0.01));
    }
}
