use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, TryStreamExt};
use reqwest::{Client, Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::chat::BlobCache;
use crate::protocol::{Attachment, ServerConfig, UPLOAD_MAX_BYTES, server_url};

#[derive(Clone)]
pub struct Api {
    pub server: Url,
    http: Client,
    cookie: Arc<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PasswordEntry {
    pub id: String,
    pub label: String,
    pub generation: u64,
    pub created_at: String,
    pub expires_at: Option<String>,
    #[serde(default)]
    pub expired: bool,
}
// Plaintext passwords must never be logged or persisted.
#[derive(Clone, Deserialize)]
pub struct CreatedPassword {
    pub host: String,
    pub id: String,
    pub label: String,
    pub password: String,
    pub generation: u64,
    pub created_at: String,
    pub expires_at: Option<String>,
    #[serde(default)]
    pub expired: bool,
}
impl CreatedPassword {
    pub fn invitation(&self) -> String {
        format!("Сервер: {}\nПароль: {}", self.host, self.password)
    }
}

impl Api {
    pub async fn presence(
        &self,
        events: mpsc::Sender<(String, Value)>,
        cancel: CancellationToken,
    ) -> Result<(), String> {
        let response = self
            .request(Method::GET, "api/presence")?
            .send()
            .await
            .map_err(|_| "Не удалось открыть список комнат")?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        loop {
            let chunk =
                tokio::select! {_=cancel.cancelled()=>return Ok(()),chunk=stream.next()=>chunk};
            let Some(chunk) = chunk else {
                return Err("Список комнат отключён".into());
            };
            let chunk = chunk.map_err(|_| "Список комнат отключён")?;
            buffer.extend_from_slice(&chunk);
            if buffer.len() > 1024 * 1024 {
                return Err("Слишком большой ответ присутствия".into());
            }
            while let Some(end) = buffer.windows(2).position(|w| w == b"\n\n") {
                let frame = String::from_utf8_lossy(&buffer[..end]);
                let event = frame.lines().find_map(|l| l.strip_prefix("event: "));
                let data = frame.lines().find_map(|l| l.strip_prefix("data: "));
                if let (Some(event), Some(data)) = (event, data)
                    && let Ok(data) = serde_json::from_str(data)
                    && events.send((event.into(), data)).await.is_err()
                {
                    return Ok(());
                }
                buffer.drain(..end + 2);
            }
        }
    }
    fn client() -> Result<Client, String> {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "Не удалось настроить HTTPS".into())
    }
    pub async fn login(server: &str, mut password: String) -> Result<Self, String> {
        let server = server_url(server)?;
        let http = Self::client()?;
        let response = http
            .post(server.join("api/login").map_err(|_| "Некорректный адрес")?)
            .form(&[("password", password.as_str())])
            .timeout(Duration::from_secs(15))
            .send()
            .await;
        password.clear();
        let response =
            response.map_err(|_| "Сервер недоступен. Проверьте адрес и HTTPS-сертификат")?;
        match response.status().as_u16() {
            204 => {}
            401 => return Err("Неверный пароль".into()),
            429 => return Err("Слишком много попыток. Повторите позже".into()),
            _ => return Err("Сервер отклонил вход".into()),
        }
        let cookie = response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|h| h.to_str().ok())
            .find_map(|h| {
                h.split(';')
                    .next()
                    .filter(|s| s.starts_with("vh_session="))
                    .map(str::to_owned)
            })
            .ok_or("Сервер не выдал сессию")?;
        let api = Self {
            server,
            http,
            cookie: Arc::new(cookie),
        };
        api.config().await?;
        api.save_session()?;
        Ok(api)
    }
    pub async fn restore(server: &str) -> Result<Option<Self>, String> {
        let server = server_url(server)?;
        let Some(cookie) = read_session(server.as_str())? else {
            return Ok(None);
        };
        let api = Self {
            server,
            http: Self::client()?,
            cookie: Arc::new(cookie),
        };
        match api.config().await {
            Ok(_) => Ok(Some(api)),
            Err(e) => Err(e),
        }
    }
    pub fn cookie(&self) -> &str {
        &self.cookie
    }
    fn request(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder, String> {
        let url = self
            .server
            .join(path)
            .map_err(|_| "Некорректный адрес API")?;
        if url.origin() != self.server.origin() {
            return Err("Запрещён другой сервер API".into());
        }
        Ok(self
            .http
            .request(method, url)
            .header(reqwest::header::COOKIE, self.cookie.as_str()))
    }
    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T, String> {
        let mut request = self.request(method, path)?.timeout(Duration::from_secs(20));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Ошибка соединения с сервером")?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        let bytes = response.bytes().await.map_err(|_| "Сервер прервал ответ")?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err("Слишком большой ответ сервера".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "Некорректный ответ сервера".into())
    }
    pub async fn config(&self) -> Result<ServerConfig, String> {
        self.json(Method::GET, "api/config", None).await
    }
    pub async fn version(&self) -> Result<String, String> {
        let value: Value = self.json(Method::GET, "api/version", None).await?;
        Ok(value["version"].as_str().unwrap_or_default().into())
    }
    pub async fn logout(&self) -> Result<(), String> {
        let result = self
            .request(Method::POST, "api/logout")?
            .timeout(Duration::from_secs(10))
            .send()
            .await;
        delete_session(self.server.as_str())?;
        result.map_err(|_| "Сессия удалена локально; сервер недоступен")?;
        Ok(())
    }
    pub async fn passwords(&self) -> Result<Vec<PasswordEntry>, String> {
        #[derive(Deserialize)]
        struct Entries {
            entries: Vec<PasswordEntry>,
        }
        Ok(self
            .json::<Entries>(Method::GET, "api/admin/connection-passwords", None)
            .await?
            .entries)
    }
    pub async fn create_password(&self, label: &str, ttl: i64) -> Result<CreatedPassword, String> {
        self.json(
            Method::POST,
            "api/admin/connection-passwords",
            Some(json!({"label":label.trim(),"ttl_seconds":ttl})),
        )
        .await
    }
    pub async fn rotate_password(&self, id: &str) -> Result<CreatedPassword, String> {
        self.json(Method::POST, &admin_path(id, "rotate")?, None)
            .await
    }
    pub async fn edit_password(&self, id: &str, action: &str, body: Value) -> Result<(), String> {
        let path = admin_path(id, action)?;
        self.empty(Method::POST, &path, Some(body)).await
    }
    pub async fn revoke_password(&self, id: &str) -> Result<(), String> {
        self.empty(Method::DELETE, &admin_path(id, "")?, None).await
    }
    pub async fn disconnect_users(&self) -> Result<(), String> {
        self.empty(
            Method::POST,
            "api/admin/connection-passwords/disconnect-users",
            None,
        )
        .await
    }
    async fn empty(&self, method: Method, path: &str, body: Option<Value>) -> Result<(), String> {
        let mut request = self.request(method, path)?.timeout(Duration::from_secs(20));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Ошибка соединения с сервером")?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(status_error(response.status()))
        }
    }
    pub async fn upload(
        &self,
        path: &Path,
        room: &str,
        progress: mpsc::Sender<f32>,
        cancel: CancellationToken,
    ) -> Result<Attachment, String> {
        validate_room(room)?;
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|_| "Не удалось открыть вложение")?;
        let size = file
            .metadata()
            .await
            .map_err(|_| "Не удалось прочитать размер вложения")?
            .len();
        if size > UPLOAD_MAX_BYTES {
            return Err("Размер файла превышает 100 МБ".into());
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_owned();
        let mime = mime_for(&name);
        let mut sent = 0;
        let stream =
            tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024).map_ok(move |bytes| {
                sent += bytes.len() as u64;
                let _ = progress.try_send(if size == 0 {
                    1.0
                } else {
                    sent as f32 / size as f32
                });
                bytes
            });
        let encoded: String = url::form_urlencoded::byte_serialize(name.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        let request = self
            .request(Method::POST, &format!("api/upload?room={room}"))?
            .header(reqwest::header::CONTENT_TYPE, mime)
            .header(reqwest::header::CONTENT_LENGTH, size)
            .header(
                reqwest::header::CONTENT_DISPOSITION,
                format!("attachment; filename*=UTF-8''{encoded}"),
            )
            .body(reqwest::Body::wrap_stream(stream));
        let response = tokio::select! {_=cancel.cancelled()=>return Err("Загрузка отменена".into()),result=request.send()=>result.map_err(|_|"Не удалось загрузить файл")?};
        if response.status() != StatusCode::CREATED {
            return Err(status_error(response.status()));
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Uploaded {
            upload_id: String,
        }
        let uploaded: Uploaded = response
            .json()
            .await
            .map_err(|_| "Некорректный ответ загрузки")?;
        let mut attachment = Attachment {
            upload_id: uploaded.upload_id,
            kind: if mime.starts_with("image/") {
                "image"
            } else {
                "file"
            }
            .into(),
            name,
            mime: mime.into(),
            size,
            ..Default::default()
        };
        if attachment.kind == "image" {
            let path = path.to_owned();
            if let Ok(Some((width, height, thumb))) =
                tokio::task::spawn_blocking(move || image_metadata(&path)).await
            {
                attachment.width = Some(width);
                attachment.height = Some(height);
                attachment.blur_thumb = Some(thumb);
            }
        }
        Ok(attachment)
    }
    pub async fn attachment(
        &self,
        attachment: &Attachment,
        room: &str,
        cache: &BlobCache,
        cancel: CancellationToken,
    ) -> Result<Option<std::path::PathBuf>, String> {
        validate_room(room)?;
        let path = cache.path(&attachment.upload_id)?;
        if path.is_file() {
            return Ok(Some(path));
        }
        let response = self
            .request(
                Method::GET,
                &format!("api/file/{}?room={room}", attachment.upload_id),
            )?
            .send()
            .await
            .map_err(|_| "Не удалось скачать файл")?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > UPLOAD_MAX_BYTES)
        {
            return Err("Файл превышает допустимый размер".into());
        }
        let temp = path.with_extension(format!("{}.part", uuid::Uuid::new_v4()));
        let result=async {
            use tokio::io::AsyncWriteExt;
            let mut file=tokio::fs::File::create(&temp).await.map_err(|_|"Не удалось создать файл")?;
            let mut stream=response.bytes_stream();let mut received=0;
            loop {let chunk=tokio::select!{_=cancel.cancelled()=>return Err("Загрузка отменена".to_owned()),chunk=stream.next()=>chunk};let Some(chunk)=chunk else{break};let chunk=chunk.map_err(|_|"Скачивание прервано")?;received+=chunk.len() as u64;if received>UPLOAD_MAX_BYTES{return Err("Файл превышает допустимый размер".into());}file.write_all(&chunk).await.map_err(|_|"Не удалось сохранить файл")?;}
            file.sync_all().await.map_err(|_|"Не удалось сохранить файл")?;drop(file);
            tokio::fs::rename(&temp,&path).await.map_err(|_|"Не удалось сохранить файл в кеше")?;Ok(())
        }.await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(temp).await;
        }
        result?;
        Ok(Some(path))
    }
    fn save_session(&self) -> Result<(), String> {
        write_session(self.server.as_str(), self.cookie.as_str())
    }
}

