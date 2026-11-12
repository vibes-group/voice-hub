use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
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
