use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

use crate::audio::FRAME;
use windows::{
    Win32::{
        Media::Audio::*,
        System::{
            Com::{
                BLOB, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize, IAgileObject,
                IAgileObject_Impl, StructuredStorage::*,
            },
            Variant::VT_BLOB,
        },
    },
    core::{HRESULT, Interface, Ref, implement},
};

struct Client(IAudioClient);
// WASAPI activation and capture both run in the process MTA.
unsafe impl Send for Client {}
#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct Activated {
    sender: Mutex<Option<mpsc::SyncSender<Result<Client, String>>>>,
}
impl IAgileObject_Impl for Activated_Impl {}
impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(
        &self,
        operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let result = unsafe {
            (|| {
                let operation = operation
                    .as_ref()
                    .ok_or("Не завершена активация системного звука")?;
                let mut status = HRESULT(0);
                let mut interface = None;
                operation
                    .GetActivateResult(&mut status, &mut interface)
                    .map_err(|_| "Ошибка активации системного звука")?;
                status
                    .ok()
                    .map_err(|_| "Захват процесса недоступен в этой версии Windows")?;
                let client = interface
                    .ok_or("Не найден системный звук")?
                    .cast::<IAudioClient>()
                    .map_err(|_| "Не найден аудиоклиент")?;
                Ok(Client(client))
            })()
        };
        if let Ok(mut sender) = self.sender.lock()
            && let Some(sender) = sender.take()
        {
            let _ = sender.send(result);
        }
        Ok(())
    }
}
pub struct ProcessAudio {
    stop: Arc<AtomicBool>,
    pub captured: tokio::sync::mpsc::Receiver<[f32; FRAME * 2]>,
}
impl Drop for ProcessAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl ProcessAudio {
    pub fn open() -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let (tx, captured) = tokio::sync::mpsc::channel(3);
        let (ready, started) = mpsc::sync_channel(1);
        std::thread::spawn(move || unsafe {
            let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).ok();
            if initialized.is_err() {
                let _ = ready.send(Err("Не удалось инициализировать системный звук".into()));
                return;
            }
            let result = capture(cancelled, tx, ready.clone());
            if let Err(error) = result {
                let _ = ready.send(Err(error));
            }
            CoUninitialize();
        });
        match started.recv_timeout(Duration::from_secs(6)) {
            Ok(Ok(())) => Ok(Self { stop, captured }),
            Ok(Err(error)) => Err(error),
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                Err("Системный звук не ответил".into())
            }
        }
    }
}
unsafe fn capture(
    stop: Arc<AtomicBool>,
    output: tokio::sync::mpsc::Sender<[f32; FRAME * 2]>,
    ready: mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    unsafe {
        let mut activation = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: std::process::id(),
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let params = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: (&mut activation as *mut AUDIOCLIENT_ACTIVATION_PARAMS)
                                .cast(),
                        },
                    },
                    ..Default::default()
                }),
            },
        };
        let (sender, received) = mpsc::sync_channel(1);
        let callback: IActivateAudioInterfaceCompletionHandler = Activated {
            sender: Mutex::new(Some(sender)),
        }
        .into();
        let _operation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&params),
            &callback,
        )
        .map_err(|_| "Захват процесса недоступен")?;
        let client = received
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "Системный звук не ответил")??
            .0;
        let format = WAVEFORMATEX {
            wFormatTag: 3,
            nChannels: 2,
            nSamplesPerSec: 48000,
            nAvgBytesPerSec: 48000 * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                200_000,
                0,
                &format,
                None,
            )
            .map_err(|_| "Не удалось настроить системный звук")?;
        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|_| "Не удалось открыть системный звук")?;
        client
            .Start()
            .map_err(|_| "Не удалось запустить системный звук")?;
        let _ = ready.send(Ok(()));
        let mut frame = [0.0f32; FRAME * 2];
        let mut offset = 0;
        let result = (|| -> Result<(), String> {
            while !stop.load(Ordering::Relaxed) && !output.is_closed() {
                while capture
                    .GetNextPacketSize()
                    .map_err(|_| "Системный звук отключён")?
                    > 0
                {
                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0, 0);
                    capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                        .map_err(|_| "Не удалось прочитать системный звук")?;
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                    for index in 0..frames as usize * 2 {
                        frame[offset] = if silent || data.is_null() {
                            0.0
                        } else {
                            std::ptr::read_unaligned(data.add(index * 4).cast::<f32>())
                        };
                        offset += 1;
                        if offset == frame.len() {
                            let _ = output.try_send(frame);
                            offset = 0;
                        }
                    }
                    capture
                        .ReleaseBuffer(frames)
                        .map_err(|_| "Не удалось освободить аудиобуфер")?;
                }
                std::thread::sleep(Duration::from_millis(3));
            }
            Ok(())
        })();
        let _ = client.Stop();
        result
    }
}
