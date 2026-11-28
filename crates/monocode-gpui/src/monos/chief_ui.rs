//! The Chief on screen: the cards of its proposals in its chat, the label a relayed command
//! carries in a Mono's chat, its entry in the Monos rail and the floating panel, and the click
//! handlers behind the card buttons. Every decision is the manager's (`chief_confirm`,
//! `chief_edit`, `chief_cancel`); the transcript only draws and reports clicks.
use super::*;
use crate::transcript::{ChiefCard, ChiefCardAction, ChiefCards, ChiefClick, ChiefTone};
use mono::chief::{self as core_chief, Proposal, ProposalState, Route, Surface};

/// The id the Chief goes by where a Mono's id would be (the floating panel's selection).
pub(crate) const CHIEF_ID: &str = core_chief::FILES_ID;

fn note_text(note: &str) -> String {
    match note {
        "sessionBusy" => "Will wait in the session queue",
        "monoBusy" => "The Mono is busy and will take it after its current turn",
        "cloudAsleep" => "Waking the cloud sandbox costs money",
        "queuePaused" => "The session's queue is paused, so it waits until you resume it",
        other => other,
    }
    .into()
}

/// The chip of a proposal's state, and how it is tinted.
fn chip(proposal: &Proposal, roster: &Roster) -> (String, ChiefTone) {
    match &proposal.state {
        ProposalState::AwaitingOwner => ("Waiting for your Send".into(), ChiefTone::Wait),
        ProposalState::QueuedAtMono => {
            let name = match &proposal.route {
                Route::ViaMono { mono_id } => roster
                    .get(mono_id)
                    .map_or_else(|| "its Mono".into(), Mono::label),
                Route::Direct => "the app".into(),
            };
            (format!("Waiting for {name}"), ChiefTone::Wait)
        }
        ProposalState::QueuedInSession => ("Queued in session".into(), ChiefTone::Wait),
        ProposalState::Sent => ("Sent".into(), ChiefTone::Good),
        ProposalState::Refused(reason) => (format!("Refused: {reason}"), ChiefTone::Bad),
        ProposalState::NotAttempted => ("Not sent".into(), ChiefTone::Quiet),
        ProposalState::Failed(reason) => (format!("Failed: {reason}"), ChiefTone::Bad),
        ProposalState::Cancelled => ("Cancelled".into(), ChiefTone::Quiet),
    }
}

/// `<Mono name> → #<id> <title> · <project> · <branch>`, or `Directly → ...`; a start reads
/// `<Mono name|Directly> → new session · <project>`.
fn route_line(proposal: &Proposal, roster: &Roster) -> String {
    let lead = match &proposal.route {
        Route::ViaMono { mono_id } => roster
            .get(mono_id)
            .map_or_else(|| "Unknown Mono".into(), Mono::label),
        Route::Direct => "Directly".into(),
    };
    let mut line = format!("{lead} \u{2192} ");
    if matches!(proposal.kind, core_chief::ProposalKind::Start { .. }) {
        line.push_str(&format!("new session \u{b7} {}", proposal.project));
        return line;
    }
    if let Some(id) = proposal.session_id {
        line.push_str(&format!("#{id} "));
    }
    line.push_str(proposal.session_title.trim());
    for part in [Some(&proposal.project), proposal.branch.as_ref()]
        .into_iter()
        .flatten()
        .filter(|p| !p.is_empty())
    {
        line.push_str(&format!(" \u{b7} {part}"));
    }
    line
}

impl Monos {
    pub(crate) fn chief_enabled(&self) -> bool {
        self.prefs.monos && self.roster.chief.as_ref().is_some_and(|c| c.enabled)
    }
    pub(crate) fn chief_label(&self) -> String {
        self.roster
            .chief
            .as_ref()
            .map(core_chief::Chief::label)
            .unwrap_or_else(|| core_chief::DEFAULT_NAME.into())
    }
    /// The chat the floating panel shows for `id`: the Chief's, or a Mono's.
    pub(super) fn chat_for(&self, id: &str) -> Option<Entity<Conversation>> {
        if id == CHIEF_ID {
            self.chief_chat().cloned().filter(|_| self.chief_enabled())
        } else {
            self.chats.get(id).cloned()
        }
    }
    /// The identity the panel shows for `id`: the Chief's stand-in record, or a Mono.
    pub(crate) fn identity_for(&self, id: &str) -> Option<Mono> {
        if id == CHIEF_ID {
            self.roster
                .chief
                .as_ref()
                .filter(|_| self.chief_enabled())
                .map(core_chief::stand_in)
        } else {
            self.roster.get(id).cloned()
        }
    }
    /// Opens the chat behind `id` (a Mono's, or the Chief's) when it is not open yet.
    pub(super) fn ensure_chat_for(&mut self, id: &str, cx: &mut Context<Self>) {
        if id == CHIEF_ID {
            self.ensure_chief_chat(cx);
        } else {
            self.ensure_chat(id, cx);
        }
    }

