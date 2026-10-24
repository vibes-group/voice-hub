use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rdev::{Button, Event, EventType, Key};
use tauri::{AppHandle, Emitter};

use crate::shortcut::{self, InputBinding};

const COOLDOWN: Duration = Duration::from_millis(80);

const MODIFIERS: [(&str, [Key; 2]); 4] = [
    ("Ctrl", [Key::ControlLeft, Key::ControlRight]),
    ("Shift", [Key::ShiftLeft, Key::ShiftRight]),
    ("Alt", [Key::Alt, Key::AltGr]),
    ("Meta", [Key::MetaLeft, Key::MetaRight]),
];

pub struct ListenerState {
    pub current: Option<InputBinding>,
    pub capturing: bool,
    pub pressed: HashSet<Key>,
    pub last_fire: Option<Instant>,
    // While the main window is focused the webview fires keyboard shortcuts
    // itself (rdev keyboard events are suppressed under focus on Windows —
    // tauri#14770), so the listener skips them to avoid a double toggle on
    // macOS/Linux.
    pub window_focused: bool,
}

impl ListenerState {
    pub fn new(current: Option<InputBinding>) -> Self {
        Self {
            current,
            capturing: false,
            pressed: HashSet::new(),
            last_fire: None,
            window_focused: false,
        }
    }
}

pub type SharedState = Arc<Mutex<ListenerState>>;

pub fn start(app: AppHandle, state: SharedState) {
    std::thread::spawn(move || {
        let result = rdev::listen(move |event: Event| {
            let mut s = state.lock().unwrap_or_else(|p| {
                log::error!("listener: state mutex poisoned, recovering stale state");
                p.into_inner()
            });
            handle_event(&mut s, &app, &event);
        });
        if let Err(err) = result {
            log::error!("rdev listen failed: {err:?}");
        }
    });
}

fn handle_event(state: &mut ListenerState, app: &AppHandle, event: &Event) {
    match &event.event_type {
        EventType::KeyPress(k) => {
            // A key already in `pressed` is OS auto-repeat, not a new press.
            // X11 sends synthetic releases between repeats; the cooldown in
            // `fire_toggle` catches those.
            if !state.pressed.insert(*k) {
                return;
            }
            // Keyboard capture happens in the webview; here we only match fires.
            if state.capturing || state.window_focused {
                return;
            }
            // Modifier-only bindings fire on a modifier press, others on the
            // non-modifier key.
            if is_modifier(k) == is_modifier_only_binding(state.current.as_ref())
                && matches_keyboard(state.current.as_ref(), &state.pressed)
            {
                fire_toggle(state, app);
            }
        }
        EventType::KeyRelease(k) => {
            state.pressed.remove(k);
        }
        EventType::ButtonPress(b) => {
            if state.capturing {
                capture_mouse(state, app, b);
            } else if matches_mouse(state.current.as_ref(), b) {
                fire_toggle(state, app);
            }
        }
        _ => {}
    }
}

fn fire_toggle(state: &mut ListenerState, app: &AppHandle) {
    let now = Instant::now();
    if state
        .last_fire
        .is_some_and(|prev| now.duration_since(prev) < COOLDOWN)
    {
        return;
    }
    state.last_fire = Some(now);
    if let Err(err) = app.emit("toggle-mute", ()) {
        log::warn!("listener: emit toggle-mute failed: {err}");
    }
}

fn capture_mouse(state: &mut ListenerState, app: &AppHandle, btn: &Button) {
    let Some(label) = button_label(btn) else {
        return;
    };
    let binding = InputBinding::Mouse { button: label };
    if let Err(err) = shortcut::save(app, Some(&binding)) {
        log::error!("save shortcut: {err}");
    }
    state.current = Some(binding.clone());
    state.pressed.clear();
    state.capturing = false;
    state.last_fire = Some(Instant::now()); // suppress retrigger from same press
    if let Err(err) = app.emit("input-captured", binding) {
        log::warn!("listener: emit input-captured failed: {err}");
    }
}

fn is_modifier(k: &Key) -> bool {
    MODIFIERS.iter().any(|(_, keys)| keys.contains(k))
}

fn is_modifier_label(label: &str) -> bool {
    MODIFIERS.iter().any(|(name, _)| *name == label)
}

fn is_modifier_only_binding(b: Option<&InputBinding>) -> bool {
    match b {
        Some(InputBinding::Keyboard { keys }) => {
            !keys.is_empty() && keys.iter().all(|k| is_modifier_label(k))
        }
        _ => false,
    }
}

fn matches_keyboard(binding: Option<&InputBinding>, pressed: &HashSet<Key>) -> bool {
    let Some(InputBinding::Keyboard { keys }) = binding else {
        return false;
    };
    let modifiers_match = MODIFIERS.iter().all(|(name, variants)| {
        keys.iter().any(|k| k == name) == variants.iter().any(|v| pressed.contains(v))
    });
    modifiers_match
        && keys
            .iter()
            .filter(|label| !is_modifier_label(label))
            .all(|label| label_to_key(label).is_some_and(|k| pressed.contains(&k)))
}

fn matches_mouse(current: Option<&InputBinding>, btn: &Button) -> bool {
    let Some(InputBinding::Mouse { button }) = current else {
        return false;
    };
    button_label(btn).as_deref() == Some(button.as_str())
}

fn button_label(b: &Button) -> Option<String> {
    match b {
        Button::Right => Some("Right".to_string()),
        Button::Middle => Some("Middle".to_string()),
        Button::Unknown(n) => Some(format!("Side{n}")),
        _ => None, // Left intentionally excluded (would block normal clicks)
    }
}

// Round-trips with the frontend's labelFromCode; keymap.json pins both sides.

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

    #[test]
    fn keymap_json_coverage() {
        let keymap: Keymap =
            serde_json::from_str(include_str!("../keymap.json")).expect("keymap.json is valid JSON");

        for entry in &keymap.labels {
            let label = &entry.label;
            if is_modifier_label(label) {
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
