//! Settings > Chief: the switch, its name and look, the model it answers with, what a command
//! may do, and its soul and memory. Every control writes through `Monos::update_chief`, so the
//! open chat follows the change the way a Mono's settings do.
use super::panels::{
    LookChanged, LookEditor, button, card, heading, setting, switch_setting, switch_track,
};
use super::*;
use crate::{
    pickers::{ModelList, ModelPicked},
    popover::{MenuAction, MenuItem},
};
use monocode_core::mono::{chief as core_chief, files, memory};

/// A dropdown that is open: its menu, where it hangs and how wide it is.
struct Open {
    menu: Entity<Menu>,
    anchor: AnchorSlot,
    width: f32,
    _events: Vec<Subscription>,
}

pub(crate) struct ChiefSettings {
    pub(super) monos: Entity<Monos>,
    name: Entity<Input>,
    soul: Entity<Input>,
    /// The text and hash of SOUL.md as last loaded or saved.
    soul_saved: Option<(String, String)>,
    soul_saving: bool,
    memory: Option<Vec<String>>,
    look: Entity<LookEditor>,
    open: Option<Open>,
    model_anchor: AnchorSlot,
    effort_anchor: AnchorSlot,
    account_anchor: AnchorSlot,
    error: Option<String>,
    /// The history rows shown in full, by proposal id.
    pub(super) expanded: HashSet<String>,
    _events: Vec<Subscription>,
}

impl ChiefSettings {
    pub(crate) fn new(monos: Entity<Monos>, cx: &mut Context<Self>) -> Self {
        let chief = monos.read(cx).roster.chief.clone().unwrap_or_default();
        let name = cx.new(|cx| {
            let mut field = Input::field("Chief name", core_chief::DEFAULT_NAME, content(0.35), cx);
            field.set_font_size(13.);
            field.set_text(chief.name.clone(), cx);
            field
        });
        let soul = cx.new(|cx| {
            let mut editor = Input::markdown_editor("Soul", cx);
            editor.set_font_size(13.);
            editor.set_placeholder("What the Chief should always keep in mind", cx);
            editor
        });
        let stand_in = core_chief::stand_in(&chief);
        let look = cx.new(|cx| LookEditor::for_settings(stand_in, cx));
        let events = vec![
            cx.observe(&monos, |_, _, cx| cx.notify()),
            cx.subscribe(&name, |this, input, _: &input::Changed, cx| {
                let name: String = input.read(cx).content.chars().take(40).collect();
                this.update(|c| c.name = name, cx);
            }),
            cx.subscribe(&look, |this, _, event: &LookChanged, cx| {
                let (mascot, color) = (event.mono.mascot.clone(), event.mono.color);
                this.update(
                    |c| {
                        c.mascot = mascot;
                        c.color = color;
                    },
                    cx,
                );
            }),
            cx.subscribe(&soul, |this, _, _: &input::Changed, cx| this.flush_soul(cx)),
        ];
        let mut this = Self {
            monos,
            name,
            soul,
            soul_saved: None,
            soul_saving: false,
            memory: None,
            look,
            open: None,
            model_anchor: AnchorSlot::default(),
            effort_anchor: AnchorSlot::default(),
            account_anchor: AnchorSlot::default(),
            error: None,
            expanded: HashSet::new(),
            _events: events,
        };
        this.reload_files(cx);
        this
    }