    // ---- Wiring the Chief's chat ----

    /// Sends what the owner types in the Chief's chat to the Chief, and routes the clicks on
    /// its cards. Called once, when the chat opens.
    pub(super) fn chief_attach(&mut self, view: &Entity<Conversation>, cx: &mut Context<Self>) {
        self.observations
            .push(cx.subscribe(view, |this, chat, event: &MonoSubmitted, cx| {
                // The Chief answers text only: attachments are never dropped silently.
                let sent = if event.attachments.is_empty() {
                    this.chief_send(event.text.clone(), cx)
                } else {
                    Err("The Chief takes text only. Remove the attachments and send again".into())
                };
                match sent {
                    Ok(()) => {
                        if this.chief_notice.take().is_some() {
                            this.changed(cx);
                        }
                    }
                    Err(error) => {
                        // What the owner typed goes back to the composer, not lost.
                        let (text, attachments) = (event.text.clone(), event.attachments.clone());
                        chat.update(cx, |chat, cx| {
                            chat.chief_restore_composer(text, attachments, cx)
                        });
                        this.chief_notice = Some(error);
                        this.changed(cx);
                    }
                }
            }));
        let transcript = view.read(cx).transcript.clone();
        self.observations.push(cx.subscribe(
            &transcript,
            |this, _, event: &ChiefCardAction, cx| {
                // A click reads the shell; it runs once the update that emitted it is over.
                let (id, click) = (event.id.clone(), event.click);
                let weak = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = weak.update(cx, |this, cx| this.chief_card_click(&id, click, cx));
                });
                let _ = this;
            },
        ));
    }

    /// A button on a card. Errors stay on the card, where the owner clicked.
    pub(crate) fn chief_card_click(&mut self, id: &str, click: ChiefClick, cx: &mut Context<Self>) {
        let result = match click {
            ChiefClick::Send => self.chief_confirm(id, Surface::Desktop, None, cx),
            ChiefClick::Cancel => self.chief_cancel(id, cx),
            ChiefClick::Edit => {
                let text = self
                    .chief_proposals()
                    .iter()
                    .find(|p| p.id == id && p.is_open())
                    .map(|p| p.instruction.clone());
                match text {
                    Some(text) => {
                        let editor = cx.new(|cx| {
                            let mut editor = Input::markdown_editor("Command", cx);
                            editor.set_text(text, cx);
                            editor
                        });
                        self.chief_editors.insert(id.to_owned(), editor);
                        Ok(())
                    }
                    None => Err("This command can no longer be edited".into()),
                }
            }
            ChiefClick::Save => match self.chief_editors.get(id) {
                Some(editor) => {
                    let text = editor.read(cx).content.to_string();
                    let saved = self.chief_edit(id, &text, cx);
                    if saved.is_ok() {
                        self.chief_editors.remove(id);
                    }
                    saved
                }
                None => Ok(()),
            },
            ChiefClick::Discard => {
                self.chief_editors.remove(id);
                Ok(())
            }
        };
        match result {
            Ok(()) => {
                self.chief_card_errors.remove(id);
            }
            Err(error) => {
                self.chief_card_errors.insert(id.to_owned(), error);
            }
        }
        self.chief_sync_cards_now(cx);
    }

    // ---- What the chats are shown ----

    /// Hands the proposals to the chats once the update that changed them is over: the notes of
    /// a card read the shell, which may be in the middle of an update right now.
    pub(crate) fn chief_sync_cards(&mut self, cx: &mut Context<Self>) {
        if self.chief_sync_pending {
            return;
        }
        self.chief_sync_pending = true;
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |this, cx| {
                this.chief_sync_pending = false;
                this.chief_sync_cards_now(cx);
            });
        })
        .detach();
    }

    /// The cards, as the Chief's chat draws them, and the relay labels, as the Monos' chats do.
    /// Reads the shell: never call it inside a shell update.
    pub(crate) fn chief_sync_cards_now(&mut self, cx: &mut Context<Self>) {
        let enabled = self.chief_enabled();
        let name = self.chief_label();
        let mut full = ChiefCards {
            name: name.clone(),
            ..Default::default()
        };
        let mut relays = full.clone();
        if enabled {
            for proposal in self.chief_proposals().to_vec() {
                let notes = if proposal.is_open() {
                    self.chief_card_notes(&proposal, cx)
                        .into_iter()
                        .map(note_text)
                        .collect()
                } else {
                    Vec::new()
                };
                let (state, tone) = chip(&proposal, &self.roster);
                let card = ChiefCard {
                    id: proposal.id.clone(),
                    route: route_line(&proposal, &self.roster),
                    instruction: proposal.instruction.clone(),
                    sent_text: proposal.sent_text_if_different().map(str::to_owned),
                    notes,
                    state,
                    tone,
                    open: proposal.is_open(),
                    error: self.chief_card_errors.get(&proposal.id).cloned(),
                    editor: self
                        .chief_editors
                        .get(&proposal.id)
                        .filter(|_| proposal.is_open())
                        .cloned(),
                };
                if matches!(proposal.route, Route::ViaMono { .. }) && !proposal.is_open() {
                    relays.cards.insert(
                        proposal.id.clone(),
                        ChiefCard {
                            route: String::new(),
                            notes: Vec::new(),
                            state: String::new(),
                            tone: ChiefTone::Quiet,
                            open: false,
                            error: None,
                            editor: None,
                            ..card.clone()
                        },
                    );
                }
                full.cards.insert(proposal.id.clone(), card);
            }
            self.chief_editors
                .retain(|id, _| full.cards.get(id).is_some_and(|c| c.open));
            self.chief_card_errors
                .retain(|id, _| full.cards.contains_key(id));
        }
        if let Some(chat) = self.chief_chat.clone() {
            let transcript = chat.read(cx).transcript.clone();
            transcript.update(cx, |t, cx| t.set_chief_cards(full, cx));
        }
        for chat in self.chats.values().cloned().collect::<Vec<_>>() {
            let transcript = chat.read(cx).transcript.clone();
            let relays = relays.clone();
            transcript.update(cx, |t, cx| t.set_chief_cards(relays, cx));
        }
    }

    /// The panel on the Chief, from the tray and the "Ask Chief" shortcut. Nothing opens while
    /// the Chief is off.
    pub(crate) fn open_chief_panel(&mut self, cx: &mut Context<Self>) {
        self.open_floating(CHIEF_ID.into(), cx);
    }

    // ---- The rail ----

    /// The Chief's entry in the Monos rail, pinned above a thin divider. `None` while it is off.
    pub(super) fn chief_rail_entry(
        &self,
        original: bool,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let chief = self
            .roster
            .chief
            .as_ref()
            .filter(|_| self.chief_enabled())?;
        let open = self.chief_open_count();
        let state = self
            .chief_chat
            .as_ref()
            .map_or("idle", |c| c.read(cx).mono_status());
        let name = chief.label();
        let label = if open == 0 {
            format!("{name}, Chief, {state}")
        } else {
            format!("{name}, Chief, {state}, {open} waiting for Send")
        };
        let (mascot, color) = (chief.mascot.clone(), chief.color);
        Some(
            div()
                .flex()
                .flex_col()
                .when(compact, |d| d.items_center().gap(px(6.)))
                .child(
                    div()
                        .id("chief-rail")
                        .debug_selector(|| "chief-rail".into())
                        .role(Role::Button)
                        .aria_label(label)
                        .h(px(32.))
                        .when(original && !compact, |d| d.ml(px(6.)).mr_2())
                        .when(compact, |d| d.w(px(32.)))
                        .px_2()
                        .rounded_md()
                        .relative()
                        .flex()
                        .items_center()
                        .gap_2()
                        .opacity(0.85)
                        .hover(|s| s.bg(content(0.08)).opacity(1.))
                        .child(
                            div()
                                .size(px(16.))
                                .relative()
                                .flex_shrink_0()
                                .child(avatar::avatar(&mascot, color, state, 16., true))
                                .when(open > 0, |d| {
                                    d.child(
                                        div()
                                            .debug_selector(|| "chief-rail-badge".into())
                                            .absolute()
                                            .right(px(-3.))
                                            .top(px(-3.))
                                            .size(px(8.))
                                            .rounded_full()
                                            .bg(accent_color()),
                                    )
                                }),
                        )
                        .when(!compact, |d| {
                            d.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(14.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(name),
                            )
                            .child(chief_tag())
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open(CHIEF_ID, window, cx);
                        })),
                )
                .child(
                    div()
                        .h(px(1.))
                        .my_1()
                        .when(!compact, |d| d.mx(px(10.)))
                        .when(compact, |d| d.w(px(20.)))
                        .bg(content(0.1)),
                )
                .into_any_element(),
        )
    }
}

/// The small "Chief" tag that keeps it from looking like one more Mono.
pub(super) fn chief_tag() -> gpui::Div {
    div()
        .debug_selector(|| "chief-tag".into())
        .flex_shrink_0()
        .px(px(5.))
        .h(px(14.))
        .flex()
        .items_center()
        .rounded_sm()
        .bg(content(0.08))
        .text_size(px(9.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(content(0.55))
        .child("Chief")
}
