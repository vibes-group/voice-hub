use std::collections::VecDeque;

use crate::settings::NoiseEngine;
use wasmtime::{Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc};

const RNNOISE: &[u8] = include_bytes!("../../frontend/src/audio/wasm/rnnoise.wasm");
const FAST: &[u8] = include_bytes!("../../frontend/src/audio/wasm/fastenhancer.wasm");
const TINY: &[u8] = include_bytes!("../../frontend/src/audio/wasm/fastenhancer-t.bin");
const BASE: &[u8] = include_bytes!("../../frontend/src/audio/wasm/fastenhancer-b.bin");

struct WasmDenoiser {
    store: Store<StoreLimits>,
    memory: Memory,
    state: i32,
    input: i32,
    output: i32,
    process: TypedFunc<(i32, i32, i32), f32>,
    destroy: TypedFunc<i32, ()>,
    free: TypedFunc<i32, ()>,
    frame_size: usize,
    bytes: Vec<u8>,
}
impl WasmDenoiser {
    fn new(kind: NoiseEngine) -> Result<Self, String> {
        let (wasm, weights) = match kind {
            NoiseEngine::RnNoise => (RNNOISE, &[][..]),
            NoiseEngine::FastTiny => (FAST, TINY),
            NoiseEngine::FastBase => (FAST, BASE),
            _ => return Err("Этот режим не использует WASM".into()),
        };
        let engine = Engine::default();
        let module = Module::new(&engine, wasm)
            .map_err(|_| "Не удалось скомпилировать модель шумоподавления")?;
        let mut linker = Linker::new(&engine);
        linker
            .func_wrap("env", "emscripten_notify_memory_growth", |_: i32| {})
            .map_err(|_| "Не удалось настроить шумоподавление")?;
        let limits = StoreLimitsBuilder::new()
            .memory_size(128 * 1024 * 1024)
            .instances(1)
            .memories(1)
            .build();
        let mut store = Store::new(&engine, limits);
        store.limiter(|limits| limits);
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|_| "Не удалось загрузить модель шумоподавления")?;
        if let Ok(initialize) = instance.get_typed_func::<(), ()>(&mut store, "_initialize") {
            initialize
                .call(&mut store, ())
                .map_err(|_| "Не удалось инициализировать модель")?;
        }
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or("У модели нет памяти")?;
        let malloc = instance
            .get_typed_func::<i32, i32>(&mut store, "malloc")
            .map_err(|_| "У модели нет malloc")?;
        let free = instance
            .get_typed_func::<i32, ()>(&mut store, "free")
            .map_err(|_| "У модели нет free")?;
        let create = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "dn_create")
            .map_err(|_| "Неверный ABI модели")?;
        let frame_size = instance
            .get_typed_func::<i32, i32>(&mut store, "dn_frame_size")
            .map_err(|_| "Неверный ABI модели")?;
        let process = instance
            .get_typed_func::<(i32, i32, i32), f32>(&mut store, "dn_process")
            .map_err(|_| "Неверный ABI модели")?;
        let destroy = instance
            .get_typed_func::<i32, ()>(&mut store, "dn_destroy")
            .map_err(|_| "Неверный ABI модели")?;
        let weights_ptr = if weights.is_empty() {
            0
        } else {
            malloc
                .call(&mut store, weights.len() as i32)
                .map_err(|_| "Не удалось выделить память модели")?
        };
        if !weights.is_empty() {
            if weights_ptr <= 0 {
                return Err("Недостаточно памяти модели".into());
            }
            memory
                .write(&mut store, weights_ptr as usize, weights)
                .map_err(|_| "Неверная память модели")?;
        }
        let state = create
            .call(&mut store, (weights_ptr, weights.len() as i32))
            .map_err(|_| "Не удалось создать шумоподавитель")?;
        if weights_ptr != 0 {
            free.call(&mut store, weights_ptr)
                .map_err(|_| "Ошибка памяти модели")?;
        }
        if state == 0 {
            return Err("Модель отклонила веса".into());
        }
        let size = frame_size
            .call(&mut store, state)
            .map_err(|_| "Неверный размер кадра модели")?;
        if !(1..=8192).contains(&size) {
            return Err("Неверный размер кадра модели".into());
        }
        let input = malloc
            .call(&mut store, size * 4)
            .map_err(|_| "Недостаточно памяти модели")?;
        let output = malloc
            .call(&mut store, size * 4)
            .map_err(|_| "Недостаточно памяти модели")?;
        if input <= 0 || output <= 0 {
            return Err("Недостаточно памяти модели".into());
        }
        Ok(Self {
            store,
            memory,
            state,
            input,
            output,
            process,
            destroy,
            free,
            frame_size: size as usize,
            bytes: vec![0; size as usize * 4],
        })
    }
    fn process(&mut self, frame: &mut [f32]) -> Result<(), String> {
        if frame.len() != self.frame_size {
            return Err("Неверный размер аудиокадра".into());
        }
        for (sample, bytes) in frame
            .iter()
            .zip(self.bytes.as_chunks_mut::<4>().0.iter_mut())
        {
            bytes.copy_from_slice(&sample.clamp(-1.0, 1.0).to_le_bytes());
        }
        self.memory
            .write(&mut self.store, self.input as usize, &self.bytes)
            .map_err(|_| "Ошибка памяти шумоподавления")?;
        self.process
            .call(&mut self.store, (self.state, self.input, self.output))
            .map_err(|_| "Ошибка обработки шумоподавления")?;
        self.memory
            .read(&self.store, self.output as usize, &mut self.bytes)
            .map_err(|_| "Ошибка памяти шумоподавления")?;
        for (sample, bytes) in frame.iter_mut().zip(self.bytes.as_chunks::<4>().0.iter()) {
            let value = f32::from_le_bytes(*bytes);
            *sample = if value.is_finite() {
                value.clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
        Ok(())
    }
}
impl Drop for WasmDenoiser {
    fn drop(&mut self) {
        let _ = self.destroy.call(&mut self.store, self.state);
        let _ = self.free.call(&mut self.store, self.input);
        let _ = self.free.call(&mut self.store, self.output);
    }
}

pub struct Denoiser {
    wasm: Option<WasmDenoiser>,
    input: VecDeque<f32>,
    output: VecDeque<f32>,
    frame: Vec<f32>,
}
impl Denoiser {
    pub fn new(kind: NoiseEngine) -> Result<Self, String> {
        let wasm = if matches!(
            kind,
            NoiseEngine::RnNoise | NoiseEngine::FastTiny | NoiseEngine::FastBase
        ) {
            Some(WasmDenoiser::new(kind)?)
        } else {
            None
        };
        let size = wasm.as_ref().map_or(0, |w| w.frame_size);
        // One model hop of startup latency, independent of CPAL callback size.
        Ok(Self {
            wasm,
            input: VecDeque::with_capacity(size * 2),
            output: std::iter::repeat_n(0.0, size).collect(),
            frame: vec![0.0; size],
        })
    }
    pub fn process(&mut self, samples: &mut [f32]) -> Result<(), String> {
        let Some(wasm) = &mut self.wasm else {
            return Ok(());
        };
        self.input.extend(samples.iter().copied());
        while self.input.len() >= self.frame.len() {
            for sample in &mut self.frame {
                *sample = self.input.pop_front().expect("complete frame");
            }
            wasm.process(&mut self.frame)?;
            self.output.extend(self.frame.iter().copied());
        }
        for sample in samples {
            *sample = self.output.pop_front().unwrap_or(0.0);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_models_accept_non_aligned_native_audio_and_remain_finite() {
        for engine in [
            NoiseEngine::RnNoise,
            NoiseEngine::FastTiny,
            NoiseEngine::FastBase,
        ] {
            let mut denoiser = Denoiser::new(engine).unwrap();
            for _ in 0..8 {
                let mut frame = [0.01; 480];
                denoiser.process(&mut frame).unwrap();
                assert!(frame.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
            }
            assert!(denoiser.input.len() < denoiser.frame.len());
        }
    }
}