    pub(super) fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.error = error;
        cx.notify();
    }

    fn chief(&self, cx: &App) -> core_chief::Chief {
        self.monos.read(cx).roster.chief.clone().unwrap_or_default()
    }
    fn update(&self, change: impl FnOnce(&mut core_chief::Chief), cx: &mut Context<Self>) {
        self.monos.update(cx, |m, cx| m.update_chief(change, cx));
    }

    // ---- Soul and memory ----

    fn reload_files(&mut self, cx: &mut Context<Self>) {
        let root = self.monos.read(cx).chief_root();
        let work = cx
            .background_executor()
            .spawn(async move { files::load(&root, core_chief::FILES_ID, None) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(loaded) => {
                        let text = loaded.soul.clone().unwrap_or_default();
                        let untouched = this
                            .soul_saved
                            .as_ref()
                            .is_none_or(|(saved, _)| *saved == this.soul.read(cx).content.as_str());
                        if untouched {
                            this.soul.update(cx, |f, cx| f.set_text(text.clone(), cx));
                        }
                        this.soul_saved = Some((text, loaded.soul_hash));
                        this.memory = Some(
                            loaded
                                .memory
                                .lines()
                                .filter(|l| l.trim_start().starts_with("- "))
                                .map(str::to_owned)
                                .collect(),
                        );
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves the soul over the text it was loaded as, one write at a time; text typed while a
    /// write runs is written when it finishes.
    fn flush_soul(&mut self, cx: &mut Context<Self>) {
        let Some((saved, hash)) = self.soul_saved.clone() else {
            return;
        };
        let text = self.soul.read(cx).content.to_string();
        if self.soul_saving || text == saved {
            return;
        }
        self.soul_saving = true;
        let root = self.monos.read(cx).chief_root();
        let write = text.clone();
        let work = cx.background_executor().spawn(async move {
            files::save(
                &root,
                core_chief::FILES_ID,
                None,
                "SOUL.md",
                &write,
                Some(&hash),
            )
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.soul_saving = false;
                match result {
                    Ok(hash) => {
                        this.soul_saved = Some((text, hash));
                        this.error = None;
                        this.flush_soul(cx);
                    }
                    Err(error) => {
                        this.error = Some(format!(
                            "Could not save the soul: {error}. Your edits are still in the editor."
                        ));
                        this.reload_files(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn forget(&mut self, fact: String, cx: &mut Context<Self>) {
        let root = self.monos.read(cx).chief_root();
        let work = cx.background_executor().spawn(async move {
            memory::edit(&root, core_chief::FILES_ID, None, |old| {
                memory::replace(old, &fact, None, &memory::date())
            })
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.error = Some(error);
                }
                this.reload_files(cx);
            });
        })
        .detach();
    }

    // ---- Dropdowns ----

    fn show(
        &mut self,
        menu: Entity<Menu>,
        anchor: AnchorSlot,
        width: f32,
        events: Vec<Subscription>,
        cx: &mut Context<Self>,
    ) {
        let closed = cx.subscribe(&menu, |this, _, _: &MenuEvent, cx| {
            this.open = None;
            cx.notify();
        });
        let mut all = events;
        all.push(closed);
        self.open = Some(Open {
            menu,
            anchor,
            width,
            _events: all,
        });
        cx.notify();
    }

    fn open_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chief = self.chief(cx);
        let profile = crate::accounts::get(cx)
            .map(|center| {
                center
                    .read(cx)
                    .profile(chief.account.as_deref().unwrap_or_default())
            })
            .unwrap_or_default();
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        let kind = chief
            .harness
            .as_deref()
            .and_then(Kind::parse)
            .unwrap_or(Kind::Claude);
        let list = cx.new(|cx| ModelList::new_for(kind, chief.model.clone(), &home, profile, cx));
        let focus = list.read(cx).search_focus(cx);
        let picked = cx.subscribe(&list, |this, _, picked: &ModelPicked, cx| {
            let (kind, model) = (picked.0, picked.1.clone());
            this.update(
                |c| {
                    c.harness = Some(kind.id().to_owned());
                    c.model = model;
                    c.effort = None;
                },
                cx,
            );
            this.open = None;
        });
        let menu = cx.new(|cx| {
            Menu::new(
                "Model",
                vec![MenuItem::Custom(list.into(), focus)],
                window,
                cx,
            )
            .width(400.)
        });
        self.show(menu, self.model_anchor.clone(), 400., vec![picked], cx);
    }

    fn efforts(&self, chief: &core_chief::Chief, cx: &App) -> Vec<(String, String)> {
        let profile = crate::accounts::get(cx)
            .map(|center| {
                center
                    .read(cx)
                    .profile(chief.account.as_deref().unwrap_or_default())
            })
            .unwrap_or_default();
        let models =
            monocode_core::catalog::cached_models_for(Kind::Claude, &profile).unwrap_or_default();
        let entry = chief
            .model
            .as_deref()
            .and_then(|id| models.iter().find(|m| m.id == id))
            .or_else(|| models.iter().find(|m| m.default).or(models.first()));
        entry
            .map(|m| monocode_core::app_access::effort_choices(Kind::Claude, m))
            .unwrap_or_default()
    }

    fn open_effort(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chief = self.chief(cx);
        let mut items: Vec<MenuItem> = vec![
            MenuAction::new("effort:", "Low (default)")
                .checked(chief.effort.is_none())
                .into(),
        ];
        items.extend(self.efforts(&chief, cx).into_iter().map(|(value, name)| {
            let checked = chief.effort.as_deref() == Some(value.as_str());
            MenuItem::from(MenuAction::new(format!("effort:{value}"), name).checked(checked))
        }));
        let menu = cx.new(|cx| Menu::new("Effort", items, window, cx));
        let picked = cx.subscribe(&menu, |this, _, event: &MenuEvent, cx| {
            if let MenuEvent::Picked(id, _) = event
                && let Some(value) = id.strip_prefix("effort:")
            {
                let value = (!value.is_empty()).then(|| value.to_owned());
                this.update(|c| c.effort = value, cx);
            }
            this.open = None;
            cx.notify();
        });
        self.show(menu, self.effort_anchor.clone(), 228., vec![picked], cx);
    }

    /// The Claude accounts to pick from; empty while there is only the one login.
    fn accounts(&self, cx: &App) -> Vec<(String, String)> {
        let Some(center) = crate::accounts::get(cx) else {
            return Vec::new();
        };
        let rows = center.read(cx).book.list(Kind::Claude);
        if rows.len() > 1 {
            rows.into_iter().map(|a| (a.id, a.label)).collect()
        } else {
            Vec::new()
        }
    }

    fn open_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chief = self.chief(cx);
        let mut items: Vec<MenuItem> = vec![
            MenuAction::new("account:", "App default")
                .checked(chief.account.is_none())
                .into(),
        ];
        items.extend(self.accounts(cx).into_iter().map(|(id, label)| {
            let checked = chief.account.as_deref() == Some(id.as_str());
            MenuItem::from(MenuAction::new(format!("account:{id}"), label).checked(checked))
        }));
        let menu = cx.new(|cx| Menu::new("Account", items, window, cx));
        let picked = cx.subscribe(&menu, |this, _, event: &MenuEvent, cx| {
            if let MenuEvent::Picked(id, _) = event
                && let Some(value) = id.strip_prefix("account:")
            {
                let value = (!value.is_empty()).then(|| value.to_owned());
                this.update(|c| c.account = value, cx);
            }
            this.open = None;
            cx.notify();
        });
        self.show(menu, self.account_anchor.clone(), 228., vec![picked], cx);
    }

    // ---- Drawing ----

    fn picker_button(
        id: &'static str,
        label: SharedString,
        anchor: &AnchorSlot,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .role(Role::Button)
            .aria_label(label.clone())
            .h(px(28.))
            .px_1p5()
            .mx(px(-6.))
            .rounded_md()
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(12.))
            .text_color(content(0.85))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.06)))
            .child(anchor.probe())
            .child(label)
            .child(icon("chevron-down", 12.))
    }

    fn switch(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        change: fn(&mut core_chief::Chief, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        switch_track(id, label, on)
            .debug_selector(move || id.into())
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.update(|c| change(c, !on), cx)))
            .into_any_element()
    }
}

