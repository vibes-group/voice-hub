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
    pub fn set_focused(&self, _: bool) {}
    pub fn can_hide() -> bool {
        false
    }
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
        return format!("Key{}", key.to_ascii_uppercase());
    }
    if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() {
        return format!("Num{key}");
    }
    if key
        .strip_prefix('f')
        .is_some_and(|n| n.parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)))
    {
        return key.to_ascii_uppercase();
    }
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
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "insert" => "Insert",
        "-" => "Minus",
        "=" => "Equal",
        "[" => "LeftBracket",
        "]" => "RightBracket",
        "\\" => "BackSlash",
        ";" => "SemiColon",
        "'" => "Quote",
        "," => "Comma",
        "." => "Dot",
        "/" => "Slash",
        "`" => "BackQuote",
        "capslock" => "CapsLock",
        "numlock" => "NumLock",
        "scrolllock" => "ScrollLock",
        "pause" => "Pause",
        "printscreen" => "PrintScreen",
        "numpad0" => "Kp0",
        "numpad1" => "Kp1",
        "numpad2" => "Kp2",
        "numpad3" => "Kp3",
        "numpad4" => "Kp4",
        "numpad5" => "Kp5",
        "numpad6" => "Kp6",
        "numpad7" => "Kp7",
        "numpad8" => "Kp8",
        "numpad9" => "Kp9",
        "numpadadd" => "KpPlus",
        "numpadsubtract" => "KpMinus",
        "numpadmultiply" => "KpMultiply",
        "numpaddivide" => "KpDivide",
        "numpadenter" => "KpReturn",
        "numpaddecimal" => "KpDelete",
        other => other,
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyboard_names_match_focused_and_global_events() {
        assert_eq!(key_name("m"), "KeyM");
        assert_eq!(key_name("f8"), "F8");
        assert_eq!(key_name("."), "Dot");
        let binding = Hotkey::Keyboard {
            key: "F8".into(),
            ctrl: true,
            shift: false,
            alt: false,
            meta: false,
        };
        assert!(focused_binding(&binding, "f8", true, false, false, false));
        assert!(!focused_binding(&binding, "f8", true, true, false, false));
    }
}
