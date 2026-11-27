//! The act tools of the Ardia upstream (design spec sections 4, 6 and 9), through the fake
//! `ardia` of `tests.rs` and the page of `fake_page.rs` (recorded replies in `fixtures/`).
use super::fake_page::{focus_input, El, FailKind, Page};
use super::tests::*;
use super::upstream::*;
use crate::mono::browser::copy::ActionDesc;
use crate::mono::browser::gate::{ActConfig, Gate, Origins, Policy, Upstream, Verdict};
use crate::mono::browser::risk::Category;
use crate::mono::browser::soul_rules::Parsed;
use crate::mono::browser::trust::approval::{
    ApprovalQueue, PendingApproval, Subscription, UserAnswer,
};
use crate::mono::browser::{BrowserLevel, MonoBrowserStore};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FORM_LOGIN: &str = include_str!("fixtures/browse_form_login.json");
pub(super) const LOGIN_URL: &str = "https://shop.example/login";

/// What runs once when a card is shown.
pub(super) type CardHook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

/// A Mono at some level with its upstream, an audit database, a card queue that answers as told,
/// and folders for the uploads.
pub(super) struct Act {
    pub(super) r: Rig,
    pub(super) up: Arc<ArdiaUpstream>,
    pub(super) mono: String,
    pub(super) db: PathBuf,
    pub(super) queue: Arc<ApprovalQueue>,
    pub(super) cfg: ActConfig,
    pub(super) origins: Origins,
    /// The cards the user was shown (in order).
    pub(super) cards: Arc<Mutex<Vec<PendingApproval>>>,
    /// What the user answers; `None`: the card stays open.
    pub(super) answer: Arc<Mutex<Option<UserAnswer>>>,
    /// Runs (once) when a card is shown, before the user's answer is given.
    pub(super) on_card: CardHook,
    _sub: Subscription,
    pub(super) dir: tempfile::TempDir,
}

static NEXT: AtomicUsize = AtomicUsize::new(1);

