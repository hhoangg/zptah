//! The zptah Browser approval surface of a Mono's conversation (spec 8.2): the approval cards, the
//! password stop notices, the stopped banner and the rule review prompt. It sits on the ledge above
//! the composer and never takes focus: the user reaches it with the mouse or by tabbing to it.
//!
//! The data comes from `monos::browser_approval`, which watches the Mono's approval queue and
//! pushes a [`Surface`] here. Answers go straight to the queue; Resume and the rule prompt are
//! emitted as [`BrowserAction`] for the Mono manager, which owns the link and the database.
use super::composer_shelf;
use super::*;
use gpui::{Div, Stateful};
use monocode_core::mono::browser::copy::{self, Lang};
use monocode_core::mono::browser::trust::approval::{self, Notice, PendingApproval, UserAnswer};

/// The rule review prompt of a Mono: the soul's browser rules changed (spec 7.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReviewPrompt {
    pub mono_label: String,
    pub changed_lines: Vec<String>,
}

/// Everything the surface shows.
#[derive(Clone, Default)]
pub(crate) struct Surface {
    pub cards: Vec<PendingApproval>,
    pub notices: Vec<Notice>,
    pub stopped: bool,
    pub review: Option<ReviewPrompt>,
}

impl Surface {
    pub(crate) fn is_empty(&self) -> bool {
        self.cards.is_empty() && self.notices.is_empty() && !self.stopped && self.review.is_none()
    }
}

/// What the user pressed that the conversation cannot do itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowserAction {
    /// Resume on the stopped banner.
    Resume,
    /// "Confirm the new rules".
    ConfirmRules,
    /// "Keep the old rules": hide the prompt, change nothing.
    KeepRules,
}
impl EventEmitter<BrowserAction> for Conversation {}

pub(super) struct State {
    pub(super) mono_id: Option<String>,
    pub(super) surface: Surface,
    focus: gpui::FocusHandle,
}

impl State {
    pub(super) fn new(cx: &mut Context<Conversation>) -> Self {
        Self {
            mono_id: None,
            surface: Surface::default(),
            focus: cx.focus_handle(),
        }
    }
}

/// `m:ss` left until `expires_ms`, or `None` when it has passed.
fn remaining(expires_ms: u64, now_ms: u64) -> Option<String> {
    let left = expires_ms.checked_sub(now_ms)? / 1000;
    Some(format!("{}:{:02}", left / 60, left % 60))
}

fn countdown_text(lang: Lang, clock: &str) -> String {
    match lang {
        Lang::En => format!("Denied automatically in {clock}"),
        Lang::Vi => format!("Tự động từ chối sau {clock}"),
    }
}

fn show_picture_label(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Page when the Mono asked",
        Lang::Vi => "Trang lúc Mono hỏi",
    }
}

fn dismiss_label(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Dismiss",
        Lang::Vi => "Đóng",
    }
}

