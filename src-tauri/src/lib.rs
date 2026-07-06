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
use tauri::image::Image;

use crate::tray_flash::TrayFlashState;

use crate::listener::{ListenerState, SharedState};

pub struct QuitFlag(pub AtomicBool);

fn decode_png_rgba(bytes: &[u8]) -> Result<Image<'static>, png::DecodingError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut reader = decoder.read_info()?;
    let buf_size = reader
        .output_buffer_size()
        .unwrap_or_else(|| reader.info().raw_bytes());
    let mut buf = vec![0u8; buf_size];
    let info = reader.next_frame(&mut buf)?;
    let raw = &buf[..info.buffer_size()];

    let rgba = match info.color_type {
        png::ColorType::Rgba => raw.to_vec(),
        png::ColorType::Rgb => raw
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        png::ColorType::Grayscale => raw
            .iter()
            .flat_map(|&g| [g, g, g, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => raw
            .chunks_exact(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Indexed => {
            return Err(png::DecodingError::IoError(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "indexed PNG output was not expanded",
            )));
        }
    };

    Ok(Image::new_owned(rgba, info.width, info.height))
}

pub fn run() {
    // Pick the initial URL from the saved host (if any). With no host, load
    // the local connect.html screen — the user enters their server there.
    let initial_url = match connection::load_host() {
        Some(host) => match connection::normalize_host(&host) {
            Ok(url) => WebviewUrl::External(url),
            Err(_) => WebviewUrl::App("connect.html".into()),
        },
        None => WebviewUrl::App("connect.html".into()),
    };

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

            // First run (no config file) → seed default and persist.
            // Existing file (even with `null`) → respect user's choice.
            let initial = match shortcut::load(&handle) {
                shortcut::LoadResult::Missing => {
                    let default = shortcut::InputBinding::default_combo();
                    if let Err(err) = shortcut::save(&handle, Some(&default)) {
                        log::warn!("seed default shortcut failed: {err}");
                    }
                    Some(default)
                }
                shortcut::LoadResult::Cleared => None,
                shortcut::LoadResult::Bound(b) => Some(b),
            };
            let state = Arc::new(Mutex::new(ListenerState::new(initial)));
            app.manage(state.clone());

            listener::start(handle.clone(), state);

            updater::init(&handle)?;

            let base_icon = tray::default_icon(&handle);

            static ALERT_PNG: &[u8] = include_bytes!("../icons/tray-alert.png");
            let alert_icon = decode_png_rgba(ALERT_PNG)
                .expect("bundled tray-alert.png failed to decode");

            app.manage(Arc::new(TrayFlashState::new(base_icon, alert_icon)));

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
