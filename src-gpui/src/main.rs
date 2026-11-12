#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod input;
mod ui;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{prelude::*, *};
use input::{Input, InputEvent};
use voice_hub_gpui_beta::player::{Player, PlayerAction, PlayerEvent};
use voice_hub_gpui_beta::settings::Hotkey;
use voice_hub_gpui_beta::system::{System, SystemEvent};
use voice_hub_gpui_beta::updater::{UpdateAction, UpdateEvent, Updater};
use voice_hub_gpui_beta::{
    api::{CreatedPassword, PasswordEntry},
    chat::History,
    controller::{Action, AdminAction, Controller, Snapshot, Update},
    device::{AudioDevice, enumerate},
    media::CaptureChoice,
    protocol::{Attachment, Peer},
    settings::{Settings, Storage},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    People,
    Chat,
    Audio,
    Screen,
    Preferences,
    Admin,
}
struct Lightbox {
    attachments: Vec<Attachment>,
    index: usize,
    zoom: f32,
}
struct Client {
    controller: Controller,
    system: System,
    updater: Updater,
    update_status: String,
    update_available: bool,
    update_ready: Option<PathBuf>,
    capturing_hotkey: bool,
    player: Option<Player>,
    playing: Option<Attachment>,
    player_position: f64,
    player_duration: f64,
    player_paused: bool,
    player_frame: Option<Arc<RenderImage>>,
    storage: Storage,
    state: Snapshot,
    server: Entity<Input>,
    name: Entity<Input>,
    password: Entity<Input>,
    compose: Entity<Input>,
    label: Entity<Input>,
    admin_label: Entity<Input>,
    admin_ttl: Entity<Input>,
    panel: Panel,
    error: Option<String>,
    toast: Option<(String, Instant)>,
    files: Vec<PathBuf>,
    microphones: Vec<AudioDevice>,
    speakers: Vec<AudioDevice>,
    captures: Vec<CaptureChoice>,
    images: BTreeMap<String, Option<PathBuf>>,
    loading: BTreeSet<String>,
    video: BTreeMap<String, (Arc<RenderImage>, u32, u32)>,
    speaking: BTreeMap<String, (bool, Instant)>,
    focused_share: Option<String>,
    lightbox: Option<Lightbox>,
    selected_peer: Option<String>,
    passwords: Vec<PasswordEntry>,
    created_password: Option<CreatedPassword>,
    history_count: usize,
    scroll: ScrollHandle,
    last_toggle: Instant,
    confirm: Option<String>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}
