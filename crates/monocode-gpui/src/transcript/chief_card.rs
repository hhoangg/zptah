//! The Chief's proposal cards, drawn by the app in the Chief's chat, and the label a relayed
//! command carries in a Mono's chat. The transcript only draws what the Monos manager hands it
//! (`ChiefCards`); every click goes back to the manager as a `ChiefCardAction`, which checks
//! the proposal again before anything is sent.
use super::*;
use crate::input::Input;
use gpui::{AnyElement, EventEmitter};
use std::collections::HashMap;

/// How a state chip is tinted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Waiting on someone (the owner's click, a Mono, a queue).
    Wait,
    Good,
    Bad,
    Quiet,
}

/// One proposal as its card shows it.
#[derive(Clone, PartialEq)]
pub(crate) struct ChiefCard {
    pub id: String,
    /// `<Mono name> -> #<id> <title> · <project> · <branch>`, or `Directly -> ...`.
    pub route: String,
    /// The full confirmed (or proposed) text, never clipped.
    pub instruction: String,
    pub notes: Vec<String>,
    /// What a Mono's bound send really carried, when it differs from `instruction` (the Mono
    /// added context around the confirmed text). Shown under "Sent text".
    pub sent_text: Option<String>,
    /// The state chip's text.
    pub state: String,
    pub tone: Tone,
    /// Waiting for the owner's click: Send, Edit and Cancel show.
    pub open: bool,
    /// The last click's refusal, shown on the card.
    pub error: Option<String>,
    /// The inline editor while the owner edits the text.
    pub editor: Option<gpui::Entity<Input>>,
}

/// Every proposal the transcript may draw, and the Chief's name for the relay label.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct ChiefCards {
    pub name: String,
    pub cards: HashMap<String, ChiefCard>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChiefClick {
    Send,
    Edit,
    Cancel,
    Save,
    Discard,
}

/// A button on a card was clicked.
pub(crate) struct ChiefCardAction {
    pub id: String,
    pub click: ChiefClick,
}
impl EventEmitter<ChiefCardAction> for TranscriptView {}

/// Whether a tool call is the Chief proposing: the app tool (`mcp__monocode_app__app` with an
/// `action` input, how the Chief really calls it) or the shell CLI (`app chief.propose ...`).
fn proposes(tool: &ToolCall, input: &serde_json::Value) -> bool {
    let proposing = |action: &str| matches!(action, "chief.propose" | "chief.proposeStart");
    if monocode_core::tool::app_tool(&tool.name) {
        return input["action"].as_str().is_some_and(proposing);
    }
    input["command"]
        .as_str()
        .or_else(|| input["cmd"].as_str())
        .and_then(monocode_core::mono::work::command_action)
        .is_some_and(|action| proposing(&action))
}

/// The ids of the proposals the Chief's `chief.propose` and `chief.proposeStart` calls returned in this turn, in order.
pub(crate) fn run_proposals(
    source: &[Row],
    turn: usize,
    end: usize,
    known: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut at = turn;
    while at < end {
        let next = (at + 1..end).find(|&i| source[i].first).unwrap_or(end);
        if source[at].role == "tool" {
            let text: String = source[at..next].iter().map(|r| r.text.as_str()).collect();
            if let Ok(tool) = serde_json::from_str::<ToolCall>(&text)
                && tool.state == ToolState::Completed
                && let Ok(input) = serde_json::from_str::<serde_json::Value>(&tool.input)
                && proposes(&tool, &input)
            {
                let receipts = serde_json::from_str::<serde_json::Value>(&tool.output)
                    .map(|v| vec![v])
                    .unwrap_or_else(|_| {
                        tool.output
                            .lines()
                            .filter_map(|line| serde_json::from_str(line.trim()).ok())
                            .collect()
                    });
                for value in receipts {
                    if value["ok"] == true
                        && let Some(id) = value["result"]["proposalId"].as_str()
                        && known(id)
                        && !found.iter().any(|f| f == id)
                    {
                        found.push(id.to_owned());
                    }
                }
            }
        }
        at = next;
    }
    found
}

