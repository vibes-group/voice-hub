use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub self_muted: bool,
    #[serde(default)]
    pub deafened: bool,
    #[serde(default)]
    pub chat_only: bool,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub screen_sharing: bool,
    #[serde(default)]
    pub screen_sharing_has_audio: bool,
    #[serde(default)]
    pub screen_sharing_video_codec: Option<String>,
}

impl Peer {
    pub fn preference_key(&self) -> &str {
        if self.client_id.is_empty() {
            &self.id
        } else {
            &self.client_id
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub upload_id: String,
    pub kind: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blur_thumb: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub from: String,
    pub text: String,
    pub ts: u64,
    #[serde(default)]
    pub client_msg_id: String,
    #[serde(default)]
    pub sender_name: String,
    #[serde(default)]
    pub sender_client_id: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub deleted_upload_ids: Vec<String>,
    #[serde(default)]
    pub pending: bool,
    #[serde(skip)]
    pub upload_progress: f32,
    #[serde(skip)]
    pub failed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_files: Vec<std::path::PathBuf>,
}

pub const CHAT_MAX_BYTES: usize = 2000;
pub const CHAT_MAX_ATTACHMENTS: usize = 10;
pub const UPLOAD_MAX_BYTES: u64 = 100 * 1024 * 1024;

pub fn validate_chat(text: &str, attachments: &[Attachment]) -> Result<(), String> {
    if text.trim().is_empty() && attachments.is_empty() {
        return Err("Введите сообщение или прикрепите файл".into());
    }
    if text.len() > CHAT_MAX_BYTES {
        return Err("Сообщение длиннее 2000 байт".into());
    }
    if attachments.len() > CHAT_MAX_ATTACHMENTS {
        return Err("Можно прикрепить до 10 файлов".into());
    }
    Ok(())
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub ice_servers: Vec<IceServer>,
    pub role: String,
}

// TURN credentials are intentionally excluded from Debug and persisted settings.
#[derive(Clone, Default, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub credential: String,
}

#[derive(Deserialize)]
pub struct Message {
    pub event: String,
    #[serde(default)]
    pub data: Value,
}

pub fn envelope(event: &str, data: impl Serialize) -> String {
    json!({ "event": event, "data": data }).to_string()
}

/// Credentials and paths in a server address must never change the login destination.
pub fn server_url(input: &str) -> Result<Url, String> {
    let input = input.trim();
    let value = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let mut url = Url::parse(&value).map_err(|_| "Некорректный адрес сервера")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Введите адрес сервера без пути, пароля и параметров".into());
    }
    if url.scheme() == "http"
        && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
    {
        return Err("Для удалённого сервера требуется HTTPS".into());
    }
    url.set_path("/");
    Ok(url)
}

pub fn ws_url(server: &Url, room: &str) -> Result<Url, String> {
    if !matches!(room, "room1" | "room2" | "room3") {
        return Err("Неизвестная комната".into());
    }
    let mut url = server
        .join(&format!("ws/{room}"))
        .map_err(|_| "Некорректный адрес")?;
    let scheme = if server.scheme() == "https" {
        "wss"
    } else {
        "ws"
    };
    url.set_scheme(scheme).map_err(|_| "Некорректная схема")?;
    Ok(url)
}

/// The server offers first. Only the m-line receiving our microphone is sendable.
pub fn microphone_mid(answer: &str) -> Option<String> {
    for section in answer.split("m=").skip(1) {
        if !section.starts_with("audio ") {
            continue;
        }
        let lines: Vec<_> = section.lines().map(str::trim).collect();
        if lines
            .iter()
            .any(|l| matches!(*l, "a=sendrecv" | "a=sendonly"))
        {
            return lines
                .iter()
                .find_map(|l| l.strip_prefix("a=mid:").map(str::to_owned));
        }
    }
    None
}

/// Byte ranges remain valid for GPUI shaping, including non-ASCII message text.
pub fn message_links(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let lower = text.to_ascii_lowercase();
    let mut links = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let Some(start) = ["https://", "http://", "www."]
            .into_iter()
            .filter_map(|prefix| lower[offset..].find(prefix).map(|index| offset + index))
            .min()
        else {
            break;
        };
        let raw = text[start..]
            .split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '\"' | '\''))
            .next()
            .unwrap_or("");
        let mut token = raw.trim_end_matches(['.', ',', '!', '?', ';', ':']);
        for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
            while token.ends_with(close)
                && token.matches(close).count() > token.matches(open).count()
            {
                token = &token[..token.len() - 1];
            }
        }
        let href = if lower[start..].starts_with("www.") {
            format!("https://{token}")
        } else {
            token.into()
        };
        if url::Url::parse(&href)
            .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        {
            links.push((start..start + token.len(), href));
        }
        offset = start + raw.len().max(1);
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_server_and_room_before_sending_password() {
        assert_eq!(
            server_url("voice.example").unwrap().as_str(),
            "https://voice.example/"
        );
        for invalid in [
            "http://voice.example",
            "https://user:secret@voice.example",
            "https://voice.example/path",
            "https://voice.example/?x=1",
            "file:///tmp/app",
        ] {
            assert!(server_url(invalid).is_err(), "{invalid}");
        }
        let local = server_url("http://127.0.0.1:8080").unwrap();
        assert_eq!(
            ws_url(&local, "room2").unwrap().as_str(),
            "ws://127.0.0.1:8080/ws/room2"
        );
        assert!(ws_url(&local, "../api/login").is_err());
    }

    #[test]
    fn reads_actual_server_peer_fixtures() {
        let data: Value =
            serde_json::from_str(include_str!("../../backend/src/sfu/testdata/welcome.json"))
                .unwrap();
        let message: Message = serde_json::from_str(&envelope("welcome", data)).unwrap();
        assert_eq!(message.event, "welcome");
        let peers: Vec<Peer> = serde_json::from_value(message.data["peers"].clone()).unwrap();
        assert!(!peers.is_empty());
    }

    #[test]
    fn chooses_microphone_instead_of_receive_slots() {
        let sdp = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:slot-0\r\na=recvonly\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:mic\r\na=sendrecv\r\n";
        assert_eq!(microphone_mid(sdp).as_deref(), Some("mic"));
    }
}