pub(super) fn act_with(level: BrowserLevel, origins: Origins) -> Act {
    let r = rig(true);
    let dir = tempfile::Builder::new()
        .prefix("zp-act-")
        .tempdir()
        .unwrap();
    for sub in ["home/proj", "out", "elsewhere"] {
        std::fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    let db = dir.path().join("browser.db");
    rusqlite::Connection::open(&db).unwrap();
    let mono = format!("mono-act-{}", NEXT.fetch_add(1, Ordering::SeqCst));
    let queue = ApprovalQueue::new(&mono);
    let cards = Arc::new(Mutex::new(Vec::new()));
    let answer = Arc::new(Mutex::new(Some(UserAnswer::AllowOnce)));
    let on_card: CardHook = Arc::new(Mutex::new(None));
    let weak = Arc::downgrade(&queue);
    let (seen, choice, hook) = (cards.clone(), answer.clone(), on_card.clone());
    let sub = queue.subscribe(move || {
        let Some(queue) = weak.upgrade() else { return };
        for card in queue.pending() {
            let choice = *choice.lock().unwrap();
            if let Some(choice) = choice {
                seen.lock().unwrap().push(card.clone());
                if let Some(hook) = hook.lock().unwrap().take() {
                    hook();
                }
                queue.answer(&card.id, choice);
            }
        }
    });
    let up = ArdiaUpstream::with_bind_delay(&mono, r.controller.clone(), Duration::from_millis(1));
    let cfg = ActConfig {
        level,
        rules: Parsed::default(),
        store: Some(MonoBrowserStore::at(db.clone())),
        mono_label: "Mono".into(),
        profile: Some("Secondary".into()),
        approval_timeout: Duration::from_secs(10),
        queue: queue.clone(),
    };
    let a = Act {
        r,
        up,
        mono,
        db,
        queue,
        cfg,
        origins,
        cards,
        answer,
        on_card,
        _sub: sub,
        dir,
    };
    a.configure(vec![a.path("home/proj")]);
    // These tests are about the tools; the untrusted-content rule has its own tests.
    a.up.set_taint_exempt(true);
    a
}
pub(super) fn act() -> Act {
    act_with(BrowserLevel::Act, no_limits())
}

impl Act {
    pub(super) fn configure(&self, project_dirs: Vec<PathBuf>) {
        self.up.configure(Config {
            profile: PROFILE.into(),
            origins: self.origins.clone(),
            output_dir: Some(self.path("out")),
            database: Some(self.db.clone()),
            act: Some(self.cfg.clone()),
            project_dirs,
            home: Some(self.path("home")),
        });
    }
    pub(super) fn call(&self, tool: &str, args: Value) -> (bool, String) {
        call(&self.up, tool, args)
    }
    pub(super) fn set_page(&self, page: Page) {
        self.r.runner.lock().page = page;
    }
    /// Opens the login page and takes a snapshot of it: the state a Mono acts from.
    pub(super) fn on_login(&self) {
        self.on(login_page());
    }
    pub(super) fn on(&self, page: Page) {
        self.on_at(page, LOGIN_URL);
    }
    pub(super) fn on_at(&self, page: Page, url: &str) {
        self.set_page(page);
        let (error, text) = self.call("browser_open", json!({"url": url}));
        assert!(!error, "{text}");
        self.snapshot();
    }
    pub(super) fn snapshot(&self) {
        let (error, text) = self.call("browser_snapshot", json!({"interactive": true}));
        assert!(!error, "{text}");
    }
    pub(super) fn sent(&self) -> Vec<Vec<String>> {
        self.r.runner.lock().act_sent.clone()
    }
    pub(super) fn answer(&self, answer: Option<UserAnswer>) {
        *self.answer.lock().unwrap() = answer;
    }
    pub(super) fn cards(&self) -> Vec<PendingApproval> {
        self.cards.lock().unwrap().clone()
    }
    pub(super) fn rows(&self) -> Vec<Row> {
        let Ok(conn) = rusqlite::Connection::open(&self.db) else {
            return Vec::new();
        };
        let Ok(mut query) = conn.prepare(
            "SELECT tool, decision, reason, summary, url, categories, tab_owner FROM mono_browser_audit WHERE mono_id=?1 ORDER BY id",
        ) else {
            return Vec::new();
        };
        query
            .query_map([&self.mono], |row| {
                Ok(Row {
                    tool: row.get(0)?,
                    decision: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    reason: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    summary: row.get(3)?,
                    owner: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                })
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }
    pub(super) fn rows_of(&self, tool: &str) -> Vec<Row> {
        self.rows().into_iter().filter(|r| r.tool == tool).collect()
    }
    pub(super) fn path(&self, tail: &str) -> PathBuf {
        self.dir.path().join(tail)
    }
}

#[derive(Debug, Clone)]
pub(super) struct Row {
    pub(super) tool: String,
    pub(super) decision: String,
    pub(super) reason: String,
    pub(super) summary: String,
    pub(super) owner: String,
}

pub(super) fn login_page() -> Page {
    let mut form = json_of(FORM_LOGIN)["data"].clone();
    form["url"] = json!(LOGIN_URL);
    Page {
        title: "Sign in - Test".into(),
        elements: vec![
            El::new("e2", "heading", "Sign in"),
            El::new("e3", "textbox", "Email")
                .attr("type", "email")
                .attr("autocomplete", "username"),
            El::new("e4", "textbox", "Secret")
                .attr("type", "password")
                .attr("autocomplete", "current-password"),
            El::new("e5", "textbox", "Code")
                .attr("type", "text")
                .attr("autocomplete", "one-time-code"),
            El::new("e6", "textbox", "Field X").attr("type", "text"),
            El::new("e7", "button", "Continue").attr("type", "submit"),
            El::new("e8", "link", "IANA").attr("href", "https://www.iana.org/domains/example"),
            El::new("e9", "button", "Add").attr("type", "button"),
            El::new("e10", "button", "Swap").attr("type", "button"),
            El::new("e11", "button", "Pick file").attr("type", "file"),
            El::new("e12", "button", "Delete account").attr("type", "button"),
            El::new("e13", "checkbox", "Remember me").attr("type", "checkbox"),
            El::new("e14", "combobox", "Color"),
        ],
        forms: vec![Some("/session".into())],
        form: Some(form),
        ..Page::default()
    }
}

/// A page without a sign-in form.
pub(super) fn plain_page() -> Page {
    Page {
        title: "Shipping notes".into(),
        elements: vec![
            El::new("e1", "textbox", "Note"),
            El::new("e2", "button", "Add").attr("type", "button"),
        ],
        forms: vec![],
        form: Some(
            json!({"url": LOGIN_URL, "count": 1, "more": false, "stopped": null,
            "fields": [{"ref": "e1", "role": "textbox", "label": "Note", "type": "", "name": "note", "value": ""}], "submits": []}),
        ),
        ..Page::default()
    }
}

pub(super) fn blocked(origin: &str) -> Origins {
    Origins::new(&[], &[origin.to_owned()])
}

// ---- levels and the tool list ----

#[test]
fn act_tools_are_not_run_at_the_read_only_level() {
    let a = act_with(BrowserLevel::ReadOnly, no_limits());
    a.on_login();
    for (tool, args) in [
        ("browser_click", json!({"ref": "e9"})),
        ("browser_type", json!({"ref": "e6", "text": "x"})),
        ("browser_fill", json!({"ref": "e6", "text": "x"})),
        ("browser_press", json!({"key": "Tab"})),
        ("browser_select", json!({"ref": "e14", "value": "blue"})),
        ("browser_check", json!({"ref": "e13"})),
        ("browser_hover", json!({"ref": "e9"})),
        ("browser_upload", json!({"ref": "e11", "paths": ["/x"]})),
        (
            "browser_request_approval",
            json!({"category": "other", "action": "x"}),
        ),
        ("browser_tabs", json!({"action": "adopt_active"})),
    ] {
        let (error, text) = a.call(tool, args);
        assert!(error && text.contains("can only read"), "{tool}: {text}");
    }
    assert!(a.sent().is_empty(), "nothing reached Ardia");
    assert_eq!(a.rows_of("browser_click")[0].decision, "blocked");
}

#[test]
fn the_gate_lists_the_act_tools_only_at_the_act_level() {
    let list = |level: BrowserLevel| -> Vec<String> {
        let a = act_with(level, no_limits());
        let gate = Gate::start(a.up.clone(), Policy::ardia(level, vec![]), "ardia-list").unwrap();
        let route = gate.route_for(Some(7));
        let reply = gate
            .handle(
                &route,
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            )
            .unwrap();
        reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let at_act = list(BrowserLevel::Act);
    for tool in ACT_TOOLS {
        assert!(at_act.iter().any(|n| n == tool), "{tool}");
    }
    for tool in ["browser_request_approval", "browser_declare_intent"] {
        assert!(at_act.iter().any(|n| n == tool), "{tool}");
    }
    assert_eq!(at_act.len(), TOOLS.len() + ACT_TOOLS.len() + 2);
    assert!(
        !at_act.iter().any(|n| n.contains("otp")),
        "fill-otp is never exposed"
    );
    let at_read = list(BrowserLevel::ReadOnly);
    assert_eq!(at_read, TOOLS.map(String::from));
    // The harness disables the same tools by name.
    let disabled = BrowserLevel::ReadOnly.disabled_tools_ardia();
    for tool in ACT_TOOLS
        .iter()
        .chain(["browser_request_approval", "browser_declare_intent"].iter())
    {
        assert!(disabled.contains(tool), "{tool}");
        assert!(!BrowserLevel::Act.disabled_tools_ardia().contains(tool));
    }
}

#[test]
fn the_policy_denies_act_tools_at_read_only_and_allows_them_at_act() {
    let read = Policy::ardia(BrowserLevel::ReadOnly, vec![]);
    let act = Policy::ardia(BrowserLevel::Act, vec![]);
    for tool in ACT_TOOLS {
        assert!(!read.offers(tool) && act.offers(tool), "{tool}");
        assert!(matches!(read.evaluate(tool, &json!({})), Verdict::Deny(_)));
        assert_eq!(act.evaluate(tool, &json!({})), Verdict::Allow);
    }
    assert!(matches!(
        read.evaluate("browser_tabs", &json!({"action": "adopt_active"})),
        Verdict::Deny(_)
    ));
    assert_eq!(
        act.evaluate("browser_tabs", &json!({"action": "adopt_active"})),
        Verdict::Allow
    );
    assert!(matches!(
        read.evaluate("browser_fill_otp", &json!({})),
        Verdict::Deny(_)
    ));
    assert!(matches!(
        act.evaluate("browser_fill_otp", &json!({})),
        Verdict::Deny(_)
    ));
}

// ---- the argv form ----

#[test]
fn the_browse_argv_is_session_json_command_then_values_after_a_double_dash() {
    use super::runner::browse_args;
    assert_eq!(
        browse_args("s", &["fill"], &["@e1", "--json", "-5"]),
        [
            "browse",
            "--session",
            "s",
            "--json",
            "fill",
            "--",
            "@e1",
            "--json",
            "-5"
        ]
    );
    assert_eq!(
        browse_args("s", &["snapshot", "-i", "-s", "main"], &[]),
        [
            "browse",
            "--session",
            "s",
            "--json",
            "snapshot",
            "-i",
            "-s",
            "main"
        ]
    );
}

#[test]
fn values_reach_the_command_only_after_a_double_dash_and_last() {
    let a = act();
    a.on_login();
    for text in ["--json", "--session=x", "-5", "-", "--", "--new-tab"] {
        let before = a.r.runner.calls().len();
        let (error, reply) = a.call("browser_fill", json!({"ref": "e6", "text": text}));
        assert!(!error, "{text}: {reply}");
        let fill = a.r.runner.calls()[before..]
            .iter()
            .find(|c| c.contains(" --json fill "))
            .cloned()
            .unwrap();
        let argv: Vec<&str> = fill.split(' ').collect();
        assert_eq!(argv[0], "browse");
        assert_eq!(argv[1], "--session");
        assert_eq!(
            argv[3], "--json",
            "--json is right after the session: {fill}"
        );
        assert_eq!(argv[4], "fill");
        assert_eq!(argv[5], "--");
        assert_eq!(argv[6], "@e6");
        assert_eq!(
            argv[7..].join(" "),
            text,
            "the text is last and nothing follows it: {fill}"
        );
        // `--json` appears once (right after the session), plus the text when it is `--json`.
        assert_eq!(
            argv.iter().filter(|a| **a == "--json").count(),
            1 + usize::from(text == "--json")
        );
    }
    // Every other value goes the same way.
    a.r.runner.lock().page.focus = Some(focus_input("text", Some("nolabel"), Some("Field X")));
    a.call("browser_press", json!({"key": "-"}));
    a.call("browser_select", json!({"ref": "e14", "value": "--json"}));
    a.call("browser_type", json!({"ref": "e6", "text": "-5"}));
    let calls = a.r.runner.calls();
    for needle in [
        "--json press -- -",
        "--json select -- @e14 --json",
        "--json type -- @e6 -5",
    ] {
        assert!(
            calls.iter().any(|c| c.ends_with(needle)),
            "{needle}: {calls:?}"
        );
    }
    // The refs and selectors that are not after `--` still refuse a leading dash.
    let (error, text) = a.call("browser_click", json!({"ref": "--session=x"}));
    assert!(error && text.contains("ref"), "{text}");
    let (error, _) = a.call("browser_snapshot", json!({"selector": "--session=x"}));
    assert!(error);
}

// ---- the fresh read ----

#[test]
fn an_act_runs_a_fresh_read_of_the_target_first() {
    let a = act();
    a.on_login();
    let before = a.r.runner.calls().len();
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(!error, "{text}");
    assert!(text.contains("- Ran click on \"Add\""), "{text}");
    let calls: Vec<String> = a.r.runner.calls()[before..].to_vec();
    let at = |needle: &str| calls.iter().position(|c| c.contains(needle));
    assert!(at("get url") < at("--json click"), "{calls:?}");
    assert!(at("snapshot -i") < at("--json click"));
    assert!(at("get attr -- @e9 type") < at("--json click"));
    assert_eq!(a.sent(), vec![vec!["click".to_owned(), "@e9".into()]]);
}

#[test]
fn a_ref_the_mono_never_saw_in_a_snapshot_is_refused() {
    let a = act();
    a.set_page(login_page());
    a.call("browser_open", json!({"url": LOGIN_URL}));
    // No snapshot yet.
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(error && text.contains("not in any snapshot"), "{text}");
    a.snapshot();
    // A ref that exists on the page but was never listed in a snapshot of it.
    a.r.runner
        .lock()
        .page
        .elements
        .push(El::new("e99", "button", "Hidden"));
    let (error, text) = a.call("browser_click", json!({"ref": "e99"}));
    assert!(error && text.contains("not in any snapshot"), "{text}");
    assert!(a.sent().is_empty());
}

#[test]
fn a_ref_that_changed_since_the_snapshot_fails_with_the_exact_text() {
    // The element is gone.
    let a = act();
    a.on_login();
    a.r.runner.lock().page.elements.retain(|e| e.r != "e9");
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(error, "{text}");
    assert_eq!(text, "the element changed, take a new snapshot");

    // Its name changed (Ardia keeps the ref).
    let a = act();
    a.on_login();
    a.r.runner
        .lock()
        .page
        .elements
        .iter_mut()
        .find(|e| e.r == "e9")
        .unwrap()
        .name = "Changed".into();
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(error && text == CHANGED, "{text}");

    // Its role changed.
    let a = act();
    a.on_login();
    a.r.runner
        .lock()
        .page
        .elements
        .iter_mut()
        .find(|e| e.r == "e9")
        .unwrap()
        .role = "link".into();
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(error && text == CHANGED, "{text}");

    // The tab is on another URL than at the snapshot.
    let a = act();
    a.on_login();
    a.r.runner.lock().url = "https://shop.example/other".into();
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(error && text == CHANGED, "{text}");
    assert!(a.sent().is_empty(), "nothing was clicked");
    assert!(a
        .rows_of("browser_click")
        .iter()
        .all(|r| r.decision == "blocked"));
}

#[test]
fn stale_superseded_and_unknown_refs_all_say_the_element_changed() {
    for code in [
        "stale_ref",
        "vocab_superseded",
        "vocab_superseded_navigated",
        "unknown_ref",
    ] {
        // Ardia refuses the ref while reading its attributes.
        let a = act();
        a.on_login();
        a.r.runner.lock().broken.insert("e9".into(), code);
        let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
        assert!(error && text == CHANGED, "{code} (click): {text}");
        // Or only at the command (a hover reads no attribute).
        let a = act();
        a.on_login();
        a.r.runner.lock().broken.insert("e9".into(), code);
        let (error, text) = a.call("browser_hover", json!({"ref": "e9"}));
        assert!(error && text == CHANGED, "{code} (hover): {text}");
        assert_eq!(
            a.sent().len(),
            1,
            "the hover command was the one that refused"
        );
    }
}

#[test]
fn a_hidden_element_is_reported_as_a_plain_failure_with_a_hint() {
    let a = act();
    a.on_login();
    a.r.runner.lock().broken.insert("e13".into(), "not_visible");
    let (error, text) = a.call("browser_hover", json!({"ref": "e13"}));
    assert!(
        error && text.contains("no visible box") && text.contains("scroll"),
        "{text}"
    );
    assert!(!text.contains("may or may not"));
    assert_eq!(a.rows_of("browser_hover")[0].decision, "auto");
    assert!(a.rows_of("browser_hover")[0].reason.contains("did not run"));
}

#[test]
fn refs_added_by_an_act_can_be_used_without_a_new_snapshot() {
    let a = act();
    let mut page = login_page();
    page.elements
        .iter_mut()
        .find(|e| e.r == "e9")
        .unwrap()
        .attrs
        .insert("adds".into(), "1".into());
    a.on(page);
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(!error, "{text}");
    // `act_click_add`: the click inserted a button `e12`.
    assert!(text.contains("button \"Inserted\" [ref=e12]"), "{text}");
    // `e12` is "Delete account" on this page: a different name than the Mono was told, so it is
    // refused; a ref with the right name goes.
    let (error, text) = a.call("browser_click", json!({"ref": "e12"}));
    assert!(error && text == CHANGED, "{text}");
    a.r.runner
        .lock()
        .page
        .elements
        .iter_mut()
        .find(|e| e.r == "e12")
        .unwrap()
        .name = "Inserted".into();
    let (error, text) = a.call("browser_click", json!({"ref": "e12"}));
    assert!(!error, "{text}");
}

#[test]
fn a_page_that_navigated_needs_a_new_snapshot() {
    let a = act();
    a.on_login();
    let (error, text) = a.call("browser_click", json!({"ref": "e8"}));
    assert!(!error, "{text}");
    assert!(text.contains("The page changed"), "{text}");
    // The old refs belong to the old page.
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(error, "{text}");
    assert!(text.contains("snapshot"), "{text}");
}

#[test]
fn the_page_before_the_act_must_be_one_the_mono_may_use() {
    let a = act_with(BrowserLevel::Act, blocked("https://bank.example"));
    a.on_login();
    a.r.runner.lock().url = "https://bank.example/home".into();
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(error && text.contains("blocked"), "{text}");
    assert!(a.sent().is_empty());
    let row = &a.rows_of("browser_click")[0];
    assert_eq!(row.decision, "blocked");
    assert!(row.reason.contains("page"), "{row:?}");
}

// ---- passwords and one-time codes ----

#[test]
fn a_password_field_is_never_typed_into_whatever_the_user_answers() {
    for tool in ["browser_fill", "browser_type"] {
        let a = act();
        a.answer(Some(UserAnswer::AllowOnce));
        a.on_login();
        // By type.
        let (error, text) = a.call(tool, json!({"ref": "e4", "text": "swordfish-test-value"}));
        assert!(error && text.contains("never allowed"), "{tool}: {text}");
        // By autocomplete: `type=text` with a one-time code hint.
        let (error, text) = a.call(tool, json!({"ref": "e5", "text": "123456"}));
        assert!(error && text.contains("never allowed"), "{tool}: {text}");
        assert!(a.sent().is_empty(), "{tool} reached Ardia");
        assert!(
            a.cards().is_empty(),
            "{tool} asked: no approval overrides it"
        );
        let rows = a.rows_of(tool);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.decision == "invariant"), "{rows:?}");
        // The user is told they have to do it themselves.
        assert_eq!(a.queue.notices().len(), 1, "{tool}");
        // The typed value is never kept.
        assert!(a
            .rows()
            .iter()
            .all(|r| !r.summary.contains("swordfish") && !r.reason.contains("swordfish")));
    }
}

#[test]
fn autocomplete_of_a_password_refuses_even_when_the_type_says_text() {
    let a = act();
    let mut page = login_page();
    page.elements.push(
        El::new("e20", "textbox", "Account key")
            .attr("type", "text")
            .attr("autocomplete", "new-password"),
    );
    a.on(page);
    let (error, text) = a.call("browser_fill", json!({"ref": "e20", "text": "x"}));
    assert!(error && text.contains("never allowed"), "{text}");
    assert!(a.sent().is_empty());
}

#[test]
fn a_field_named_like_a_password_or_code_is_never_typed_into_in_english_and_vietnamese() {
    let a = act();
    let mut page = plain_page();
    let names = [
        "Password",
        "Enter your passcode",
        "PIN",
        "Verification code",
        "Mật khẩu",
        "Nhập mật khẩu",
        "Mã PIN",
        "Mã OTP",
    ];
    for (i, name) in names.iter().enumerate() {
        page.elements
            .push(El::new(&format!("e{}", 30 + i), "textbox", name));
    }
    a.on_at(page, "https://shop.example/blog");
    a.answer(Some(UserAnswer::AllowOnce));
    for (i, name) in names.iter().enumerate() {
        let r = format!("e{}", 30 + i);
        let (error, text) = a.call("browser_fill", json!({"ref": r, "text": "x"}));
        assert!(error && text.contains("never"), "{name}: {text}");
    }
    assert!(a.sent().is_empty());
    assert!(a.cards().is_empty());
}

fn unknown_kind_page() -> Page {
    let mut page = login_page();
    // The type of "Field X" could not be read: no attribute, and the form lists no type.
    page.elements
        .iter_mut()
        .find(|e| e.r == "e6")
        .unwrap()
        .attrs
        .clear();
    page.form.as_mut().unwrap()["fields"][3]["type"] = json!("");
    page
}

#[test]
fn a_textbox_of_unknown_kind_on_a_sign_in_page_asks_first() {
    let a = act();
    a.on(unknown_kind_page());
    let (error, text) = a.call("browser_fill", json!({"ref": "e6", "text": "hello"}));
    assert!(!error, "{text}");
    let cards = a.cards();
    assert_eq!(cards.len(), 1, "asked before typing");
    assert!(cards[0]
        .facts
        .categories
        .contains(&Category::CredentialEntry));
    assert_eq!(a.sent().len(), 1);
    // Denied: nothing typed.
    let b = act();
    b.answer(Some(UserAnswer::Deny));
    b.on(unknown_kind_page());
    let (error, text) = b.call("browser_fill", json!({"ref": "e6", "text": "hello"}));
    assert!(error && text.contains("Denied by the user"), "{text}");
    assert!(b.sent().is_empty());
}

#[test]
fn a_textbox_of_unknown_kind_on_an_ordinary_page_does_not_ask() {
    let a = act();
    a.on_at(plain_page(), "https://shop.example/blog");
    let (error, text) = a.call("browser_fill", json!({"ref": "e1", "text": "hello"}));
    assert!(!error, "{text}");
    assert!(a.cards().is_empty());
    assert_eq!(a.sent().len(), 1);
}

#[test]
fn a_known_field_on_a_sign_in_page_does_not_ask() {
    let a = act();
    a.on_login();
    let (error, text) = a.call(
        "browser_fill",
        json!({"ref": "e3", "text": "me@example.test"}),
    );
    assert!(!error, "{text}");
    assert!(a.cards().is_empty());
}

#[test]
fn fill_otp_is_not_a_tool() {
    let a = act();
    a.on_login();
    let (error, text) = a.call("browser_fill_otp", json!({"ref": "e5", "text": "123456"}));
    assert!(error && text.contains("not an available"), "{text}");
    assert!(a.sent().is_empty());
}

#[test]
fn a_character_key_is_refused_in_a_password_field_and_when_the_focus_is_unknown() {
    let a = act();
    a.on_login();
    // Focus in the password field.
    a.r.runner.lock().page.focus = Some(focus_input("password", Some("pw"), Some("Secret")));
    let (error, text) = a.call("browser_press", json!({"key": "a"}));
    assert!(error && text.contains("never allowed"), "{text}");
    // A paste chord is a character key too.
    let (error, _) = a.call("browser_press", json!({"key": "Control+v"}));
    assert!(error);
    // Focus the page could not name (an eval that fails, or an iframe): lean to refusing.
    a.r.runner.lock().focus_fails = true;
    let (error, text) = a.call("browser_press", json!({"key": "a"}));
    assert!(error && text.contains("could not tell"), "{text}");
    a.r.runner.lock().focus_fails = false;
    a.r.runner.lock().page.focus = Some(json!({"tag": "IFRAME"}));
    let (error, text) = a.call("browser_press", json!({"key": "a"}));
    assert!(error && text.contains("could not tell"), "{text}");
    assert!(a.sent().is_empty());
    // Focus in an ordinary field: allowed.
    a.r.runner.lock().page.focus = Some(focus_input("text", Some("nolabel"), Some("Field X")));
    let (error, text) = a.call("browser_press", json!({"key": "a"}));
    assert!(!error, "{text}");
    // Keys that type nothing stay under the normal rules, whatever has the focus.
    a.r.runner.lock().page.focus = Some(focus_input("password", Some("pw"), Some("Secret")));
    for key in ["Tab", "Escape", "ArrowDown", "Backspace"] {
        let (error, text) = a.call("browser_press", json!({"key": key}));
        assert!(!error, "{key}: {text}");
    }
}

// ---- the decision, grants and cards ----

#[test]
fn a_risky_click_asks_and_a_conversation_grant_covers_the_next_one() {
    let a = act();
    a.on_login();
    a.answer(Some(UserAnswer::AllowConversation));
    let (error, text) = a.call(
        "browser_click",
        json!({"ref": "e12", "purpose": "clean up"}),
    );
    assert!(!error, "{text}");
    let cards = a.cards();
    assert_eq!(cards.len(), 1);
    assert!(cards[0].facts.categories.contains(&Category::Destructive));
    assert_eq!(cards[0].facts.purpose.as_deref(), Some("clean up"));
    // The grant covers the same category on this site.
    let (error, text) = a.call("browser_click", json!({"ref": "e12"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1, "no second card");
    let rows = a.rows_of("browser_click");
    assert_eq!(rows[0].decision, "asked_allowed_task");
    assert_eq!(rows[1].decision, "auto");
    // A denial runs nothing.
    let b = act();
    b.on_login();
    b.answer(Some(UserAnswer::Deny));
    let (error, text) = b.call("browser_click", json!({"ref": "e12"}));
    assert!(error && text.contains("Denied by the user"), "{text}");
    assert!(b.sent().is_empty());
    assert_eq!(b.rows_of("browser_click")[0].decision, "asked_denied");
}

#[test]
fn the_soul_rules_apply_to_act_tools() {
    let mut a = act();
    a.cfg.rules = crate::mono::browser::soul_rules::parse(
        "## Browser rules\n```browser-rules\nask: click \"add\" on shop.example\nblock: click \"swap\" on shop.example\n```\n",
    );
    a.configure(vec![a.path("home/proj")]);
    a.on_login();
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(!error, "{text}");
    let cards = a.cards();
    assert_eq!(cards.len(), 1);
    assert!(cards[0].facts.categories.contains(&Category::SoulRule));
    let (error, text) = a.call("browser_click", json!({"ref": "e10"}));
    assert!(error && text.contains("browser rules"), "{text}");
    assert_eq!(a.sent().len(), 1);
}

#[test]
fn when_nobody_can_answer_an_ask_is_a_deny() {
    let a = act();
    a.on_login();
    a.queue.set_can_answer(false);
    let (error, text) = a.call("browser_click", json!({"ref": "e12"}));
    assert!(error && text.contains("nobody could answer"), "{text}");
    assert!(a.sent().is_empty());
}

#[test]
fn a_page_that_changed_while_the_user_was_asked_is_not_acted_on() {
    let a = act();
    a.on_login();
    a.answer(None);
    let worker = std::thread::spawn({
        let up = a.up.clone();
        move || call(&up, "browser_click", json!({"ref": "e12"}))
    });
    assert!(wait_until(|| !a.queue.pending().is_empty()));
    // The page loses the element while the card is open: the user answers, then it is gone.
    a.r.runner.lock().page.elements.retain(|e| e.r != "e12");
    let id = a.queue.pending()[0].id.clone();
    a.queue.answer(&id, UserAnswer::AllowOnce);
    let (error, text) = worker.join().unwrap();
    assert!(error && text == CHANGED, "{text}");
    assert!(a.sent().is_empty());
}

#[test]
fn a_payment_always_asks_even_with_a_grant() {
    let a = act();
    let mut page = login_page();
    page.elements
        .push(El::new("e40", "button", "Pay now").attr("type", "button"));
    a.on(page);
    a.answer(Some(UserAnswer::AllowConversation));
    for _ in 0..2 {
        let (error, text) = a.call("browser_click", json!({"ref": "e40"}));
        assert!(!error, "{text}");
    }
    assert_eq!(a.cards().len(), 2, "money is asked every time");
}

#[test]
fn the_mono_can_ask_for_approval_itself() {
    let a = act();
    a.on_login();
    let (error, text) = a.call(
        "browser_request_approval",
        json!({"category": "send_message", "action": "send the draft", "details": "to the team"}),
    );
    assert!(!error && text.contains("Approved"), "{text}");
    let card = &a.cards()[0];
    assert_eq!(card.facts.agent_action.as_deref(), Some("send the draft"));
    a.answer(Some(UserAnswer::Deny));
    let (error, text) = a.call(
        "browser_request_approval",
        json!({"category": "payment", "action": "pay"}),
    );
    assert!(error && text.contains("Denied"), "{text}");
    assert_eq!(a.rows_of("browser_request_approval").len(), 2);
}

#[test]
fn declared_intent_goes_through_the_gate_at_the_act_level_only() {
    let a = act();
    let gate = Gate::start(
        a.up.clone(),
        Policy::ardia(BrowserLevel::Act, vec![]),
        "ardia-decl",
    )
    .unwrap();
    let route = gate.route_for(Some(7));
    let reply = gate
        .handle(
            &route,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "browser_declare_intent", "arguments": {"actions": [{"kind": "write", "target_desc": "Add item", "url": "https://shop.example/login"}]}}}),
        )
        .unwrap();
    assert!(reply["result"]["isError"] != true, "{reply}");
    let read = act_with(BrowserLevel::ReadOnly, no_limits());
    let gate = Gate::start(
        read.up.clone(),
        Policy::ardia(BrowserLevel::ReadOnly, vec![]),
        "ardia-decl2",
    )
    .unwrap();
    let route = gate.route_for(Some(7));
    let reply = gate
        .handle(
            &route,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "browser_declare_intent", "arguments": {"actions": []}}}),
        )
        .unwrap();
    assert_eq!(reply["result"]["isError"], true);
}