fn chip_colors(tone: Tone) -> (gpui::Hsla, gpui::Hsla) {
    match tone {
        Tone::Wait => (
            rgb(0xfbbf24).into(),
            gpui::Hsla::from(rgb(0xfbbf24)).opacity(0.12),
        ),
        Tone::Good => (
            rgb(0x34d399).into(),
            gpui::Hsla::from(rgb(0x34d399)).opacity(0.12),
        ),
        Tone::Bad => (
            rgb(0xf87171).into(),
            gpui::Hsla::from(rgb(0xf87171)).opacity(0.12),
        ),
        Tone::Quiet => (content(0.55), content(0.06)),
    }
}

impl TranscriptView {
    /// The proposals the Monos manager hands this chat. A change of what a card holds changes its
    /// height, so the list is laid out again.
    pub(crate) fn set_chief_cards(&mut self, cards: ChiefCards, cx: &mut Context<Self>) {
        if self.chief == cards {
            return;
        }
        self.chief = cards;
        self.rebuild(0, cx);
    }

    /// An open proposal no turn placed (its call is not in the loaded rows, or in a shape the
    /// transcript does not read) is drawn after the last turn, so Send is never unreachable.
    pub(super) fn push_unplaced_chief_cards(&mut self) {
        // The tail rows of an earlier build are decided again.
        while self.rows.last().is_some_and(|row| {
            matches!(row.body, Body::ChiefProposal(_)) && row.key.1 >= usize::MAX - 16
        }) {
            self.rows.pop();
        }
        if self.messages.is_empty() {
            return;
        }
        let mut open: Vec<&String> =
            self.chief
                .cards
                .values()
                .filter(|card| card.open)
                .map(|card| &card.id)
                .filter(|id| {
                    !self.rows.iter().any(
                        |row| matches!(&row.body, Body::ChiefProposal(placed) if placed == *id),
                    )
                })
                .collect();
        open.sort();
        let message = self.messages.len() - 1;
        let at = self.source.len();
        let rows: Vec<UiRow> = open
            .into_iter()
            .enumerate()
            .map(|(k, id)| UiRow {
                key: (at, usize::MAX - 16 + k.min(15)),
                body: Body::ChiefProposal(id.clone()),
                message,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            })
            .collect();
        self.rows.extend(rows);
    }