fn admin_path(id: &str, action: &str) -> Result<String, String> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("Некорректный идентификатор пароля".into());
    }
    if !matches!(action, "" | "rotate" | "rename" | "ttl") {
        return Err("Некорректная операция".into());
    }
    Ok(format!(
        "api/admin/connection-passwords/{id}{}{}",
        if action.is_empty() { "" } else { "/" },
        action
    ))
}
fn validate_room(room: &str) -> Result<(), String> {
    if matches!(room, "room1" | "room2" | "room3") {
        Ok(())
    } else {
        Err("Неизвестная комната".into())
    }
}
fn status_error(status: StatusCode) -> String {
    match status.as_u16() {
        401 => "Сессия истекла. Войдите снова",
        403 => "Недостаточно прав",
        409 => "Достигнут лимит паролей",
        413 => "Размер файла превышает 100 МБ",
        429 => "Слишком много запросов. Повторите позже",
        507 => "Хранилище сервера заполнено",
        _ => "Сервер отклонил запрос",
    }
    .into()
}
pub fn mime_for(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "avif" => "image/avif",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}
fn image_metadata(path: &Path) -> Option<(u32, u32, String)> {
    use base64::Engine;
    let mut reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let thumbnail = image.thumbnail(20, 20).to_rgb8();
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 50)
        .encode_image(&thumbnail)
        .ok()?;
    Some((
        image.width(),
        image.height(),
        format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ),
    ))
}
#[cfg(target_os = "windows")]
fn keyring(server: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(
        "VoiceHub-GPUI",
        &crate::settings::Storage::namespace(server, "session"),
    )
    .map_err(|_| "Не удалось открыть хранилище учётных данных Windows".into())
}
#[cfg(target_os = "windows")]
fn write_session(server: &str, cookie: &str) -> Result<(), String> {
    keyring(server)?
        .set_password(cookie)
        .map_err(|_| "Не удалось сохранить сессию в защищённом хранилище Windows".into())
}
#[cfg(target_os = "windows")]
fn read_session(server: &str) -> Result<Option<String>, String> {
    match keyring(server)?.get_password() {
        Ok(cookie) => Ok(Some(cookie)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("Не удалось прочитать защищённую сессию".into()),
    }
}
#[cfg(target_os = "windows")]
fn delete_session(server: &str) -> Result<(), String> {
    match keyring(server)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err("Не удалось удалить защищённую сессию".into()),
    }
}
#[cfg(not(target_os = "windows"))]
fn write_session(_: &str, _: &str) -> Result<(), String> {
    Ok(())
}
#[cfg(not(target_os = "windows"))]
fn read_session(_: &str) -> Result<Option<String>, String> {
    Ok(None)
}
#[cfg(not(target_os = "windows"))]
fn delete_session(_: &str) -> Result<(), String> {
    Ok(())
}