// ---- origins ----

#[test]
fn a_link_to_a_blocked_origin_is_not_followed() {
    let a = act_with(BrowserLevel::Act, blocked("https://www.iana.org"));
    a.on_login();
    let (error, text) = a.call("browser_click", json!({"ref": "e8"}));
    assert!(error && text.contains("link leads to"), "{text}");
    assert!(a.sent().is_empty());
    assert_eq!(a.rows_of("browser_click")[0].decision, "blocked");
    // A link off the allowed list is refused too.
    let b = act_with(
        BrowserLevel::Act,
        Origins::new(&["https://shop.example".to_owned()], &[]),
    );
    b.on_login();
    let (error, text) = b.call("browser_click", json!({"ref": "e8"}));
    assert!(error && text.contains("allowed list"), "{text}");
}

#[test]
fn a_submit_whose_form_posts_to_a_blocked_origin_is_not_clicked() {
    let a = act_with(BrowserLevel::Act, blocked("https://pay.example"));
    let mut page = login_page();
    page.forms = vec![Some("https://pay.example/charge".into())];
    a.on(page);
    let (error, text) = a.call("browser_click", json!({"ref": "e7"}));
    assert!(error && text.contains("form leads to"), "{text}");
    assert!(a.sent().is_empty());
    // A button that is not a submit does not post the form.
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(!error, "{text}");
}

