use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};
use url::Url;

use crate::connection;
use crate::tray;

const INTERVAL: Duration = Duration::from_secs(60 * 60);
const FOCUS_THROTTLE: Duration = Duration::from_secs(15 * 60);
const PROGRESS_EMIT_THROTTLE: Duration = Duration::from_millis(100);
const SERVER_TIMEOUT: Duration = Duration::from_secs(8);

// Event payloads, mirrored in frontend/src/ipc.ts.
#[derive(Serialize, Clone)]
struct UpdateAvailablePayload {
    version: String,
}

#[derive(Serialize, Clone)]
struct UpdateProgressPayload {
    downloaded: u64,
    total: Option<u64>,
}

#[derive(Serialize, Clone)]
struct UpdateInstallingPayload {}

#[derive(Serialize, Clone)]
struct UpdateErrorPayload {
    message: String,
}

#[derive(Default)]
struct UpdaterState {
    last_checked: Option<Instant>,
    pending: Option<Update>,
    installing: bool,
}

type SharedUpdater = Arc<Mutex<UpdaterState>>;

/// Starts the startup check and the hourly loop. The tray must already exist.
pub fn init(app: &AppHandle) {
    app.manage(SharedUpdater::default());

    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        check(h, /* force */ true).await;
    });

    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(INTERVAL).await;
            check(h.clone(), /* force */ false).await;
        }
    });
}

pub fn pending_version(app: &AppHandle) -> Option<String> {
    let state = app.try_state::<SharedUpdater>()?;
    let s = state.lock().ok()?;
    s.pending.as_ref().map(|u| u.version.clone())
}

fn server_endpoint(host: &str) -> Result<Url, String> {
    let mut endpoint = connection::normalize_host(host)?;
    endpoint.set_path("/desktop/latest.json");
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Ok(endpoint)
}

async fn discover_update(app: &AppHandle) -> Result<Option<Update>, String> {
    let Some(host) = connection::load_host() else {
        return Ok(None);
    };
    app.updater_builder()
        .endpoints(vec![server_endpoint(&host)?])
        .map_err(|e| format!("configure endpoint: {e}"))?
        .timeout(SERVER_TIMEOUT)
        .build()
        .map_err(|e| format!("build updater: {e}"))?
        .check()
        .await
        .map_err(|e| format!("check: {e}"))
}

/// Check for an update; `force` skips the focus throttle.
pub async fn check(app: AppHandle, force: bool) {
    let shared: SharedUpdater = match app.try_state::<SharedUpdater>() {
        Some(s) => s.inner().clone(),
        None => return,
    };

    match shared.lock() {
        Ok(mut s) => {
            if !force && s.last_checked.is_some_and(|prev| prev.elapsed() < FOCUS_THROTTLE) {
                return;
            }
            s.last_checked = Some(Instant::now());
        }
        Err(err) => log::error!("updater: state mutex poisoned (last_checked): {err}"),
    }

    let update = match discover_update(&app).await {
        Ok(Some(u)) => u,
        Ok(None) => return,
        Err(err) => {
            log::error!("updater: check failed: {err}");
            return;
        }
    };

    let version = update.version.clone();
    match shared.lock() {
        Ok(mut s) => {
            let already_known = s.pending.as_ref().is_some_and(|u| u.version == version);
            s.pending = Some(update);
            if already_known {
                return;
            }
        }
        Err(err) => {
            log::error!("updater: state mutex poisoned (pending): {err}");
            return;
        }
    }

    if let Err(err) = app.emit(
        "update-available",
        UpdateAvailablePayload {
            version: version.clone(),
        },
    ) {
        log::warn!("updater: emit update-available failed: {err}");
    }
    if let Err(err) = tray::set_update_available(&app, &version) {
        log::error!("updater: tray rebuild failed: {err}");
    }
}

async fn install(update: Update, app: &AppHandle) -> Result<(), String> {
    let progress_app = app.clone();
    let installing_app = app.clone();
    let mut downloaded: u64 = 0;
    let mut last_emit = Instant::now() - PROGRESS_EMIT_THROTTLE;

    update
        .download_and_install(
            move |chunk_len, content_len| {
                downloaded += chunk_len as u64;
                let done = content_len.is_some_and(|c| downloaded >= c);
                if !done && last_emit.elapsed() < PROGRESS_EMIT_THROTTLE {
                    return;
                }
                last_emit = Instant::now();
                let payload = UpdateProgressPayload {
                    downloaded,
                    total: content_len,
                };
                if let Err(err) = progress_app.emit("update-progress", payload) {
                    log::warn!("updater: emit update-progress failed: {err}");
                }
            },
            move || {
                if let Err(err) =
                    installing_app.emit("update-installing", UpdateInstallingPayload {})
                {
                    log::warn!("updater: emit update-installing failed: {err}");
                }
            },
        )
        .await
        .map_err(|e| format!("install: {e}"))
}

#[tauri::command]
pub async fn apply_update(app: AppHandle) -> Result<(), String> {
    let update = {
        let state = app
            .try_state::<SharedUpdater>()
            .ok_or_else(|| "updater state missing".to_string())?;
        let mut s = state.lock().map_err(|e| format!("lock: {e}"))?;
        if s.installing {
            return Err("install already in progress".to_string());
        }
        let update = s.pending.take().ok_or_else(|| "no pending update".to_string())?;
        s.installing = true;
        update
    };

    if let Err(err) = install(update, &app).await {
        log::error!("updater: install failed: {err}");
        if let Some(state) = app.try_state::<SharedUpdater>() {
            match state.lock() {
                Ok(mut s) => s.installing = false,
                Err(err) => log::error!("updater: state mutex poisoned (install reset): {err}"),
            }
        }
        let payload = UpdateErrorPayload {
            message: err.clone(),
        };
        if let Err(emit_err) = app.emit("update-error", payload) {
            log::warn!("updater: emit update-error failed: {emit_err}");
        }
        // Re-discover the update so tray + banner can offer a retry.
        let h = app.clone();
        tauri::async_runtime::spawn(async move {
            check(h, true).await;
        });
        return Err(err);
    }

    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_endpoint_uses_selected_origin() {
        let endpoint = server_endpoint("https://voice.example.com:8443/old?x=1")
            .expect("selected host should produce an endpoint");
        assert_eq!(
            endpoint.as_str(),
            "https://voice.example.com:8443/desktop/latest.json"
        );
    }
}
