//! The approval surface of the zptah Browser extension: the card in the conversation, how answers
//! reach the queue, the timeout, the stopped banner, the grants list and the rule confirmation.
//! Nothing here touches a browser: the Mono's side is a thread that asks the queue the way the gate
//! does.
use super::workflows_ui::tests::click;
use super::*;
use crate::conversation::browser_cards::BrowserAction;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::browser::copy::{self, ActionDesc, CardFacts, Lang};
use monocode_core::mono::browser::risk::Category;
use monocode_core::mono::browser::trust::approval::{self, Outcome, Request, UserAnswer};
use monocode_core::mono::browser::trust::grants::GrantBook;
use std::collections::BTreeSet;
use std::sync::Mutex as StdMutex;
use std::thread::JoinHandle;

/// The app language is process wide; tests that read card text take turns.
static LANGUAGE: StdMutex<()> = StdMutex::new(());

struct Rig {
    s: Setup,
    visual: VisualTestContext,
    _lang: std::sync::MutexGuard<'static, ()>,
}

/// A Mono with the extension backend on, showing its conversation.
fn open(cx: &mut TestAppContext, language: Lang) -> Rig {
    let mut rig = open_settings(cx, language);
    rig.s.manager.update(&mut rig.visual, |m, cx| {
        m.settings = None;
        cx.notify();
    });
    rig.settle();
    rig
}

fn open_settings(cx: &mut TestAppContext, language: Lang) -> Rig {
    let lang = LANGUAGE.lock().unwrap_or_else(|e| e.into_inner());
    copy::set_app_language(language);
    let s = setup(cx);
    cx.executor().allow_parking();
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::input::bind(cx);
        crate::popover::bind(cx);
    });
    let mut visual = VisualTestContext::from_window(s.window.into(), cx);
    click(&mut visual, "mono-settings-Browser");
    visual.refresh().unwrap();
    click(&mut visual, "mono-browser-enabled");
    visual.refresh().unwrap();
    click(&mut visual, "mono-browser-use-extension");
    visual.refresh().unwrap();
    // The settings page keeps the old language choice; the tests set the copy language directly.
    copy::set_app_language(language);
    Rig {
        s,
        visual,
        _lang: lang,
    }
}

fn request(mono: &str, offer: bool) -> Request {
    let mut facts = CardFacts::new(
        "Mira",
        "airbnb.com",
        ActionDesc::Click {
            label: "Book now".into(),
        },
        vec![Category::Sharing],
    );
    facts.purpose = Some("Reserve the flat you picked".into());
    Request {
        mono_id: mono.into(),
        session_id: 1,
        tool: "browser_click".into(),
        tab: Some(7),
        facts,
        screenshot: None,
        offer_conversation: offer,
    }
}

/// The Mono's gate: asks and waits on its own thread.
fn ask(mono: &str, offer: bool, timeout: Duration) -> JoinHandle<(String, Outcome)> {
    let queue = approval::queue_for(mono);
    let req = request(mono, offer);
    std::thread::spawn(move || queue.ask(req, timeout, Duration::from_millis(10), || {}))
}