#[test]
fn enter_in_a_ref_checks_every_form_action_of_the_page() {
    let a = act_with(BrowserLevel::Act, blocked("https://pay.example"));
    let mut page = login_page();
    // The first form is fine; a second one posts to a blocked origin: lean to refusing.
    page.forms = vec![
        Some("/session".into()),
        Some("https://pay.example/charge".into()),
    ];
    a.on(page);
    let (error, text) = a.call("browser_press", json!({"key": "Enter", "ref": "e3"}));
    assert!(error && text.contains("form leads to"), "{text}");
    assert!(a.sent().is_empty());
    // With only the safe form it goes.
    let b = act_with(BrowserLevel::Act, blocked("https://pay.example"));
    b.on_login();
    let (error, text) = b.call("browser_press", json!({"key": "Enter", "ref": "e3"}));
    assert!(!error, "{text}");
    assert_eq!(b.sent().len(), 1);
}

#[test]
fn a_form_whose_action_cannot_be_read_asks_before_it_is_submitted() {
    let a = act();
    let mut page = login_page();
    page.forms = vec![Some("http://[bad".into())];
    a.on(page);
    let (error, text) = a.call("browser_click", json!({"ref": "e7"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1, "incomplete signals lean to asking");
}

#[test]
fn a_click_that_leaves_the_tab_on_a_blocked_page_stops_the_tab() {
    let a = act_with(BrowserLevel::Act, blocked("https://bank.example"));
    let mut page = login_page();
    page.elements.push(
        El::new("e50", "button", "Go")
            .attr("type", "button")
            .attr("navigates_to", "https://bank.example/login"),
    );
    a.on(page);
    let (error, text) = a.call("browser_click", json!({"ref": "e50"}));
    assert!(error && text.contains("Stopped"), "{text}");
    assert!(text.contains("closed"), "an own tab is closed: {text}");
    // The click ran and is audited, and so is the hit.
    assert_eq!(a.sent().len(), 1);
    assert_eq!(a.rows_of("browser_click").len(), 1);
    assert_eq!(a.rows_of("origin_blocked").len(), 1);
    assert!(a.r.runner.lock().tabs.is_empty());
    // Further calls in the tab are refused for the turn.
    let (error, _) = a.call("browser_read", json!({}));
    assert!(error);
}

// ---- audit ----

#[test]
fn one_row_per_act_with_the_url_after_the_posts_and_no_typed_value() {
    let a = act();
    a.r.runner.lock().enter_to = Some("https://shop.example/done".into());
    a.on_login();
    a.r.runner.lock().page.focus = Some(
        json!({"tag": "INPUT", "type": "email", "name": "email", "label": "Email", "fi": 0, "sb": "Continue", "ce": false, "sh": false}),
    );
    let (error, text) = a.call(
        "browser_fill",
        json!({"ref": "e3", "text": "swordfish-test-value"}),
    );
    assert!(!error, "{text}");
    let (error, text) = a.call("browser_press", json!({"key": "Enter"}));
    assert!(!error, "{text}");
    let fill = a.rows_of("browser_fill");
    assert_eq!(fill.len(), 1);
    assert!(
        fill[0].summary.contains("typed 20 chars into Email"),
        "{:?}",
        fill[0]
    );
    assert!(fill[0]
        .summary
        .contains("url after https://shop.example/login"));
    assert_eq!(fill[0].decision, "auto");
    let press = a.rows_of("browser_press");
    assert_eq!(press.len(), 1);
    assert!(
        press[0]
            .summary
            .contains("wrote: POST https://shop.example/session"),
        "{:?}",
        press[0]
    );
    assert!(press[0]
        .summary
        .contains("url after https://shop.example/done"));
    for row in a.rows() {
        assert!(!row.summary.contains("swordfish") && !row.reason.contains("swordfish"));
    }
}

// ---- uploads ----

pub(super) fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn upload(a: &Act, path: &Path) -> (bool, String) {
    a.call(
        "browser_upload",
        json!({"ref": "e11", "paths": [path.to_string_lossy()]}),
    )
}

#[test]
fn an_upload_from_the_project_or_the_output_folder_always_asks_and_shows_path_and_size() {
    let a = act();
    a.on_login();
    let file = a.path("home/proj/report.txt");
    write(&file, &[b'x'; 2048]);
    let shot = a.path("out/shot-1.png");
    write(&shot, b"png");
    for path in [&file, &shot] {
        let (error, text) = upload(&a, path);
        assert!(!error, "{}: {text}", path.display());
    }
    let cards = a.cards();
    assert_eq!(cards.len(), 2, "every upload asks");
    let canonical = std::fs::canonicalize(&file).unwrap();
    let details = cards[0].facts.agent_details.clone().unwrap();
    assert!(
        details.contains(&format!("{} (2.0 KB)", canonical.display())),
        "{details}"
    );
    assert!(matches!(cards[0].facts.action, ActionDesc::Upload { .. }));
    // The path that reaches `ardia` is the one that was checked.
    assert_eq!(
        a.sent()[0],
        vec![
            "upload".to_owned(),
            "@e11".into(),
            canonical.to_string_lossy().into_owned()
        ]
    );
}

#[test]
fn an_upload_asks_every_time_even_after_allow_in_this_conversation() {
    let a = act();
    a.on_login();
    a.answer(Some(UserAnswer::AllowConversation));
    let file = a.path("home/proj/a.txt");
    write(&file, b"a");
    for _ in 0..3 {
        let (error, text) = upload(&a, &file);
        assert!(!error, "{text}");
    }
    assert_eq!(a.cards().len(), 3);
    assert!(a.cards().iter().all(|c| !c.offer_conversation));
}

#[test]
fn an_upload_outside_the_allowed_folders_is_refused_unless_the_user_gave_the_path() {
    let a = act();
    a.on_login();
    let outside = a.path("elsewhere/cv.pdf");
    write(&outside, b"pdf");
    let (error, text) = upload(&a, &outside);
    assert!(error && text.contains("outside"), "{text}");
    assert!(a.sent().is_empty());
    assert!(a.cards().is_empty(), "refused before any card");
    assert_eq!(a.rows_of("browser_upload")[0].decision, "blocked");
    // The user wrote that path in this conversation: allowed (and still asked).
    a.up.run_started(
        0,
        &format!("please attach {} to the form", outside.display()),
    );
    a.snapshot();
    let (error, text) = upload(&a, &outside);
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1);
    // A path that only contains the given one is not the given one.
    let other = a.path("elsewhere/cv.pdf.bak");
    write(&other, b"x");
    let (error, _) = upload(&a, &other);
    assert!(error);
}

#[test]
fn dotfiles_credentials_keychains_and_symlinks_into_them_are_never_uploaded() {
    let a = act();
    a.on_login();
    let home = a.path("home");
    // The home folder itself is an allowed folder here, so each credential place is checked on
    // its own, and a dotfile inside a project folder too.
    a.configure(vec![home.clone()]);
    write(&home.join("proj/.env"), b"A=1");
    write(&home.join("proj/.git/config"), b"x");
    write(&home.join(".ssh/id_test"), b"key");
    write(&home.join(".aws/credentials"), b"key");
    write(&home.join(".gnupg/pubring.kbx"), b"key");
    write(&home.join(".config/gcloud/creds.json"), b"key");
    write(&home.join(".kube/config"), b"key");
    write(&home.join("Library/Keychains/login.keychain-db"), b"key");
    write(&home.join("proj/work.keychain-db"), b"key");
    write(&home.join("proj/fine.txt"), b"fine");
    #[cfg(unix)]
    std::os::unix::fs::symlink(home.join(".ssh/id_test"), home.join("proj/innocent.txt")).unwrap();
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut refused = vec![
        home.join("proj/.env"),
        home.join("proj/.git/config"),
        home.join(".ssh/id_test"),
        home.join(".aws/credentials"),
        home.join(".gnupg/pubring.kbx"),
        home.join(".config/gcloud/creds.json"),
        home.join(".kube/config"),
        home.join("Library/Keychains/login.keychain-db"),
        home.join("proj/work.keychain-db"),
    ];
    #[cfg(unix)]
    refused.push(home.join("proj/innocent.txt"));
    for path in &refused {
        let (error, text) = upload(&a, path);
        assert!(error, "{} was uploaded: {text}", path.display());
    }
    assert!(a.sent().is_empty());
    assert!(a.cards().is_empty());
    // The user naming a dotfile does not change it.
    a.up.run_started(0, &format!("upload {}", home.join("proj/.env").display()));
    a.snapshot();
    let (error, _) = upload(&a, &home.join("proj/.env"));
    assert!(error);
    // A plain file next to them goes.
    let (error, text) = upload(&a, &home.join("proj/fine.txt"));
    assert!(!error, "{text}");
}

#[test]
fn a_missing_file_a_folder_a_relative_path_and_no_files_are_refused() {
    let a = act();
    a.on_login();
    let missing = a.path("home/proj/nope.txt");
    let (error, text) = upload(&a, &missing);
    assert!(error && text.contains("does not exist"), "{text}");
    let folder = a.path("home/proj/sub");
    std::fs::create_dir_all(&folder).unwrap();
    let (error, text) = upload(&a, &folder);
    assert!(error && text.contains("not a regular file"), "{text}");
    let (error, text) = a.call(
        "browser_upload",
        json!({"ref": "e11", "paths": ["rel/file.txt"]}),
    );
    assert!(error && text.contains("absolute"), "{text}");
    let (error, _) = a.call("browser_upload", json!({"ref": "e11", "paths": []}));
    assert!(error);
    assert!(a.sent().is_empty());
}

// ---- tab ownership ----

pub(super) fn with_user_tab(a: &Act, url: &str) {
    a.r.runner.lock().user_tab = Some(("tab-user-1".into(), url.into(), "My tab".into()));
}

#[test]
fn the_users_own_tab_is_used_only_when_asked_and_approved_once_per_tab() {
    let a = act();
    a.on_login();
    with_user_tab(&a, "https://docs.example/page");
    let (error, text) = a.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(!error, "{text}");
    let cards = a.cards();
    assert_eq!(cards.len(), 1, "the first time asks");
    assert!(matches!(
        cards[0].facts.action,
        ActionDesc::TabTransfer { .. }
    ));
    let (_, list) = a.call("browser_tabs", json!({"action": "list"}));
    assert!(
        list.contains("tab-user-1 (the user's tab) (current)"),
        "{list}"
    );
    // Closing it only lets go of it.
    let (error, text) = a.call(
        "browser_tabs",
        json!({"action": "close", "tab": "tab-user-1"}),
    );
    assert!(!error && text.contains("stays open"), "{text}");
    let (_, list) = a.call("browser_tabs", json!({"action": "list"}));
    assert!(!list.contains("tab-user-1"), "{list}");
    // The same tab again: no second card.
    let (error, text) = a.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1);
    assert_eq!(a.r.runner.count("tabs close tab-user-1"), 0);
    // Another user tab is a new question.
    a.call(
        "browser_tabs",
        json!({"action": "close", "tab": "tab-user-1"}),
    );
    a.r.runner.lock().user_tab = Some((
        "tab-user-2".into(),
        "https://docs.example/b".into(),
        "Other".into(),
    ));
    let (error, text) = a.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 2);
}