impl Client {
    fn new(
        system: System,
        mut system_events: tokio::sync::mpsc::Receiver<SystemEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let storage = Storage::open().expect("open application data");
        let (settings, error) = match Settings::load(&storage) {
            Ok(settings) => (settings, None),
            Err(error) => (Settings::default(), Some(error)),
        };
        let mut controller = Controller::start(storage.clone(), settings.clone());
        let mut updates = controller.updates.take().expect("updates");
        let mut frames = controller.frames.take().expect("frames");
        system.configure(settings.hotkey.clone(), false, false);
        let mut updater = Updater::start(storage.clone());
        let mut update_events = updater.events.take().expect("updater events");
        let tasks = vec![
            cx.spawn(async move |entity, cx| {
                while let Some(event) = system_events.recv().await {
                    if entity
                        .update(cx, |this, cx| this.system_event(event, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }),
            cx.spawn(async move |entity, cx| {
                while let Some(event) = update_events.recv().await {
                    if entity
                        .update(cx, |this, cx| this.updater_event(event, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }),
            cx.spawn(async move |entity, cx| {
                while let Some(update) = updates.recv().await {
                    if entity
                        .update(cx, |this, cx| this.update(update, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }),
            cx.spawn(async move |entity, cx| {
                while let Some(frame) = frames.recv().await {
                    if entity
                        .update(cx, |this, cx| {
                            if (frame.publisher == "self" && !this.state.publishing)
                                || (frame.publisher != "self"
                                    && !this.state.watching.contains(&frame.publisher))
                            {
                                return;
                            }
                            if let Some(buffer) = image::RgbaImage::from_raw(
                                frame.width,
                                frame.height,
                                (*frame.bgra).clone(),
                            ) {
                                let render =
                                    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
                                this.video
                                    .insert(frame.publisher, (render, frame.width, frame.height));
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }),
        ];
        let server = cx.new(|cx| Input::new(&settings.server, false, cx));
        let name = cx.new(|cx| Input::new(&settings.name, false, cx));
        let password = cx.new(|cx| Input::new("", true, cx));
        let compose = cx.new(|cx| Input::new("", false, cx).multiline());
        let subscriptions = vec![
            cx.observe_window_activation(window, |this, window, _| {
                this.system.set_focused(window.is_window_active());
                if window.is_window_active() {
                    System::flash(false);
                    let _ = this.updater.actions.try_send(UpdateAction::Check {
                        server: this.state.settings.server.clone(),
                        force: false,
                    });
                }
            }),
            cx.subscribe(&password, |this, _, event, cx| {
                if matches!(event, InputEvent::Submit) {
                    this.login(cx);
                }
            }),
            cx.subscribe(&compose, |this, _, event, cx| match event {
                InputEvent::Submit => this.send_chat(cx),
                InputEvent::PasteImage(image) => this.paste_image(image, cx),
            }),
        ];
        let state = Snapshot {
            settings,
            authenticated: false,
            connected: false,
            status: "Загрузка…".into(),
            role: String::new(),
            self_id: String::new(),
            peers: BTreeMap::new(),
            rooms: BTreeMap::new(),
            messages: vec![],
            version: String::new(),
            publishing: false,
            watching: BTreeSet::new(),
        };
        let client = Self {
            controller,
            system,
            updater,
            update_status: String::new(),
            update_available: false,
            update_ready: None,
            capturing_hotkey: false,
            player: None,
            playing: None,
            player_position: 0.0,
            player_duration: 0.0,
            player_paused: true,
            player_frame: None,
            storage,
            state,
            server,
            name,
            password,
            compose,
            label: cx.new(|cx| Input::new("", false, cx)),
            admin_label: cx.new(|cx| Input::new("", false, cx)),
            admin_ttl: cx.new(|cx| Input::new("0", false, cx)),
            panel: Panel::People,
            error,
            toast: None,
            files: vec![],
            microphones: enumerate(true),
            speakers: enumerate(false),
            captures: vec![],
            images: BTreeMap::new(),
            loading: BTreeSet::new(),
            video: BTreeMap::new(),
            speaking: BTreeMap::new(),
            focused_share: None,
            lightbox: None,
            selected_peer: None,
            passwords: vec![],
            created_password: None,
            history_count: 50,
            scroll: ScrollHandle::new(),
            last_toggle: Instant::now() - Duration::from_secs(1),
            confirm: None,
            _tasks: tasks,
            _subscriptions: subscriptions,
        };
        client.act(Action::Restore);
        let _ = client.updater.actions.try_send(UpdateAction::Check {
            server: client.state.settings.server.clone(),
            force: true,
        });
        client
    }
    fn act(&self, action: Action) {
        let _ = self.controller.actions.try_send(action);
    }
    fn configure(&mut self, cx: &mut Context<Self>) {
        self.system
            .configure(self.state.settings.hotkey.clone(), false, false);
        self.act(Action::Configure(Box::new(self.state.settings.clone())));
        cx.notify();
    }
    fn update(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Snapshot(snapshot) => {
                let old_room = self.state.settings.room.clone();
                let old_server = self.state.settings.server.clone();
                let was_bottom =
                    self.scroll.max_offset().height + self.scroll.offset().y < px(80.0);
                let new_message = self.state.messages.last().map(|m| &m.id)
                    != snapshot.messages.last().map(|m| &m.id);
                self.state = *snapshot;
                if self.panel == Panel::Chat && was_bottom && new_message {
                    self.scroll.scroll_to_bottom();
                }
                if old_server != self.state.settings.server {
                    self.update_ready = None;
                    self.update_available = false;
                    let _ = self.updater.actions.try_send(UpdateAction::Check {
                        server: self.state.settings.server.clone(),
                        force: true,
                    });
                }
                if old_room != self.state.settings.room || old_server != self.state.settings.server
                {
                    self.images.clear();
                    self.loading.clear();
                    self.video.clear();
                    self.focused_share = None;
                    self.lightbox = None;
                    self.player = None;
                    self.playing = None;
                    self.history_count = 50;
                    self.scroll.scroll_to_bottom();
                }
                self.video.retain(|id, _| {
                    if id == "self" {
                        self.state.publishing
                    } else {
                        self.state.watching.contains(id)
                    }
                });
                self.load_images();
            }
            Update::Error(error) => self.error = Some(error),
            Update::Ping(name) => {
                if self.state.settings.ping_sound {
                    voice_hub_gpui_beta::device::feedback(
                        self.state.settings.speaker.clone(),
                        voice_hub_gpui_beta::device::Feedback::Ping,
                    );
                }
                if self.state.settings.ping_flash {
                    System::flash(true);
                }
                let at = Instant::now();
                self.toast = Some((format!("{name} зовёт вас"), at));
                cx.spawn(async move |entity, cx| {
                    cx.background_executor().timer(Duration::from_secs(8)).await;
                    let _ = entity.update(cx, |this, cx| {
                        if this.toast.as_ref().is_some_and(|(_, time)| *time == at) {
                            this.toast = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            Update::Levels(levels) => {
                for (id, speaking) in levels {
                    let id = if id == "self" {
                        self.state.self_id.clone()
                    } else {
                        id
                    };
                    self.speaking.insert(id, (speaking, Instant::now()));
                }
            }
            Update::Captures(captures) => {
                self.captures = captures;
                self.panel = Panel::Screen;
            }
            Update::Upload { id, progress } => {
                if let Some(message) = self
                    .state
                    .messages
                    .iter_mut()
                    .find(|m| m.client_msg_id == id)
                {
                    message.upload_progress = progress;
                }
            }
            Update::Passwords(entries) => self.passwords = entries,
            Update::Password(password) => self.created_password = Some(password),
            Update::FileFailed { upload_id, error } => {
                self.loading.remove(&upload_id);
                self.images.insert(upload_id.clone(), None);
                self.error = Some(error);
                if self
                    .playing
                    .as_ref()
                    .is_some_and(|a| a.upload_id == upload_id)
                {
                    self.player = None;
                    self.playing = None;
                    self.player_frame = None;
                }
            }
            Update::File { attachment, path } => {
                self.loading.remove(&attachment.upload_id);
                if self
                    .playing
                    .as_ref()
                    .is_some_and(|playing| playing.upload_id == attachment.upload_id)
                    && self.player.is_none()
                {
                    if let Some(path) = &path {
                        match Player::open(path, &self.state.settings.server) {
                            Ok((player, mut events)) => {
                                self.player = Some(player);
                                let expected_id = attachment.upload_id.clone();
                                cx.spawn(async move |entity, cx| {
                                    while let Some(event) = events.recv().await {
                                        if entity
                                            .update(cx, |this, cx| {
                                                if this
                                                    .playing
                                                    .as_ref()
                                                    .is_none_or(|a| a.upload_id != expected_id)
                                                {
                                                    return;
                                                }
                                                match event {
                                                    PlayerEvent::Status {
                                                        position,
                                                        duration,
                                                        paused,
                                                    } => {
                                                        this.player_position = position;
                                                        this.player_duration = duration;
                                                        this.player_paused = paused;
                                                    }
                                                    PlayerEvent::Frame(frame) => {
                                                        if let Some(buffer) =
                                                            image::RgbaImage::from_raw(
                                                                frame.width,
                                                                frame.height,
                                                                (*frame.bgra).clone(),
                                                            )
                                                        {
                                                            this.player_frame =
                                                                Some(Arc::new(RenderImage::new(
                                                                    vec![image::Frame::new(buffer)],
                                                                )));
                                                            if let Some(player) = &this.player {
                                                                player.hide();
                                                            }
                                                        }
                                                    }
                                                    PlayerEvent::Error(error) => {
                                                        this.error = Some(error);
                                                        this.player = None;
                                                        this.playing = None;
                                                        this.player_frame = None;
                                                    }
                                                }
                                                cx.notify();
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                })
                                .detach();
                            }
                            Err(error) => self.error = Some(error),
                        }
                    } else {
                        self.error = Some("Видео недоступно".into());
                    }
                }
                self.images.insert(attachment.upload_id, path);
            }
        }
        cx.notify();
    }
    fn system_event(&mut self, event: SystemEvent, cx: &mut Context<Self>) {
        match event {
            SystemEvent::Show => System::show(),
            SystemEvent::ChangeServer => {
                System::show();
                self.act(Action::ChangeServer);
            }
            SystemEvent::Autostart => {
                let enabled = !self.state.settings.autostart;
                match System::autostart(enabled) {
                    Ok(()) => {
                        self.state.settings.autostart = enabled;
                        self.configure(cx);
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            SystemEvent::CheckUpdate => {
                let _ = self.updater.actions.try_send(UpdateAction::Check {
                    server: self.state.settings.server.clone(),
                    force: true,
                });
            }
            SystemEvent::Exit => cx.quit(),
            SystemEvent::ToggleMute => self.toggle_mute(cx),
            SystemEvent::Captured(binding) => {
                self.capturing_hotkey = false;
                self.state.settings.hotkey = Some(binding);
                self.configure(cx);
            }
            SystemEvent::Error(error) => self.error = Some(error),
        }
        cx.notify();
    }
    fn updater_event(&mut self, event: UpdateEvent, cx: &mut Context<Self>) {
        match event {
            UpdateEvent::Checking => self.update_status = "Проверка обновлений…".into(),
            UpdateEvent::Available { version, notes: _ } => {
                self.update_status = format!("Доступна версия {version}");
                self.update_available = true;
            }
            UpdateEvent::Progress { received, total } => {
                self.update_status = format!(
                    "Загрузка: {:.1} / {:.1} МБ",
                    received as f64 / 1048576.0,
                    total.unwrap_or(0) as f64 / 1048576.0
                )
            }
            UpdateEvent::Ready(path) => {
                self.update_ready = Some(path);
                self.update_status = "Обновление проверено. Можно перезапустить приложение".into();
            }
            UpdateEvent::Current => self.update_status = "Установлена последняя версия".into(),
            UpdateEvent::Error(error) => self.update_status = error,
        }
        cx.notify();
    }
    fn check_update(&self) {
        let _ = self.updater.actions.try_send(UpdateAction::Check {
            server: self.state.settings.server.clone(),
            force: true,
        });
    }
    fn install_update(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = &self.update_ready {
            match voice_hub_gpui_beta::updater::install(path, &self.storage) {
                Ok(()) => cx.quit(),
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                }
            }
        } else {
            let _ = self.updater.actions.try_send(UpdateAction::Install);
        }
    }
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let modifiers = &key.modifiers;
        if self.capturing_hotkey {
            if key.key == "escape" {
                self.capturing_hotkey = false;
                self.system
                    .configure(self.state.settings.hotkey.clone(), false, false);
            } else {
                let binding = Hotkey::Keyboard {
                    key: voice_hub_gpui_beta::system::key_name(&key.key),
                    ctrl: modifiers.control,
                    shift: modifiers.shift,
                    alt: modifiers.alt,
                    meta: modifiers.platform,
                };
                self.capturing_hotkey = false;
                self.state.settings.hotkey = Some(binding);
                self.configure(cx);
            }
            cx.notify();
            return;
        }
        if self.state.settings.hotkey.as_ref().is_some_and(|binding| {
            voice_hub_gpui_beta::system::focused_binding(
                binding,
                &key.key,
                modifiers.control,
                modifiers.shift,
                modifiers.alt,
                modifiers.platform,
            )
        }) {
            self.toggle_mute(cx);
            return;
        }
        if self.lightbox.is_some() {
            match key.key.as_str() {
                "escape" => {
                    self.lightbox = None;
                    cx.notify();
                }
                "left" => self.lightbox_step(-1, cx),
                "right" => self.lightbox_step(1, cx),
                _ => {}
            }
        } else if self.panel == Panel::Screen && matches!(key.key.as_str(), "left" | "right") {
            let ids = self.video.keys().cloned().collect::<Vec<_>>();
            if !ids.is_empty() {
                let index = self
                    .focused_share
                    .as_ref()
                    .and_then(|id| ids.iter().position(|i| i == id))
                    .unwrap_or(0);
                let delta = if key.key == "left" { ids.len() - 1 } else { 1 };
                self.focused_share = Some(ids[(index + delta) % ids.len()].clone());
                cx.notify();
            }
        }
    }
    fn login(&mut self, cx: &mut Context<Self>) {
        if self.state.status == "Вход…" {
            return;
        }
        let server = self.server.read(cx).text.clone();
        if let Err(error) = voice_hub_gpui_beta::protocol::server_url(&server) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.error = None;
        let password = self.password.read(cx).text.clone();
        self.password.update(cx, |input, cx| input.clear(cx));
        self.state.settings.name = self.name.read(cx).text.trim().into();
        self.configure(cx);
        self.state.status = "Вход…".into();
        self.act(Action::Login { server, password });
    }
    fn send_chat(&mut self, cx: &mut Context<Self>) {
        let text = self.compose.read(cx).text.clone();
        if text.trim().is_empty() && self.files.is_empty() {
            return;
        }
        if text.len() > 2000 {
            self.error = Some("Сообщение длиннее 2000 байт".into());
            cx.notify();
            return;
        }
        let action = Action::Send {
            text,
            files: std::mem::take(&mut self.files),
        };
        if self.controller.actions.try_send(action).is_ok() {
            self.compose.update(cx, |input, cx| input.clear(cx));
        }
        cx.notify();
    }
    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if self.last_toggle.elapsed() < Duration::from_millis(80) {
            return;
        }
        self.last_toggle = Instant::now();
        self.state.settings.toggle_mute();
        voice_hub_gpui_beta::device::feedback(
            self.state.settings.speaker.clone(),
            if self.state.settings.muted {
                voice_hub_gpui_beta::device::Feedback::Mute
            } else {
                voice_hub_gpui_beta::device::Feedback::Unmute
            },
        );
        self.configure(cx);
    }
    fn toggle_deafen(&mut self, cx: &mut Context<Self>) {
        self.state.settings.toggle_deafen();
        voice_hub_gpui_beta::device::feedback(
            self.state.settings.speaker.clone(),
            if self.state.settings.deafened {
                voice_hub_gpui_beta::device::Feedback::Mute
            } else {
                voice_hub_gpui_beta::device::Feedback::Unmute
            },
        );
        self.configure(cx);
    }
    fn add_files(&mut self, files: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.files.len() + files.len() > 10 {
            self.error = Some("Можно прикрепить до 10 файлов".into());
        } else {
            self.files.extend(files);
        }
        cx.notify();
    }
    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Прикрепить файлы".into()),
        });
        cx.spawn(async move |entity, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = entity.update(cx, |this, cx| this.add_files(paths, cx));
            }
        })
        .detach();
    }
    fn paste_image(&mut self, image: &gpui::Image, cx: &mut Context<Self>) {
        let extension = match image.format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
            ImageFormat::Gif => "gif",
            ImageFormat::Bmp => "bmp",
            ImageFormat::Tiff => "tiff",
            ImageFormat::Svg => "svg",
        };
        let path =
            self.storage
                .root()
                .join(format!("clipboard-{}.{}", uuid::Uuid::new_v4(), extension));
        match std::fs::write(&path, &image.bytes) {
            Ok(()) => self.add_files(vec![path], cx),
            Err(_) => {
                self.error = Some("Не удалось вставить изображение".into());
                cx.notify();
            }
        }
    }
    fn open_video(&mut self, attachment: Attachment, cx: &mut Context<Self>) {
        self.player = None;
        self.playing = Some(attachment.clone());
        self.player_frame = None;
        self.act(Action::Download {
            attachment,
            destination: None,
        });
        cx.notify();
    }
    fn download(&mut self, attachment: Attachment, cx: &mut Context<Self>) {
        let directory = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = cx.prompt_for_new_path(&directory, Some(&attachment.name));
        cx.spawn(async move |entity, cx| {
            if let Ok(Ok(Some(path))) = path.await {
                let _ = entity.update(cx, |this, _| {
                    this.act(Action::Download {
                        attachment,
                        destination: Some(path),
                    })
                });
            }
        })
        .detach();
    }
    fn download_all(&mut self, attachments: Vec<Attachment>, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Сохранить все вложения".into()),
        });
        cx.spawn(async move |entity, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(directory) = paths.first()
            {
                let directory = directory.clone();
                let _ = entity.update(cx, |this, _| {
                    for attachment in attachments {
                        let name = PathBuf::from(&attachment.name)
                            .file_name()
                            .map(|s| s.to_owned())
                            .unwrap_or_else(|| "file".into());
                        this.act(Action::Download {
                            attachment,
                            destination: Some(directory.join(name)),
                        });
                    }
                });
            }
        })
        .detach();
    }
    fn load_images(&mut self) {
        if self.panel != Panel::Chat && self.lightbox.is_none() {
            return;
        }
        for attachment in self
            .state
            .messages
            .iter()
            .rev()
            .take(self.history_count)
            .flat_map(|m| {
                m.attachments
                    .iter()
                    .filter(|a| a.kind == "image" && !m.deleted_upload_ids.contains(&a.upload_id))
            })
        {
            if !self.images.contains_key(&attachment.upload_id)
                && self.loading.insert(attachment.upload_id.clone())
            {
                self.act(Action::Download {
                    attachment: attachment.clone(),
                    destination: None,
                });
            }
        }
    }
    fn peer_name(&self, peer: &Peer) -> String {
        self.state
            .settings
            .peers
            .get(peer.preference_key())
            .filter(|p| !p.label.is_empty())
            .map_or_else(
                || {
                    if peer.display_name.is_empty() {
                        "Гость".into()
                    } else {
                        peer.display_name.clone()
                    }
                },
                |p| p.label.clone(),
            )
    }
    fn open_lightbox(&mut self, attachment: &Attachment, cx: &mut Context<Self>) {
        let attachments = self
            .state
            .messages
            .iter()
            .flat_map(|m| {
                m.attachments
                    .iter()
                    .filter(|a| a.kind == "image" && !m.deleted_upload_ids.contains(&a.upload_id))
            })
            .cloned()
            .collect::<Vec<_>>();
        let index = attachments
            .iter()
            .position(|a| a.upload_id == attachment.upload_id)
            .unwrap_or(0);
        self.lightbox = Some(Lightbox {
            attachments,
            index,
            zoom: 1.0,
        });
        self.load_images();
        cx.notify();
    }
    fn lightbox_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(lightbox) = &mut self.lightbox {
            let len = lightbox.attachments.len();
            if len > 0 {
                lightbox.index =
                    (lightbox.index as isize + delta).rem_euclid(len as isize) as usize;
                lightbox.zoom = 1.0;
                let attachment = &lightbox.attachments[lightbox.index];
                if !self.images.contains_key(&attachment.upload_id)
                    && self.loading.insert(attachment.upload_id.clone())
                {
                    let _ = self.controller.actions.try_send(Action::Download {
                        attachment: attachment.clone(),
                        destination: None,
                    });
                }
            }
        }
        cx.notify();
    }
}

fn main() {
    let (events, system_events) = tokio::sync::mpsc::channel(64);
    let system = match System::start(events) {
        Ok(Some(system)) => system,
        Ok(None) => return,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    };
    Application::new().run(move |cx: &mut App| {
        input::init(cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1120.0), px(780.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Voice Hub — GPUI".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(720.0), px(540.0))),
                ..Default::default()
            },
            move |window, cx| {
                #[cfg(target_os = "windows")]
                window.on_window_should_close(cx, |_, _| {
                    if System::can_hide() {
                        System::hide();
                        false
                    } else {
                        true
                    }
                });
                cx.new(|cx| Client::new(system, system_events, window, cx))
            },
        )
        .expect("open GPUI window");
        if std::env::args().any(|arg| arg == "--hidden") && System::can_hide() {
            System::hide();
        } else {
            cx.activate(true);
        }
    });
}