    /// How many rows draw this proposal, for the tests.
    #[cfg(test)]
    pub(crate) fn proposal_rows(&self, id: &str) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(&row.body, Body::ChiefProposal(p) if p == id))
            .count()
    }

    /// What the transcript was handed, for the tests.
    #[cfg(test)]
    pub(crate) fn chief_cards(&self) -> &ChiefCards {
        &self.chief
    }

    fn chief_click(&self, id: &str, click: ChiefClick, cx: &mut Context<Self>) {
        cx.emit(ChiefCardAction {
            id: id.to_owned(),
            click,
        });
    }

    /// A proposal's card row.
    pub(super) fn chief_card_row(
        &mut self,
        ix: usize,
        id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (top, bottom) = self.mono_card_spacing(ix);
        let body = match self.chief.cards.get(id).cloned() {
            Some(card) => self.chief_card(&card, cx),
            None => div().into_any_element(),
        };
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_w(px(COLUMN))
                    .px(px(COLUMN_PAD))
                    .pt(px(top))
                    .pb(px(bottom))
                    .child(body),
            )
            .into_any_element()
    }

    fn chief_card(&self, card: &ChiefCard, cx: &mut Context<Self>) -> AnyElement {
        let (ink, fill) = chip_colors(card.tone);
        let id = card.id.clone();
        let button = |name: &'static str, label: &'static str, primary: bool, click: ChiefClick| {
            let target = id.clone();
            let base = if primary {
                crate::settings::widgets::primary_button(
                    SharedString::from(format!("chief-{name}-{target}")),
                    label,
                    false,
                )
            } else {
                crate::settings::widgets::secondary_button(
                    SharedString::from(format!("chief-{name}-{target}")),
                    label,
                    false,
                    false,
                )
            };
            base.debug_selector({
                let key = format!("chief-{name}-{target}");
                move || key.clone()
            })
            .on_click(cx.listener(move |this, _, _, cx| this.chief_click(&target, click, cx)))
        };
        div()
            .id(SharedString::from(format!("chief-card-{}", card.id)))
            .debug_selector({
                let key = format!("chief-card-{}", card.id);
                move || key.clone()
            })
            .role(Role::Group)
            .aria_label("Command proposed by the Chief")
            .max_w(px(480.))
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_xl()
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.04))
            .child(
                div()
                    .debug_selector({
                        let key = format!("chief-route-{}", card.id);
                        move || key.clone()
                    })
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(content(0.6))
                    .child(card.route.clone()),
            )
            .child(match &card.editor {
                Some(editor) => div()
                    .debug_selector({
                        let key = format!("chief-editor-{}", card.id);
                        move || key.clone()
                    })
                    .min_h(px(72.))
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.15))
                    .bg(content(0.04))
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .child(editor.clone())
                    .into_any_element(),
                None => div()
                    .debug_selector({
                        let key = format!("chief-text-{}", card.id);
                        move || key.clone()
                    })
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(content(0.95))
                    .child(card.instruction.clone())
                    .into_any_element(),
            })
            .children(card.notes.iter().map(|note| {
                div()
                    .text_size(px(11.5))
                    .line_height(px(16.))
                    .text_color(rgb(0xfbbf24))
                    .child(note.clone())
            }))
            .when_some(card.sent_text.clone(), |d, sent| {
                let open = self.chief_sent_open.contains(&card.id);
                let target = card.id.clone();
                d.child(
                    div()
                        .id(SharedString::from(format!("chief-sent-{}", card.id)))
                        .debug_selector({
                            let key = format!("chief-sent-{}", card.id);
                            move || key.clone()
                        })
                        .role(Role::Button)
                        .aria_label("Sent text")
                        .cursor_pointer()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(content(0.6))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.chief_sent_open.remove(&target) {
                                this.chief_sent_open.insert(target.clone());
                            }
                            this.rebuild(0, cx);
                        }))
                        .child(format!(
                            "Sent text {}",
                            if open { "\u{25be}" } else { "\u{25b8}" }
                        ))
                        .when(open, |d| {
                            d.child(
                                div()
                                    .debug_selector({
                                        let key = format!("chief-sent-text-{}", card.id);
                                        move || key.clone()
                                    })
                                    .text_color(content(0.8))
                                    .child(sent),
                            )
                        }),
                )
            })
            .when_some(card.error.clone(), |d, error| {
                d.child(
                    div()
                        .debug_selector({
                            let key = format!("chief-error-{}", card.id);
                            move || key.clone()
                        })
                        .text_size(px(11.5))
                        .line_height(px(16.))
                        .text_color(rgb(0xf87171))
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .debug_selector({
                                let key = format!("chief-state-{}", card.id);
                                move || key.clone()
                            })
                            .flex_shrink_0()
                            .px_2()
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .bg(fill)
                            .text_size(px(11.))
                            .text_color(ink)
                            .child(card.state.clone()),
                    )
                    .child(div().flex_1())
                    .when(card.open && card.editor.is_some(), |d| {
                        d.child(button("discard", "Discard", false, ChiefClick::Discard))
                            .child(button("save", "Save", true, ChiefClick::Save))
                    })
                    .when(card.open && card.editor.is_none(), |d| {
                        d.child(button("cancel", "Cancel", false, ChiefClick::Cancel))
                            .child(button("edit", "Edit", false, ChiefClick::Edit))
                            .child(button("send", "Send", true, ChiefClick::Send))
                    }),
            )
            .into_any_element()
    }

    /// The internal turn that carried a Chief command, drawn as the owner's own words.
    pub(super) fn chief_relay_row(&mut self, ix: usize, id: &str) -> AnyElement {
        let (top, bottom) = self.mono_card_spacing(ix);
        let name = if self.chief.name.is_empty() {
            "the Chief".to_owned()
        } else {
            self.chief.name.clone()
        };
        let text = self
            .chief
            .cards
            .get(id)
            .map(|card| card.instruction.clone())
            .unwrap_or_default();
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_w(px(COLUMN))
                    .px(px(COLUMN_PAD))
                    .pt(px(top.max(8.)))
                    .pb(px(bottom))
                    .child(
                        div()
                            .id(SharedString::from(format!("chief-relay-{id}")))
                            .debug_selector({
                                let key = format!("chief-relay-{id}");
                                move || key.clone()
                            })
                            .role(Role::Group)
                            .aria_label(format!("From you, via {name}"))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .line_height(px(16.))
                                    .text_color(content(0.5))
                                    .child(format!("From you, via {name}")),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(20.))
                                    .text_color(content(0.85))
                                    .child(text),
                            ),
                    ),
            )
            .into_any_element()
    }
}
