use crate::settings::Hotkey;

pub enum SystemEvent {
    Show,
    ChangeServer,
    Autostart,
    CheckUpdate,
    Exit,
    ToggleMute,
    Captured(Hotkey),
    Error(String),
}
#[cfg(target_os = "windows")]
#[path = "system_windows.rs"]
mod platform;
#[cfg(target_os = "windows")]
pub use platform::*;

#[cfg(not(target_os = "windows"))]
pub struct System;
#[cfg(not(target_os = "windows"))]
impl System {
    pub fn start(_: tokio::sync::mpsc::Sender<SystemEvent>) -> Result<Option<Self>, String> {
        Ok(Some(Self))
    }
    pub fn configure(&self, _: Option<Hotkey>, _: bool, _: bool) {}
    pub fn capture(&self) {}
    pub fn hide() {}
    pub fn show() {}
    pub fn flash(_: bool) {}
    pub fn autostart(_: bool) -> Result<(), String> {
        Ok(())
    }
}

pub fn focused_binding(
    binding: &Hotkey,
    key: &str,
    ctrl: bool,
    shift: bool,
    alt: bool,
    meta: bool,
) -> bool {
    match binding {
        Hotkey::Keyboard {
            key: expected,
            ctrl: c,
            shift: s,
            alt: a,
            meta: m,
        } => *c == ctrl && *s == shift && *a == alt && *m == meta && expected == &key_name(key),
        _ => false,
    }
}
pub fn key_name(key: &str) -> String {
    if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() {
        format!("Key{}", key.to_ascii_uppercase())
    } else if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() {
        format!("Num{key}")
    } else {
        match key {
            "enter" => "Return",
            "escape" => "Escape",
            "space" => "Space",
            "backspace" => "Backspace",
            "delete" => "Delete",
            "up" => "UpArrow",
            "down" => "DownArrow",
            "left" => "LeftArrow",
            "right" => "RightArrow",
            "tab" => "Tab",
            other => other,
        }
        .to_owned()
    }
}
