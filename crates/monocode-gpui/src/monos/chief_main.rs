//! The Chief in the main window: its chat in the Mono view, like a Mono's, with a header that
//! links to the Chief's settings and history, and the details panel on the right. The Chief has
//! no habits, workflows, projects, browser or files, so none of their panels exist for it: the
//! view is the chat, its status and the link to Settings > Chief. The floating panel keeps
//! working beside it (the same chat is shown in one place at a time, see `floating_shows`).
use super::chief_ui::CHIEF_ID;
use super::panels::button;
use super::*;

/// The main window asks for Settings > Chief (the shell opens it). `history`: the History
/// section comes first on the page, so the owner lands on it.
pub(crate) struct ShowChiefSettings {
    pub history: bool,
}
impl EventEmitter<ShowChiefSettings> for Monos {}

fn status_label(state: &str) -> &'static str {
    match state {
        "working" => "Working",
        "needs-you" => "Needs you",
        _ => "Idle",
    }
}

impl Monos {
    /// Whether the Chief is what the main window shows.
    pub(crate) fn chief_active(&self) -> bool {
        self.active.as_deref() == Some(CHIEF_ID)
    }

    /// The identity of what the main window shows: a Mono, or the Chief's stand-in.
    pub(super) fn active_identity(&self) -> Option<Mono> {
        self.active.as_deref().and_then(|id| self.identity_for(id))
    }

    fn chief_state(&self, cx: &App) -> &'static str {
        match self.chief_chat().map(|c| c.read(cx).mono_status()) {
            Some("working") => "working",
            Some("needs-you") => "needs-you",
            _ => "idle",
        }
    }

    /// The Chief's chat as the main window shows it. Chat, status and a header; nothing that
    /// belongs to a Mono's work.
    pub(super) fn chief_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(chief) = self.identity_for(CHIEF_ID) else {
            return div().into_any_element();
        };
        let state = self.chief_state(cx);
        let note = self
            .chief_status
            .clone()
            .or_else(|| self.chief_notice.clone());
        let header = self.chief_header(&chief, state, note.as_deref(), cx);
        div()
            .id("chief-main")
            .debug_selector(|| "chief-main".into())
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .map(|d| {
                if self.floating_shows(CHIEF_ID, cx) {
                    d.child(self.floating_placeholder(&chief, cx))
                } else if let Some(chat) = self.chief_chat().cloned() {
                    d.child(div().flex_1().min_h_0().child(chat))
                } else {
                    d.child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(13.))
                            .text_color(content(0.45))
                            .child(note.unwrap_or_else(|| "Opening the chat...".into())),
                    )
                }
            })
            .into_any_element()
    }

    fn chief_header(
        &self,
        chief: &Mono,
        state: &'static str,
        note: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let original = self.original_chrome;
        let open = self.chief_open_count();
        div()
            .flex_shrink_0()
            .h(px(44.))
            .flex()
            .items_center()
            .gap_2()
            .pl_4()
            // Clear of the window's corner controls the Mono topbar also keeps free.
            .pr(px(48.))
            .border_b_1()
            .border_color(content(0.07))
            .when(!original, |d| {
                d.child(avatar::avatar(
                    &chief.mascot,
                    chief.color,
                    state,
                    24.,
                    state == "idle",
                ))
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(chief.label()),
                )
                .child(super::chief_ui::chief_tag())
            })
            .child(
                div()
                    .debug_selector(|| "chief-main-status".into())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(if note.is_some() {
                        rgb(0xf87171).into()
                    } else {
                        content(0.45)
                    })
                    .child(match note {
                        Some(note) => note.to_owned(),
                        None if open > 0 => {
                            format!("{} - {open} waiting for Send", status_label(state))
                        }
                        None => status_label(state).to_owned(),
                    }),
            )
            .child(
                button("chief-main-history", "History")
                    .debug_selector(|| "chief-main-history".into())
                    .on_click(
                        cx.listener(|_, _, _, cx| cx.emit(ShowChiefSettings { history: true })),
                    ),
            )
            .child(
                button("chief-main-settings", "Chief settings")
                    .debug_selector(|| "chief-main-settings".into())
                    .on_click(
                        cx.listener(|_, _, _, cx| cx.emit(ShowChiefSettings { history: false })),
                    ),
            )
            .into_any_element()
    }

    /// The details panel beside the Chief's chat: who it is, what it waits for, and the way to
    /// its settings. No Mono panels.
    pub(super) fn chief_details(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(chief) = self.identity_for(CHIEF_ID) else {
            return div().into_any_element();
        };
        let state = self.chief_state(cx);
        let open = self.chief_open_count();
        div()
            .id("chief-details")
            .debug_selector(|| "chief-details".into())
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .px_6()
            .pt_10()
            .child(avatar::avatar(
                &chief.mascot,
                chief.color,
                state,
                56.,
                false,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(chief.label()),
                    )
                    .child(super::chief_ui::chief_tag()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child(match open {
                        0 => status_label(state).to_owned(),
                        n => format!("{} - {n} waiting for Send", status_label(state)),
                    }),
            )
            .child(
                button("chief-details-settings", "Chief settings")
                    .debug_selector(|| "chief-details-settings".into())
                    .on_click(
                        cx.listener(|_, _, _, cx| cx.emit(ShowChiefSettings { history: false })),
                    ),
            )
            .into_any_element()
    }
}
