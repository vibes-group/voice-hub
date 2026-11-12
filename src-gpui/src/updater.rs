use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{protocol::server_url, settings::Storage};

pub enum UpdateAction {
    Check { server: String, force: bool },
    Install,
}
pub enum UpdateEvent {
    Checking,
    Available { version: String, notes: String },
    Progress { received: u64, total: Option<u64> },
    Ready(PathBuf),
    Current,
    Error(String),
}
#[derive(Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub platforms: std::collections::BTreeMap<String, Platform>,
}
#[derive(Clone, Deserialize)]
pub struct Platform {
    pub url: String,
    pub signature: String,
}
pub struct Updater {
    pub actions: mpsc::Sender<UpdateAction>,
    pub events: Option<mpsc::Receiver<UpdateEvent>>,
    stop: CancellationToken,
}
impl Drop for Updater {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl Updater {
    pub fn start(storage: Storage) -> Self {
        let (actions, mut rx) = mpsc::channel(8);
        let (events, erx) = mpsc::channel(16);
        let stop = CancellationToken::new();
        let cancelled = stop.clone();
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move{
                let mut server=String::new();let mut last:Option<Instant>=None;let mut pending:Option<(String,Manifest)>=None;let mut interval=tokio::time::interval_at(tokio::time::Instant::now()+Duration::from_secs(3600),Duration::from_secs(3600));
                loop {
                    let action=tokio::select!{_=cancelled.cancelled()=>break,action=rx.recv()=>{let Some(action)=action else{break};action},_=interval.tick()=>UpdateAction::Check{server:server.clone(),force:true}};
                    match action {
                        UpdateAction::Check{server:next,force}=>{
                            let changed=server!=next;if changed{pending=None;last=None;}server=next;if server.is_empty(){continue;}
                            if !force&&last.is_some_and(|at|at.elapsed()<Duration::from_secs(900)){continue;}last=Some(Instant::now());
                            if force{let _=events.send(UpdateEvent::Checking).await;}
                            let result=tokio::select!{_=cancelled.cancelled()=>break,result=discover(&server)=>result};match result{Ok(Some(manifest))=>{let _=events.send(UpdateEvent::Available{version:manifest.version.clone(),notes:manifest.notes.clone()}).await;pending=Some((server.clone(),manifest));},Ok(None)=>{if force{let _=events.send(UpdateEvent::Current).await;}},Err(error)=>{if force{let _=events.send(UpdateEvent::Error(error)).await;}}}
                        },
                        UpdateAction::Install=>{if let Some((server,manifest))=&pending{let result=tokio::select!{_=cancelled.cancelled()=>break,result=download(server,manifest,&storage,&events)=>result};let event=match result{Ok(path)=>UpdateEvent::Ready(path),Err(error)=>UpdateEvent::Error(error)};let _=events.send(event).await;}}
                    }
                }
            });
        });
        Self {
            actions,
            events: Some(erx),
            stop,
        }
    }
}
async fn discover(server: &str) -> Result<Option<Manifest>, String> {
    let server = server_url(server)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "Не удалось настроить обновления")?;
    let response = client
        .get(
            server
                .join("desktop/gpui-latest.json")
                .map_err(|_| "Некорректный сервер обновлений")?,
        )
        .send()
        .await
        .map_err(|_| "Сервер обновлений недоступен")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err("Сервер отклонил проверку обновлений".into());
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "Ответ обновления прерван")?;
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err("Слишком большой манифест обновления".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|_| "Некорректный манифест GPUI-обновления")?;
    let next = semver::Version::parse(manifest.version.trim_start_matches('v'))
        .map_err(|_| "Некорректная версия обновления")?;
    let current = semver::Version::parse(
        option_env!("APP_RELEASE_VERSION")
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .trim_start_matches('v'),
    )
    .map_err(|_| "Некорректная версия приложения")?;
    if next <= current {
        return Ok(None);
    }
    if !manifest.platforms.contains_key("windows-x86_64") {
        return Err("Нет сборки для этой платформы".into());
    }
    Ok(Some(manifest))
}
async fn download(
    server: &str,
    manifest: &Manifest,
    storage: &Storage,
    events: &mpsc::Sender<UpdateEvent>,
) -> Result<PathBuf, String> {
    let server = server_url(server)?;
    let platform = manifest
        .platforms
        .get("windows-x86_64")
        .ok_or("Нет сборки Windows")?;
    let filename = format!(
        "Voice-Hub-GPUI_v{}_windows-x86_64.exe",
        manifest.version.trim_start_matches('v')
    );
    let url = server
        .join(&platform.url)
        .map_err(|_| "Некорректный адрес обновления")?;
    if url.origin() != server.origin()
        || !url.path().starts_with("/desktop/files/")
        || url.path_segments().and_then(|segments| segments.last()) != Some(filename.as_str())
    {
        return Err("Обновление должно загружаться с вашего сервера".into());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|_| "Не удалось настроить загрузку обновления")?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "Не удалось скачать обновление")?;
    if !response.status().is_success() {
        return Err("Сервер отклонил загрузку обновления".into());
    }
    let total = response.content_length();
    if total.is_some_and(|n| n > 300 * 1024 * 1024) {
        return Err("Обновление слишком большое".into());
    }
    let folder = storage.root().join("updates");
    tokio::fs::create_dir_all(&folder)
        .await
        .map_err(|_| "Не удалось создать папку обновления")?;
    let path = folder.join(format!("{}.exe", uuid::Uuid::new_v4()));
    let result: Result<(), String> = async {
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(|_| "Не удалось сохранить обновление")?;
        let key = public_key()?;
        let signature = decode_signature(&platform.signature)?;
        if signature.trusted_comment() != format!("Voice Hub GPUI {filename}") {
            return Err("Подпись относится к другой версии GPUI".into());
        }
        let mut verifier = key
            .verify_stream(&signature)
            .map_err(|_| "Неподдерживаемая подпись обновления")?;
        let mut stream = response.bytes_stream();
        let mut received = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "Загрузка обновления прервана")?;
            received += chunk.len() as u64;
            if received > 300 * 1024 * 1024 {
                return Err("Обновление слишком большое".into());
            }
            verifier.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|_| "Не удалось записать обновление")?;
            let _ = events.try_send(UpdateEvent::Progress { received, total });
        }
        verifier
            .finalize()
            .map_err(|_| "Подпись обновления недействительна")?;
        file.sync_all()
            .await
            .map_err(|_| "Не удалось сохранить обновление")?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
    }
    result?;
    Ok(path)
}
fn public_key() -> Result<minisign_verify::PublicKey, String> {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../src-tauri/tauri.conf.json"))
            .map_err(|_| "Нет ключа обновлений")?;
    let encoded = config["plugins"]["updater"]["pubkey"]
        .as_str()
        .ok_or("Нет ключа обновлений")?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Некорректный ключ обновлений")?;
    minisign_verify::PublicKey::decode(
        std::str::from_utf8(&decoded).map_err(|_| "Некорректный ключ обновлений")?,
    )
    .map_err(|_| "Некорректный ключ обновлений".into())
}
fn decode_signature(encoded: &str) -> Result<minisign_verify::Signature, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| "Некорректная подпись обновления")?;
    minisign_verify::Signature::decode(
        std::str::from_utf8(&bytes).map_err(|_| "Некорректная подпись обновления")?,
    )
    .map_err(|_| "Некорректная подпись обновления".into())
}

