use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{
    Message as WsMessage, client::IntoClientRequest, protocol::WebSocketConfig,
};
use tokio_util::sync::CancellationToken;

#[cfg(target_os = "windows")]
use crate::media::Media;
use crate::{
    api::{Api, CreatedPassword, PasswordEntry},
    chat::{BlobCache, History, PingLimiter},
    media::{CaptureChoice, VideoImage},
    protocol::{Attachment, ChatMessage, Message, Peer, envelope, ws_url},
    settings::{Settings, Storage},
};
#[cfg(not(target_os = "windows"))]
use unsupported::Media;

pub enum Action {
    Login {
        server: String,
        password: String,
    },
    Restore,
    Logout,
    ChangeServer,
    Configure(Box<Settings>),
    Voice(bool),
    Room(String),
    Rename(String),
    Send {
        text: String,
        files: Vec<PathBuf>,
    },
    Retry(String),
    CancelUpload(String),
    DeleteMessage(String),
    Ping(String),
    Download {
        attachment: Attachment,
        destination: Option<PathBuf>,
    },
    DeleteFiles(Vec<String>),
    Captures,
    StartScreen(CaptureChoice),
    StopScreen,
    Watch {
        publisher: String,
        layer: u8,
    },
    Unwatch(String),
    Admin(AdminAction),
    Stop,
}
pub enum AdminAction {
    List,
    Create { label: String, ttl: i64 },
    Rotate(String),
    Rename { id: String, label: String },
    Ttl { id: String, ttl: i64 },
    Revoke(String),
    DisconnectUsers,
}

#[derive(Clone)]
pub struct Snapshot {
    pub settings: Settings,
    pub authenticated: bool,
    pub connected: bool,
    pub status: String,
    pub role: String,
    pub self_id: String,
    pub peers: BTreeMap<String, Peer>,
    pub rooms: BTreeMap<String, Vec<Peer>>,
    pub messages: Vec<ChatMessage>,
    pub version: String,
    pub publishing: bool,
    pub watching: BTreeSet<String>,
}
pub enum Update {
    Snapshot(Box<Snapshot>),
    Error(String),
    Ping(String),
    Levels(Vec<(String, bool)>),
    Captures(Vec<CaptureChoice>),
    Upload {
        id: String,
        progress: f32,
    },
    Passwords(Vec<PasswordEntry>),
    Password(CreatedPassword),
    File {
        attachment: Attachment,
        path: Option<PathBuf>,
    },
}

