// Host stored in OS keychain; password never persisted (exchanged for a
// session cookie by the webview's login form).
//
// We do NOT call `clear_all_browsing_data` on disconnect/change_server: on
// WebView2 it wipes localStorage for every origin, killing displayName,
// clientId and per-peer prefs. Cookie expires server-side.

use keyring::Entry;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use url::Url;

const KEYRING_SERVICE: &str = "voice-hub";
const KEYRING_USER: &str = "host";

const CONNECT_PATH: &str = "connect.html";

fn keyring_entry() -> Result<Entry, String> {
    Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| format!("keyring init: {e}"))
}

/// Read the saved host. None if no host stored or keychain unavailable.
pub fn load_host() -> Option<String> {
    let entry = match keyring_entry() {
        Ok(e) => e,
        Err(err) => {
            log::warn!("keyring init failed: {err}");
            return None;
        }
    };
    match entry.get_password() {
        Ok(host) if host.is_empty() => None,
        Ok(host) => Some(host),
        Err(keyring::Error::NoEntry) => None,
        Err(err) => {
            log::warn!("keyring read failed: {err}");
            None
        }
    }
}

fn save_host(host: &str) -> Result<(), String> {
    keyring_entry()?
        .set_password(host)
        .map_err(|e| format!("keyring write: {e}"))
}

fn delete_host() {
    match keyring_entry() {
        Ok(entry) => {
            if let Err(err) = entry.delete_credential() {
                if !matches!(err, keyring::Error::NoEntry) {
                    log::warn!("keyring delete failed: {err}");
                }
            }
        }
        Err(err) => log::warn!("keyring init for delete failed: {err}"),
    }
}

/// Normalize "vh.example.com" / "https://vh.example.com:8443" into an absolute
/// URL with an explicit scheme. HTTP is permitted only for localhost.
pub fn normalize_host(input: &str) -> Result<Url, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Введите адрес сервера".into());
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let url = Url::parse(&candidate).map_err(|e| format!("Неверный адрес: {e}"))?;
    match url.scheme() {
        "https" => Ok(url),
        "http" => {
            let host = url.host_str().unwrap_or("");
            if host == "localhost" || host == "127.0.0.1" || host == "[::1]" {
                Ok(url)
            } else {
                Err("HTTP разрешён только для localhost. Используйте HTTPS.".into())
            }
        }
        other => Err(format!("Неподдерживаемая схема: {other}")),
    }
}

/// Tauri's local content origin pointing at connect.html. Differs by platform;
/// constructed at runtime so navigate() can return there from a remote page.
fn connect_url() -> Url {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let base = "tauri://localhost/";
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    let base = "http://tauri.localhost/";
    // Both bases are compile-time-constant valid URL prefixes and CONNECT_PATH
    // is a plain filename, so parsing cannot fail in practice.
    Url::parse(&format!("{base}{CONNECT_PATH}"))
        .unwrap_or_else(|e| unreachable!("connect_url: constant base + path failed to parse: {e}"))
}

#[derive(Serialize)]
pub struct ConnectionState {
    pub has_host: bool,
    pub host: Option<String>,
}

#[tauri::command]
pub fn get_state() -> ConnectionState {
    let host = load_host();
    ConnectionState {
        has_host: host.is_some(),
        host,
    }
}

/// Save host and navigate the main webview to it.
#[tauri::command]
pub fn set_host(app: AppHandle, host: String) -> Result<(), String> {
    let url = normalize_host(&host)?;
    save_host(url.as_str())?;
    navigate_to(&app, url)
}

#[tauri::command]
pub fn disconnect(app: AppHandle) -> Result<(), String> {
    delete_host();
    navigate_to(&app, connect_url())
}

#[tauri::command]
pub fn change_server(app: AppHandle) -> Result<(), String> {
    navigate_to(&app, connect_url())
}

fn navigate_to(app: &AppHandle, url: Url) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    window.navigate(url).map_err(|e| format!("navigate: {e}"))?;
    if let Err(err) = window.show() {
        log::warn!("navigate_to: window.show failed: {err}");
    }
    if let Err(err) = window.set_focus() {
        log::warn!("navigate_to: window.set_focus failed: {err}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize_host;

    fn ok(input: &str) {
        assert!(
            normalize_host(input).is_ok(),
            "expected Ok for {:?}, got Err",
            input
        );
    }

    fn err(input: &str) {
        assert!(
            normalize_host(input).is_err(),
            "expected Err for {:?}, got Ok",
            input
        );
    }

    #[test]
    fn https_host_accepted() {
        ok("https://voicehub.example.com");
    }

    #[test]
    fn https_host_with_port_accepted() {
        ok("https://voicehub.example.com:443");
    }

    #[test]
    fn http_localhost_accepted() {
        ok("http://localhost");
    }

    #[test]
    fn http_loopback_ipv4_accepted() {
        ok("http://127.0.0.1");
    }

    #[test]
    fn http_non_local_rejected() {
        err("http://example.com");
    }

    #[test]
    fn bare_hostname_defaults_to_https() {
        let url = normalize_host("voicehub.example.com").expect("bare hostname should be Ok");
        assert_eq!(url.scheme(), "https");
    }

    #[test]
    fn bare_hostname_with_port_accepted() {
        let url = normalize_host("host:8080").expect("bare hostname:port should be Ok");
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.port(), Some(8080));
    }

    #[test]
    fn garbage_input_rejected() {
        err("not a url !!@@##");
    }

    #[test]
    fn empty_input_rejected() {
        err("");
        err("   ");
    }

    #[test]
    fn trailing_slash_path_stripped() {
        // normalize_host does not strip the path — it validates scheme and host.
        // A trailing slash is valid and kept. Verify the result is Ok and the
        // host is correct.
        let url = normalize_host("https://voicehub.example.com/").expect("trailing slash Ok");
        assert_eq!(url.host_str(), Some("voicehub.example.com"));
    }
}