#[test]
fn a_denied_adoption_takes_nothing_and_a_blocked_or_foreign_tab_is_refused() {
    let a = act();
    a.on_login();
    with_user_tab(&a, "https://docs.example/page");
    a.answer(Some(UserAnswer::Deny));
    let (error, text) = a.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(error && text.contains("Denied"), "{text}");
    let (_, list) = a.call("browser_tabs", json!({"action": "list"}));
    assert!(!list.contains("tab-user-1"));
    assert_eq!(a.r.runner.count("--json tab tab-user-1"), 0, "never bound");
    // A tab on a blocked page is not taken.
    let b = act_with(BrowserLevel::Act, blocked("https://bank.example"));
    b.on_login();
    with_user_tab(&b, "https://bank.example/accounts");
    let (error, text) = b.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(error && text.contains("blocked"), "{text}");
    assert!(b.cards().is_empty());
    // Naming another tab id is rejected and audited.
    let c = act();
    c.on_login();
    with_user_tab(&c, "https://docs.example/page");
    let (error, _) = c.call(
        "browser_tabs",
        json!({"action": "adopt_active", "tab": "tab-someone-else"}),
    );
    assert!(error);
    assert_eq!(c.rows_of("tab_rejected").len(), 1);
}

#[test]
fn the_users_tab_is_never_closed_never_remembered_and_unbound_after_a_hit() {
    let a = act_with(BrowserLevel::Act, blocked("https://bank.example"));
    let state_file = a.path("tabs.json");
    a.r.controller.set_state_file(state_file.clone());
    let mut page = login_page();
    page.elements.push(
        El::new("e50", "button", "Go")
            .attr("type", "button")
            .attr("navigates_to", "https://bank.example/login"),
    );
    a.on(page);
    with_user_tab(&a, "https://docs.example/page");
    let (error, text) = a.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(!error, "{text}");
    a.snapshot();
    // The tab is not in zptah's own set, so it is not written to the file of tabs to close.
    let saved = std::fs::read_to_string(&state_file).unwrap_or_default();
    assert!(!saved.contains("tab-user-1"), "{saved}");
    assert!(a
        .r
        .controller
        .session_tabs("zptah-mono-1-0")
        .iter()
        .all(|t| t != "tab-user-1"));
    // A click that leaves it on a blocked page: left open, unbound, audited as the user's.
    let (error, text) = a.call("browser_click", json!({"ref": "e50"}));
    assert!(error && text.contains("stays open"), "{text}");
    assert_eq!(a.r.runner.count("tabs close tab-user-1"), 0);
    assert_eq!(a.rows_of("browser_click")[0].owner, "user");
    // The conversation ending and the cleanup never close it.
    a.up.end_conversation(0);
    a.r.controller.close_all_tabs();
    assert_eq!(a.r.runner.count("tabs close tab-user-1"), 0);
}

