use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rdev::{Button, Event, EventType, Key};
use tauri::{AppHandle, Emitter};

use crate::shortcut::{self, InputBinding};

const COOLDOWN: Duration = Duration::from_millis(80);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Capturing,
}

pub struct ListenerState {
    pub current: Option<InputBinding>,
    pub mode: Mode,
    pub pressed: HashSet<Key>,
    pub last_fire: Option<Instant>,
    // True while the Tauri main window is focused. The webview-level
    // listener handles keyboard fires under focus (since rdev keyboard
    // events are suppressed under focus on Windows — tauri#14770), so
    // we skip keyboard fires here while focused to avoid double-toggle
    // on macOS/Linux where rdev is not suppressed.
    pub window_focused: bool,
}

impl ListenerState {
    pub fn new(current: Option<InputBinding>) -> Self {
        Self {
            current,
            mode: Mode::Normal,
            pressed: HashSet::new(),
            last_fire: None,
            window_focused: false,
        }
    }
}

pub type SharedState = Arc<Mutex<ListenerState>>;

pub fn start(app: AppHandle, state: SharedState) {
    std::thread::spawn(move || {
        let cb_state = state.clone();
        let cb_app = app.clone();
        let result = rdev::listen(move |event: Event| {
            let mut s = cb_state.lock().unwrap_or_else(|p| {
                log::error!("listener: state mutex poisoned, recovering stale state");
                p.into_inner()
            });
            handle_event(&mut s, &cb_app, &event);
        });
        if let Err(err) = result {
            log::error!("rdev listen failed: {err:?}");
        }
    });
}

fn handle_event(state: &mut ListenerState, app: &AppHandle, event: &Event) {
    match &event.event_type {
        EventType::KeyPress(k) => {
            let was_modifier = is_modifier(k);
            // Skip OS auto-repeat: if the key is already in `pressed`, this is
            // a repeat event from holding the key, not a new press edge. On
            // platforms that send synthetic KeyRelease between repeats (X11),
            // the cooldown below catches the residual; press-edge handles
            // Wayland/Windows/macOS where no synthetic release is delivered.
            let newly_pressed = state.pressed.insert(*k);
            if !newly_pressed {
                return;
            }
            // Keyboard capture happens in the webview (Tauri+rdev keyboard
            // events are suppressed when the window is focused on Windows —
            // tauri-apps/tauri#14770). The listener only handles fire-time
            // matching for keyboard.
            if state.mode == Mode::Normal {
                let modifier_only = is_modifier_only_binding(state.current.as_ref());
                let trigger = if modifier_only { was_modifier } else { !was_modifier };
                if trigger {
                    try_fire_keyboard(state, app);
                }
            }
        }
        EventType::KeyRelease(k) => {
            state.pressed.remove(k);
        }
        EventType::ButtonPress(b) => match state.mode {
            Mode::Normal => {
                if matches_mouse(state.current.as_ref(), b) {
                    fire_toggle(state, app);
                }
            }
            Mode::Capturing => try_capture_mouse(state, app, b),
        },
        _ => {}
    }
}

// ---- Normal mode ----

fn try_fire_keyboard(state: &mut ListenerState, app: &AppHandle) {
    // The webview-level listener owns keyboard fires while the window is
    // focused — see ListenerState::window_focused.
    if state.window_focused {
        return;
    }
    let Some(binding) = state.current.clone() else {
        return;
    };
    if !matches_keyboard(&binding, &state.pressed) {
        return;
    }
    fire_toggle(state, app);
}

fn fire_toggle(state: &mut ListenerState, app: &AppHandle) {
    let now = Instant::now();
    if let Some(prev) = state.last_fire {
        if now.duration_since(prev) < COOLDOWN {
            return;
        }
    }
    state.last_fire = Some(now);
    if let Err(err) = app.emit("toggle-mute", ()) {
        log::warn!("listener: emit toggle-mute failed: {err}");
    }
}

fn is_modifier_only_binding(b: Option<&InputBinding>) -> bool {
    match b {
        Some(InputBinding::Keyboard { keys }) if !keys.is_empty() => {
            keys.iter().all(|k| is_modifier_label(k))
        }
        _ => false,
    }
}

fn is_modifier_label(label: &str) -> bool {
    matches!(label, "Ctrl" | "Shift" | "Alt" | "Meta")
}

