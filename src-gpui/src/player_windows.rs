use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use super::{PlayerAction, PlayerEvent};
use windows::{
    Win32::{
        Foundation::*, Graphics::Gdi::*, Media::MediaFoundation::*, System::Com::*,
        UI::WindowsAndMessaging::*,
    },
    core::{BSTR, Interface, implement, w},
};

#[implement(IMFMediaEngineNotify)]
struct Notify {
    actions: mpsc::Sender<PlayerAction>,
}
impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, _: usize, _: u32) -> windows::core::Result<()> {
        if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
            let _ = self.actions.send(PlayerAction::Fallback);
        }
        Ok(())
    }
}
pub struct Player {
    child: isize,
    actions: mpsc::Sender<PlayerAction>,
}
impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.actions.send(PlayerAction::Stop);
        unsafe {
            let _ = DestroyWindow(HWND(self.child as *mut _));
        }
    }
}
impl Player {
    pub fn open(
        path: &Path,
        server: &str,
    ) -> Result<(Self, tokio::sync::mpsc::Receiver<PlayerEvent>), String> {
        unsafe {
            let parent = FindWindowW(None, w!("Voice Hub — GPUI"))
                .map_err(|_| "Не найдено окно приложения")?;
            let child = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!(""),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                Some(parent),
                None,
                None,
                None,
            )
            .map_err(|_| "Не удалось создать видеоплеер")?;
            let (actions, rx) = mpsc::channel();
            let (events, event_rx) = tokio::sync::mpsc::channel(8);
            let hwnd = child.0 as isize;
            let path = path.to_owned();
            let server = server.to_owned();
            let callback_actions = actions.clone();
            std::thread::spawn(move || {
                if run(path, server, hwnd, rx, callback_actions, events.clone()).is_err() {
                    let _ = events.blocking_send(PlayerEvent::Error(
                        "Не удалось запустить системный видеоплеер".into(),
                    ));
                }
            });
            Ok((
                Self {
                    child: hwnd,
                    actions,
                },
                event_rx,
            ))
        }
    }
    pub fn action(&self, action: PlayerAction) {
        let _ = self.actions.send(action);
    }
    pub fn place(&self, x: i32, y: i32, width: i32, height: i32, clip: [i32; 4]) {
        unsafe {
            let child = HWND(self.child as *mut _);
            if clip[2] <= clip[0] || clip[3] <= clip[1] {
                let _ = ShowWindow(child, SW_HIDE);
                return;
            }
            let region = CreateRectRgn(clip[0] - x, clip[1] - y, clip[2] - x, clip[3] - y);
            if SetWindowRgn(child, Some(region), true) == 0 {
                let _ = DeleteObject(region.into());
            }
            let _ = MoveWindow(child, x, y, width.max(1), height.max(1), true);
            let _ = ShowWindow(child, SW_SHOWNA);
        }
        self.action(PlayerAction::Resize(width, height));
    }
    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(HWND(self.child as *mut _), SW_HIDE);
        }
    }
}
fn run(
    path: std::path::PathBuf,
    server: String,
    child: isize,
    rx: mpsc::Receiver<PlayerAction>,
    callback_actions: mpsc::Sender<PlayerAction>,
    events: tokio::sync::mpsc::Sender<PlayerEvent>,
) -> windows::core::Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        if MFStartup(MF_VERSION, MFSTARTUP_FULL).is_err() {
            CoUninitialize();
            if let Err(error) = crate::decoder::run(path, server, rx, events.clone()) {
                let _ = events.blocking_send(PlayerEvent::Error(error));
            }
            return Ok(());
        }
        let mut fallback = false;
        let result = (|| -> windows::core::Result<()> {
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 3)?;
            let attributes = attributes.expect("MFCreateAttributes");
            let notify: IMFMediaEngineNotify = Notify {
                actions: callback_actions,
            }
            .into();
            attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
            attributes.SetUINT64(&MF_MEDIA_ENGINE_PLAYBACK_HWND, child as u64)?;
            let engine = factory.CreateInstance(0, &attributes)?;
            let extension: IMFMediaEngineEx = engine.cast()?;
            let source = url::Url::from_file_path(&path)
                .map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
            engine.SetSource(&BSTR::from(source.as_str()))?;
            engine.SetAutoPlay(true)?;
            loop {
                match rx.recv_timeout(Duration::from_millis(150)) {
                    Ok(PlayerAction::Toggle) => {
                        if engine.IsPaused().as_bool() {
                            engine.Play()?;
                        } else {
                            engine.Pause()?;
                        }
                    }
                    Ok(PlayerAction::Seek(position)) => {
                        engine.SetCurrentTime(position.max(0.0).min(engine.GetDuration()))?;
                    }
                    Ok(PlayerAction::Volume(volume)) => {
                        engine.SetVolume(volume.clamp(0.0, 1.0))?;
                    }
                    Ok(PlayerAction::Rate(rate)) => {
                        engine.SetPlaybackRate(rate.clamp(0.25, 4.0))?;
                    }
                    Ok(PlayerAction::Resize(width, height)) => {
                        let rect = RECT {
                            left: 0,
                            top: 0,
                            right: width,
                            bottom: height,
                        };
                        extension.UpdateVideoStream(None, Some(&rect), None)?;
                    }
                    Ok(PlayerAction::Fallback) => {
                        fallback = true;
                        break;
                    }
                    Ok(PlayerAction::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                let duration = engine.GetDuration();
                let position = engine.GetCurrentTime();
                let _ = events.try_send(PlayerEvent::Status {
                    position: if position.is_finite() { position } else { 0.0 },
                    duration: if duration.is_finite() { duration } else { 0.0 },
                    paused: engine.IsPaused().as_bool(),
                });
            }
            engine.Shutdown()?;
            Ok(())
        })();
        let _ = MFShutdown();
        CoUninitialize();
        if fallback || result.is_err() {
            let _ = ShowWindow(HWND(child as *mut _), SW_HIDE);
            if let Err(error) = crate::decoder::run(path, server, rx, events.clone()) {
                let _ = events.blocking_send(PlayerEvent::Error(error));
            }
            Ok(())
        } else {
            result
        }
    }
}
