#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod input;

use std::sync::atomic::Ordering;

use gpui::{prelude::*, *};
use input::Input;
use voice_hub_gpui_beta::protocol::Peer;
use voice_hub_gpui_beta::session::{Command, Connect, Session, Update};

struct Client {
    server: Entity<Input>,
    name: Entity<Input>,
    password: Entity<Input>,
    room: usize,
    session: Option<Session>,
    status: String,
    peers: Vec<Peer>,
    error: Option<String>,
    muted: bool,
    deafened: bool,
    connected: bool,
    _updates: Option<Task<()>>,
}

impl Client {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            server: cx.new(|cx| Input::new("", false, cx)),
            name: cx.new(|cx| Input::new("Гость", false, cx)),
            password: cx.new(|cx| Input::new("", true, cx)),
            room: 1,
            session: None,
            status: "Отключён".into(),
            peers: vec![],
            error: None,
            muted: true,
            deafened: false,
            connected: false,
            _updates: None,
        }
    }

    fn update(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Status(status) => {
                self.connected = status == "Голос подключён";
                self.status = status;
            }
            Update::Peers(peers) => self.peers = peers,
            Update::Error(error) => self.error = Some(error),
            Update::Closed => self.reset(),
        }
        cx.notify();
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        if self.session.is_some() {
            return;
        }
        self.error = None;
        let options = Connect {
            server: self.server.read(cx).text.clone(),
            password: self.password.read(cx).text.clone(),
            name: self.name.read(cx).text.clone(),
            room: format!("room{}", self.room),
        };
        if let Err(error) = voice_hub_gpui_beta::protocol::server_url(&options.server) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.password.update(cx, |input, cx| input.clear(cx));
        self.muted = true;
        self.deafened = false;
        self.connected = false;
        self.status = "Вход на сервер…".into();
        let mut session = Session::start(options);
        let mut updates = session.updates.take().expect("new session receiver");
        self.session = Some(session);
        self._updates = Some(cx.spawn(async move |entity, cx| {
            while let Some(update) = updates.recv().await {
                let closed = matches!(update, Update::Closed);
                if entity
                    .update(cx, |this, cx| this.update(update, cx))
                    .is_err()
                    || closed
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn leave(&mut self) {
        self._updates = None;
        self.reset();
    }

    fn reset(&mut self) {
        self.session = None;
        self.peers.clear();
        self.status = "Отключён".into();
        self.connected = false;
        self.muted = true;
        self.deafened = false;
    }

    fn state(&mut self, muted: bool, deafened: bool, cx: &mut Context<Self>) {
        if let Some(session) = &self.session {
            if session
                .commands
                .try_send(Command::State { muted, deafened })
                .is_err()
            {
                self.error = Some("Не удалось изменить состояние. Переподключитесь".into());
                cx.notify();
                return;
            }
            session.muted.store(muted || deafened, Ordering::Relaxed);
            session.deafened.store(deafened, Ordering::Relaxed);
            self.muted = muted || deafened;
            self.deafened = deafened;
            cx.notify();
        }
    }
}

fn button(id: &'static str, label: impl Into<SharedString>, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_4()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .bg(rgb(if active { 0x236555 } else { 0x23303e }))
        .hover(|s| s.bg(rgb(0x34576a)))
        .child(label.into())
}

fn field(label: &str, input: Entity<Input>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(rgb(0x98afc5))
                .child(label.to_owned()),
        )
        .child(input)
}

impl Render for Client {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let in_session = self.session.is_some();
        div().size_full().bg(rgb(0x0b1118)).text_color(rgb(0xe3edf7)).font_family("Segoe UI")
            .flex().flex_col().p_8().gap_5().id("client").overflow_y_scroll()
            .child(div().flex().justify_between().items_center()
                .child(div().text_2xl().font_weight(FontWeight::BOLD).child("VOICE HUB"))
                .child(div().text_xs().text_color(rgb(0x64dfc3)).child(format!("GPUI BETA · {}", option_env!("APP_RELEASE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))))))
            .child(div().text_sm().text_color(rgb(0x98afc5)).child("Нативный голос · без WebView"))
            .child(div().text_sm().p_3().rounded_md().bg(rgb(0x172c30)).child("Используйте наушники. В beta нет эхоподавления, шумоподавления и демонстрации экрана. Закрытие окна завершает разговор."))
            .when(!in_session, |view| view
                .child(field("Сервер · https://voice.example", self.server.clone()))
                .child(field("Ваше имя", self.name.clone()))
                .child(field("Пароль сервера", self.password.clone()))
                .child(div().flex().gap_2().items_center().child("Комната")
                    .children((1..=3).map(|room| div().id(("room", room)).px_4().py_2().rounded_md().cursor_pointer()
                        .bg(rgb(if self.room == room { 0x236555 } else { 0x23303e })).child(room.to_string())
                        .on_click(cx.listener(move |this, _, _, cx| { this.room = room; cx.notify(); })))))
                .child(button("connect", "Подключиться", true).on_click(cx.listener(|this, _, _, cx| this.connect(cx)))))
            .child(div().flex().gap_2().child(div().text_color(rgb(if self.connected { 0x64dfc3 } else { 0x98afc5 })).child(self.status.clone())))
            .when_some(self.error.clone(), |view, error| view.child(div().text_sm().text_color(rgb(0xff9e9e)).child(error)))
            .when(in_session, |view| view
                .child(div().flex().gap_2().flex_wrap()
                    .when(self.connected, |view| view
                        .child(button("mute", if self.muted { "Включить микрофон" } else { "Выключить микрофон" }, !self.muted)
                            .on_click(cx.listener(|this, _, _, cx| this.state(!this.muted, false, cx))))
                        .child(button("deafen", if self.deafened { "Включить звук" } else { "Выключить весь звук" }, self.deafened)
                            .on_click(cx.listener(|this, _, _, cx| this.state(this.muted, !this.deafened, cx)))))
                    .child(button("leave", "Отключиться", false).on_click(cx.listener(|this, _, _, cx| { this.leave(); cx.notify(); }))))
                .child(div().text_sm().child(format!("Комната {} · участников: {}", self.room, self.peers.len() + 1)))
                .child(div().p_3().rounded_md().bg(rgb(0x17212c)).child(format!("{} (вы) · {}", self.name.read(cx).text, if self.deafened { "звук выключен" } else if self.muted { "микрофон выключен" } else { "в эфире" })))
                .children(self.peers.iter().map(|peer| div().p_3().rounded_md().bg(rgb(0x17212c)).child(format!("{} · {}", if peer.display_name.is_empty() { "Гость" } else { &peer.display_name }, if peer.chat_only { "в чате" } else if peer.deafened { "звук выключен" } else if peer.self_muted { "микрофон выключен" } else { "в эфире" })))))
            .child(div().text_xs().text_color(rgb(0x7790a8)).child("Устройства по умолчанию · 48 кГц · IPv4 / UDP · пароль не сохраняется"))
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        input::init(cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(620.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Voice Hub — GPUI Beta".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(480.0), px(640.0))),
                ..Default::default()
            },
            |_, cx| cx.new(Client::new),
        )
        .expect("open GPUI window");
        cx.activate(true);
    });
}