fn matches_keyboard(binding: &InputBinding, pressed: &HashSet<Key>) -> bool {
    let keys = match binding {
        InputBinding::Keyboard { keys } => keys,
        _ => return false,
    };

    let want_ctrl = keys.iter().any(|k| k == "Ctrl");
    let want_shift = keys.iter().any(|k| k == "Shift");
    let want_alt = keys.iter().any(|k| k == "Alt");
    let want_meta = keys.iter().any(|k| k == "Meta");

    if want_ctrl != has_ctrl(pressed) {
        return false;
    }
    if want_shift != has_shift(pressed) {
        return false;
    }
    if want_alt != has_alt(pressed) {
        return false;
    }
    if want_meta != has_meta(pressed) {
        return false;
    }

    for label in keys {
        if is_modifier_label(label) {
            continue;
        }
        let Some(target) = label_to_key(label) else {
            return false;
        };
        if !pressed.contains(&target) {
            return false;
        }
    }
    true
}

fn matches_mouse(current: Option<&InputBinding>, btn: &Button) -> bool {
    let Some(InputBinding::Mouse { button }) = current else {
        return false;
    };
    button_label(btn).as_deref() == Some(button.as_str())
}

// ---- Capturing mode (mouse only) ----

fn try_capture_mouse(state: &mut ListenerState, app: &AppHandle, btn: &Button) {
    let Some(label) = button_label(btn) else {
        return;
    };
    let binding = InputBinding::Mouse { button: label };
    finalize_capture(state, app, binding);
}

fn finalize_capture(state: &mut ListenerState, app: &AppHandle, binding: InputBinding) {
    if let Err(err) = shortcut::save(app, Some(&binding)) {
        log::error!("save shortcut: {err}");
    }
    state.current = Some(binding.clone());
    state.pressed.clear();
    state.mode = Mode::Normal;
    state.last_fire = Some(Instant::now()); // suppress retrigger from same press
    if let Err(err) = app.emit("input-captured", binding) {
        log::warn!("listener: emit input-captured failed: {err}");
    }
}

// ---- Modifier helpers ----

fn is_modifier(k: &Key) -> bool {
    matches!(
        k,
        Key::ControlLeft
            | Key::ControlRight
            | Key::ShiftLeft
            | Key::ShiftRight
            | Key::Alt
            | Key::AltGr
            | Key::MetaLeft
            | Key::MetaRight
    )
}

fn has_ctrl(p: &HashSet<Key>) -> bool {
    p.contains(&Key::ControlLeft) || p.contains(&Key::ControlRight)
}
fn has_shift(p: &HashSet<Key>) -> bool {
    p.contains(&Key::ShiftLeft) || p.contains(&Key::ShiftRight)
}
fn has_alt(p: &HashSet<Key>) -> bool {
    p.contains(&Key::Alt) || p.contains(&Key::AltGr)
}
fn has_meta(p: &HashSet<Key>) -> bool {
    p.contains(&Key::MetaLeft) || p.contains(&Key::MetaRight)
}

// ---- Mouse button mapping ----

fn button_label(b: &Button) -> Option<String> {
    match b {
        Button::Right => Some("Right".to_string()),
        Button::Middle => Some("Middle".to_string()),
        Button::Unknown(n) => Some(format!("Side{n}")),
        _ => None, // Left intentionally excluded (would block normal clicks)
    }
}

// ---- Key label mapping (round-trip with frontend labelFromCode) ----