impl Rig {
    fn mono(&self) -> String {
        self.s.id.clone()
    }
    fn on(&mut self, selector: &'static str) -> bool {
        self.visual.debug_bounds(selector).is_some()
    }
    fn settle(&mut self) {
        self.visual.run_until_parked();
        self.visual.refresh().unwrap();
    }
    /// Waits until the queue holds `n` cards, then lets the app react to the notification.
    fn wait_for_cards(&mut self, n: usize) {
        let queue = approval::queue_for(&self.mono());
        for _ in 0..500 {
            if queue.pending().len() == n {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(queue.pending().len(), n);
        // The pump runs on the app executor: give it turns until the surface follows.
        for _ in 0..200 {
            self.settle();
            if self.surface_cards() == n {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the conversation never showed {n} cards");
    }
    fn surface_cards(&mut self) -> usize {
        let id = self.mono();
        self.s.manager.read_with(&self.visual, |m, cx| {
            m.chats[&id].read(cx).browser_surface().cards.len()
        })
    }
    fn click(&mut self, selector: &'static str) {
        click(&mut self.visual, selector);
        self.settle();
    }
}

fn answered(handle: JoinHandle<(String, Outcome)>) -> Outcome {
    handle.join().unwrap().1
}

#[gpui::test]
fn a_waiting_card_reaches_its_conversation_and_an_answer_reaches_the_queue(
    cx: &mut TestAppContext,
) {
    let mut rig = open(cx, Lang::En);
    for answer in [
        UserAnswer::AllowOnce,
        UserAnswer::AllowConversation,
        UserAnswer::Deny,
    ] {
        let asked = ask(&rig.mono(), true, Duration::from_secs(30));
        rig.wait_for_cards(1);
        let id = rig.mono();
        let card = rig.s.manager.read_with(&rig.visual, |m, cx| {
            m.chats[&id].read(cx).browser_surface().cards[0].clone()
        });
        assert!(card.offer_conversation && card.text().allow_conversation.is_some());
        // The conversation's own buttons call this.
        let chat = rig
            .s
            .manager
            .read_with(&rig.visual, |m, _| m.chats[&id].clone());
        let card_id = card.id.clone();
        assert!(chat.update(&mut rig.visual, |c, cx| {
            c.browser_answer(&card_id, answer, cx)
        }));
        assert_eq!(answered(asked), Outcome::Answered(answer));
        rig.settle();
        assert_eq!(rig.surface_cards(), 0, "the card goes once answered");
    }
}

#[gpui::test]
fn the_card_text_follows_the_language_and_hides_the_conversation_answer_when_not_offered(
    cx: &mut TestAppContext,
) {
    let mut rig = open(cx, Lang::Vi);
    let asked = ask(&rig.mono(), false, Duration::from_secs(30));
    rig.wait_for_cards(1);
    let id = rig.mono();
    let card = rig.s.manager.read_with(&rig.visual, |m, cx| {
        m.chats[&id].read(cx).browser_surface().cards[0].clone()
    });
    let text = card.text();
    assert_eq!(text.deny, copy::card(Lang::Vi, &card.facts).deny);
    assert_ne!(text.deny, copy::card(Lang::En, &card.facts).deny);
    assert!(text.allow_conversation.is_none());
    assert!(text.headline.contains("Mira"), "{}", text.headline);
    approval::queue_for(&id).answer(&card.id, UserAnswer::Deny);
    assert_eq!(answered(asked), Outcome::Answered(UserAnswer::Deny));
}

#[gpui::test]
fn a_card_nobody_answers_times_out_as_a_deny_and_goes_away(cx: &mut TestAppContext) {
    let mut rig = open(cx, Lang::En);
    let asked = ask(&rig.mono(), true, Duration::from_millis(400));
    rig.wait_for_cards(1);
    assert_eq!(answered(asked), Outcome::TimedOut);
    for _ in 0..200 {
        rig.settle();
        if rig.surface_cards() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui::test]
fn a_notice_and_the_attention_state_follow_the_queue(cx: &mut TestAppContext) {
    let mut rig = open(cx, Lang::En);
    let queue = approval::queue_for(&rig.mono());
    queue.post_notice(
        approval::NoticeKind::PasswordNeeded,
        Some(3),
        "Mira",
        "accounts.example.com",
    );
    let id = rig.mono();
    for _ in 0..200 {
        rig.settle();
        let shown = rig.s.manager.read_with(&rig.visual, |m, cx| {
            m.chats[&id].read(cx).browser_surface().notices.len()
        });
        if shown == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(rig.s.manager.read_with(&rig.visual, |m, cx| {
        m.chats[&id].read(cx).needs_approval()
    }));
    let notice = queue.notices()[0].id.clone();
    let chat = rig
        .s
        .manager
        .read_with(&rig.visual, |m, _| m.chats[&id].clone());
    assert_eq!(
        chat.read_with(&rig.visual, |c, _| c.browser_inputs().len()),
        1,
        "the notification lists it"
    );
    assert!(queue.dismiss_notice(&notice));
    rig.settle();
    assert!(!rig.s.manager.read_with(&rig.visual, |m, cx| {
        m.chats[&id].read(cx).needs_approval()
    }));
}

#[gpui::test]
fn settings_lists_conversation_grants_and_revokes_them(cx: &mut TestAppContext) {
    let mut rig = open_settings(cx, Lang::En);
    let id = rig.mono();
    let store = rig
        .s
        .manager
        .read_with(&rig.visual, |m, _| m.browser_store().unwrap());
    let mut book = GrantBook::stored(store.clone(), &id, 1);
    book.grant("airbnb.com", &BTreeSet::from([Category::Sharing]));
    book.grant("shop.example", &BTreeSet::from([Category::PostPublic]));
    rig.s
        .manager
        .update(&mut rig.visual, |m, cx| m.browser_approval_page_refresh(cx));
    rig.settle();
    assert!(rig.on("mono-browser-grant-revoke-0") && rig.on("mono-browser-grant-revoke-1"));
    rig.click("mono-browser-grant-revoke-0");
    let left = store.list_grants(&id, None);
    assert_eq!(left.len(), 1, "one site revoked: {left:?}");
    assert!(!rig.on("mono-browser-grant-revoke-1"));
    rig.click("mono-browser-grants-revoke-all");
    assert!(store.list_grants(&id, None).is_empty());
    assert!(rig.on("mono-browser-grants-empty"));
}

const RULES: &str =
    "## Browser rules\n\n```browser-rules\nask: click \"Book\" on airbnb.com\n```\n";
const OTHER_RULES: &str =
    "## Browser rules\n\n```browser-rules\nask: click \"Pay\" on shop.example\n```\n";

fn write_soul(rig: &Rig, text: &str) {
    mono::files::save(&rig.s.root, &rig.s.id, None, "SOUL.md", text, None).unwrap();
}

#[gpui::test]
fn a_changed_rule_list_waits_for_confirmation_in_settings_and_in_the_conversation(
    cx: &mut TestAppContext,
) {
    let mut rig = open_settings(cx, Lang::En);
    let id = rig.mono();
    write_soul(&rig, RULES);
    // The first set is recorded as confirmed.
    rig.s
        .manager
        .update(&mut rig.visual, |m, cx| m.browser_approval_page_refresh(cx));
    rig.settle();
    assert!(!rig.on("mono-browser-rules-confirm"));
    // Replacing a rule is not a pure addition: it waits.
    write_soul(&rig, OTHER_RULES);
    rig.s.manager.update(&mut rig.visual, |m, cx| {
        m.browser_approval_page_refresh(cx);
        m.browser_approval_refresh(&id, true, cx);
    });
    rig.settle();
    assert!(rig.on("mono-browser-rules-confirm"));
    let review = |rig: &Rig| {
        let id = rig.mono();
        rig.s.manager.read_with(&rig.visual, |m, cx| {
            m.chats[&id].read(cx).browser_surface().review.clone()
        })
    };
    assert!(review(&rig).is_some());
    // "Keep the old rules" hides the prompt in the conversation and changes nothing.
    rig.s.manager.update(&mut rig.visual, |m, cx| {
        m.browser_action(&id, BrowserAction::KeepRules, cx)
    });
    rig.settle();
    assert!(review(&rig).is_none());
    assert!(rig.on("mono-browser-rules-confirm"));
    let store = rig
        .s
        .manager
        .read_with(&rig.visual, |m, _| m.browser_store().unwrap());
    assert_eq!(store.confirmed_rules(&id).unwrap().len(), 1);
    assert!(store.confirmed_rules(&id).unwrap()[0].contains("airbnb.com"));
    rig.click("mono-browser-rules-confirm");
    assert!(!rig.on("mono-browser-rules-confirm"));
    assert!(store.confirmed_rules(&id).unwrap()[0].contains("shop.example"));
}

#[gpui::test]
fn the_language_choice_is_saved_and_changes_the_card_language(cx: &mut TestAppContext) {
    let mut rig = open_settings(cx, Lang::En);
    rig.click("mono-browser-language-vi");
    assert_eq!(copy::app_language(), Lang::Vi);
    let saved = locked(&rig.s.store)
        .unwrap()
        .setting("mono-app-language")
        .unwrap();
    assert_eq!(saved.as_deref(), Some("vi"));
    rig.click("mono-browser-language-en");
    assert_eq!(copy::app_language(), Lang::En);
    assert_eq!(
        browser_approval::LanguagePref::from_id("bogus"),
        browser_approval::LanguagePref::System
    );
}

#[gpui::test]
fn the_preview_switch_turns_the_extension_backend_on_and_off(cx: &mut TestAppContext) {
    let mut rig = open_settings(cx, Lang::En);
    let on = |rig: &Rig| {
        rig.s
            .manager
            .read_with(&rig.visual, |m, _| m.browser_setting().use_extension)
    };
    assert!(on(&rig));
    assert!(rig.on("mono-browser-grants-empty"));
    rig.click("mono-browser-use-extension");
    assert!(!on(&rig));
    assert!(!rig.on("mono-browser-grants-empty"));
}

#[test]
fn language_choices_round_trip_and_the_explicit_ones_resolve_to_themselves() {
    use browser_approval::LanguagePref;
    for pref in LanguagePref::ALL {
        assert_eq!(LanguagePref::from_id(pref.id()), pref);
    }
    assert_eq!(LanguagePref::Vi.resolve(), Lang::Vi);
    assert_eq!(LanguagePref::En.resolve(), Lang::En);
}