impl gpui::Render for ChiefSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chief = self.chief(cx);
        let status = self.monos.read(cx).chief_status.clone();
        let kind = chief
            .harness
            .as_deref()
            .and_then(Kind::parse)
            .unwrap_or(Kind::Claude);
        let model = chief.model.clone().unwrap_or_else(|| "App default".into());
        let effort = match &chief.effort {
            Some(value) => self
                .efforts(&chief, cx)
                .into_iter()
                .find(|(v, _)| v == value)
                .map_or_else(|| value.clone(), |(_, name)| name),
            None => "Low (default)".into(),
        };
        let accounts = self.accounts(cx);
        let account = chief
            .account
            .as_ref()
            .map(|id| {
                accounts
                    .iter()
                    .find(|(a, _)| a == id)
                    .map_or_else(|| "Removed account".to_owned(), |(_, label)| label.clone())
            })
            .unwrap_or_else(|| "App default".into());
        let facts: Vec<String> = self.memory.clone().unwrap_or_default();
        // Opened from the main view's History button: the history comes first.
        let mut history = Some(self.history_section(cx));
        let history_top = self
            .monos
            .read(cx)
            .chief_history_first()
            .then(|| history.take())
            .flatten();
        let body = div()
            .id("chief-settings")
            .debug_selector(|| "chief-settings".into())
            .w_full()
            .flex()
            .flex_col()
            .child(heading(
                "Chief",
                Some(
                    "An assistant above your Monos. It reads what is going on across your sessions and proposes commands for them; nothing is sent until you press Send on its card.",
                ),
            ))
            .child(
                card()
                    .child(switch_setting(
                        "Turn the Chief on",
                        "Experimental. Off by default. Turning it off keeps its settings, soul, memory and history.",
                        false,
                        self.switch("chief-enabled", "Turn the Chief on", chief.enabled, |c, on| c.enabled = on, cx),
                    )),
            )
            .when_some(history_top, |d, history| d.child(history))
            .when_some(status, |d, status| {
                d.child(
                    div()
                        .debug_selector(|| "chief-status".into())
                        .px_1()
                        .pt_2()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(rgb(0xf87171))
                        .child(status),
                )
            })
            .child(heading("Identity", None))
            .child(
                card()
                    .child(setting(
                        "Name",
                        Some("What the Chief is called in the rail, the panel and the menu bar."),
                        div()
                            .debug_selector(|| "chief-name-field".into())
                            .w(px(208.))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.1))
                            .child(self.name.clone())
                            .into_any_element(),
                    ))
                    .child(
                        div()
                            .border_t_1()
                            .border_color(content(0.04))
                            .child(div().px_1().child(self.look.clone())),
                    ),
            )
            .child(heading("Model", Some("The Chief runs on Claude Code, the only harness that can be limited to the app tool.")))
            .child(
                card()
                    .child(setting(
                        "Harness",
                        None,
                        div()
                            .debug_selector(|| "chief-harness".into())
                            .child(kind.label())
                            .into_any_element(),
                    ))
                    .child(
                        setting(
                            "Model",
                            Some("What it thinks and replies with."),
                            Self::picker_button(
                                "chief-model",
                                model.into(),
                                &self.model_anchor,
                            )
                            .on_click(cx.listener(|this, _, w, cx| this.open_model(w, cx)))
                            .into_any_element(),
                        )
                        .border_t_1()
                        .border_color(content(0.04)),
                    )
                    .child(
                        setting(
                            "Effort",
                            None,
                            Self::picker_button(
                                "chief-effort",
                                effort.into(),
                                &self.effort_anchor,
                            )
                            .on_click(cx.listener(|this, _, w, cx| this.open_effort(w, cx)))
                            .into_any_element(),
                        )
                        .border_t_1()
                        .border_color(content(0.04)),
                    )
                    .when(!accounts.is_empty(), |d| {
                        d.child(
                            setting(
                                "Account",
                                Some("The sign-in it thinks and replies with."),
                                Self::picker_button(
                                    "chief-account",
                                    account.into(),
                                    &self.account_anchor,
                                )
                                .on_click(cx.listener(|this, _, w, cx| this.open_account(w, cx)))
                                .into_any_element(),
                            )
                            .border_t_1()
                            .border_color(content(0.04)),
                        )
                    }),
            )
            .child(heading("Commands", None))
            .child(
                card()
                    .child(switch_setting(
                        "Allow Send from web devices",
                        "A paired web device with Control access may press Send on a card. Off by default.",
                        false,
                        self.switch(
                            "chief-allow-web-send",
                            "Allow Send from web devices",
                            chief.allow_web_send,
                            |c, on| c.allow_web_send = on,
                            cx,
                        ),
                    ))
                    .child(
                        switch_setting(
                            "Notify me when a command is delivered",
                            "Tell me when a session has received a command the Chief proposed. Off by default.",
                            false,
                            self.switch(
                                "chief-notify-on-delivery",
                                "Notify me when a command is delivered",
                                chief.notify_on_delivery,
                                |c, on| c.notify_on_delivery = on,
                                cx,
                            ),
                        )
                        .border_t_1()
                        .border_color(content(0.04)),
                    ),
            )
            .when_some(history, |d, history| d.child(history))
            .child(heading("Soul", Some("Standing instructions the Chief keeps in every conversation.")))
            .child(
                card().child(
                    div()
                        .id("chief-soul-editor")
                        .debug_selector(|| "chief-soul-editor".into())
                        .max_h(px(280.))
                        .min_h(px(120.))
                        .overflow_y_scroll()
                        .px_3()
                        .py_3()
                        .font_family(crate::fonts::mono())
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(content(0.85))
                        .child(self.soul.clone()),
                ),
            )
            .child(heading("Memory", Some("What the Chief has learned. It loads into every conversation.")))
            .child(
                card().child(
                    div()
                        .id("chief-memory")
                        .debug_selector(|| "chief-memory".into())
                        .max_h(px(280.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .when(self.memory.is_none(), |d| {
                            d.child(div().px_3().py_4().text_size(px(12.)).text_color(content(0.45)).child("Loading..."))
                        })
                        .when(self.memory.as_ref().is_some_and(Vec::is_empty), |d| {
                            d.child(div().px_3().py_4().text_size(px(12.)).text_color(content(0.45)).child("Nothing remembered yet"))
                        })
                        .children(facts.into_iter().enumerate().map(|(ix, line)| {
                            let shown = memory::fact(&line);
                            let find = shown.clone();
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_1p5()
                                .when(ix > 0, |d| d.border_t_1().border_color(content(0.04)))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .text_color(content(0.8))
                                        .child(shown),
                                )
                                .child(
                                    button(("chief-forget", ix), "Forget").on_click(cx.listener(
                                        move |this, _, _, cx| this.forget(find.clone(), cx),
                                    )),
                                )
                        })),
                ),
            )
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .px_1()
                        .pt_2()
                        .text_size(px(12.))
                        .text_color(rgb(0xf87171))
                        .child(error),
                )
            });
        div()
            .size_full()
            .child(body)
            .children(self.open.as_ref().map(|open| {
                menu_popover(&open.menu, Anchor::Slot(open.anchor.clone()), cx)
                    .align(crate::popover::Align::Start)
                    .without_animation()
                    .priority(90)
                    .width(open.width)
                    .into_any_element()
            }))
    }
}