fn label_to_key(label: &str) -> Option<Key> {
    Some(match label {
        // Letters
        "A" => Key::KeyA,
        "B" => Key::KeyB,
        "C" => Key::KeyC,
        "D" => Key::KeyD,
        "E" => Key::KeyE,
        "F" => Key::KeyF,
        "G" => Key::KeyG,
        "H" => Key::KeyH,
        "I" => Key::KeyI,
        "J" => Key::KeyJ,
        "K" => Key::KeyK,
        "L" => Key::KeyL,
        "M" => Key::KeyM,
        "N" => Key::KeyN,
        "O" => Key::KeyO,
        "P" => Key::KeyP,
        "Q" => Key::KeyQ,
        "R" => Key::KeyR,
        "S" => Key::KeyS,
        "T" => Key::KeyT,
        "U" => Key::KeyU,
        "V" => Key::KeyV,
        "W" => Key::KeyW,
        "X" => Key::KeyX,
        "Y" => Key::KeyY,
        "Z" => Key::KeyZ,
        // Digits (top row)
        "0" => Key::Num0,
        "1" => Key::Num1,
        "2" => Key::Num2,
        "3" => Key::Num3,
        "4" => Key::Num4,
        "5" => Key::Num5,
        "6" => Key::Num6,
        "7" => Key::Num7,
        "8" => Key::Num8,
        "9" => Key::Num9,
        // Function row
        "F1" => Key::F1,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F4" => Key::F4,
        "F5" => Key::F5,
        "F6" => Key::F6,
        "F7" => Key::F7,
        "F8" => Key::F8,
        "F9" => Key::F9,
        "F10" => Key::F10,
        "F11" => Key::F11,
        "F12" => Key::F12,
        // Edit / nav
        "Space" => Key::Space,
        "Tab" => Key::Tab,
        "Enter" => Key::Return,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Escape" => Key::Escape,
        "Insert" => Key::Insert,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "Up" => Key::UpArrow,
        "Down" => Key::DownArrow,
        "Left" => Key::LeftArrow,
        "Right" => Key::RightArrow,
        // Locks / system
        "CapsLock" => Key::CapsLock,
        "NumLock" => Key::NumLock,
        "ScrollLock" => Key::ScrollLock,
        "Pause" => Key::Pause,
        "PrintScreen" => Key::PrintScreen,
        // Symbols
        "-" => Key::Minus,
        "=" => Key::Equal,
        "[" => Key::LeftBracket,
        "]" => Key::RightBracket,
        "\\" => Key::BackSlash,
        ";" => Key::SemiColon,
        "'" => Key::Quote,
        "," => Key::Comma,
        "." => Key::Dot,
        "/" => Key::Slash,
        "`" => Key::BackQuote,
        // Numpad
        "Num0" => Key::Kp0,
        "Num1" => Key::Kp1,
        "Num2" => Key::Kp2,
        "Num3" => Key::Kp3,
        "Num4" => Key::Kp4,
        "Num5" => Key::Kp5,
        "Num6" => Key::Kp6,
        "Num7" => Key::Kp7,
        "Num8" => Key::Kp8,
        "Num9" => Key::Kp9,
        "Num+" => Key::KpPlus,
        "Num-" => Key::KpMinus,
        "Num*" => Key::KpMultiply,
        "Num/" => Key::KpDivide,
        "NumEnter" => Key::KpReturn,
        "Num." => Key::KpDelete,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct KeymapEntry {
        label: String,
    }

    #[derive(serde::Deserialize)]
    struct Keymap {
        labels: Vec<KeymapEntry>,
    }

    const MODIFIERS: &[&str] = &["Ctrl", "Shift", "Alt", "Meta"];

    #[test]
    fn keymap_json_coverage() {
        let keymap: Keymap =
            serde_json::from_str(include_str!("../keymap.json")).expect("keymap.json is valid JSON");

        for entry in &keymap.labels {
            let label = &entry.label;
            if MODIFIERS.contains(&label.as_str()) {
                continue;
            }
            assert!(
                label_to_key(label).is_some(),
                "label_to_key returned None for label {:?} — keymap.json and Rust are out of sync",
                label
            );
        }
    }

    #[derive(serde::Deserialize)]
    struct MousemapButton {
        label: String,
        rdev_variant: String,
    }

    #[derive(serde::Deserialize)]
    struct Mousemap {
        buttons: Vec<MousemapButton>,
    }

    #[test]
    fn mousemap_json_coverage() {
        let mousemap: Mousemap = serde_json::from_str(include_str!("../mousemap.json"))
            .expect("mousemap.json is valid JSON");

        for entry in &mousemap.buttons {
            let btn = match entry.rdev_variant.as_str() {
                "Button::Right" => Button::Right,
                "Button::Middle" => Button::Middle,
                "Button::Unknown(6)" => Button::Unknown(6),
                "Button::Unknown(7)" => Button::Unknown(7),
                "Button::Unknown(8)" => Button::Unknown(8),
                other => panic!("mousemap.json has unrecognised rdev_variant {:?} — update this test", other),
            };
            let got = button_label(&btn).unwrap_or_else(|| {
                panic!(
                    "button_label returned None for rdev_variant {:?} (label {:?})",
                    entry.rdev_variant, entry.label
                )
            });
            assert_eq!(
                got, entry.label,
                "button_label({:?}) = {:?}, want {:?}",
                entry.rdev_variant, got, entry.label
            );
        }

        // Verify the Side{n} pattern for a synthetic value not listed in the static entries.
        let synthetic = Button::Unknown(42);
        let label = button_label(&synthetic)
            .expect("button_label must return Some for Button::Unknown(42)");
        assert_eq!(label, "Side42", "side_pattern: Button::Unknown(42) must produce \"Side42\"");
    }
}
