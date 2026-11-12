#![cfg(feature = "desktop")]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use voice_hub_gpui_beta::{
    chat::now_ms,
    controller::{Action, Controller, Snapshot, Update},
    protocol::envelope,
    settings::{Settings, Storage},
};

async fn until(
    controller: &mut Controller,
    condition: impl Fn(&Snapshot) -> bool,
) -> Box<Snapshot> {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            match controller.updates.as_mut().unwrap().recv().await.unwrap() {
                Update::Snapshot(snapshot) if condition(&snapshot) => return snapshot,
                Update::Error(error) => panic!("Unexpected client error: {error}"),
                _ => {}
            }
        }
    })
    .await
    .expect("Session did not reach expected state")
}
async fn serve(mut stream: TcpStream, connections: Arc<AtomicUsize>, client_id: String) {
    let mut peek = [0; 2048];
    let count = stream.peek(&mut peek).await.unwrap();
    let request = String::from_utf8_lossy(&peek[..count]);
    if request.starts_with("GET /ws/") {
        let room = if request.starts_with("GET /ws/room2 ") {
            "room2"
        } else {
            "room1"
        };
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let hello = ws.next().await.unwrap().unwrap().into_text().unwrap();
        let hello: Value = serde_json::from_str(&hello).unwrap();
        assert_eq!(hello["data"]["clientId"], client_id);
        assert_eq!(hello["data"]["chatOnly"], true);
        let index = connections.fetch_add(1, Ordering::SeqCst);
        let self_id = format!("self-{index}");
        ws.send(envelope("welcome",json!({"id":self_id,"peers":[{"id":"remote","clientId":"other-installation","displayName":"Remote","chatOnly":true}]})).into()).await.unwrap();
        while let Some(Ok(message)) = ws.next().await {
            let Ok(text) = message.into_text() else {
                continue;
            };
            let message: Value = serde_json::from_str(&text).unwrap();
            match message["event"].as_str() {
                Some("chat-send") => {
                    let data = &message["data"];
                    assert_eq!(data["text"], "hello\nfrom native");
                    ws.send(envelope("chat",json!({"id":"canonical-message","from":self_id,"senderName":"Tester","text":data["text"],"clientMsgId":data["clientMsgId"],"ts":now_ms(),"attachments":[]})).into()).await.unwrap();
                    if index == 0 {
                        ws.close(None).await.unwrap();
                        break;
                    }
                }
                Some("chat-delete") => {
                    assert_eq!(message["data"]["id"], "canonical-message");
                    ws.send(envelope("chat-deleted", message["data"].clone()).into())
                        .await
                        .unwrap();
                }
                Some("ping") => {
                    assert_eq!(message["data"]["to"], "remote");
                    ws.send(envelope("ping", json!({"from":"remote","fromName":room})).into())
                        .await
                        .unwrap();
                }
                _ => panic!("Chat-only client sent a voice-only event"),
            }
        }
        return;
    }
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    let header_end = loop {
        let count = stream.read(&mut chunk).await.unwrap();
        if count == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
    let first = headers.lines().next().unwrap().to_owned();
    let length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .and_then(|s| s.parse::<usize>().ok())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + length {
        let count = stream.read(&mut chunk).await.unwrap();
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let (status, extra, body) = if first.starts_with("POST /api/login ") {
        assert_eq!(&bytes[header_end..], b"password=test-password");
        (
            "204 No Content",
            "Set-Cookie: vh_session=test-session; HttpOnly; SameSite=Strict\r\n",
            String::new(),
        )
    } else {
        assert!(headers.contains("vh_session=test-session"));
        if first.starts_with("GET /api/config ") {
            (
                "200 OK",
                "",
                json!({"role":"user","iceServers":[]}).to_string(),
            )
        } else if first.starts_with("GET /api/version ") {
            ("200 OK", "", json!({"version":"server-test"}).to_string())
        } else if first.starts_with("GET /api/presence ") {
            ("200 OK","Content-Type: text/event-stream\r\n","event: presence-snapshot\ndata: {\"rooms\":{\"room1\":{\"peers\":[]},\"room2\":{\"peers\":[]}}}\n\n".into())
        } else if first.starts_with("POST /api/logout ") {
            ("204 No Content", "", String::new())
        } else {
            panic!("Unexpected API request: {first}")
        }
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_survives_reconnect_and_room_switch_without_opening_voice_devices() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let settings = Settings {
        name: "Tester".into(),
        ..Default::default()
    };
    let client_id = settings.client_id.clone();
    let count = connections.clone();
    let service = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let count = count.clone();
            let id = client_id.clone();
            tokio::spawn(serve(stream, count, id));
        }
    });
    let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
    let storage = Storage::at(root.clone()).unwrap();
    let mut controller = Controller::start(storage, settings.clone());
    controller
        .actions
        .send(Action::Login {
            server,
            password: "test-password".into(),
            name: "Tester".into(),
        })
        .await
        .unwrap();
    let snapshot = until(&mut controller, |s| s.connected).await;
    assert!(!snapshot.settings.voice_active);
    assert!(snapshot.peers["remote"].chat_only);
    controller
        .actions
        .send(Action::Send {
            text: "hello\nfrom native".into(),
            files: vec![],
        })
        .await
        .unwrap();
    let snapshot = until(&mut controller, |s| {
        s.messages
            .iter()
            .any(|m| m.id == "canonical-message" && !m.pending)
    })
    .await;
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(snapshot.messages[0].sender_client_id, settings.client_id);
    let snapshot = until(&mut controller, |s| s.connected && s.self_id == "self-1").await;
    assert_eq!(snapshot.messages.len(), 1);
    controller
        .actions
        .send(Action::DeleteMessage("canonical-message".into()))
        .await
        .unwrap();
    until(&mut controller, |s| s.messages.is_empty() && s.connected).await;
    controller
        .actions
        .send(Action::Room("room2".into()))
        .await
        .unwrap();
    let mut stale = snapshot.settings.clone();
    stale.server = "https://stale.example/".into();
    stale.client_id = uuid::Uuid::new_v4().to_string();
    stale.name = "Stale name".into();
    stale.voice_active = true;
    stale.input_volume = 125;
    controller
        .actions
        .send(Action::Configure(Box::new(stale)))
        .await
        .unwrap();
    let snapshot = until(&mut controller, |s| {
        s.connected && s.settings.room == "room2" && s.settings.input_volume == 125
    })
    .await;
    assert!(!snapshot.settings.voice_active);
    assert_eq!(snapshot.settings.client_id, settings.client_id);
    assert_eq!(snapshot.settings.name, "Tester");
    assert!(snapshot.settings.server.starts_with("http://127.0.0.1:"));
    controller.actions.send(Action::Logout).await.unwrap();
    until(&mut controller, |s| !s.authenticated).await;
    drop(controller);
    service.abort();
    std::fs::remove_dir_all(root).unwrap();
    assert!(connections.load(Ordering::SeqCst) >= 3);
}