enum Job {
    UploadProgress {
        scope: Scope,
        id: String,
        progress: f32,
    },
    UploadDone {
        scope: Scope,
        id: String,
        result: Result<Vec<Attachment>, String>,
    },
    File(Attachment, Result<Option<PathBuf>, String>),
    Admin(Result<AdminResult, String>),
}
enum AdminResult {
    Entries(Vec<PasswordEntry>),
    Created(CreatedPassword),
    Changed,
}
#[derive(Clone, PartialEq, Eq)]
struct Scope {
    server: String,
    room: String,
}
struct Upload {
    files: Vec<PathBuf>,
    cancel: CancellationToken,
}
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub struct Controller {
    pub actions: mpsc::Sender<Action>,
    pub updates: Option<mpsc::Receiver<Update>>,
    pub frames: Option<mpsc::Receiver<VideoImage>>,
    stop: CancellationToken,
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl Controller {
    pub fn start(storage: Storage, settings: Settings) -> Self {
        let (actions, rx) = mpsc::channel(64);
        let (updates, urx) = mpsc::channel(64);
        let (frames, frx) = mpsc::channel(2);
        let stop = CancellationToken::new();
        let cancelled = stop.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => runtime.block_on(async move {
                    let worker = Worker::new(storage, settings, updates.clone(), frames);
                    if let Err(e) = worker.run(rx, cancelled).await {
                        let _ = updates.send(Update::Error(e)).await;
                    }
                }),
                Err(_) => {
                    let _ = updates
                        .blocking_send(Update::Error("Не удалось запустить сетевой поток".into()));
                }
            }
        });
        Self {
            actions,
            updates: Some(urx),
            frames: Some(frx),
            stop,
        }
    }
}
struct Worker {
    storage: Storage,
    state: Snapshot,
    history: Option<History>,
    api: Option<Api>,
    socket: Option<Socket>,
    media: Option<Media>,
    tx: mpsc::Sender<Update>,
    frames: mpsc::Sender<VideoImage>,
    jobs: mpsc::Sender<Job>,
    results: mpsc::Receiver<Job>,
    presence: mpsc::Sender<(String, Value)>,
    presence_events: mpsc::Receiver<(String, Value)>,
    presence_stop: CancellationToken,
    uploads: BTreeMap<String, Upload>,
    pings: PingLimiter,
    reconnect: usize,
    reconnect_at: tokio::time::Instant,
}
impl Worker {
    fn new(
        storage: Storage,
        settings: Settings,
        tx: mpsc::Sender<Update>,
        frames: mpsc::Sender<VideoImage>,
    ) -> Self {
        let (jobs, results) = mpsc::channel(64);
        let (presence, presence_events) = mpsc::channel(64);
        Self {
            storage,
            state: Snapshot {
                settings,
                authenticated: false,
                connected: false,
                status: "Отключён".into(),
                role: String::new(),
                self_id: String::new(),
                peers: BTreeMap::new(),
                rooms: BTreeMap::new(),
                messages: vec![],
                version: String::new(),
                publishing: false,
                watching: BTreeSet::new(),
            },
            history: None,
            api: None,
            socket: None,
            media: None,
            tx,
            frames,
            jobs,
            results,
            presence,
            presence_events,
            presence_stop: CancellationToken::new(),
            uploads: BTreeMap::new(),
            pings: PingLimiter::default(),
            reconnect: 0,
            reconnect_at: tokio::time::Instant::now(),
        }
    }
    fn scope(&self) -> Scope {
        Scope {
            server: self.state.settings.server.clone(),
            room: self.state.settings.room.clone(),
        }
    }
    async fn emit(&mut self) {
        if let Some(history) = &self.history {
            self.state.messages = history.messages.clone();
        }
        self.state.publishing = self.media.as_ref().is_some_and(Media::is_publishing);
        let _ = self
            .tx
            .send(Update::Snapshot(Box::new(self.state.clone())))
            .await;
    }
    async fn error(&self, error: impl Into<String>) {
        let _ = self.tx.send(Update::Error(error.into())).await;
    }
    async fn run(
        mut self,
        mut actions: mpsc::Receiver<Action>,
        stop: CancellationToken,
    ) -> Result<(), String> {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut versions = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(300),
            Duration::from_secs(300),
        );
        self.emit().await;
        loop {
            tokio::select! {
                _=stop.cancelled()=>break,
                action=actions.recv()=>{
                    let Some(action)=action else{break};if matches!(action,Action::Stop){break;}
                    let result=tokio::select!{_=stop.cancelled()=>break,result=self.action(action)=>result};
                    if let Err(e)=result{self.error(e).await;}self.emit().await;
                },
                incoming=async{match &mut self.socket{Some(socket)=>socket.next().await,None=>std::future::pending().await}}=>{
                    match incoming {
                        Some(Ok(WsMessage::Text(text)))=>if let Ok(message)=serde_json::from_str::<Message>(&text){if let Err(e)=self.message(message).await{self.error(e).await;}self.emit().await;},
                        Some(Ok(WsMessage::Ping(data)))=>{if let Some(socket)=&mut self.socket{let _=socket.send(WsMessage::Pong(data)).await;}},
                        Some(Ok(WsMessage::Close(_)))|Some(Err(_))|None=>{self.disconnected();self.emit().await;},
                        _=>{},
                    }
                },
                _=tokio::time::sleep_until(self.reconnect_at),if self.api.is_some()&&self.socket.is_none()=>{
                    let result=tokio::select!{_=stop.cancelled()=>break,result=self.open_socket()=>result};
                    if let Err(e)=result{self.error(e).await;self.disconnected();}self.emit().await;
                },
                _=tick.tick(),if self.media.is_some()=>{
                    if let Some(media)=&mut self.media{match media.tick().await{
                        Ok((signals,levels))=>{for signal in signals{let _=self.send(signal).await;}if !levels.is_empty(){let _=self.tx.try_send(Update::Levels(levels));}},
                        Err(e)=>{self.error(e).await;self.media=None;self.disconnected();self.emit().await;}
                    }}
                },
                Some(job)=self.results.recv()=>{if let Err(e)=self.job(job).await{self.error(e).await;}self.emit().await;},
                Some((event,data))=self.presence_events.recv()=>{self.presence_event(&event,data);self.emit().await;},
                _=versions.tick(),if self.api.is_some()=>{if let Some(api)=&self.api&& let Ok(version)=api.version().await{self.state.version=version;self.emit().await;}},
            }
        }
        self.presence_stop.cancel();
        for upload in self.uploads.values() {
            upload.cancel.cancel();
        }
        Ok(())
    }
    async fn authenticated(&mut self, api: Api) -> Result<(), String> {
        let config = api.config().await?;
        self.state.role = config.role;
        self.state.authenticated = true;
        self.state.settings.server = api.server.to_string();
        self.state.status = "Подключение к комнате…".into();
        self.state.version = api.version().await.unwrap_or_default();
        self.api = Some(api.clone());
        self.load_history()?;
        self.state.settings.save(&self.storage)?;
        self.presence_stop.cancel();
        self.presence_stop = CancellationToken::new();
        let cancelled = self.presence_stop.clone();
        let events = self.presence.clone();
        tokio::spawn(async move {
            loop {
                let result = api.presence(events.clone(), cancelled.clone()).await;
                if cancelled.is_cancelled() {
                    break;
                }
                if result.is_err() {
                    tokio::select! {_=cancelled.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(2))=>{}}
                }
            }
        });
        self.reconnect = 0;
        self.reconnect_at = tokio::time::Instant::now();
        Ok(())
    }
    fn load_history(&mut self) -> Result<(), String> {
        self.history = Some(History::load(
            self.storage.clone(),
            &self.state.settings.server,
            &self.state.settings.room,
        )?);
        Ok(())
    }
    async fn close_socket(&mut self) {
        if let Some(mut socket) = self.socket.take() {
            let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
        }
        self.state.connected = false;
    }
    fn disconnected(&mut self) {
        self.socket = None;
        self.state.connected = false;
        let delays = [1, 2, 4, 8, 15, 30, 30];
        let seconds = delays[self.reconnect.min(delays.len() - 1)];
        self.reconnect += 1;
        self.reconnect_at = tokio::time::Instant::now() + Duration::from_secs(seconds);
        self.state.status = format!("Соединение потеряно · повтор через {seconds} с");
    }
    async fn open_socket(&mut self) -> Result<(), String> {
        let api = self.api.as_ref().ok_or("Войдите на сервер")?;
        let config = api.config().await?;
        self.state.role = config.role.clone();
        if self.state.settings.voice_active {
            if self.media.is_none() {
                self.media = Some(Media::new(
                    &config,
                    self.state.settings.clone(),
                    self.frames.clone(),
                )?);
            } else if let Some(media) = &mut self.media {
                media.reset_audio(&config)?;
            }
        } else {
            self.media = None;
        }
        let ws = ws_url(&api.server, &self.state.settings.room)?;
        let mut request = ws
            .as_str()
            .into_client_request()
            .map_err(|_| "Некорректный адрес WebSocket")?;
        request.headers_mut().insert(
            "cookie",
            api.cookie().parse().map_err(|_| "Некорректная сессия")?,
        );
        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(256 * 1024);
        config.max_frame_size = Some(256 * 1024);
        let (mut socket, _) = tokio::time::timeout(
            Duration::from_secs(15),
            tokio_tungstenite::connect_async_with_config(request, Some(config), false),
        )
        .await
        .map_err(|_| "Сервер не ответил")?
        .map_err(|_| "Не удалось подключиться к комнате")?;
        socket.send(WsMessage::Text(envelope("hello",json!({"displayName":self.state.settings.name,"clientId":self.state.settings.client_id,"chatOnly":!self.state.settings.voice_active})).into())).await.map_err(|_|"Соединение прервано")?;
        self.socket = Some(socket);
        self.state.status = "Подключение…".into();
        Ok(())
    }
    async fn send(&mut self, text: String) -> Result<(), String> {
        let Some(socket) = &mut self.socket else {
            return Err("Нет соединения с комнатой".into());
        };
        if socket.send(WsMessage::Text(text.into())).await.is_err() {
            self.disconnected();
            return Err("Соединение с комнатой прервано".into());
        }
        Ok(())
    }
    async fn action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::Login { server, password } => {
                self.clear().await;
                let api = Api::login(&server, password).await?;
                self.authenticated(api).await
            }
            Action::Restore => {
                if !self.state.settings.server.is_empty()
                    && let Some(api) = Api::restore(&self.state.settings.server).await?
                {
                    self.authenticated(api).await?;
                }
                Ok(())
            }
            Action::Logout => {
                let api = self.api.clone();
                self.clear().await;
                if let Some(api) = api {
                    api.logout().await?;
                }
                Ok(())
            }
            Action::ChangeServer => {
                self.clear().await;
                Ok(())
            }
            Action::Configure(settings) => {
                let previous_mode = self.state.settings.screen.mode.clone();
                if let Some(media) = &mut self.media {
                    media.configure(*settings.clone())?;
                }
                self.state.settings = *settings;
                self.state.settings.save(&self.storage)?;
                if self.state.connected && self.state.settings.voice_active {
                    self.send(envelope("set-state",json!({"selfMuted":self.state.settings.muted,"deafened":self.state.settings.deafened}))).await?;
                    if self.state.publishing && previous_mode != self.state.settings.screen.mode {
                        self.send(envelope(
                            "screen-share-mode-change",
                            json!({"mode":self.state.settings.screen.mode}),
                        ))
                        .await?;
                    }
                }
                Ok(())
            }
            Action::Voice(active) => {
                self.state.settings.voice_active = active;
                self.media = None;
                self.close_socket().await;
                self.state.watching.clear();
                self.reconnect_at = tokio::time::Instant::now();
                self.state.settings.save(&self.storage)
            }
            Action::Room(room) => {
                ws_url(&self.api.as_ref().ok_or("Войдите на сервер")?.server, &room)?;
                self.close_socket().await;
                self.media = None;
                self.state.watching.clear();
                self.state.settings.room = room;
                self.state.peers.clear();
                self.load_history()?;
                self.reconnect_at = tokio::time::Instant::now();
                self.state.settings.save(&self.storage)
            }
            Action::Rename(name) => {
                let name = name.trim();
                if name.is_empty() || name.chars().count() > 64 {
                    return Err("Имя должно содержать от 1 до 64 символов".into());
                }
                self.state.settings.name = name.into();
                self.state.settings.save(&self.storage)?;
                if self.state.settings.voice_active {
                    self.send(envelope("set-displayname", json!({"displayName":name})))
                        .await
                } else {
                    self.close_socket().await;
                    self.reconnect_at = tokio::time::Instant::now();
                    Ok(())
                }
            }
            Action::Send { text, files } => self.send_chat(text, files).await,
            Action::Retry(id) => {
                if let Some(upload) = self.uploads.get(&id) {
                    let files = upload.files.clone();
                    self.start_upload(id, files)?;
                } else {
                    let history = self.history.as_mut().ok_or("Нет истории")?;
                    let message = history
                        .messages
                        .iter_mut()
                        .find(|m| {
                            m.client_msg_id == id
                                && m.sender_client_id == self.state.settings.client_id
                        })
                        .ok_or("Сообщение не найдено")?;
                    message.failed = false;
                    self.send_pending().await?;
                }
                Ok(())
            }
            Action::CancelUpload(id) => {
                if let Some(upload) = self.uploads.get(&id) {
                    upload.cancel.cancel();
                }
                Ok(())
            }
            Action::DeleteMessage(id) => {
                let history = self.history.as_ref().ok_or("Нет истории")?;
                let message = history
                    .messages
                    .iter()
                    .find(|m| m.id == id)
                    .ok_or("Сообщение не найдено")?;
                if !History::may_delete(
                    message,
                    &self.state.settings.client_id,
                    self.state.role == "admin",
                ) {
                    return Err("Можно удалить только своё сообщение".into());
                }
                if message.pending {
                    self.history.as_mut().expect("history").delete(&id);
                    self.history.as_mut().expect("history").save()
                } else {
                    self.send(envelope("chat-delete", json!({"id":id}))).await
                }
            }
            Action::Ping(target) => {
                if target == self.state.self_id {
                    return Ok(());
                }
                if self.pings.remaining(&target) > 0 {
                    return Err("Следующий пинг доступен через 10 секунд".into());
                }
                self.send(envelope("ping", json!({"to":target}))).await?;
                self.pings.mark(target);
                Ok(())
            }
            Action::Download {
                attachment,
                destination,
            } => {
                let api = self.api.clone().ok_or("Войдите на сервер")?;
                let scope = self.scope();
                let cache = BlobCache::new(&self.storage, &scope.server)?;
                let jobs = self.jobs.clone();
                tokio::spawn(async move {
                    let result = async {
                        let path = api
                            .attachment(&attachment, &scope.room, &cache, CancellationToken::new())
                            .await?;
                        if let (Some(path), Some(destination)) = (&path, destination) {
                            tokio::fs::copy(path, &destination)
                                .await
                                .map_err(|_| "Не удалось сохранить файл")?;
                            return Ok(Some(destination));
                        }
                        Ok(path)
                    }
                    .await;
                    let _ = jobs.send(Job::File(attachment, result)).await;
                });
                Ok(())
            }
            Action::DeleteFiles(ids) => {
                let cache = BlobCache::new(&self.storage, &self.state.settings.server)?;
                for id in &ids {
                    cache.delete(id)?;
                }
                if let Some(history) = &mut self.history {
                    history.mark_deleted(&ids.into_iter().collect());
                    history.save()?;
                }
                Ok(())
            }
            Action::Captures => {
                let _ = self.tx.send(Update::Captures(Media::captures())).await;
                Ok(())
            }
            Action::StartScreen(choice) => {
                let media = self.media.as_mut().ok_or("Сначала войдите в голос")?;
                let signal = media.start_screen(choice).await?;
                self.send(signal).await
            }
            Action::StopScreen => {
                if let Some(media) = &mut self.media {
                    media.stop_screen();
                }
                self.send(envelope("screen-share-stop", json!({}))).await
            }
            Action::Watch { publisher, layer } => {
                let media = self.media.as_mut().ok_or("Сначала войдите в голос")?;
                let signal = media.subscribe(&publisher, layer)?;
                self.send(signal).await?;
                self.state.watching.insert(publisher);
                Ok(())
            }
            Action::Unwatch(publisher) => {
                if let Some(media) = &mut self.media {
                    media.unsubscribe(&publisher);
                }
                self.state.watching.remove(&publisher);
                self.send(envelope(
                    "screen-share-unsubscribe",
                    json!({"publisherId":publisher}),
                ))
                .await
            }
            Action::Admin(action) => self.admin(action),
            Action::Stop => Ok(()),
        }
    }
    async fn clear(&mut self) {
        self.close_socket().await;
        self.presence_stop.cancel();
        self.media = None;
        self.api = None;
        for upload in self.uploads.values() {
            upload.cancel.cancel();
        }
        self.uploads.clear();
        self.state.authenticated = false;
        self.state.connected = false;
        self.state.role.clear();
        self.state.peers.clear();
        self.state.rooms.clear();
        self.state.watching.clear();
        self.state.messages.clear();
        self.history = None;
        self.state.status = "Отключён".into();
    }
    async fn message(&mut self, message: Message) -> Result<(), String> {
        let data = message.data;
        match message.event.as_str() {
            "welcome" => {
                self.state.self_id = data["id"].as_str().ok_or("Нет id в welcome")?.into();
                let peers: Vec<Peer> = serde_json::from_value(data["peers"].clone())
                    .map_err(|_| "Некорректный список участников")?;
                self.state.peers = peers.into_iter().map(|p| (p.id.clone(), p)).collect();
                self.state.peers.insert(
                    self.state.self_id.clone(),
                    Peer {
                        id: self.state.self_id.clone(),
                        display_name: self.state.settings.name.clone(),
                        client_id: self.state.settings.client_id.clone(),
                        chat_only: !self.state.settings.voice_active,
                        self_muted: self.state.settings.muted,
                        deafened: self.state.settings.deafened,
                        ..Default::default()
                    },
                );
                self.state.connected = true;
                self.reconnect = 0;
                self.state.status = if self.state.settings.voice_active {
                    "Голос подключён"
                } else {
                    "В чате"
                }
                .into();
                if self.state.settings.voice_active {
                    self.send(envelope("set-state",json!({"selfMuted":self.state.settings.muted,"deafened":self.state.settings.deafened}))).await?;
                }
                if let Some(media) = &self.media
                    && let Some(token) = &media.resume_token
                {
                    self.send(envelope(
                        "screen-share-resume",
                        json!({"sessionToken":token}),
                    ))
                    .await?;
                }
                self.send_pending().await?;
            }
            "peer-joined" | "peer-info" => {
                let peer: Peer =
                    serde_json::from_value(data.clone()).map_err(|_| "Некорректный участник")?;
                if !peer.client_id.is_empty() {
                    self.state
                        .peers
                        .retain(|id, p| id == &peer.id || p.client_id != peer.client_id);
                }
                self.state.peers.insert(peer.id.clone(), peer);
            }
            "peer-left" => {
                if let Some(id) = data["id"].as_str() {
                    self.state.peers.remove(id);
                    self.state.watching.remove(id);
                    if let Some(media) = &mut self.media {
                        media.unsubscribe(id);
                    }
                }
            }
            "peer-state" => {
                if let Some(peer) = data["id"]
                    .as_str()
                    .and_then(|id| self.state.peers.get_mut(id))
                {
                    peer.self_muted = data["selfMuted"].as_bool().unwrap_or(false);
                    peer.deafened = data["deafened"].as_bool().unwrap_or(false);
                }
            }
            "chat" => {
                let message: ChatMessage =
                    serde_json::from_value(data.clone()).map_err(|_| "Некорректное сообщение")?;
                if let Some(history) = &mut self.history {
                    history.receive(message, &self.state.self_id, &self.state.settings.client_id);
                    history.save()?;
                }
            }
            "chat-deleted" => {
                if let (Some(history), Some(id)) = (&mut self.history, data["id"].as_str()) {
                    history.delete(id);
                    history.save()?;
                }
            }
            "ping" => {
                if !self.state.settings.mute_pings {
                    let _ = self
                        .tx
                        .send(Update::Ping(
                            data["fromName"].as_str().unwrap_or("Участник").into(),
                        ))
                        .await;
                }
            }
            "screen-share-available" => {
                if let Some(id) = data["publisherId"].as_str()
                    && let Some(peer) = self.state.peers.get_mut(id)
                {
                    peer.screen_sharing = true;
                    peer.screen_sharing_has_audio =
                        data["hasSystemAudio"].as_bool().unwrap_or(false);
                    peer.screen_sharing_video_codec =
                        data["videoCodec"].as_str().map(str::to_owned);
                }
            }
            "screen-share-ended" => {
                if let Some(id) = data["publisherId"].as_str() {
                    self.state.watching.remove(id);
                    if let Some(peer) = self.state.peers.get_mut(id) {
                        peer.screen_sharing = false;
                    }
                }
            }
            _ => {}
        }
        if let Some(media) = &mut self.media {
            media.set_peers(self.state.peers.values().cloned().collect());
            let signals = media.signal(&message.event, &data).await?;
            for signal in signals {
                self.send(signal).await?;
            }
        }
        Ok(())
    }
    async fn send_chat(&mut self, text: String, files: Vec<PathBuf>) -> Result<(), String> {
        if !self.state.connected {
            return Err("Дождитесь подключения к комнате".into());
        }
        if files.len() > crate::protocol::CHAT_MAX_ATTACHMENTS {
            return Err("Можно прикрепить до 10 файлов".into());
        }
        let mut attachments = Vec::new();
        for file in &files {
            let meta = tokio::fs::metadata(file)
                .await
                .map_err(|_| "Файл недоступен")?;
            if !meta.is_file() || meta.len() > crate::protocol::UPLOAD_MAX_BYTES {
                return Err("Вложение должно быть файлом размером до 100 МБ".into());
            }
            let name = file
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("file")
                .to_owned();
            attachments.push(Attachment {
                upload_id: format!("temp:{}", uuid::Uuid::new_v4()),
                kind: if crate::api::mime_for(&name).starts_with("image/") {
                    "image"
                } else {
                    "file"
                }
                .into(),
                mime: crate::api::mime_for(&name).into(),
                name,
                size: meta.len(),
                ..Default::default()
            });
        }
        let history = self.history.as_mut().ok_or("Нет истории комнаты")?;
        let id = history.optimistic(
            text,
            attachments,
            self.state.self_id.clone(),
            self.state.settings.name.clone(),
            self.state.settings.client_id.clone(),
        )?;
        if files.is_empty() {
            self.send_pending().await
        } else {
            self.start_upload(id, files)
        }
    }
    fn start_upload(&mut self, id: String, files: Vec<PathBuf>) -> Result<(), String> {
        if let Some(upload) = self.uploads.remove(&id) {
            upload.cancel.cancel();
        }
        let api = self.api.clone().ok_or("Войдите на сервер")?;
        let scope = self.scope();
        let cache = BlobCache::new(&self.storage, &scope.server)?;
        let jobs = self.jobs.clone();
        let cancelled = CancellationToken::new();
        self.uploads.insert(
            id.clone(),
            Upload {
                files: files.clone(),
                cancel: cancelled.clone(),
            },
        );
        if let Some(message) = self
            .history
            .as_mut()
            .and_then(|h| h.messages.iter_mut().find(|m| m.client_msg_id == id))
        {
            message.failed = false;
            message.upload_progress = 0.0;
        }
        tokio::spawn(async move {
            let total = files.len().max(1) as f32;
            let mut attachments = Vec::new();
            let result=async{
                for (index,file) in files.iter().enumerate(){let(progress,mut rx)=mpsc::channel(4);let upload=api.upload(file,&scope.room,progress,cancelled.clone());tokio::pin!(upload);
                    let attachment=loop{tokio::select!{result=&mut upload=>break result?,Some(progress)=rx.recv()=>{let _=jobs.send(Job::UploadProgress{scope:scope.clone(),id:id.clone(),progress:(index as f32+progress)/total}).await;}}};
                    cache.put(&attachment.upload_id,file).await?;attachments.push(attachment);
                }Ok(attachments)
            }.await;
            let _ = jobs.send(Job::UploadDone { scope, id, result }).await;
        });
        Ok(())
    }
    async fn send_pending(&mut self) -> Result<(), String> {
        let pending = self
            .history
            .as_ref()
            .map(|h| {
                h.pending(&self.state.settings.client_id)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for message in pending {
            self.send(envelope("chat-send",json!({"text":message.text,"clientMsgId":message.client_msg_id,"attachments":message.attachments}))).await?;
        }
        Ok(())
    }
    async fn job(&mut self, job: Job) -> Result<(), String> {
        match job {
            Job::UploadProgress {
                scope,
                id,
                progress,
            } => {
                if scope == self.scope() {
                    if let Some(message) = self
                        .history
                        .as_mut()
                        .and_then(|h| h.messages.iter_mut().find(|m| m.client_msg_id == id))
                    {
                        message.upload_progress = progress;
                    }
                    let _ = self.tx.try_send(Update::Upload { id, progress });
                }
                Ok(())
            }
            Job::UploadDone { scope, id, result } => {
                if scope != self.scope() {
                    let mut history =
                        History::load(self.storage.clone(), &scope.server, &scope.room)?;
                    if let Some(message) =
                        history.messages.iter_mut().find(|m| m.client_msg_id == id)
                    {
                        match result {
                            Ok(attachments) => message.attachments = attachments,
                            Err(_) => message.failed = true,
                        };
                    }
                    history.save()?;
                    return Ok(());
                }
                let history = self.history.as_mut().ok_or("Нет истории")?;
                let message = history
                    .messages
                    .iter_mut()
                    .find(|m| m.client_msg_id == id)
                    .ok_or("Сообщение удалено")?;
                match result {
                    Ok(attachments) => {
                        message.attachments = attachments;
                        message.upload_progress = 1.0;
                        self.uploads.remove(&id);
                        history.save()?;
                        self.send_pending().await
                    }
                    Err(e) => {
                        message.failed = true;
                        history.save()?;
                        Err(e)
                    }
                }
            }
            Job::File(attachment, result) => {
                let _ = self
                    .tx
                    .send(Update::File {
                        attachment,
                        path: result?,
                    })
                    .await;
                Ok(())
            }
            Job::Admin(result) => {
                match result? {
                    AdminResult::Entries(entries) => {
                        let _ = self.tx.send(Update::Passwords(entries)).await;
                    }
                    AdminResult::Created(password) => {
                        let _ = self.tx.send(Update::Password(password)).await;
                        self.admin(AdminAction::List)?;
                    }
                    AdminResult::Changed => self.admin(AdminAction::List)?,
                }
                Ok(())
            }
        }
    }
    fn admin(&mut self, action: AdminAction) -> Result<(), String> {
        if self.state.role != "admin" {
            return Err("Доступно администратору".into());
        }
        let api = self.api.clone().ok_or("Войдите на сервер")?;
        let jobs = self.jobs.clone();
        tokio::spawn(async move {
            let result = async {
                match action {
                    AdminAction::List => Ok(AdminResult::Entries(api.passwords().await?)),
                    AdminAction::Create { label, ttl } => Ok(AdminResult::Created(
                        api.create_password(&label, ttl).await?,
                    )),
                    AdminAction::Rotate(id) => {
                        Ok(AdminResult::Created(api.rotate_password(&id).await?))
                    }
                    AdminAction::Rename { id, label } => {
                        api.edit_password(&id, "rename", json!({"label":label.trim()}))
                            .await?;
                        Ok(AdminResult::Changed)
                    }
                    AdminAction::Ttl { id, ttl } => {
                        api.edit_password(&id, "ttl", json!({"ttl_seconds":ttl}))
                            .await?;
                        Ok(AdminResult::Changed)
                    }
                    AdminAction::Revoke(id) => {
                        api.revoke_password(&id).await?;
                        Ok(AdminResult::Changed)
                    }
                    AdminAction::DisconnectUsers => {
                        api.disconnect_users().await?;
                        Ok(AdminResult::Changed)
                    }
                }
            }
            .await;
            let _ = jobs.send(Job::Admin(result)).await;
        });
        Ok(())
    }
    fn presence_event(&mut self, event: &str, data: Value) {
        match event {
            "presence-snapshot" => {
                if let Some(rooms) = data["rooms"].as_object() {
                    self.state.rooms.clear();
                    for (room, data) in rooms {
                        if let Ok(peers) = serde_json::from_value(data["peers"].clone()) {
                            self.state.rooms.insert(room.clone(), peers);
                        }
                    }
                }
            }
            "presence-peer-joined" | "presence-peer-updated" => {
                if let (Some(room), Ok(peer)) = (
                    data["room"].as_str(),
                    serde_json::from_value::<Peer>(data["peer"].clone()),
                ) {
                    let peers = self.state.rooms.entry(room.into()).or_default();
                    peers.retain(|p| {
                        p.id != peer.id
                            && (peer.client_id.is_empty() || p.client_id != peer.client_id)
                    });
                    peers.push(peer);
                }
            }
            "presence-peer-left" => {
                if let (Some(room), Some(id)) = (data["room"].as_str(), data["id"].as_str())
                    && let Some(peers) = self.state.rooms.get_mut(room)
                {
                    peers.retain(|p| p.id != id);
                }
            }
            _ => {}
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod unsupported {
    use super::*;
    use crate::protocol::ServerConfig;
    pub struct Media {
        pub resume_token: Option<String>,
    }
    impl Media {
        pub fn new(
            _: &ServerConfig,
            _: Settings,
            _: mpsc::Sender<VideoImage>,
        ) -> Result<Self, String> {
            Err("Нативный WebRTC включён в сборке Windows".into())
        }
        pub fn reset_audio(&mut self, _: &ServerConfig) -> Result<(), String> {
            Ok(())
        }
        pub fn captures() -> Vec<CaptureChoice> {
            vec![]
        }
        pub fn configure(&mut self, _: Settings) -> Result<(), String> {
            Ok(())
        }
        pub fn set_peers(&mut self, _: Vec<Peer>) {}
        pub fn is_publishing(&self) -> bool {
            false
        }
        pub async fn signal(&mut self, _: &str, _: &Value) -> Result<Vec<String>, String> {
            Ok(vec![])
        }
        pub async fn start_screen(&mut self, _: CaptureChoice) -> Result<String, String> {
            Err("Нет захвата экрана".into())
        }
        pub fn stop_screen(&mut self) {}
        pub fn subscribe(&mut self, _: &str, _: u8) -> Result<String, String> {
            Err("Нет видеоподключения".into())
        }
        pub fn unsubscribe(&mut self, _: &str) {}
        pub async fn tick(&mut self) -> Result<(Vec<String>, Vec<(String, bool)>), String> {
            Ok((vec![], vec![]))
        }
    }
}