/// The helper waits for the current process, replaces the signed executable and relaunches.
#[cfg(target_os = "windows")]
pub fn install(path: &std::path::Path, storage: &Storage) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let executable = std::env::current_exe().map_err(|_| "Не найден путь приложения")?;
    let script = storage.root().join("install-update.ps1");
    std::fs::write(&script,r#"$ErrorActionPreference = 'Stop'
$old = $env:UPDATE_TARGET
$new = $env:UPDATE_SOURCE
$backup = "$old.previous"
Wait-Process -Id ([int]$env:UPDATE_PID) -ErrorAction SilentlyContinue
try {
  Move-Item -LiteralPath $old -Destination $backup -Force
  try { Move-Item -LiteralPath $new -Destination $old -Force } catch { Move-Item -LiteralPath $backup -Destination $old -Force; throw }
  Start-Process -FilePath $old
  Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
} catch { Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show('Не удалось установить обновление. Исходная версия сохранена.', 'Voice Hub') | Out-Null }
"#).map_err(|_|"Не удалось подготовить установку")?;
    let powershell = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or("Не найдена папка Windows")?
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    std::process::Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .env("UPDATE_TARGET", executable)
        .env("UPDATE_SOURCE", path)
        .env("UPDATE_PID", std::process::id().to_string())
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|_| "Не удалось запустить установку обновления")?;
    Ok(())
}
#[cfg(not(target_os = "windows"))]
pub fn install(_: &std::path::Path, _: &Storage) -> Result<(), String> {
    Err("Автоматическое обновление доступно в сборке Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_update_key_is_valid_and_unsigned_payloads_are_rejected() {
        let key = public_key().unwrap();
        assert!(decode_signature("bm90IGEgc2lnbmF0dXJl").is_err());
        let _ = key;
    }
}
