//! The zptah Browser approval surface of a Mono's conversation (spec 8.2): the approval cards, the
//! password stop notices, the "Working in Ardia" row with its Stop button and the rule review prompt. It sits on the ledge above
//! the composer and never takes focus: the user reaches it with the mouse or by tabbing to it.
//!
//! The data comes from `monos::browser_approval`, which watches the Mono's approval queue and
//! pushes a [`Surface`] here. Answers go straight to the queue; Stop and the rule prompt are
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
    /// A browser call of this conversation is running (including one waiting for an approval):
    /// the "Working in Ardia" row with its Stop button shows.
    pub working: bool,
    pub review: Option<ReviewPrompt>,
    /// An Ardia that zptah started is running: its indicator and Quit button show.
    pub ardia_running: bool,
}

impl Surface {
    pub(crate) fn is_empty(&self) -> bool {
        self.cards.is_empty()
            && self.notices.is_empty()
            && !self.working
            && self.review.is_none()
            && !self.ardia_running
    }
}

/// What the user pressed that the conversation cannot do itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowserAction {
    /// "Stop" on the "Working in Ardia" row: the Mono's browser work of this turn ends.
    StopBrowser,
    /// "Confirm the new rules".
    ConfirmRules,
    /// "Keep the old rules": hide the prompt, change nothing.
    KeepRules,
    /// "Quit" on the row of an Ardia that zptah started.
    QuitArdia,
}
impl EventEmitter<BrowserAction> for Conversation {}

pub(super) struct State {
    pub(super) mono_id: Option<String>,
    pub(super) surface: Surface,
    /// One handle per open card, so Enter and Escape only ever answer the focused card.
    focus: std::cell::RefCell<std::collections::HashMap<String, gpui::FocusHandle>>,
    /// When each open card was first drawn, and when the user last answered one: clicks inside
    /// [`ACTIVATION_DELAY`] of either are ignored, so a double-click on a card that is just being
    /// replaced by the next one cannot answer the new card.
    first_drawn: std::cell::RefCell<std::collections::HashMap<String, std::time::Instant>>,
    last_answered: std::cell::Cell<Option<std::time::Instant>>,
    delay: std::time::Duration,
}

/// How long a card ignores clicks after it first appears or after the previous card was answered.
pub(crate) const ACTIVATION_DELAY: std::time::Duration = std::time::Duration::from_millis(600);

impl State {
    pub(super) fn new() -> Self {
        Self {
            mono_id: None,
            surface: Surface::default(),
            focus: Default::default(),
            first_drawn: Default::default(),
            last_answered: Default::default(),
            delay: ACTIVATION_DELAY,
        }
    }

