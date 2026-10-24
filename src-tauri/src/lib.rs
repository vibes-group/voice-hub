mod commands;
mod connection;
mod listener;
mod shortcut;
mod tray;
mod tray_flash;
mod updater;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::listener::{ListenerState, SharedState};
use crate::tray_flash::TrayFlashState;

pub struct QuitFlag(pub AtomicBool);

pub fn run() {
    let initial_url = connection::load_host()
        .and_then(|host| connection::normalize_host(&host).ok())
        .map_or_else(
            || WebviewUrl::App(connection::CONNECT_PATH.into()),
            WebviewUrl::External,
        );

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Warn)
                .build(),
        )
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            tray::show_main(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_shortcut,
            commands::set_shortcut,
            commands::clear_shortcut,
            commands::start_capture,
            commands::cancel_capture,
            connection::get_state,
            connection::set_host,
            connection::change_server,
            updater::apply_update,
            tray_flash::flash_attention,
        ])
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            match event {
                WindowEvent::Focused(focused) => {
                    if let Some(state) = window.try_state::<SharedState>() {
                        match state.lock() {
                            Ok(mut s) => {
                                s.window_focused = *focused;
                                // Drop any keys we believe are held: Windows
                                // suppresses rdev keyboard events under focus, so
                                // a release that happened across a focus
                                // transition can leave a stale key in `pressed`.
                                s.pressed.clear();
                            }
                            Err(err) => {
                                log::error!("focus event: state mutex poisoned: {err}");
                            }
                        }
                    }
                    if *focused {
                        if let Err(e) = tray_flash::revert_if_active(window.app_handle()) {
                            log::warn!("focus: tray_flash revert failed: {e}");
                        }
                        let app = window.app_handle().clone();
                        tauri::async_runtime::spawn(async move {
                            updater::check(app, /* force */ false).await;
                        });
                    }
                }
                WindowEvent::CloseRequested { api, .. } => {
                    let flag = window.state::<QuitFlag>();
                    if !flag.0.load(Ordering::SeqCst) {
                        api.prevent_close();
                        if let Err(err) = window.hide() {
                            log::warn!("close request: window.hide failed: {err}");
                        }
                    }
                }
                _ => {}
            }
        })
        .setup(move |app| {
            app.manage(QuitFlag(AtomicBool::new(false)));

            let start_hidden = std::env::args().any(|a| a == "--hidden");

            WebviewWindowBuilder::new(app, "main", initial_url)
                .title("Voice Hub")
                .inner_size(1440.0, 985.0)
                .resizable(false)
                .maximizable(false)
                .visible(!start_hidden)
                // Let the webview keep HTML5 drag-and-drop events (chat file
                // drop) instead of the native handler swallowing them.
                .disable_drag_drop_handler()
                .build()?;

            let handle = app.handle().clone();

            let binding = shortcut::load_or_seed(&handle);
            let state = Arc::new(Mutex::new(ListenerState::new(binding)));
            app.manage(state.clone());
            listener::start(handle.clone(), state);

            tray::init(&handle)?;
            updater::init(&handle);

            app.manage(TrayFlashState::new(
                tray::default_icon(&handle),
                tauri::include_image!("icons/tray-alert.png"),
            ));

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
