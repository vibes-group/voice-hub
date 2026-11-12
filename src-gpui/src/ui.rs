use super::*;
use voice_hub_gpui_beta::settings::NoiseEngine;

fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .px_3()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .bg(rgb(if active { 0x226454 } else { 0x22303e }))
        .hover(|s| s.bg(rgb(0x34576a)))
        .child(label.into())
}
fn row() -> Div {
    div().flex().items_center().gap_2().flex_wrap()
}
fn column() -> Div {
    div().flex().flex_col().gap_3()
}
fn card() -> Div {
    column().p_4().rounded_lg().bg(rgb(0x141e29))
}
fn title(text: impl Into<SharedString>) -> Div {
    div()
        .font_weight(FontWeight::BOLD)
        .text_lg()
        .child(text.into())
}
fn hint(text: impl Into<SharedString>) -> Div {
    div().text_sm().text_color(rgb(0x92a8bc)).child(text.into())
}
fn field(label: &str, input: Entity<Input>) -> Div {
    column().gap_1().child(hint(label.to_owned())).child(input)
}
#[derive(Clone)]
enum Volume {
    Input,
    Output,
    Peer(String),
    Screen(String),
}

impl Client {
    fn volume(
        &self,
        id: &str,
        label: &str,
        value: u16,
        max: u16,
        target: Volume,
        cx: &mut Context<Self>,
    ) -> Div {
        row()
            .child(
                div()
                    .w(px(185.0))
                    .text_sm()
                    .child(format!("{label} · {value}%")),
            )
            .children([-25i32, -5, 5, 25].into_iter().map(|delta| {
                let target = target.clone();
                button(format!("{id}:{delta}"), format!("{delta:+}"), false).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let value = (value as i32 + delta).clamp(0, max as i32) as u16;
                        match &target {
                            Volume::Input => this.state.settings.input_volume = value,
                            Volume::Output => this.state.settings.output_volume = value,
                            Volume::Peer(key) => {
                                this.state
                                    .settings
                                    .peers
                                    .entry(key.clone())
                                    .or_default()
                                    .volume = value
                            }
                            Volume::Screen(key) => {
                                this.state
                                    .settings
                                    .peers
                                    .entry(key.clone())
                                    .or_default()
                                    .screen_volume = value
                            }
                        };
                        this.configure(cx);
                    },
                ))
            }))
    }
    fn header(&self, cx: &mut Context<Self>) -> Div {
        row()
            .justify_between()
            .child(
                column()
                    .gap_1()
                    .child(title("VOICE HUB"))
                    .child(hint(format!(
                        "GPUI · {}",
                        option_env!("APP_RELEASE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
                    ))),
            )
            .when(self.state.authenticated, |view| {
                view.child(
                    row()
                        .child(hint(self.state.settings.server.clone()))
                        .child(button("change-server", "Другой сервер", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.act(Action::ChangeServer);
                                this.created_password = None;
                                cx.notify();
                            }),
                        ))
                        .child(
                            button("logout", "Выйти", false)
                                .on_click(cx.listener(|this, _, _, _| this.act(Action::Logout))),
                        ),
                )
            })
    }
    fn login_view(&self, cx: &mut Context<Self>) -> Div {
        column()
            .max_w(px(560.0))
            .w_full()
            .mx_auto()
            .py_8()
            .child(title("Подключиться к серверу"))
            .child(field("Сервер", self.server.clone()))
            .child(field("Ваше имя", self.name.clone()))
            .child(field("Пароль сервера", self.password.clone()))
            .child(
                button("login", "Войти", true)
                    .on_click(cx.listener(|this, _, _, cx| this.login(cx))),
            )
            .child(hint(
                "Сессия сохраняется в защищённом хранилище Windows. Пароль не сохраняется.",
            ))
    }
    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        column()
            .w(px(225.0))
            .flex_shrink_0()
            .child(hint("КОМНАТЫ"))
            .children((1..=3).map(|number| {
                let room = format!("room{number}");
                let count = self.state.rooms.get(&room).map_or(0, Vec::len);
                let active = self.state.settings.room == room;
                column()
                    .gap_1()
                    .child(
                        button(
                            format!("room:{room}"),
                            format!("Комната {number} · {count}"),
                            active,
                        )
                        .w_full()
                        .on_click(
                            cx.listener(move |this, _, _, _| this.act(Action::Room(room.clone()))),
                        ),
                    )
                    .children(
                        self.state
                            .rooms
                            .get(&format!("room{number}"))
                            .into_iter()
                            .flatten()
                            .take(8)
                            .map(|peer| {
                                hint(format!(
                                    "{} {}",
                                    if peer.chat_only { "◦" } else { "●" },
                                    self.peer_name(peer)
                                ))
                            }),
                    )
            }))
            .child(div().h(px(10.0)))
            .child(
                button(
                    "voice",
                    if self.state.settings.voice_active {
                        "Покинуть голос"
                    } else {
                        "Войти в голос"
                    },
                    self.state.settings.voice_active,
                )
                .on_click(cx.listener(|this, _, _, _| {
                    this.act(Action::Voice(!this.state.settings.voice_active))
                })),
            )
            .child(
                button(
                    "mute",
                    if self.state.settings.muted {
                        "Микрофон выключен"
                    } else {
                        "Микрофон включён"
                    },
                    !self.state.settings.muted,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx))),
            )
            .child(
                button(
                    "deafen",
                    if self.state.settings.deafened {
                        "Весь звук выключен"
                    } else {
                        "Звук включён"
                    },
                    self.state.settings.deafened,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_deafen(cx))),
            )
            .child(div().h(px(10.0)))
            .children(
                [
                    (Panel::People, "Участники"),
                    (Panel::Chat, "Чат"),
                    (Panel::Audio, "Звук"),
                    (Panel::Screen, "Демонстрации"),
                    (Panel::Preferences, "Настройки"),
                ]
                .into_iter()
                .map(|(panel, label)| {
                    button(format!("panel:{label}"), label, self.panel == panel).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.panel = panel;
                            if panel == Panel::Audio {
                                this.microphones = enumerate(true);
                                this.speakers = enumerate(false);
                            }
                            this.load_images();
                            cx.notify();
                        }),
                    )
                }),
            )
            .when(self.state.role == "admin", |view| {
                view.child(
                    button("admin", "Пароли подключения", self.panel == Panel::Admin).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.panel = Panel::Admin;
                            this.act(Action::Admin(AdminAction::List));
                            cx.notify();
                        }),
                    ),
                )
            })
    }
    fn people_view(&self, cx: &mut Context<Self>) -> Div {
        let mut peers = self.state.peers.values().cloned().collect::<Vec<_>>();
        peers.sort_by(|a, b| {
            a.chat_only
                .cmp(&b.chat_only)
                .then_with(|| a.preference_key().cmp(b.preference_key()))
        });
        column()
            .child(title(format!("Участники · {}", peers.len())))
            .child(row().child(self.name.clone()).child(
                button("rename", "Изменить имя", false).on_click(cx.listener(|this, _, _, cx| {
                    this.act(Action::Rename(this.name.read(cx).text.clone()))
                })),
            ))
            .children(peers.into_iter().map(|peer| {
                let id = peer.id.clone();
                let self_peer = id == self.state.self_id;
                let key = peer.preference_key().to_owned();
                let pref = self
                    .state
                    .settings
                    .peers
                    .get(&key)
                    .cloned()
                    .unwrap_or_default();
                let selected = self.selected_peer.as_deref() == Some(&id);
                let label = self.peer_name(&peer);
                let speaking = self.speaking.get(&id).is_some_and(|(active, at)| {
                    *active && at.elapsed() < Duration::from_millis(300)
                });
                card()
                    .border_1()
                    .border_color(rgb(if speaking { 0x64dfc3 } else { 0x24303c }))
                    .child(
                        row()
                            .justify_between()
                            .child(
                                column()
                                    .gap_1()
                                    .child(div().font_weight(FontWeight::BOLD).child(format!(
                                        "{}{}",
                                        label,
                                        if self_peer { " (вы)" } else { "" }
                                    )))
                                    .child(hint(if peer.chat_only {
                                        "В чате"
                                    } else if peer.deafened {
                                        "Звук выключен"
                                    } else if peer.self_muted {
                                        "Микрофон выключен"
                                    } else {
                                        "В голосе"
                                    })),
                            )
                            .when(peer.screen_sharing, |v| {
                                let id = id.clone();
                                v.child(
                                    button(format!("watch:{id}"), "Смотреть экран", true).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.act(Action::Watch {
                                                publisher: id.clone(),
                                                layer: 2,
                                            });
                                            this.panel = Panel::Screen;
                                            this.focused_share = Some(id.clone());
                                            cx.notify();
                                        }),
                                    ),
                                )
                            })
                            .when(!self_peer, |v| {
                                let id = id.clone();
                                v.child(button(format!("ping:{id}"), "Позвать", false).on_click(
                                    cx.listener(move |this, _, _, _| {
                                        this.act(Action::Ping(id.clone()))
                                    }),
                                ))
                            })
                            .when(!self_peer, |v| {
                                let id = id.clone();
                                let label = pref.label.clone();
                                v.child(
                                    button(format!("peer-options:{id}"), "Настроить", selected)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.selected_peer =
                                                if this.selected_peer.as_deref() == Some(&id) {
                                                    None
                                                } else {
                                                    Some(id.clone())
                                                };
                                            this.label
                                                .update(cx, |input, cx| input.set(&label, cx));
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .when(selected && !self_peer, |v| {
                        let mute_key = key.clone();
                        let label_key = key.clone();
                        v.child(self.volume(
                            &format!("peer-volume:{id}"),
                            "Голос",
                            pref.volume,
                            500,
                            Volume::Peer(key.clone()),
                            cx,
                        ))
                        .child(self.volume(
                            &format!("screen-volume:{id}"),
                            "Системный звук",
                            pref.screen_volume,
                            500,
                            Volume::Screen(key),
                            cx,
                        ))
                        .child(
                            row().child(
                                button(
                                    format!("local-mute:{id}"),
                                    if pref.muted {
                                        "Слышать участника"
                                    } else {
                                        "Заглушить локально"
                                    },
                                    pref.muted,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        let pref = this
                                            .state
                                            .settings
                                            .peers
                                            .entry(mute_key.clone())
                                            .or_default();
                                        pref.muted = !pref.muted;
                                        this.configure(cx);
                                    },
                                )),
                            ),
                        )
                        .child(
                            row().child(self.label.clone()).child(
                                button(format!("local-label:{id}"), "Сохранить подпись", false)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let label = this
                                            .label
                                            .read(cx)
                                            .text
                                            .trim()
                                            .chars()
                                            .take(32)
                                            .collect();
                                        this.state
                                            .settings
                                            .peers
                                            .entry(label_key.clone())
                                            .or_default()
                                            .label = label;
                                        this.configure(cx);
                                    })),
                            ),
                        )
                    })
            }))
    }
    fn audio_view(&self, cx: &mut Context<Self>) -> Div {
        column().child(title("Звук"))
            .child(card().child(self.volume("input-volume","Микрофон",self.state.settings.input_volume,300,Volume::Input,cx)).child(self.volume("output-volume","Вывод",self.state.settings.output_volume,300,Volume::Output,cx)))
            .child(card().child(title("Шумоподавление")).child(row().children(NoiseEngine::ALL.into_iter().map(|engine|button(format!("noise:{engine:?}"),engine.label(),self.state.settings.noise==engine).on_click(cx.listener(move|this,_,_,cx|{this.state.settings.noise=engine;this.configure(cx);}))))).child(hint("Эхоподавление и автоматическая регулировка усиления работают во всех режимах.")))
            .child(card().child(title("Микрофон")).children(self.microphones.iter().cloned().map(|device|{let active=device.id==self.state.settings.microphone;button(format!("mic:{}",device.id),device.name,active).on_click(cx.listener(move|this,_,_,cx|{this.state.settings.microphone=device.id.clone();this.configure(cx);} ))})))
            .child(card().child(title("Устройство вывода")).children(self.speakers.iter().cloned().map(|device|{let active=device.id==self.state.settings.speaker;button(format!("speaker:{}",device.id),device.name,active).on_click(cx.listener(move|this,_,_,cx|{this.state.settings.speaker=device.id.clone();this.configure(cx);} ))})))
            .child(row().child(button("refresh-devices","Обновить устройства",false).on_click(cx.listener(|this,_,_,cx|{this.microphones=enumerate(true);this.speakers=enumerate(false);cx.notify();}))).child(button("reset-audio","Сбросить звук",false).on_click(cx.listener(|this,_,_,cx|{this.state.settings.reset_audio();this.configure(cx);}))))
    }
    fn attachment_view(
        &self,
        attachment: &Attachment,
        deleted: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let download = attachment.clone();
        let remove = attachment.upload_id.clone();
        let open = attachment.clone();
        let image = self
            .images
            .get(&attachment.upload_id)
            .and_then(|p| p.clone());
        column()
            .p_3()
            .rounded_md()
            .bg(rgb(0x0e1720))
            .child(
                row()
                    .child(div().text_sm().child(attachment.name.clone()))
                    .child(hint(format!(
                        "{:.1} МБ",
                        attachment.size as f64 / 1024.0 / 1024.0
                    ))),
            )
            .when(deleted, |v| v.child(hint("Файл удалён локально")))
            .when(!deleted && attachment.mime.starts_with("video/"), |v| {
                let attachment = attachment.clone();
                v.child(
                    button(
                        format!("play-video:{}", attachment.upload_id),
                        "▶ Смотреть видео",
                        true,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_video(attachment.clone(), cx)),
                    ),
                )
            })
            .when(!deleted, |v| {
                v.when(attachment.kind == "image", |v| {
                    v.child(
                        div()
                            .id(SharedString::from(format!(
                                "image:{}",
                                attachment.upload_id
                            )))
                            .cursor_pointer()
                            .when_some(image.clone(), |v, path| {
                                v.child(
                                    img(path)
                                        .max_w_full()
                                        .h(px(210.0))
                                        .object_fit(ObjectFit::Contain),
                                )
                            })
                            .when(image.is_none(), |v| {
                                v.child(hint(if self.loading.contains(&attachment.upload_id) {
                                    "Загрузка изображения…"
                                } else {
                                    "Изображение недоступно"
                                }))
                            })
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_lightbox(&open, cx)),
                            ),
                    )
                })
                .child(
                    row()
                        .child(
                            button(
                                format!("download:{}", attachment.upload_id),
                                "Сохранить",
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.download(download.clone(), cx),
                            )),
                        )
                        .child(
                            button(
                                format!("delete-file:{}", attachment.upload_id),
                                "Удалить файл локально",
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, _| {
                                    this.act(Action::DeleteFiles(vec![remove.clone()]))
                                },
                            )),
                        ),
                )
            })
    }
    fn player_view(&self, cx: &mut Context<Self>) -> Div {
        card()
            .child(
                row()
                    .justify_between()
                    .child(title(
                        self.playing
                            .as_ref()
                            .map_or("Видео", |a| a.name.as_str())
                            .to_owned(),
                    ))
                    .child(
                        button("fullscreen-player", "Полный экран", false)
                            .on_click(|_, window, _| window.toggle_fullscreen()),
                    )
                    .child(
                        button("close-player", "Закрыть видео", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.player = None;
                                this.playing = None;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .when_some(self.player_frame.clone(), |view, frame| {
                view.child(
                    img(frame)
                        .w_full()
                        .h(px(320.0))
                        .object_fit(ObjectFit::Contain),
                )
            })
            .when(self.player_frame.is_none(), |view| {
                view.child(
                    canvas(|_, _, _| (), {
                        let entity = cx.entity();
                        move |bounds, _, window, cx| {
                            if let Some(player) = &entity.read(cx).player {
                                let factor = window.scale_factor();
                                player.place(
                                    (f32::from(bounds.origin.x) * factor) as i32,
                                    (f32::from(bounds.origin.y) * factor) as i32,
                                    (f32::from(bounds.size.width) * factor) as i32,
                                    (f32::from(bounds.size.height) * factor) as i32,
                                );
                            }
                        }
                    })
                    .w_full()
                    .h(px(320.0)),
                )
            })
            .child(
                row()
                    .child(
                        button(
                            "play-pause",
                            if self.player_paused { "▶" } else { "Ⅱ" },
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, _| {
                            if let Some(player) = &this.player {
                                player.action(PlayerAction::Toggle);
                            }
                        })),
                    )
                    .children([-10.0, 10.0].into_iter().map(|seconds| {
                        button(format!("seek:{seconds}"), format!("{seconds:+} с"), false).on_click(
                            cx.listener(move |this, _, _, _| {
                                if let Some(player) = &this.player {
                                    player
                                        .action(PlayerAction::Seek(this.player_position + seconds));
                                }
                            }),
                        )
                    }))
                    .child(hint(format!(
                        "{:.0}:{:02.0} / {:.0}:{:02.0}",
                        (self.player_position / 60.0).floor(),
                        self.player_position % 60.0,
                        (self.player_duration / 60.0).floor(),
                        self.player_duration % 60.0
                    )))
                    .children([0.0, 0.5, 1.0].into_iter().map(|volume| {
                        button(
                            format!("player-volume:{volume}"),
                            format!("Звук {}%", (volume * 100.0) as u32),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, _| {
                            if let Some(player) = &this.player {
                                player.action(PlayerAction::Volume(volume));
                            }
                        }))
                    }))
                    .children([0.5, 1.0, 1.5, 2.0].into_iter().map(|rate| {
                        button(format!("player-rate:{rate}"), format!("{rate}×"), false).on_click(
                            cx.listener(move |this, _, _, _| {
                                if let Some(player) = &this.player {
                                    player.action(PlayerAction::Rate(rate));
                                }
                            }),
                        )
                    })),
            )
    }
    fn chat_view(&self, cx: &mut Context<Self>) -> Div {
        column().child(title("Чат")).when(self.playing.is_some(),|v|v.child(self.player_view(cx)))
            .when(self.state.messages.len()>self.history_count,|v|v.child(button("older","Предыдущие сообщения",false).on_click(cx.listener(|this,_,_,cx|{this.history_count+=50;this.load_images();cx.notify();}))))
            .children(self.state.messages.iter().skip(self.state.messages.len().saturating_sub(self.history_count)).map(|message|{
                let delete_id=message.id.clone();let retry=message.client_msg_id.clone();let cancel=message.client_msg_id.clone();let copy=message.text.clone();let attachments=message.attachments.clone();
                card().child(row().justify_between().child(div().font_weight(FontWeight::BOLD).child(if message.sender_name.is_empty(){self.state.peers.get(&message.from).map_or("Гость",|p|p.display_name.as_str()).to_owned()}else{message.sender_name.clone()}))
                    .child(hint(format!("{:02}:{:02}",(message.ts/3600000)%24,(message.ts/60000)%60))))
                    .when(!message.text.is_empty(),|v|v.child(div().text_sm().child(message.text.clone())))
                    .children(message.attachments.iter().map(|a|self.attachment_view(a,message.deleted_upload_ids.contains(&a.upload_id),cx)))
                    .when(message.pending,|v|v.child(hint(if message.failed{"Не отправлено".into()}else{format!("Отправка · {}%",(message.upload_progress*100.0) as u32)})))
                    .child(row().child(button(format!("copy:{}",message.id),"Копировать",false).on_click(move|_,_,cx|cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))))
                        .when(message.attachments.len()>1,|v|v.child(button(format!("download-all:{}",message.id),"Сохранить всё",false).on_click(cx.listener(move|this,_,_,cx|this.download_all(attachments.clone(),cx)))))
                        .when(message.failed,|v|v.child(button(format!("retry:{}",message.id),"Повторить",true).on_click(cx.listener(move|this,_,_,_|this.act(Action::Retry(retry.clone()))))))
                        .when(message.pending&&!message.failed,|v|v.child(button(format!("cancel:{}",message.id),"Отменить загрузку",false).on_click(cx.listener(move|this,_,_,_|this.act(Action::CancelUpload(cancel.clone()))))))
                        .when(History::may_delete(message,&self.state.settings.client_id,self.state.role=="admin"),|v|v.child(button(format!("delete:{}",message.id),"Удалить сообщение",false).on_click(cx.listener(move|this,_,_,_|this.act(Action::DeleteMessage(delete_id.clone())))))))
            }))
            .child(card().child(self.compose.clone()).children(self.files.iter().enumerate().map(|(index,path)|row().child(hint(path.file_name().and_then(|p|p.to_str()).unwrap_or("file").to_owned())).child(button(format!("remove-pending:{index}"),"×",false).on_click(cx.listener(move|this,_,_,cx|{if index<this.files.len(){this.files.remove(index);}cx.notify();})))))
                .child(row().child(button("attach","Прикрепить",false).on_click(cx.listener(|this,_,_,cx|this.pick_files(cx)))).child(button("send","Отправить",true).on_click(cx.listener(|this,_,_,cx|this.send_chat(cx))))).child(hint("Enter — отправить · Shift+Enter — новая строка · Ctrl+V — вставить изображение · файлы можно перетащить в окно")))
    }
    fn screen_view(&self, cx: &mut Context<Self>) -> Div {
        column()
            .child(title("Демонстрация экрана"))
            .child(
                card()
                    .child(
                        row()
                            .child(
                                button("gaming", "Игры · 1080p60", false).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.state.settings.screen.gaming();
                                        this.configure(cx);
                                    },
                                )),
                            )
                            .child(
                                button("presentation", "Экран · исходное разрешение", false)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.settings.screen.presentation();
                                        this.configure(cx);
                                    })),
                            ),
                    )
                    .child(
                        row().child(hint("Разрешение")).children(
                            ["source", "720", "1080", "1440", "2160"].into_iter().map(
                                |resolution| {
                                    button(
                                        format!("resolution:{resolution}"),
                                        if resolution == "source" {
                                            "Исходное".into()
                                        } else {
                                            format!("{resolution}p")
                                        },
                                        self.state.settings.screen.resolution == resolution,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.state.settings.screen.resolution =
                                                resolution.into();
                                            this.configure(cx);
                                        },
                                    ))
                                },
                            ),
                        ),
                    )
                    .child(row().child(hint("Частота кадров")).children(
                        [15u32, 30, 60].into_iter().map(|fps| {
                            button(
                                format!("fps:{fps}"),
                                format!("{fps} FPS"),
                                self.state.settings.screen.fps == fps,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.state.settings.screen.fps = fps;
                                    this.configure(cx);
                                },
                            ))
                        }),
                    ))
                    .child(
                        row()
                            .children(["av1", "vp9"].into_iter().map(|codec| {
                                button(
                                    format!("codec:{codec}"),
                                    codec.to_uppercase(),
                                    self.state.settings.screen.codec == codec,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.state.settings.screen.codec = codec.into();
                                        this.configure(cx);
                                    },
                                ))
                            }))
                            .children(
                                [("sharp", "Чёткость"), ("motion", "Движение")]
                                    .into_iter()
                                    .map(|(mode, label)| {
                                        button(
                                            format!("mode:{mode}"),
                                            label,
                                            self.state.settings.screen.mode == mode,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.state.settings.screen.mode = mode.into();
                                                this.configure(cx);
                                            }),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        button(
                            "system-audio",
                            format!(
                                "Системный звук: {}",
                                if self.state.settings.screen.system_audio {
                                    "вкл."
                                } else {
                                    "выкл."
                                }
                            ),
                            self.state.settings.screen.system_audio,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.settings.screen.system_audio =
                                !this.state.settings.screen.system_audio;
                            this.configure(cx);
                        })),
                    ),
            )
            .child(
                row()
                    .child(
                        button("choose-screen", "Выбрать окно или экран", true)
                            .on_click(cx.listener(|this, _, _, _| this.act(Action::Captures))),
                    )
                    .when(self.state.publishing, |v| {
                        v.child(
                            button("stop-screen", "Завершить демонстрацию", false).on_click(
                                cx.listener(|this, _, _, _| this.act(Action::StopScreen)),
                            ),
                        )
                    }),
            )
            .children(self.captures.iter().cloned().map(|capture| {
                button(
                    format!("capture:{}:{}", capture.window, capture.id),
                    format!(
                        "{} · {}",
                        if capture.window {
                            "Окно"
                        } else {
                            "Экран"
                        },
                        capture.title
                    ),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.act(Action::StartScreen(capture.clone()));
                    this.captures.clear();
                    cx.notify();
                }))
            }))
            .children(
                self.state
                    .peers
                    .values()
                    .filter(|p| p.screen_sharing && p.id != self.state.self_id)
                    .map(|peer| {
                        let id = peer.id.clone();
                        let watching = self.state.watching.contains(&id);
                        row()
                            .child(hint(format!(
                                "{} · {}",
                                self.peer_name(peer),
                                peer.screen_sharing_video_codec
                                    .as_deref()
                                    .unwrap_or("видео")
                            )))
                            .child(
                                button(
                                    format!("screen-watch:{id}"),
                                    if watching {
                                        "Перестать смотреть"
                                    } else {
                                        "Смотреть"
                                    },
                                    watching,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, _| {
                                        if watching {
                                            this.act(Action::Unwatch(id.clone()));
                                        } else {
                                            this.act(Action::Watch {
                                                publisher: id.clone(),
                                                layer: 2,
                                            });
                                        }
                                    },
                                )),
                            )
                    }),
            )
            .children(
                self.video
                    .iter()
                    .filter(|(id, _)| {
                        self.focused_share
                            .as_ref()
                            .is_none_or(|focused| focused == *id)
                    })
                    .map(|(id, (image, width, height))| {
                        let focus = id.clone();
                        let unwatch = id.clone();
                        card()
                            .child(
                                row()
                                    .justify_between()
                                    .child(hint(format!(
                                        "{} · {width}×{height}",
                                        if id == "self" {
                                            "Ваш экран".into()
                                        } else {
                                            self.state.peers.get(id).map_or_else(
                                                || "Демонстрация".into(),
                                                |p| self.peer_name(p),
                                            )
                                        }
                                    )))
                                    .child(
                                        button(
                                            format!("focus:{id}"),
                                            if self.focused_share.is_some() {
                                                "Все экраны"
                                            } else {
                                                "Развернуть"
                                            },
                                            false,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.focused_share = if this.focused_share.is_some()
                                                {
                                                    None
                                                } else {
                                                    Some(focus.clone())
                                                };
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                img(image.clone())
                                    .w_full()
                                    .h(px(if self.focused_share.is_some() {
                                        480.0
                                    } else {
                                        260.0
                                    }))
                                    .object_fit(ObjectFit::Contain),
                            )
                            .when(id != "self", |v| {
                                v.child(
                                    row().child(hint("Качество")).children(
                                        [(0u8, "Низкое"), (1, "Среднее"), (2, "Высокое")]
                                            .into_iter()
                                            .map(|(layer, label)| {
                                                let id = unwatch.clone();
                                                button(format!("layer:{id}:{layer}"), label, false)
                                                    .on_click(cx.listener(move |this, _, _, _| {
                                                        this.act(Action::Watch {
                                                            publisher: id.clone(),
                                                            layer,
                                                        })
                                                    }))
                                            }),
                                    ),
                                )
                            })
                    }),
            )
    }
    fn preferences_view(&self, cx: &mut Context<Self>) -> Div {
        column()
            .child(title("Настройки"))
            .child(
                card()
                    .child(title("Пинги"))
                    .children(
                        [
                            (0, "Звук пинга", self.state.settings.ping_sound),
                            (1, "Не принимать пинги", self.state.settings.mute_pings),
                            (2, "Мигать окном и треем", self.state.settings.ping_flash),
                        ]
                        .into_iter()
                        .map(|(index, label, value)| {
                            button(
                                format!("ping-setting:{index}"),
                                format!("{label}: {}", if value { "вкл." } else { "выкл." }),
                                value,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    match index {
                                        0 => this.state.settings.ping_sound = !value,
                                        1 => this.state.settings.mute_pings = !value,
                                        _ => this.state.settings.ping_flash = !value,
                                    };
                                    this.configure(cx);
                                },
                            ))
                        }),
                    )
                    .child(
                        button("reset-pings", "Сбросить пинги", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.state.settings.ping_sound = true;
                                this.state.settings.mute_pings = false;
                                this.state.settings.ping_flash = false;
                                this.configure(cx);
                            },
                        )),
                    ),
            )
            .child(
                card()
                    .child(title("Глобальная клавиша микрофона"))
                    .child(hint(
                        self.state
                            .settings
                            .hotkey
                            .as_ref()
                            .map_or_else(|| "Не назначена".into(), Hotkey::label),
                    ))
                    .child(
                        button(
                            "capture-hotkey",
                            if self.capturing_hotkey {
                                "Нажмите клавишу или кнопку мыши · Esc отмена"
                            } else {
                                "Назначить клавишу или кнопку мыши"
                            },
                            self.capturing_hotkey,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.capturing_hotkey = true;
                            this.system.capture();
                            cx.notify();
                        })),
                    )
                    .child(
                        row()
                            .child(button("hotkey-default", "Ctrl+Shift+M", false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.state.settings.hotkey = Settings::default().hotkey;
                                    this.configure(cx);
                                }),
                            ))
                            .child(button("hotkey-clear", "Очистить", false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.state.settings.hotkey = None;
                                    this.configure(cx);
                                }),
                            )),
                    ),
            )
            .child(
                card().child(title("Автозапуск")).child(
                    button(
                        "autostart",
                        if self.state.settings.autostart {
                            "Включён"
                        } else {
                            "Выключен"
                        },
                        self.state.settings.autostart,
                    )
                    .on_click(
                        cx.listener(|this, _, _, cx| this.system_event(SystemEvent::Autostart, cx)),
                    ),
                ),
            )
            .child(
                card()
                    .child(title("Обновления"))
                    .child(hint(self.update_status.clone()))
                    .child(
                        row()
                            .child(
                                button("check-update", "Проверить", false)
                                    .on_click(cx.listener(|this, _, _, _| this.check_update())),
                            )
                            .when(self.update_available, |v| {
                                v.child(
                                    button(
                                        "install-update",
                                        if self.update_ready.is_some() {
                                            "Перезапустить и установить"
                                        } else {
                                            "Скачать обновление"
                                        },
                                        true,
                                    )
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.install_update(cx)),
                                    ),
                                )
                            }),
                    ),
            )
            .child(hint(format!("Версия сервера: {}", self.state.version)))
    }
    fn admin_view(&self, cx: &mut Context<Self>) -> Div {
        column()
            .child(title("Пароли подключения"))
            .child(
                card()
                    .child(field("Метка", self.admin_label.clone()))
                    .child(field(
                        "Срок действия, секунды · 0 = бессрочно · −1 = отключить",
                        self.admin_ttl.clone(),
                    ))
                    .child(
                        row()
                            .child(button("admin-create", "Создать пароль", true).on_click(
                                cx.listener(|this, _, _, cx| {
                                    let ttl = this.admin_ttl.read(cx).text.parse().unwrap_or(0);
                                    this.act(Action::Admin(AdminAction::Create {
                                        label: this.admin_label.read(cx).text.clone(),
                                        ttl,
                                    }));
                                }),
                            ))
                            .child(button("admin-refresh", "Обновить", false).on_click(
                                cx.listener(|this, _, _, _| {
                                    this.act(Action::Admin(AdminAction::List))
                                }),
                            )),
                    ),
            )
            .when_some(self.created_password.clone(), |v, password| {
                v.child(
                    card()
                        .child(title("Новый пароль"))
                        .child(div().child(password.password.clone()))
                        .child(hint("Сохраните сейчас. Повторно посмотреть пароль нельзя."))
                        .child(
                            row()
                                .child(button("copy-password", "Копировать пароль", true).on_click(
                                    {
                                        let password = password.password.clone();
                                        move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                password.clone(),
                                            ))
                                        }
                                    },
                                ))
                                .child(
                                    button("copy-invitation", "Копировать приглашение", false)
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                password.invitation(),
                                            ))
                                        }),
                                )
                                .child(button("hide-password", "Скрыть", false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.created_password = None;
                                        cx.notify();
                                    }),
                                )),
                        ),
                )
            })
            .children(self.passwords.iter().map(|entry| {
                let rotate = entry.id.clone();
                let rename = entry.id.clone();
                let ttl = entry.id.clone();
                let revoke = entry.id.clone();
                let confirmed = self.confirm.as_deref() == Some(&format!("revoke:{}", entry.id));
                card()
                    .child(
                        row()
                            .justify_between()
                            .child(title(entry.label.clone()))
                            .child(hint(if entry.expired {
                                "Истёк".into()
                            } else {
                                entry
                                    .expires_at
                                    .clone()
                                    .unwrap_or_else(|| "Бессрочно".into())
                            })),
                    )
                    .child(hint(format!(
                        "Создан: {} · поколение {}",
                        entry.created_at, entry.generation
                    )))
                    .child(
                        row()
                            .child(
                                button(format!("rotate:{}", entry.id), "Новый пароль", false)
                                    .on_click(cx.listener(move |this, _, _, _| {
                                        this.act(Action::Admin(AdminAction::Rotate(rotate.clone())))
                                    })),
                            )
                            .child(
                                button(
                                    format!("rename-password:{}", entry.id),
                                    "Взять метку из поля",
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.act(Action::Admin(AdminAction::Rename {
                                            id: rename.clone(),
                                            label: this.admin_label.read(cx).text.clone(),
                                        }))
                                    },
                                )),
                            )
                            .child(
                                button(format!("ttl:{}", entry.id), "Применить срок", false)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.act(Action::Admin(AdminAction::Ttl {
                                            id: ttl.clone(),
                                            ttl: this.admin_ttl.read(cx).text.parse().unwrap_or(0),
                                        }))
                                    })),
                            )
                            .child(
                                button(
                                    format!("revoke:{}", entry.id),
                                    if confirmed {
                                        "Подтвердить отзыв"
                                    } else {
                                        "Отозвать"
                                    },
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        let key = format!("revoke:{revoke}");
                                        if this.confirm.as_deref() == Some(&key) {
                                            this.act(Action::Admin(AdminAction::Revoke(
                                                revoke.clone(),
                                            )));
                                            this.confirm = None;
                                        } else {
                                            this.confirm = Some(key);
                                        }
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
            }))
            .child(
                button(
                    "disconnect-users",
                    if self.confirm.as_deref() == Some("disconnect") {
                        "Подтвердить отключение всех пользователей"
                    } else {
                        "Отключить всех пользователей"
                    },
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.confirm.as_deref() == Some("disconnect") {
                        this.act(Action::Admin(AdminAction::DisconnectUsers));
                        this.confirm = None;
                    } else {
                        this.confirm = Some("disconnect".into());
                    }
                    cx.notify();
                })),
            )
    }
    fn lightbox_view(&self, lightbox: &Lightbox, cx: &mut Context<Self>) -> Div {
        let attachment = lightbox.attachments[lightbox.index].clone();
        let download = attachment.clone();
        let delete = attachment.upload_id.clone();
        let path = self
            .images
            .get(&attachment.upload_id)
            .and_then(|p| p.clone());
        column()
            .absolute()
            .inset_0()
            .p_5()
            .bg(rgba(0x060b12f5))
            .child(
                row()
                    .justify_between()
                    .child(title(format!(
                        "{} · {}/{}",
                        attachment.name,
                        lightbox.index + 1,
                        lightbox.attachments.len()
                    )))
                    .child(
                        button("close-lightbox", "Закрыть · Esc", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.lightbox = None;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .id("lightbox-image")
                    .flex_1()
                    .overflow_scroll()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(path, |v, path| {
                        v.child(
                            img(path)
                                .w(relative(lightbox.zoom))
                                .h_full()
                                .object_fit(ObjectFit::Contain),
                        )
                    }),
            )
            .child(
                row()
                    .justify_center()
                    .child(
                        button("previous-image", "←", false)
                            .on_click(cx.listener(|this, _, _, cx| this.lightbox_step(-1, cx))),
                    )
                    .child(
                        button("next-image", "→", false)
                            .on_click(cx.listener(|this, _, _, cx| this.lightbox_step(1, cx))),
                    )
                    .child(
                        button("zoom-in", "+", false).on_click(cx.listener(|this, _, _, cx| {
                            if let Some(lightbox) = &mut this.lightbox {
                                lightbox.zoom = (lightbox.zoom * 1.25).min(5.0);
                            }
                            cx.notify();
                        })),
                    )
                    .child(button("zoom-out", "−", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(lightbox) = &mut this.lightbox {
                                lightbox.zoom = (lightbox.zoom / 1.25).max(0.25);
                            }
                            cx.notify();
                        },
                    )))
                    .child(button("download-image", "Сохранить", false).on_click(
                        cx.listener(move |this, _, _, cx| this.download(download.clone(), cx)),
                    ))
                    .child(button("delete-image", "Удалить локально", false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.act(Action::DeleteFiles(vec![delete.clone()]));
                            this.lightbox = None;
                            cx.notify();
                        }),
                    )),
            )
    }
}
impl Render for Client {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if (self.panel != Panel::Chat || self.lightbox.is_some() || !self.state.authenticated)
            && let Some(player) = &self.player
        {
            player.hide();
        }
        div()
            .id("client")
            .size_full()
            .relative()
            .bg(rgb(0x0b1118))
            .text_color(rgb(0xe3edf7))
            .font_family("Segoe UI")
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .on_drop(cx.listener(|this, files: &ExternalPaths, _, cx| {
                this.add_files(files.paths().to_vec(), cx)
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.key_down(event, cx)))
            .child(self.header(cx))
            .when(self.update_available, |v| {
                v.child(
                    row().child(hint(self.update_status.clone())).child(
                        button(
                            "update-banner",
                            if self.update_ready.is_some() {
                                "Перезапустить"
                            } else {
                                "Обновить"
                            },
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                    ),
                )
            })
            .child(
                row()
                    .justify_between()
                    .child(hint(self.state.status.clone()))
                    .when_some(self.error.clone(), |v, error| {
                        v.child(div().text_sm().text_color(rgb(0xff9e9e)).child(error))
                            .child(button("clear-error", "×", false).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.error = None;
                                    cx.notify();
                                },
                            )))
                    }),
            )
            .when_some(
                self.toast
                    .as_ref()
                    .filter(|(_, at)| at.elapsed() < Duration::from_secs(8))
                    .map(|(text, _)| text.clone()),
                |v, text| v.child(div().p_3().rounded_md().bg(rgb(0x225046)).child(text)),
            )
            .when(!self.state.authenticated, |v| v.child(self.login_view(cx)))
            .when(self.state.authenticated, |v| {
                v.child(
                    div()
                        .flex()
                        .gap_5()
                        .flex_1()
                        .min_h_0()
                        .child(self.sidebar(cx))
                        .child(
                            div()
                                .id("content")
                                .track_scroll(&self.scroll)
                                .flex_1()
                                .min_w_0()
                                .overflow_y_scroll()
                                .child(match self.panel {
                                    Panel::People => self.people_view(cx),
                                    Panel::Chat => self.chat_view(cx),
                                    Panel::Audio => self.audio_view(cx),
                                    Panel::Screen => self.screen_view(cx),
                                    Panel::Preferences => self.preferences_view(cx),
                                    Panel::Admin => self.admin_view(cx),
                                }),
                        ),
                )
            })
            .when_some(self.lightbox.as_ref(), |v, lightbox| {
                v.child(self.lightbox_view(lightbox, cx))
            })
    }
}