#[test]
fn the_users_tab_is_not_navigated_away_without_the_normal_rules() {
    let a = act();
    a.on_login();
    with_user_tab(&a, "https://docs.example/page");
    let (error, text) = a.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1);
    // An address outside the task asks, as for any navigation.
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://elsewhere.example/x"}),
    );
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 2, "a second card for the navigation");
    assert!(a.cards()[1]
        .facts
        .categories
        .contains(&Category::UnrelatedDomain));
    // Denied: the tab does not move.
    a.answer(Some(UserAnswer::Deny));
    let before = a.r.runner.count("--json open");
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://another.example/y"}),
    );
    assert!(error && text.contains("Denied"), "{text}");
    assert_eq!(a.r.runner.count("--json open"), before);
    // One of the Mono's own tabs moves without asking.
    let b = act();
    b.on_login();
    let (error, text) = b.call(
        "browser_navigate",
        json!({"url": "https://elsewhere.example/x"}),
    );
    assert!(!error, "{text}");
    assert!(b.cards().is_empty());
}

#[test]
fn any_other_tab_id_is_rejected_and_audited() {
    let a = act();
    a.on_login();
    for action in ["select", "close"] {
        let (error, text) = a.call(
            "browser_tabs",
            json!({"action": action, "tab": "tab-users-own"}),
        );
        assert!(
            error && text.contains("not one this conversation opened"),
            "{text}"
        );
    }
    assert_eq!(a.rows_of("tab_rejected").len(), 2);
    assert_eq!(a.r.runner.count("tab-users-own"), 0);
}

