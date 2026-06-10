use std::time::Instant;

use tauri::{AppHandle, State};

use crate::listener::{Mode, SharedState};
use crate::shortcut::{self, InputBinding};

#[tauri::command]
pub fn get_shortcut(state: State<'_, SharedState>) -> Option<InputBinding> {
    state.lock().ok().and_then(|s| s.current.clone())
}

#[tauri::command]
pub fn set_shortcut(
    app: AppHandle,
    state: State<'_, SharedState>,
    binding: InputBinding,
) -> Result<(), String> {
    shortcut::save(&app, Some(&binding))?;
    let mut s = state.lock().map_err(|e| format!("lock: {e}"))?;
    s.current = Some(binding);
    s.last_fire = Some(Instant::now());
    Ok(())
}

#[tauri::command]
pub fn clear_shortcut(app: AppHandle, state: State<'_, SharedState>) -> Result<(), String> {
    shortcut::save(&app, None)?;
    let mut s = state.lock().map_err(|e| format!("lock: {e}"))?;
    s.current = None;
    s.mode = Mode::Normal;
    s.pressed.clear();
    s.last_fire = Some(Instant::now());
    Ok(())
}

#[tauri::command]
pub fn start_capture(state: State<'_, SharedState>) {
    match state.lock() {
        Ok(mut s) => {
            s.mode = Mode::Capturing;
            s.pressed.clear();
        }
        Err(err) => log::error!("start_capture: state mutex poisoned: {err}"),
    }
}

#[tauri::command]
pub fn cancel_capture(state: State<'_, SharedState>) {
    match state.lock() {
        Ok(mut s) => {
            s.mode = Mode::Normal;
            s.pressed.clear();
        }
        Err(err) => log::error!("cancel_capture: state mutex poisoned: {err}"),
    }
}