    /// Whether a click on card `id` must be ignored now: it appeared, or the previous card was
    /// answered, less than the activation delay ago. A card that was never drawn cannot have been
    /// clicked, so it is not held.
    fn held(&self, id: &str) -> bool {
        let recent = |at: std::time::Instant| at.elapsed() < self.delay;
        self.first_drawn
            .borrow()
            .get(id)
            .is_some_and(|at| recent(*at))
            || self.last_answered.get().is_some_and(recent)
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
    /// The focus handle of an open card (tests focus it the way a Tab key would).
    #[cfg(test)]
    pub(crate) fn browser_card_focus(&self, id: &str) -> Option<gpui::FocusHandle> {
        self.browser.focus.borrow().get(id).cloned()
    }

    /// Tells the conversation which Mono it belongs to (the approval queue is the Mono's).
    pub(crate) fn set_browser_mono(&mut self, mono_id: &str) {
        self.browser.mono_id = Some(mono_id.to_owned());
    }

    /// Sets the activation delay (tests use a short one, or none).
    #[cfg(test)]
    pub(crate) fn set_browser_activation_delay(&mut self, delay: std::time::Duration) {
        self.browser.delay = delay;
    }

    /// What the browser surface shows now.
    #[cfg(test)]
    pub(crate) fn browser_surface(&self) -> &Surface {
        &self.browser.surface
    }

    /// Replaces what the browser surface shows.
    pub(crate) fn set_browser_surface(&mut self, surface: Surface, cx: &mut Context<Self>) {
        self.browser
            .first_drawn
            .borrow_mut()
            .retain(|id, _| surface.cards.iter().any(|c| &c.id == id));
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

    /// An answer the user gave with the mouse or keyboard. Ignored while the card is held (see
    /// [`ACTIVATION_DELAY`]); otherwise [`Self::browser_answer`], remembering when it happened.
    fn browser_user_answer(&mut self, id: &str, answer: UserAnswer, cx: &mut Context<Self>) {
        if self.browser.held(id) {
            return;
        }
        if self.browser_answer(id, answer, cx) {
            self.browser
                .last_answered
                .set(Some(std::time::Instant::now()));
        }
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
        self.browser
            .first_drawn
            .borrow_mut()
            .entry(id.clone())
            .or_insert_with(std::time::Instant::now);
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
                        this.browser_user_answer(&deny_id, UserAnswer::Deny, cx);
                    })),
            );
        if let Some(label) = text.allow_conversation {
            actions = actions.child(
                button("browser-card-conversation".into(), label)
                    .bg(content(0.06))
                    .hover(|s| s.bg(content(0.1)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.browser_user_answer(&conv_id, UserAnswer::AllowConversation, cx);
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
                    this.browser_user_answer(&once_id, UserAnswer::AllowOnce, cx);
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
        let focus = self
            .browser
            .focus
            .borrow_mut()
            .entry(card.id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
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
                        this.browser_user_answer(&key_id, UserAnswer::AllowOnce, cx);
                    }
                    "escape" => {
                        this.browser_user_answer(&key_id, UserAnswer::Deny, cx);
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

    /// "Working in Ardia" with its Stop button.
    fn browser_working_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let (message, stop) = match copy::app_language() {
            Lang::En => ("Working in Ardia", "Stop"),
            Lang::Vi => ("Đang làm việc trong Ardia", "Dừng"),
        };
        div()
            .id("browser-working")
            .debug_selector(|| "browser-working".into())
            .role(Role::Group)
            .aria_label(message)
            .flex()
            .items_center()
            .gap(px(10.))
            .py(px(6.))
            .child(icon("globe", 14.).text_color(content(0.6)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_color(content(0.8))
                    .child(message),
            )
            .child(
                button("browser-stop".into(), stop)
                    .bg(content(0.06))
                    .hover(|s| s.bg(content(0.1)))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(BrowserAction::StopBrowser))),
            )
            .into_any_element()
    }

    /// "Ardia running (started by zptah)" with its Quit button.
    fn browser_ardia_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let (message, quit) = match copy::app_language() {
            Lang::En => ("Ardia running (started by zptah)", "Quit"),
            Lang::Vi => ("Ardia đang chạy (do zptah mở)", "Thoát"),
        };
        div()
            .id("browser-ardia-running")
            .debug_selector(|| "browser-ardia-running".into())
            .role(Role::Group)
            .aria_label(message)
            .flex()
            .items_center()
            .gap(px(10.))
            .py(px(6.))
            .child(icon("globe", 14.).text_color(content(0.6)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_color(content(0.8))
                    .child(message),
            )
            .child(
                button("browser-ardia-quit".into(), quit)
                    .bg(content(0.06))
                    .hover(|s| s.bg(content(0.1)))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(BrowserAction::QuitArdia))),
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
        self.browser
            .focus
            .borrow_mut()
            .retain(|id, _| surface.cards.iter().any(|c| &c.id == id));
        if surface.is_empty() {
            return div().into_any_element();
        }
        let mut rows: Vec<AnyElement> = Vec::new();
        if surface.working {
            rows.push(self.browser_working_row(cx));
        }
        if surface.ardia_running {
            rows.push(self.browser_ardia_row(cx));
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
        conversation_held(cx, mono, Duration::ZERO)
    }

    fn conversation_held(
        cx: &mut TestAppContext,
        mono: &str,
        delay: Duration,
    ) -> WindowHandle<Conversation> {
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
                    view.set_browser_activation_delay(delay);
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
    fn a_card_ignores_clicks_until_its_activation_delay_has_passed(cx: &mut TestAppContext) {
        let mono = format!("card-held-{}", monocode_core::mono::new_id());
        let delay = Duration::from_millis(400);
        let window = conversation_held(cx, &mono, delay);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let queue = approval::queue_for(&mono);
        let _listening = queue.subscribe(|| {});
        let asked = ask(&mono, true, Duration::from_secs(30));
        let pending = wait_pending(&mono);
        show(
            &window,
            visual,
            Surface {
                cards: pending.clone(),
                ..Default::default()
            },
        );
        // A double-click right after the card appeared answers nothing.
        for _ in 0..2 {
            let center = visual.debug_bounds("browser-card-once").unwrap().center();
            visual.simulate_click(center, Default::default());
            visual.refresh().unwrap();
        }
        assert_eq!(queue.pending().len(), 1, "the card is still open");
        assert!(visual.debug_bounds("browser-card").is_some());
        std::thread::sleep(delay + Duration::from_millis(100));
        let center = visual.debug_bounds("browser-card-once").unwrap().center();
        visual.simulate_click(center, Default::default());
        visual.refresh().unwrap();
        assert_eq!(
            asked.join().unwrap().1,
            Outcome::Answered(UserAnswer::AllowOnce)
        );
    }

    #[gpui::test]
    fn a_click_right_after_answering_a_card_does_not_answer_the_next_one(cx: &mut TestAppContext) {
        let mono = format!("card-next-{}", monocode_core::mono::new_id());
        let delay = Duration::from_millis(400);
        let window = conversation_held(cx, &mono, delay);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let queue = approval::queue_for(&mono);
        let _listening = queue.subscribe(|| {});
        let first = ask(&mono, false, Duration::from_secs(30));
        wait_pending(&mono);
        let second = ask(&mono, false, Duration::from_secs(30));
        for _ in 0..500 {
            if queue.pending().len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let pending = queue.pending();
        assert_eq!(pending.len(), 2);
        // Both are drawn, then the delay passes: only the answer of the first starts a new hold.
        show(
            &window,
            visual,
            Surface {
                cards: pending.clone(),
                ..Default::default()
            },
        );
        std::thread::sleep(delay + Duration::from_millis(100));
        let click = |visual: &mut VisualTestContext| {
            let center = visual.debug_bounds("browser-card-deny").unwrap().center();
            visual.simulate_click(center, Default::default());
            visual.refresh().unwrap();
        };
        click(visual);
        assert_eq!(queue.pending().len(), 1, "the first card was answered");
        click(visual);
        assert_eq!(
            queue.pending().len(),
            1,
            "the second card ignored the follow-up click"
        );
        std::thread::sleep(delay + Duration::from_millis(100));
        click(visual);
        assert!(queue.pending().is_empty());
        first.join().unwrap();
        second.join().unwrap();
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
                let handle = view.browser_card_focus(&pending[0].id).unwrap();
                assert!(!handle.is_focused(window))
            })
            .unwrap();
        approval::queue_for(&mono).answer(&pending[0].id, UserAnswer::Deny);
        asked.join().unwrap();
    }

    #[gpui::test]
    fn enter_and_escape_answer_only_the_focused_card(cx: &mut TestAppContext) {
        let mono = format!("card-keys-{}", monocode_core::mono::new_id());
        let window = conversation(cx, &mono);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let queue = approval::queue_for(&mono);
        let _listening = queue.subscribe(|| {});
        let first = ask(&mono, true, Duration::from_secs(30));
        wait_pending(&mono);
        let second = ask(&mono, true, Duration::from_secs(30));
        for _ in 0..500 {
            if queue.pending().len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let pending = queue.pending();
        assert_eq!(pending.len(), 2);
        show(
            &window,
            visual,
            Surface {
                cards: pending.clone(),
                ..Default::default()
            },
        );
        // The composer has focus: Enter answers nothing.
        window
            .update(visual, |view, window, cx| {
                window.focus(&view.input.focus_handle(cx), cx)
            })
            .unwrap();
        visual.simulate_keystrokes("enter");
        assert_eq!(queue.pending().len(), 2);
        // Focus the second card the way Tab would: only it answers.
        let id = pending[1].id.clone();
        window
            .update(visual, |view, window, cx| {
                let handle = view.browser_card_focus(&id).unwrap();
                window.focus(&handle, cx)
            })
            .unwrap();
        visual.simulate_keystrokes("enter");
        let left = queue.pending();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, pending[0].id);
        assert_eq!(
            second.join().unwrap().1,
            Outcome::Answered(UserAnswer::AllowOnce)
        );
        queue.answer(&pending[0].id, UserAnswer::Deny);
        first.join().unwrap();
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
    fn the_review_prompt_and_the_notice_emit_their_actions(cx: &mut TestAppContext) {
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
                review: Some(ReviewPrompt {
                    mono_label: "Mira".into(),
                    changed_lines: vec!["ask: click \"Pay\" on shop.example".into()],
                }),
                ..Default::default()
            },
        );
        for button in ["browser-rules-confirm", "browser-rules-keep"] {
            let center = visual.debug_bounds(button).unwrap().center();
            visual.simulate_click(center, Default::default());
            visual.refresh().unwrap();
        }
        assert_eq!(
            *seen.lock().unwrap(),
            vec![BrowserAction::ConfirmRules, BrowserAction::KeepRules]
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

    #[gpui::test]
    fn the_ardia_row_shows_for_an_ardia_zptah_started_and_quit_emits_its_action(
        cx: &mut TestAppContext,
    ) {
        let mono = format!("card-ardia-{}", monocode_core::mono::new_id());
        let window = conversation(cx, &mono);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let entity = window.entity(visual).unwrap();
        let _sub = visual.update(|_, cx| {
            cx.subscribe(&entity, move |_, event: &BrowserAction, _| {
                log.lock().unwrap().push(*event)
            })
        });
        assert!(visual.debug_bounds("browser-ardia-running").is_none());
        show(&window, visual, Surface::default());
        assert!(visual.debug_bounds("browser-shelf").is_none());
        show(
            &window,
            visual,
            Surface {
                ardia_running: true,
                ..Default::default()
            },
        );
        for part in [
            "browser-shelf",
            "browser-ardia-running",
            "browser-ardia-quit",
        ] {
            assert!(visual.debug_bounds(part).is_some(), "{part} is on screen");
        }
        assert!(
            !window
                .read_with(visual, |view, _| view.browser_needs_attention())
                .unwrap(),
            "an indicator is not something waiting for the user"
        );
        let center = visual.debug_bounds("browser-ardia-quit").unwrap().center();
        visual.simulate_click(center, Default::default());
        visual.refresh().unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![BrowserAction::QuitArdia]);
    }

    #[gpui::test]
    fn the_working_row_shows_only_while_working_and_stop_emits_its_action(cx: &mut TestAppContext) {
        let mono = format!("card-working-{}", monocode_core::mono::new_id());
        let window = conversation(cx, &mono);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let entity = window.entity(visual).unwrap();
        let _sub = visual.update(|_, cx| {
            cx.subscribe(&entity, move |_, event: &BrowserAction, _| {
                log.lock().unwrap().push(*event)
            })
        });
        show(&window, visual, Surface::default());
        assert!(visual.debug_bounds("browser-working").is_none());
        assert!(visual.debug_bounds("browser-shelf").is_none());
        show(
            &window,
            visual,
            Surface {
                working: true,
                ..Default::default()
            },
        );
        for part in ["browser-shelf", "browser-working", "browser-stop"] {
            assert!(visual.debug_bounds(part).is_some(), "{part} is on screen");
        }
        assert!(
            !window
                .read_with(visual, |view, _| view.browser_needs_attention())
                .unwrap(),
            "work in progress is not something waiting for the user"
        );
        let center = visual.debug_bounds("browser-stop").unwrap().center();
        visual.simulate_click(center, Default::default());
        visual.refresh().unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![BrowserAction::StopBrowser]);
        show(&window, visual, Surface::default());
        assert!(visual.debug_bounds("browser-working").is_none());
        assert!(visual.debug_bounds("browser-stop").is_none());
    }
}