// ---- Stop ----

#[test]
fn stop_kills_the_call_in_flight_releases_the_holds_and_denies_the_rest_of_the_turn() {
    let a = act();
    a.on_login();
    a.r.runner.lock().hang_on = Some("--json read".into());
    let worker = std::thread::spawn({
        let up = a.up.clone();
        move || call(&up, "browser_read", json!({}))
    });
    assert!(wait_until(|| a.r.runner.lock().hanging == 1));
    assert!(a.up.working(0));
    assert!(crate::mono::browser::ardia::browser_working(0));
    a.up.stop_turn(0);
    let (error, text) = worker.join().unwrap();
    assert!(error && text.contains("stopped the browser"), "{text}");
    assert_eq!(a.r.runner.lock().killed, 1, "the process was killed");
    assert!(
        wait_until(|| a.r.runner.count("tabs.release") == 1),
        "the holds are released"
    );
    assert!(!a.up.working(0));
    // The rest of the turn: reads and acts alike.
    a.r.runner.lock().hang_on = None;
    let before = a.r.runner.calls().len();
    for (tool, args) in [
        ("browser_read", json!({})),
        ("browser_click", json!({"ref": "e9"})),
        ("browser_snapshot", json!({})),
        ("browser_open", json!({"url": "https://example.com"})),
    ] {
        let (error, text) = a.call(tool, args);
        assert!(
            error && text.contains("stopped the browser"),
            "{tool}: {text}"
        );
    }
    assert_eq!(a.r.runner.calls().len(), before, "nothing reached ardia");
    assert_eq!(a.rows_of("browser_stop").len(), 1);
    // The next user message starts the turn again.
    a.up.run_started(0, "carry on");
    let (error, text) = a.call("browser_read", json!({}));
    assert!(!error, "{text}");
}

