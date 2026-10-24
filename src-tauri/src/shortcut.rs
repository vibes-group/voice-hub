use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InputBinding {
    Keyboard { keys: Vec<String> },
    Mouse { button: String },
}

fn config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("config dir: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir config dir: {e}"))?;
    Ok(dir.join("shortcut.json"))
}

/// The persisted binding. A stored `null` is the user's explicit "no shortcut";
/// a missing or unreadable file is a first run and gets the default saved.
pub fn load_or_seed(app: &tauri::AppHandle) -> Option<InputBinding> {
    if let Some(stored) = load(app) {
        return stored;
    }
    let default = InputBinding::Keyboard {
        keys: vec!["Ctrl".into(), "Shift".into(), "M".into()],
    };
    if let Err(err) = save(app, Some(&default)) {
        log::warn!("seed default shortcut failed: {err}");
    }
    Some(default)
}

fn load(app: &tauri::AppHandle) -> Option<Option<InputBinding>> {
    let path = config_path(app)
        .inspect_err(|err| log::warn!("shortcut: resolve config path failed: {err}"))
        .ok()?;
    let raw = match fs::read(&path) {
        Ok(r) => r,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            log::warn!("shortcut: read {} failed: {err}", path.display());
            return None;
        }
    };
    serde_json::from_slice(&raw)
        .inspect_err(|err| log::warn!("shortcut: parse {} failed: {err}", path.display()))
        .ok()
}

pub fn save(app: &tauri::AppHandle, binding: Option<&InputBinding>) -> Result<(), String> {
    let path = config_path(app)?;
    let json = serde_json::to_vec_pretty(&binding).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixtures are the on-disk format contract.
    #[test]
    fn binding_fixtures_roundtrip() {
        let fixtures: &[(&str, &str)] = &[
            (
                "binding-keyboard-plain",
                include_str!("../testdata/binding-keyboard-plain.json"),
            ),
            (
                "binding-keyboard-modifiers",
                include_str!("../testdata/binding-keyboard-modifiers.json"),
            ),
            (
                "binding-mouse",
                include_str!("../testdata/binding-mouse.json"),
            ),
            (
                "binding-cleared",
                include_str!("../testdata/binding-cleared.json"),
            ),
        ];

        for (name, content) in fixtures {
            let parsed: Option<InputBinding> = serde_json::from_str(content)
                .unwrap_or_else(|e| panic!("fixture {name}: failed to parse: {e}"));
            let serialised = serde_json::to_string_pretty(&parsed)
                .unwrap_or_else(|e| panic!("fixture {name}: failed to serialise: {e}"));
            assert_eq!(
                serialised, *content,
                "fixture {name}: re-serialised output does not match fixture bytes\n\
                 got:  {serialised:?}\n\
                 want: {content:?}"
            );
        }
    }
}
