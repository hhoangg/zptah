//! Settings > Chief > History: the proposal records, newest first (at most the last 100 the
//! roster keeps), with a button that reveals the command log on disk. Reads the roster; the
//! only thing it changes is which rows are expanded.
use super::chief_settings::ChiefSettings;
use super::panels::{button, card, heading};
use super::*;
use mono::chief::{self as core_chief, Proposal, ProposalState, Route};

/// One proposal as a row of the list.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct HistoryRow {
    pub id: String,
    pub time: String,
    pub route: String,
    pub session: String,
    pub state: &'static str,
    pub reason: Option<String>,
    pub instruction: String,
    /// What the Mono's send really carried, when it differs from the instruction.
    pub sent_text: Option<String>,
}

fn state_parts(state: &ProposalState) -> (&'static str, Option<String>) {
    match state {
        ProposalState::AwaitingOwner => ("Waiting for your Send", None),
        ProposalState::QueuedAtMono => ("Waiting for the Mono", None),
        ProposalState::QueuedInSession => ("Queued in session", None),
        ProposalState::Sent => ("Sent", None),
        ProposalState::Refused(reason) => ("Refused", Some(reason.clone())),
        ProposalState::NotAttempted => ("Not sent", None),
        ProposalState::Failed(reason) => ("Failed", Some(reason.clone())),
        ProposalState::Cancelled => ("Cancelled", None),
    }
}