#[test]
fn stop_kills_an_act_in_flight_and_says_it_may_have_run() {
    let a = act();
    a.on_login();
    a.r.runner.lock().hang_on = Some("--json click".into());
    let worker = std::thread::spawn({
        let up = a.up.clone();
        move || call(&up, "browser_click", json!({"ref": "e9"}))
    });
    assert!(wait_until(|| a.r.runner.lock().hanging == 1));
    a.up.stop_turn(0);
    let (error, text) = worker.join().unwrap();
    assert!(error && text.contains("may or may not have run"), "{text}");
    assert_eq!(a.r.runner.lock().killed, 1);
    assert!(a.rows_of("browser_click")[0].reason.contains("unknown"));
}

#[test]
fn stop_withdraws_an_open_card() {
    let a = act();
    a.on_login();
    a.answer(None);
    let worker = std::thread::spawn({
        let up = a.up.clone();
        move || call(&up, "browser_click", json!({"ref": "e12"}))
    });
    assert!(wait_until(|| !a.queue.pending().is_empty()));
    a.up.stop_turn(0);
    let (error, text) = worker.join().unwrap();
    assert!(error && text.contains("stopped the browser"), "{text}");
    assert!(a.queue.pending().is_empty());
    assert!(a.sent().is_empty());
}

#[test]
fn the_core_api_stops_a_monos_conversation_by_id() {
    let r = rig(true);
    let mono = format!("mono-stop-{}", NEXT.fetch_add(1, Ordering::SeqCst));
    let up = ArdiaUpstream::with_bind_delay(&mono, r.controller.clone(), Duration::from_millis(1));
    up.configure(Config {
        profile: PROFILE.into(),
        ..Config::default()
    });
    call(&up, "browser_open", json!({"url": "https://example.com"}));
    crate::mono::browser::stop_browser(&mono, 0);
    let (error, text) = call(&up, "browser_read", json!({}));
    assert!(error && text.contains("stopped the browser"), "{text}");
    // Another conversation of the same Mono is not stopped.
    assert!(!up.working(5));
}

#[test]
fn a_gate_that_was_shut_down_refuses_the_rest_of_the_turn() {
    use crate::mono::browser::server::BrowserServers;
    let a = act();
    let servers = BrowserServers::new();
    let gate = servers
        .gate_for_ardia(
            &a.mono,
            a.up.clone(),
            Policy::ardia(BrowserLevel::Act, vec![]),
        )
        .unwrap();
    let route = gate.route_for(Some(3));
    let call_gate = |tool: &str| {
        gate.handle(
            &route,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": {}}}),
        )
        .unwrap()["result"]
            .clone()
    };
    assert_eq!(call_gate("browser_tabs")["isError"], false);
    servers.stop(&a.mono);
    let result = call_gate("browser_tabs");
    assert_eq!(result["isError"], true);
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("turned off"),
        "{result}"
    );
    assert!(gate.is_shut_down());
}

// ---- the retry rule ----

#[test]
fn an_act_whose_outcome_is_unknown_is_never_repeated_but_a_read_is_repeated_once() {
    for kind in [FailKind::Gone, FailKind::Timeout, FailKind::Garbage] {
        let a = act();
        a.on_login();
        a.r.runner.lock().fail_on = Some(("click".into(), kind));
        let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
        assert!(
            error && text.contains("may or may not have run"),
            "{kind:?}: {text}"
        );
        assert_eq!(
            a.sent().len(),
            1,
            "{kind:?}: the command was not sent again"
        );
        let row = &a.rows_of("browser_click")[0];
        assert!(row.reason.contains("outcome is unknown"), "{row:?}");
    }
    // A read call that finds Ardia gone is checked again and run once more.
    let a = act();
    a.on_login();
    let reads = a.r.runner.count("--json read");
    a.r.runner.lock().gone_on_next_browse = true;
    let (error, text) = a.call("browser_read", json!({}));
    assert!(!error, "{text}");
    assert_eq!(a.r.runner.count("--json read") - reads, 2);
}

#[test]
fn a_refused_act_command_is_a_plain_failure_not_an_unknown_outcome() {
    let a = act();
    a.on_login();
    let (error, text) = a.call("browser_hover", json!({"ref": "e9"}));
    assert!(!error, "{text}");
    a.r.runner.lock().broken.insert("e9".into(), "stale_ref");
    let (error, text) = a.call("browser_hover", json!({"ref": "e9"}));
    assert!(error && !text.contains("may or may not"), "{text}");
}

// ---- the other tools ----

#[test]
fn select_check_and_hover_run_the_matching_commands() {
    let a = act();
    a.on_login();
    for (tool, args, cmd) in [
        (
            "browser_select",
            json!({"ref": "e14", "value": "blue"}),
            "select",
        ),
        ("browser_check", json!({"ref": "e13"}), "check"),
        (
            "browser_check",
            json!({"ref": "e13", "checked": false}),
            "uncheck",
        ),
        ("browser_hover", json!({"ref": "e9"}), "hover"),
    ] {
        let (error, text) = a.call(tool, args);
        assert!(!error, "{tool}: {text}");
        assert_eq!(a.sent().last().unwrap()[0], cmd);
    }
    // Hovering never asks.
    assert!(a.cards().is_empty());
}

#[test]
fn a_terms_checkbox_asks() {
    let a = act();
    let mut page = login_page();
    page.elements.push(
        El::new("e60", "checkbox", "I agree to the terms and conditions").attr("type", "checkbox"),
    );
    a.on(page);
    let (error, text) = a.call("browser_check", json!({"ref": "e60"}));
    assert!(!error, "{text}");
    let cards = a.cards();
    assert_eq!(cards.len(), 1);
    assert!(cards[0].facts.categories.contains(&Category::Terms));
}

#[test]
fn page_text_in_an_act_reply_is_cleaned() {
    let a = act();
    let mut page = login_page();
    page.elements
        .push(El::new("e70", "button", "Fine\u{200b}\u{202e} button").attr("type", "button"));
    a.on(page);
    let (error, text) = a.call("browser_click", json!({"ref": "e70"}));
    assert!(!error, "{text}");
    assert!(
        !text.contains('\u{200b}') && !text.contains('\u{202e}'),
        "{text:?}"
    );
}
