use std::collections::HashSet;
use std::mem::size_of;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::{Hotkey, SystemEvent};
use rdev::{Button, EventType, Key};
use tokio::sync::mpsc;
use windows::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::GetModuleHandleW, Registry::*, Threading::CreateMutexW},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

const CLASS: PCWSTR = w!("VoiceHubGpuiTray");
const TITLE: PCWSTR = w!("Voice Hub — GPUI");
const WM_TRAY: u32 = WM_APP + 1;
const WM_SHOW: u32 = WM_APP + 2;
const WM_FLASH: u32 = WM_APP + 3;
static TRAY_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TRAY_LIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static EVENTS: OnceLock<mpsc::Sender<SystemEvent>> = OnceLock::new();
struct Listener {
    binding: Option<Hotkey>,
    capturing: bool,
    pressed: HashSet<Key>,
    last: Option<Instant>,
    capture_modifiers: Option<Hotkey>,
    focused: bool,
}
pub struct System {
    mutex: HANDLE,
    listener: Arc<Mutex<Listener>>,
}
impl Drop for System {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.mutex);
            if let Ok(window) = FindWindowW(CLASS, None) {
                let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
}
impl System {
    pub fn start(events: mpsc::Sender<SystemEvent>) -> Result<Option<Self>, String> {
        unsafe {
            let mutex = CreateMutexW(None, false, w!("Local\\VoiceHubGpuiSingleInstance"))
                .map_err(|_| "Не удалось запустить приложение")?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                if let Ok(window) = FindWindowW(CLASS, None) {
                    let _ = PostMessageW(Some(window), WM_SHOW, WPARAM(0), LPARAM(0));
                }
                let _ = CloseHandle(mutex);
                return Ok(None);
            }
            let _ = EVENTS.set(events.clone());
            std::thread::spawn(move || {
                if tray().is_err() {
                    let _ = events.blocking_send(SystemEvent::Error(
                        "Не удалось создать значок в трее".into(),
                    ));
                }
            });
            let listener = Arc::new(Mutex::new(Listener {
                binding: None,
                capturing: false,
                pressed: HashSet::new(),
                last: None,
                capture_modifiers: None,
                focused: false,
            }));
            let shared = listener.clone();
            std::thread::spawn(move || {
                let result = rdev::listen(move |event| {
                    let Ok(mut state) = shared.lock() else { return };
                    match event.event_type {
                        EventType::KeyPress(key) => {
                            if !state.pressed.insert(key) {
                                return;
                            }
                            let ctrl = state.pressed.contains(&Key::ControlLeft)
                                || state.pressed.contains(&Key::ControlRight);
                            let shift = state.pressed.contains(&Key::ShiftLeft)
                                || state.pressed.contains(&Key::ShiftRight);
                            let alt = state.pressed.contains(&Key::Alt)
                                || state.pressed.contains(&Key::AltGr);
                            let meta = state.pressed.contains(&Key::MetaLeft)
                                || state.pressed.contains(&Key::MetaRight);
                            let name = format!("{key:?}");
                            let modifier = matches!(
                                key,
                                Key::ControlLeft
                                    | Key::ControlRight
                                    | Key::ShiftLeft
                                    | Key::ShiftRight
                                    | Key::Alt
                                    | Key::AltGr
                                    | Key::MetaLeft
                                    | Key::MetaRight
                            );
                            if state.capturing && modifier {
                                state.capture_modifiers = Some(Hotkey::Keyboard {
                                    key: String::new(),
                                    ctrl,
                                    shift,
                                    alt,
                                    meta,
                                });
                            }
                            if state.capturing && !modifier {
                                let binding = Hotkey::Keyboard {
                                    key: name.clone(),
                                    ctrl,
                                    shift,
                                    alt,
                                    meta,
                                };
                                state.binding = Some(binding.clone());
                                state.capturing = false;
                                state.last = Some(Instant::now());
                                emit(SystemEvent::Captured(binding));
                            } else if !state.capturing
                                && (!state.focused || modifier)
                                && matches!(&state.binding,Some(Hotkey::Keyboard{key,ctrl:c,shift:s,alt:a,meta:m})if ((key.is_empty()&&modifier)||(key==&name&&!modifier))&&*c==ctrl&&*s==shift&&*a==alt&&*m==meta)
                            {
                                fire(&mut state);
                            }
                        }
                        EventType::KeyRelease(key) => {
                            state.pressed.remove(&key);
                            if state.capturing
                                && state.pressed.is_empty()
                                && let Some(binding) = state.capture_modifiers.take()
                            {
                                state.binding = Some(binding.clone());
                                state.capturing = false;
                                state.last = Some(Instant::now());
                                emit(SystemEvent::Captured(binding));
                            }
                        }
                        EventType::ButtonPress(button) => {
                            let button = match button {
                                Button::Right => Some(2),
                                Button::Middle => Some(3),
                                Button::Unknown(n) => Some(n),
                                _ => None,
                            };
                            if let Some(button) = button {
                                if state.capturing {
                                    let binding = Hotkey::Mouse(button);
                                    state.binding = Some(binding.clone());
                                    state.capturing = false;
                                    state.last = Some(Instant::now());
                                    emit(SystemEvent::Captured(binding));
                                } else if state.binding == Some(Hotkey::Mouse(button)) {
                                    fire(&mut state);
                                }
                            }
                        }
                        _ => {}
                    }
                });
                if result.is_err() {
                    emit(SystemEvent::Error("Глобальная клавиша недоступна".into()));
                }
            });
            Ok(Some(Self { mutex, listener }))
        }
    }
    pub fn configure(&self, binding: Option<Hotkey>, _: bool, _: bool) {
        if let Ok(mut state) = self.listener.lock() {
            state.binding = binding;
            state.capturing = false;
            state.capture_modifiers = None;
        }
    }
    pub fn set_focused(&self, focused: bool) {
        if let Ok(mut state) = self.listener.lock() {
            state.focused = focused;
        }
    }
    pub fn can_hide() -> bool {
        TRAY_READY.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn capture(&self) {
        if let Ok(mut state) = self.listener.lock() {
            state.capturing = true;
            state.pressed.clear();
            state.capture_modifiers = None;
        }
    }
    pub fn hide() {
        unsafe {
            if let Ok(window) = FindWindowW(None, TITLE) {
                let _ = ShowWindow(window, SW_HIDE);
            }
        }
    }
    pub fn show() {
        unsafe {
            if let Ok(window) = FindWindowW(None, TITLE) {
                let _ = ShowWindow(window, SW_RESTORE);
                let _ = SetForegroundWindow(window);
            }
        }
    }
    pub fn flash(enabled: bool) {
        unsafe {
            if let Ok(tray) = FindWindowW(CLASS, None) {
                let _ = PostMessageW(Some(tray), WM_FLASH, WPARAM(enabled as usize), LPARAM(0));
            }
            if let Ok(window) = FindWindowW(None, TITLE) {
                let info = FLASHWINFO {
                    cbSize: size_of::<FLASHWINFO>() as u32,
                    hwnd: window,
                    dwFlags: if enabled {
                        FLASHW_ALL | FLASHW_TIMERNOFG
                    } else {
                        FLASHW_STOP
                    },
                    uCount: 3,
                    dwTimeout: 0,
                };
                let _ = FlashWindowEx(&info);
            }
        }
    }
    pub fn autostart(enabled: bool) -> Result<(), String> {
        unsafe {
            let mut key = HKEY::default();
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
                Some(0),
                KEY_SET_VALUE,
                &mut key,
            )
            .ok()
            .map_err(|_| "Не удалось изменить автозапуск")?;
            let result = if enabled {
                let path = std::env::current_exe().map_err(|_| "Не найден путь приложения")?;
                let command = format!("\"{}\" --hidden", path.display())
                    .encode_utf16()
                    .chain(Some(0))
                    .collect::<Vec<_>>();
                let bytes =
                    std::slice::from_raw_parts(command.as_ptr().cast::<u8>(), command.len() * 2);
                RegSetValueExW(key, w!("VoiceHubGpui"), Some(0), REG_SZ, Some(bytes)).ok()
            } else {
                let status = RegDeleteValueW(key, w!("VoiceHubGpui"));
                if status == ERROR_FILE_NOT_FOUND {
                    Ok(())
                } else {
                    status.ok()
                }
            };
            let _ = RegCloseKey(key);
            result.map_err(|_| "Не удалось изменить автозапуск".into())
        }
    }
}
fn emit(event: SystemEvent) {
    if let Some(events) = EVENTS.get() {
        let _ = events.try_send(event);
    }
}
fn fire(state: &mut Listener) {
    if state
        .last
        .is_none_or(|last| last.elapsed() > Duration::from_millis(80))
    {
        state.last = Some(Instant::now());
        emit(SystemEvent::ToggleMute);
    }
}
unsafe fn tray() -> Result<(), String> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|_| "Не удалось создать трей")?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err("Не удалось зарегистрировать трей".into());
        }
        let window = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!("Voice Hub"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|_| "Не удалось создать трей")?;
        let mut icon = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: LoadIconW(None, IDI_APPLICATION).map_err(|_| "Не удалось загрузить значок")?,
            ..Default::default()
        };
        for (to, from) in icon.szTip.iter_mut().zip("Voice Hub".encode_utf16()) {
            *to = from;
        }
        if !Shell_NotifyIconW(NIM_ADD, &icon).as_bool() {
            return Err("Не удалось добавить значок".into());
        }
        TRAY_READY.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        TRAY_READY.store(false, std::sync::atomic::Ordering::Relaxed);
        let _ = Shell_NotifyIconW(NIM_DELETE, &icon);
        Ok(())
    }
}
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match message {
            WM_FLASH => {
                if wparam.0 != 0 {
                    let _ = SetTimer(Some(window), 1, 500, None);
                } else {
                    let _ = KillTimer(Some(window), 1);
                    tray_icon(window, false);
                }
                LRESULT(0)
            }
            WM_TIMER => {
                let previous = TRAY_LIT.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                tray_icon(window, !previous);
                LRESULT(0)
            }
            WM_SHOW => {
                emit(SystemEvent::Show);
                LRESULT(0)
            }
            WM_TRAY => {
                match lparam.0 as u32 {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => emit(SystemEvent::Show),
                    WM_RBUTTONUP => {
                        if let Ok(menu) = CreatePopupMenu() {
                            for (id, label) in [
                                (1, "Показать Voice Hub"),
                                (2, "Другой сервер"),
                                (3, "Автозапуск"),
                                (4, "Проверить обновления"),
                                (5, "Выход"),
                            ] {
                                let label = label.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
                                let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(label.as_ptr()));
                            }
                            let mut point = POINT::default();
                            let _ = GetCursorPos(&mut point);
                            let _ = SetForegroundWindow(window);
                            let chosen = TrackPopupMenu(
                                menu,
                                TPM_RETURNCMD | TPM_NONOTIFY,
                                point.x,
                                point.y,
                                Some(0),
                                window,
                                None,
                            )
                            .0;
                            match chosen {
                                1 => emit(SystemEvent::Show),
                                2 => emit(SystemEvent::ChangeServer),
                                3 => emit(SystemEvent::Autostart),
                                4 => emit(SystemEvent::CheckUpdate),
                                5 => emit(SystemEvent::Exit),
                                _ => {}
                            }
                            let _ = DestroyMenu(menu);
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(window);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

unsafe fn tray_icon(window: HWND, lit: bool) {
    unsafe {
        if let Ok(icon) = LoadIconW(
            None,
            if lit {
                IDI_INFORMATION
            } else {
                IDI_APPLICATION
            },
        ) {
            let data = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: window,
                uID: 1,
                uFlags: NIF_ICON,
                hIcon: icon,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }
}