fn time_label(at: u64) -> String {
    chrono::DateTime::from_timestamp_millis(at as i64)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%b %-d, %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn route_label(proposal: &Proposal, roster: &Roster) -> String {
    match &proposal.route {
        Route::ViaMono { mono_id } => roster
            .get(mono_id)
            .map_or_else(|| "Unknown Mono".into(), Mono::label),
        Route::Direct => "Directly".into(),
    }
}

/// The rows, newest first, capped at [`core_chief::MAX_HISTORY`].
pub(super) fn history_rows(proposals: &[Proposal], roster: &Roster) -> Vec<HistoryRow> {
    proposals
        .iter()
        .rev()
        .take(core_chief::MAX_HISTORY)
        .map(|proposal| {
            let (state, reason) = state_parts(&proposal.state);
            let mut session = if matches!(proposal.kind, core_chief::ProposalKind::Start { .. }) {
                format!("New session \u{b7} {} ", proposal.project)
            } else {
                String::new()
            };
            if let Some(id) = proposal.session_id {
                session.push_str(&format!("#{id} "));
            }
            session.push_str(proposal.session_title.trim());
            HistoryRow {
                id: proposal.id.clone(),
                time: time_label(proposal.created_at),
                route: route_label(proposal, roster),
                session: session.trim().to_owned(),
                state,
                reason: reason.filter(|r| !r.trim().is_empty()),
                instruction: proposal.instruction.clone(),
                sent_text: proposal.sent_text_if_different().map(str::to_owned),
            }
        })
        .collect()
}

impl Monos {
    /// Whether Settings > Chief puts its History section first.
    pub(crate) fn chief_history_first(&self) -> bool {
        self.chief_history_first
    }
    pub(crate) fn set_chief_history_first(&mut self, first: bool, cx: &mut Context<Self>) {
        if self.chief_history_first != first {
            self.chief_history_first = first;
            cx.notify();
        }
    }
    /// The Chief's command log (`<app data>/chief/commands.jsonl`).
    pub(crate) fn chief_log_path(&self) -> std::path::PathBuf {
        core_chief::dir(&self.chief_root()).join(core_chief::AUDIT_FILE)
    }
}

impl ChiefSettings {
    /// Reveals the command log in the file manager, or says there is none yet.
    pub(super) fn open_log(&mut self, cx: &mut Context<Self>) {
        let path = self.monos.read(cx).chief_log_path();
        if path.exists() {
            cx.reveal_path(&path);
            self.set_error(None, cx);
        } else {
            self.set_error(Some("No command has been logged yet".into()), cx);
        }
    }

    pub(super) fn toggle_history_row(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_owned());
        }
        cx.notify();
    }

    pub(super) fn history_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let rows = {
            let monos = self.monos.read(cx);
            history_rows(monos.chief_proposals(), &monos.roster)
        };
        let reveal = crate::file_preview::reveal_label();
        div()
            .child(heading(
                "History",
                Some("The last 100 commands the Chief proposed, newest first."),
            ))
            .child(
                card()
                    .id("chief-history")
                    .debug_selector(|| "chief-history".into())
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .when(rows.is_empty(), |d| {
                        d.child(
                            div()
                                .px_3()
                                .py_4()
                                .text_size(px(12.))
                                .text_color(content(0.45))
                                .child("No commands yet"),
                        )
                    })
                    .children(rows.into_iter().enumerate().map(|(ix, row)| {
                        let open = self.expanded.contains(&row.id);
                        let id = row.id.clone();
                        div()
                            .id(("chief-history-row", ix))
                            .debug_selector(move || format!("chief-history-row-{ix}"))
                            .role(gpui::Role::Button)
                            .aria_label(format!("{}, {}, {}", row.time, row.session, row.state))
                            .cursor_pointer()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .px_3()
                            .py_2()
                            .when(ix > 0, |d| d.border_t_1().border_color(content(0.04)))
                            .hover(|s| s.bg(content(0.04)))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_history_row(&id, cx)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(12.))
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_color(content(0.45))
                                            .child(row.time.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(content(0.85))
                                            .child(format!(
                                                "{} \u{2192} {}",
                                                row.route, row.session
                                            )),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_color(content(0.65))
                                            .child(row.state),
                                    ),
                            )
                            .when_some(row.reason.clone(), |d, reason| {
                                d.child(
                                    div()
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .text_color(rgb(0xf87171))
                                        .child(reason),
                                )
                            })
                            .child(
                                div()
                                    .debug_selector(move || format!("chief-history-text-{ix}"))
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .text_color(content(0.6))
                                    .when(!open, |d| d.truncate())
                                    .child(row.instruction.clone()),
                            )
                            .when_some(row.sent_text.clone().filter(|_| open), |d, sent| {
                                d.child(
                                    div()
                                        .debug_selector(move || format!("chief-history-sent-{ix}"))
                                        .flex()
                                        .flex_col()
                                        .gap_0p5()
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .child(div().text_color(content(0.45)).child("Sent text"))
                                        .child(div().text_color(content(0.6)).child(sent)),
                                )
                            })
                    })),
            )
            .child(
                div().flex().pt_2().child(
                    button("chief-open-log", format!("Open log file ({reveal})"))
                        .debug_selector(|| "chief-open-log".into())
                        .on_click(cx.listener(|this, _, _, cx| this.open_log(cx))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mono::chief::Surface;

    fn proposal(at: u64, session: i64, route: Route) -> Proposal {
        let mut p = Proposal::new(
            at,
            session,
            "Login refactor",
            "web",
            None,
            route,
            "Add tests for the expired-token case",
            Surface::Desktop,
        )
        .unwrap();
        p.created_at = at;
        p
    }

    #[test]
    fn the_history_lists_the_newest_first_with_route_session_state_and_reason() {
        let mut roster = Roster::default();
        roster.monos.push(Mono {
            id: "m1".into(),
            name: "Atlas".into(),
            ..Default::default()
        });
        let first = proposal(1_000, 5, Route::Direct);
        let mut second = proposal(
            2_000,
            7,
            Route::ViaMono {
                mono_id: "m1".into(),
            },
        );
        second.state = ProposalState::Refused("The Mono was busy".into());
        let rows = history_rows(&[first, second], &roster);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].session, "#7 Login refactor");
        assert_eq!(rows[0].route, "Atlas");
        assert_eq!(rows[0].state, "Refused");
        assert_eq!(rows[0].reason.as_deref(), Some("The Mono was busy"));
        assert_eq!(rows[1].route, "Directly");
        assert_eq!(rows[1].state, "Waiting for your Send");
        assert_eq!(rows[1].reason, None);
        assert!(rows[1].instruction.contains("expired-token"));
    }

    #[test]
    fn the_history_shows_at_most_the_last_hundred() {
        let all: Vec<Proposal> = (0..core_chief::MAX_HISTORY as u64 + 20)
            .map(|at| proposal(at + 1, 1, Route::Direct))
            .collect();
        let rows = history_rows(&all, &Roster::default());
        assert_eq!(rows.len(), core_chief::MAX_HISTORY);
        assert_eq!(rows[0].id, all.last().unwrap().id);
    }
}