fn button(id: SharedString, label: &str) -> Stateful<Div> {
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .role(Role::Button)
        .aria_label(label.to_owned())
        .cursor_pointer()
        .flex_shrink_0()
        .h(px(26.))
        .px(px(10.))
        .rounded_md()
        .flex()
        .items_center()
        .text_size(px(12.))
        .child(label.to_owned())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

impl Conversation {
    /// Tells the conversation which Mono it belongs to (the approval queue is the Mono's).
    pub(crate) fn set_browser_mono(&mut self, mono_id: &str) {
        self.browser.mono_id = Some(mono_id.to_owned());
    }

    /// What the browser surface shows now.
    #[cfg(test)]
    pub(crate) fn browser_surface(&self) -> &Surface {
        &self.browser.surface
    }

    /// Replaces what the browser surface shows.
    pub(crate) fn set_browser_surface(&mut self, surface: Surface, cx: &mut Context<Self>) {
        self.browser.surface = surface;
        cx.notify();
    }

    /// Open browser cards and notices, for the attention state and the notification.
    pub(crate) fn browser_inputs(&self) -> Vec<crate::notifications::approvals::PendingInput> {
        use crate::notifications::approvals::{InputKind, PendingInput};
        let lang = copy::app_language();
        // A Question has no Allow button in the toast: the card itself is the only place to answer.
        self.browser
            .surface
            .cards
            .iter()
            .map(|c| PendingInput {
                request_id: c.id.clone(),
                kind: InputKind::Question,
                label: c.text_in(lang).headline,
            })
            .chain(self.browser.surface.notices.iter().map(|n| PendingInput {
                request_id: n.id.clone(),
                kind: InputKind::Question,
                label: n.text_in(lang).headline,
            }))
            .collect()
    }

    pub(crate) fn browser_needs_attention(&self) -> bool {
        !self.browser.surface.cards.is_empty() || !self.browser.surface.notices.is_empty()
    }

    /// Delivers an answer to the card. False when the card is gone (answered, timed out, stopped).
    pub(crate) fn browser_answer(
        &mut self,
        id: &str,
        answer: UserAnswer,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mono_id) = self.browser.mono_id.clone() else {
            return false;
        };
        let delivered = approval::queue_for(&mono_id).answer(id, answer);
        self.browser.surface.cards.retain(|c| c.id != id);
        cx.notify();
        delivered
    }

    fn browser_dismiss(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(mono_id) = self.browser.mono_id.clone() {
            approval::queue_for(&mono_id).dismiss_notice(id);
        }
        self.browser.surface.notices.retain(|n| n.id != id);
        cx.notify();
    }

    fn browser_card(&self, card: &PendingApproval, cx: &mut Context<Self>) -> AnyElement {
        let lang = copy::app_language();
        let text = card.text_in(lang);
        let id = card.id.clone();
        let (once_id, conv_id, deny_id, key_id) = (id.clone(), id.clone(), id.clone(), id.clone());
        let mut ink = shell_fill();
        ink.a = 1.;
        let mut actions = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                button("browser-card-deny".into(), text.deny)
                    .text_color(content(0.5))
                    .hover(|s| s.bg(content(0.08)).text_color(content(0.9)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.browser_answer(&deny_id, UserAnswer::Deny, cx);
                    })),
            );
        if let Some(label) = text.allow_conversation {
            actions = actions.child(
                button("browser-card-conversation".into(), label)
                    .bg(content(0.06))
                    .hover(|s| s.bg(content(0.1)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.browser_answer(&conv_id, UserAnswer::AllowConversation, cx);
                    })),
            );
        }
        actions = actions.child(
            button("browser-card-once".into(), text.allow_once)
                .bg(content(1.))
                .text_color(ink)
                .font_weight(gpui::FontWeight::MEDIUM)
                .hover(move |s| s.bg(content(0.85)).text_color(ink))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.browser_answer(&once_id, UserAnswer::AllowOnce, cx);
                })),
        );
        let now = monocode_core::time::now_ms();
        let clock = remaining(card.expires_ms, now);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .py(px(6.))
            .child(
                div()
                    .debug_selector(|| "browser-card-headline".into())
                    .text_color(content(0.9))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(text.headline.clone()),
            )
            .children(text.lines.iter().cloned().map(|line| {
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.7))
                    .child(line)
            }));
        if let Some(purpose) = text.purpose.clone() {
            body = body.child(
                div()
                    .debug_selector(|| "browser-card-purpose".into())
                    .border_l_2()
                    .border_color(content(0.15))
                    .pl_2()
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child(purpose),
            );
        }
        if let Some(shot) = &card.screenshot {
            body = body.child(
                div()
                    .debug_selector(|| "browser-card-picture".into())
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child(show_picture_label(lang)),
                    )
                    .child(
                        gpui::img(shot.path.clone())
                            .max_h(px(160.))
                            .max_w(px(320.))
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.1)),
                    ),
            );
        }
        if let Some(clock) = clock {
            body = body.child(
                div()
                    .debug_selector(|| "browser-card-countdown".into())
                    .text_size(px(11.))
                    .text_color(content(0.4))
                    .child(countdown_text(lang, &clock)),
            );
        }
        let focus = self.browser.focus.clone();
        div()
            .id(SharedString::from(format!("browser-card-{id}")))
            .debug_selector(|| "browser-card".into())
            .role(Role::Group)
            .aria_label(text.headline.clone())
            .track_focus(&focus)
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                // Only while the user put focus on the card: Enter allows once, Escape denies.
                match event.keystroke.key.as_str() {
                    "enter" => {
                        this.browser_answer(&key_id, UserAnswer::AllowOnce, cx);
                    }
                    "escape" => {
                        this.browser_answer(&key_id, UserAnswer::Deny, cx);
                    }
                    _ => {}
                }
            }))
            .flex()
            .items_start()
            .gap(px(10.))
            .child(
                icon("square-lock", 14.)
                    .text_color(gpui::rgb(0xfbbf24))
                    .mt(px(9.)),
            )
            .child(div().min_w_0().flex_1().child(body))
            .child(div().pt(px(6.)).child(actions))
            .into_any_element()
    }

    fn browser_notice(&self, notice: &Notice, cx: &mut Context<Self>) -> AnyElement {
        let lang = copy::app_language();
        let text = notice.text_in(lang);
        let id = notice.id.clone();
        div()
            .id(SharedString::from(format!("browser-notice-{id}")))
            .debug_selector(|| "browser-notice".into())
            .role(Role::Group)
            .aria_label(text.headline.clone())
            .flex()
            .items_start()
            .gap(px(10.))
            .py(px(6.))
            .child(
                icon("circle-alert", 14.)
                    .text_color(gpui::rgb(0xffb900))
                    .mt(px(3.)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_color(content(0.9))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(text.headline.clone()),
                    )
                    .children(text.lines.iter().cloned().map(|line| {
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(content(0.7))
                            .child(line)
                    })),
            )
            .child(
                button("browser-notice-dismiss".into(), dismiss_label(lang))
                    .bg(content(0.06))
                    .hover(|s| s.bg(content(0.1)))
                    .on_click(cx.listener(move |this, _, _, cx| this.browser_dismiss(&id, cx))),
            )
            .into_any_element()
    }

    fn browser_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let (message, resume) = copy::stopped_banner(copy::app_language());
        div()
            .id("browser-stopped-banner")
            .debug_selector(|| "browser-stopped-banner".into())
            .role(Role::Group)
            .aria_label(message)
            .flex()
            .items_center()
            .gap(px(10.))
            .py(px(6.))
            .child(icon("circle-alert", 14.).text_color(gpui::rgb(0xffb900)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_color(content(0.8))
                    .child(message),
            )
            .child(
                button("browser-resume".into(), resume)
                    .bg(content(1.))
                    .text_color({
                        let mut ink = shell_fill();
                        ink.a = 1.;
                        ink
                    })
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(BrowserAction::Resume))),
            )
            .into_any_element()
    }

    fn browser_review(&self, prompt: &ReviewPrompt, cx: &mut Context<Self>) -> AnyElement {
        let text = copy::rule_review(
            copy::app_language(),
            &prompt.mono_label,
            &prompt.changed_lines,
        );
        div()
            .id("browser-rule-review")
            .debug_selector(|| "browser-rule-review".into())
            .role(Role::Group)
            .aria_label(text.headline.clone())
            .flex()
            .items_start()
            .gap(px(10.))
            .py(px(6.))
            .child(
                icon("square-lock", 14.)
                    .text_color(gpui::rgb(0xfbbf24))
                    .mt(px(3.)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_color(content(0.9))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(text.headline.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(content(0.7))
                            .child(text.line.clone()),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .gap(px(6.))
                    .child(
                        button("browser-rules-keep".into(), text.keep)
                            .text_color(content(0.5))
                            .hover(|s| s.bg(content(0.08)))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(BrowserAction::KeepRules))),
                    )
                    .child(
                        button("browser-rules-confirm".into(), text.confirm)
                            .bg(content(0.06))
                            .hover(|s| s.bg(content(0.1)))
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(BrowserAction::ConfirmRules)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The ledge above the composer; an empty element when there is nothing to show.
    pub(super) fn browser_shelf(&self, cx: &mut Context<Self>) -> AnyElement {
        let surface = self.browser.surface.clone();
        if surface.is_empty() {
            return div().into_any_element();
        }
        let mut rows: Vec<AnyElement> = Vec::new();
        if surface.stopped {
            rows.push(self.browser_banner(cx));
        }
        if let Some(prompt) = &surface.review {
            rows.push(self.browser_review(prompt, cx));
        }
        for notice in &surface.notices {
            rows.push(self.browser_notice(notice, cx));
        }
        for card in &surface.cards {
            rows.push(self.browser_card(card, cx));
        }
        composer_shelf::shelf("browser-shelf")
            .debug_selector(|| "browser-shelf".into())
            .child(
                composer_shelf::card("browser-surface", self.classic_layout)
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, WindowHandle};
    use monocode_core::mono::browser::copy::{ActionDesc, CardFacts};
    use monocode_core::mono::browser::risk::Category;
    use monocode_core::mono::browser::trust::approval::{Outcome, Request};
    use monocode_core::store::Store;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn conversation(cx: &mut TestAppContext, mono: &str) -> WindowHandle<Conversation> {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("codex", "/tmp").unwrap();
        let mono = mono.to_owned();
        cx.update(|cx| {
            input::bind(cx);
            cx.set_reduce_motion(true);
            cx.set_global(crate::settings::Preferences {
                composer_mascot: false,
                follow_up: "queue".into(),
                ..Default::default()
            });
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                    view.loaded = true;
                    view.summary.has_messages = true;
                    view.set_browser_mono(&mono);
                    view
                })
            })
            .unwrap()
        })
    }

    fn ask(
        mono: &str,
        offer: bool,
        timeout: Duration,
    ) -> std::thread::JoinHandle<(String, Outcome)> {
        let queue = approval::queue_for(mono);
        let mut facts = CardFacts::new(
            "Mira",
            "airbnb.com",
            ActionDesc::Click {
                label: "Book now".into(),
            },
            vec![Category::Sharing],
        );
        facts.purpose = Some("Reserve the flat".into());
        let request = Request {
            mono_id: mono.into(),
            session_id: 1,
            tool: "browser_click".into(),
            tab: Some(1),
            facts,
            screenshot: None,
            offer_conversation: offer,
        };
        std::thread::spawn(move || queue.ask(request, timeout, Duration::from_millis(10), || {}))
    }

    fn wait_pending(mono: &str) -> Vec<PendingApproval> {
        let queue = approval::queue_for(mono);
        for _ in 0..500 {
            if !queue.pending().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        queue.pending()
    }

    fn show(window: &WindowHandle<Conversation>, visual: &mut VisualTestContext, surface: Surface) {
        window
            .update(visual, |view, _, cx| view.set_browser_surface(surface, cx))
            .unwrap();
        visual.refresh().unwrap();
    }

    #[test]
    fn the_countdown_reads_minutes_and_seconds_and_ends_at_zero() {
        assert_eq!(remaining(10_000 + 592_000, 10_000).as_deref(), Some("9:52"));
        assert_eq!(remaining(5_000, 5_000).as_deref(), Some("0:00"));
        assert_eq!(remaining(5_000, 5_001), None);
    }

    #[gpui::test]
    fn the_card_shows_its_parts_and_each_button_answers_the_queue(cx: &mut TestAppContext) {
        let mono = format!("card-buttons-{}", monocode_core::mono::new_id());
        let window = conversation(cx, &mono);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let _listening = approval::queue_for(&mono).subscribe(|| {});
        assert!(!visual.debug_bounds("browser-shelf").is_some());
        for (button, expected) in [
            ("browser-card-once", UserAnswer::AllowOnce),
            ("browser-card-conversation", UserAnswer::AllowConversation),
            ("browser-card-deny", UserAnswer::Deny),
        ] {
            let asked = ask(&mono, true, Duration::from_secs(30));
            let pending = wait_pending(&mono);
            show(
                &window,
                visual,
                Surface {
                    cards: pending,
                    ..Default::default()
                },
            );
            for part in [
                "browser-shelf",
                "browser-card",
                "browser-card-headline",
                "browser-card-purpose",
                "browser-card-countdown",
                "browser-card-once",
                "browser-card-conversation",
                "browser-card-deny",
            ] {
                assert!(visual.debug_bounds(part).is_some(), "{part} is on screen");
            }
            let center = visual.debug_bounds(button).unwrap().center();
            visual.simulate_click(center, Default::default());
            visual.refresh().unwrap();
            assert_eq!(asked.join().unwrap().1, Outcome::Answered(expected));
            assert!(visual.debug_bounds("browser-card").is_none());
        }
    }

    #[gpui::test]
    fn the_conversation_button_is_hidden_when_the_card_does_not_offer_it(cx: &mut TestAppContext) {
        let mono = format!("card-hidden-{}", monocode_core::mono::new_id());
        let window = conversation(cx, &mono);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let _listening = approval::queue_for(&mono).subscribe(|| {});
        let asked = ask(&mono, false, Duration::from_secs(30));
        let pending = wait_pending(&mono);
        show(
            &window,
            visual,
            Surface {
                cards: pending.clone(),
                ..Default::default()
            },
        );
        assert!(visual.debug_bounds("browser-card-once").is_some());
        assert!(visual.debug_bounds("browser-card-conversation").is_none());
        // The card did not take focus.
        window
            .update(visual, |view, window, _| {
                assert!(!view.browser.focus.is_focused(window))
            })
            .unwrap();
        approval::queue_for(&mono).answer(&pending[0].id, UserAnswer::Deny);
        asked.join().unwrap();
    }

    #[gpui::test]
    fn a_timed_out_card_is_a_deny_and_leaves_the_surface(cx: &mut TestAppContext) {
        let mono = format!("card-timeout-{}", monocode_core::mono::new_id());
        let window = conversation(cx, &mono);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let queue = approval::queue_for(&mono);
        let _listening = queue.subscribe(|| {});
        let asked = ask(&mono, true, Duration::from_millis(150));
        let pending = wait_pending(&mono);
        show(
            &window,
            visual,
            Surface {
                cards: pending,
                ..Default::default()
            },
        );
        assert!(visual.debug_bounds("browser-card").is_some());
        assert_eq!(asked.join().unwrap().1, Outcome::TimedOut);
        show(
            &window,
            visual,
            Surface {
                cards: queue.pending(),
                ..Default::default()
            },
        );
        assert!(visual.debug_bounds("browser-card").is_none());
        assert!(!queue.answer("gone", UserAnswer::AllowOnce));
    }

    #[gpui::test]
    fn the_banner_the_review_prompt_and_the_notice_emit_their_actions(cx: &mut TestAppContext) {
        let mono = format!("card-banner-{}", monocode_core::mono::new_id());
        let window = conversation(cx, &mono);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let queue = approval::queue_for(&mono);
        let _listening = queue.subscribe(|| {});
        queue.post_notice(
            approval::NoticeKind::PasswordNeeded,
            Some(4),
            "Mira",
            "accounts.example.com",
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let entity = window.entity(visual).unwrap();
        let _sub = visual.update(|_, cx| {
            cx.subscribe(&entity, move |_, event: &BrowserAction, _| {
                log.lock().unwrap().push(*event)
            })
        });
        show(
            &window,
            visual,
            Surface {
                notices: queue.notices(),
                stopped: true,
                review: Some(ReviewPrompt {
                    mono_label: "Mira".into(),
                    changed_lines: vec!["ask: click \"Pay\" on shop.example".into()],
                }),
                ..Default::default()
            },
        );
        for button in [
            "browser-resume",
            "browser-rules-confirm",
            "browser-rules-keep",
        ] {
            let center = visual.debug_bounds(button).unwrap().center();
            visual.simulate_click(center, Default::default());
            visual.refresh().unwrap();
        }
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                BrowserAction::Resume,
                BrowserAction::ConfirmRules,
                BrowserAction::KeepRules
            ]
        );
        assert!(visual.debug_bounds("browser-notice").is_some());
        let center = visual
            .debug_bounds("browser-notice-dismiss")
            .unwrap()
            .center();
        visual.simulate_click(center, Default::default());
        visual.refresh().unwrap();
        assert!(queue.notices().is_empty());
        assert!(visual.debug_bounds("browser-notice").is_none());
    }
}
