//! The act tools of the extension upstream (spec 6.2, 7.1, 7.6, 8): click, type, fill, select,
//! press, hover, drag, scroll, navigate, dialogs, uploads and the agent's own approval request.
//!
//! Every write is two steps (the T14 contract in `shared/protocol.json`): `act.prepare` returns
//! what will run (element and form signals, the extension's own risk categories, a digest and,
//! when the tab shows, a picture); the app classifies it with [`TrustState::evaluate`] (the
//! extension's categories plus the ones only the app knows: an unnamed recipient, an unrelated
//! domain, a sensitive site, taint, the soul rules); then `act.commit` runs it, bound to the
//! digest the user approved. Anything the extension answers differently from what was prepared
//! (`E_STALE_PREPARE`) is reported, never retried blindly.
//!
//! Asking: [`ApprovalQueue::ask`] blocks the call until the user answers (or the card times out)
//! and keeps the prepared action alive with `act.extend`. A card never exists without the facts
//! of the prepare; a missing picture never turns an ask into an allow. Nobody who could answer
//! (no front end, a plan or read-only run) is a deny.
//!
//! One audit row per decision is written here (`trust::audit`); typed values are never stored.

use super::super::super::copy::{self, ActionDesc, CardFacts};
use super::super::super::link::protocol::{Action, Envelope, Event, FillField, Method};
use super::super::super::ownership;
use super::super::super::risk::signals::{
    classify_field, registrable_domain_of, site_category, ActionKind, ElementSignals, FieldAttrs,
    FieldKind, FormSignals,
};
use super::super::super::risk::Category;
use super::super::super::soul_rules::Parsed;
use super::super::super::trust::approval::{
    offers_conversation, queue_for, ApprovalQueue, CardScreenshot, Outcome, Request, UserAnswer,
};
use super::super::super::trust::audit::{typed_summary, AuditDecision, DecisionAudit};
use super::super::super::trust::grants::GrantBook;
use super::super::super::trust::{
    Action as TrustAction, Answer, BrowserLevel, Config, Evaluation, Tier, TrustState,
};
use super::super::super::{output, MonoBrowserStore};
use super::super::{audit_url, RunContext};
use super::*;
use crate::mono::browser::risk::decide::Decision;
use std::collections::BTreeSet;
use std::sync::Weak;

mod own_tabs;

/// How often the prepared action is kept alive while a card is open (a prepare lives 30 s).
const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// How far ahead each keep-alive pushes the prepare's expiry.
const EXTEND_MS: u64 = 30_000;
/// The most bytes of one upload chunk (`upload.put` accepts 512 KiB decoded).
const UPLOAD_CHUNK: usize = 384 * 1024;
/// The most the agent may upload in one call (the extension's limit, `E_TOO_LARGE` beyond it).
const MAX_UPLOAD_BYTES: u64 = 32 * 1024 * 1024;

const ACT_TOOLS: [&str; 14] = [
    "browser_click",
    "browser_type",
    "browser_fill_form",
    "browser_select_option",
    "browser_press_key",
    "browser_hover",
    "browser_drag",
    "browser_mouse_wheel",
    "browser_navigate",
    "browser_navigate_back",
    "browser_navigate_forward",
    "browser_handle_dialog",
    "browser_file_upload",
    super::super::APPROVAL_TOOL,
];

pub(super) fn is_act_tool(name: &str) -> bool {
    ACT_TOOLS.contains(&name)
}

// ---- configuration and per-conversation state ----

/// What the wiring tells the upstream about its Mono. Set before the first run and on every
/// launch (the level and rules are live settings).
#[derive(Clone)]
pub struct ActConfig {
    pub level: BrowserLevel,
    /// The soul rules in force (`soul_rules::reconcile`).
    pub rules: Parsed,
    /// Where grants and audit rows live. None (tests, no database): grants stay in memory and no
    /// audit row is written.
    pub store: Option<MonoBrowserStore>,
    /// The Mono's name as the card says it (`Mono::label()`).
    pub mono_label: String,
    /// The paired browser profile's name, for the audit row.
    pub profile: Option<String>,
    /// How long a card waits for the user.
    pub approval_timeout: Duration,
    /// Where the cards of this Mono go.
    pub queue: Arc<ApprovalQueue>,
}

pub(super) struct ActShared {
    config: Mutex<ActConfig>,
    state: Mutex<ActState>,
}

#[derive(Default)]
struct ActState {
    /// What the gate remembers, per app session (one conversation each).
    trust: HashMap<i64, TrustState>,
    /// The text of the last JavaScript dialog the extension reported, per tab.
    dialogs: HashMap<u64, String>,
    /// Single keys pressed per (session, tab), for the card-number check only.
    key_text: HashMap<(i64, u64), String>,
    /// The toolbar Stop was pressed and not resumed.
    stopped: bool,
}

impl ActShared {
    pub(super) fn new(mono_id: &str) -> Self {
        Self {
            config: Mutex::new(ActConfig {
                level: BrowserLevel::Act,
                rules: Parsed::default(),
                store: None,
                mono_label: mono_id.to_owned(),
                profile: None,
                approval_timeout: super::super::APPROVAL_TIMEOUT,
                queue: queue_for(mono_id),
            }),
            state: Mutex::new(ActState::default()),
        }
    }
}

/// The extension upstreams alive in this process, so a link event (Stop) finds them.
fn upstreams() -> &'static Mutex<Vec<Weak<ExtensionUpstream>>> {
    static ALL: std::sync::OnceLock<Mutex<Vec<Weak<ExtensionUpstream>>>> =
        std::sync::OnceLock::new();
    ALL.get_or_init(|| Mutex::new(Vec::new()))
}

pub(super) fn register(upstream: &Arc<ExtensionUpstream>) {
    let mut all = lock(upstreams());
    all.retain(|w| w.strong_count() > 0);
    all.push(Arc::downgrade(upstream));
}

/// The upstream of a Mono, if one was built in this process (the newest).
pub fn upstream_for(mono_id: &str) -> Option<Arc<ExtensionUpstream>> {
    lock(upstreams())
        .iter()
        .rev()
        .filter_map(Weak::upgrade)
        .find(|u| u.mono_id == mono_id)
}

/// Hands an extension event of the link `instance_id` to the upstreams on that link. The code
/// that owns the link manager's event receiver calls this for every `LinkEvent::Extension`.
#[allow(dead_code)] // called by the link event pump (the Connect card, spec ticket T8)
pub(crate) fn dispatch_event(instance_id: &str, envelope: &Envelope) {
    let all: Vec<Arc<ExtensionUpstream>> =
        lock(upstreams()).iter().filter_map(Weak::upgrade).collect();
    // Ownership changes are the link's, not one Mono's: the mirror follows them, and the Mono the
    // user took a tab back from stops relying on what it was allowed there.
    if envelope.method.as_deref() == Some(Event::TabOwnership.as_str()) {
        let params = envelope.params.clone().unwrap_or(Value::Null);
        let change = ownership::with_table(instance_id, |table| {
            table.apply_event(&params, crate::time::now_ms())
        });
        // Only the user dragging the tab back into the group ends the Stop mark.
        let dragged_back = params["by"].as_str() == Some("user_drag");
        if let Some(change) = change.as_ref().filter(|_| dragged_back) {
            for upstream in all.iter().filter(|u| {
                change.to.as_deref() == Some(u.mono_id.as_str())
                    && (u.link)().is_some_and(|l| l.instance_id() == instance_id)
            }) {
                upstream.tab_given_back(change.tab);
            }
        }
        // A tab the user took back, or dragged into another Mono's group, is no longer the old
        // Mono's: what it was allowed on that site ends.
        if let Some(change) =
            change.filter(|c| c.taken_back || (c.from.is_some() && c.to != c.from))
        {
            for upstream in all.iter().filter(|u| {
                change.from.as_deref() == Some(u.mono_id.as_str())
                    && (u.link)().is_some_and(|l| l.instance_id() == instance_id)
            }) {
                upstream.tab_taken_back(change.tab);
            }
        }
        return;
    }
    // The overlay's Stop names its Mono in the params (`tab.stopped {tab, mono, by}`). Only that
    // Mono's upstream acts, and only when the mirror says it owns the tab; an event without a
    // Mono, or for a tab the Mono does not own, changes nothing.
    if envelope.method.as_deref() == Some(Event::TabStopped.as_str()) {
        let params = envelope.params.clone().unwrap_or(Value::Null);
        let (Some(mono), Some(tab)) = (params["mono"].as_str(), params["tab"].as_u64()) else {
            return;
        };
        for upstream in all.iter().filter(|u| {
            u.mono_id == mono && (u.link)().is_some_and(|l| l.instance_id() == instance_id)
        }) {
            let owns = ownership::with_table(instance_id, |t| t.owner_of(tab) == Some(mono));
            if owns {
                upstream.tab_stopped_by_user(tab);
            }
        }
        return;
    }
    for upstream in all {
        let on_link = (upstream.link)().is_some_and(|l| l.instance_id() == instance_id);
        let for_mono = envelope
            .mono
            .as_deref()
            .is_none_or(|m| m == upstream.mono_id);
        if on_link && for_mono {
            upstream.note_event(envelope);
        }
    }
}

/// What the app shows in the stopped banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionStatus {
    /// The toolbar Stop is on: no Mono can use the browser until it is resumed.
    pub stopped: bool,
    /// Tabs the extension holds a debugger session on.
    pub debugger_tabs: Vec<u64>,
}

// ---- the public control surface ----

impl ExtensionUpstream {
    /// Applies the Mono's settings. A change of the level, the rules or the origin lists ends every
    /// grant of the Mono (spec 7.6: grants end when the Mono's browser settings change).
    pub fn configure_act(&self, config: ActConfig, origins: Origins) {
        let changed = {
            let mut current = lock(&self.act.config);
            let changed = current.level != config.level
                || current.rules.lines != config.rules.lines
                || self.origins() != origins;
            *current = config;
            changed
        };
        self.set_origins(origins);
        if changed {
            self.end_all_grants();
        }
    }

    /// The config as it is now.
    pub fn act_config(&self) -> ActConfig {
        lock(&self.act.config).clone()
    }

    /// The approval queue of this Mono.
    pub fn approvals(&self) -> Arc<ApprovalQueue> {
        lock(&self.act.config).queue.clone()
    }

    /// Whether the toolbar Stop is on, asked of the extension. `Err`: the extension is not
    /// connected.
    pub fn extension_status(&self) -> Result<ExtensionStatus, String> {
        let result = self.ask_open(Method::StatusGet, json!({}))?;
        let stopped = result["stopped"].as_bool().unwrap_or(false);
        lock(&self.act.state).stopped = stopped;
        Ok(ExtensionStatus {
            stopped,
            debugger_tabs: result["debugger_attached"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .collect(),
        })
    }

    /// The user pressed Resume on the stopped banner.
    pub fn resume(&self) -> Result<(), String> {
        self.ask_open(Method::StopResume, json!({}))?;
        lock(&self.act.state).stopped = false;
        Ok(())
    }

    /// Whether a Stop was seen and not resumed (from events or from an `E_STOPPED` answer).
    pub fn stopped(&self) -> bool {
        lock(&self.act.state).stopped
    }

    /// The conversation closed or was cleared: its grants and memory end.
    pub fn end_conversation(&self, session: i64) {
        let removed = lock(&self.act.state).trust.remove(&session);
        match removed {
            Some(mut state) => state.end_conversation(),
            None => {
                let config = self.act_config();
                GrantBook::stored_or_memory(&config.store, &self.mono_id, session)
                    .end_conversation();
            }
        }
        self.approvals().withdraw_session(session);
    }

    /// Ends every grant of this Mono (Stop, a change of settings, the user's "revoke all").
    pub fn end_all_grants(&self) {
        if let Some(store) = self.act_config().store {
            let _ = store.revoke_grants(&self.mono_id, None, None);
        }
        for state in lock(&self.act.state).trust.values_mut() {
            state.grants_mut().end_conversation();
        }
    }

    /// `ask` for the methods that are not Mono-scoped (`status.get`, `stop.resume`).
    fn ask_open(&self, method: Method, params: Value) -> Result<Value, String> {
        let Some(link) = (self.link)().filter(LinkHandle::is_open) else {
            return Err(not_connected());
        };
        link.request(method, None, params, Duration::from_secs(10))
            .map_err(|e| request_error_text(e, method))
    }

    // ---- the run's life ----

    pub(super) fn begin_run(&self, session: i64, message: &str) {
        self.with_trust(session, |state, _| state.begin_run(message));
    }

    pub(super) fn end_run(&self, session: i64) {
        // Cards of a run that is over are withdrawn; the debugger detaches 2 minutes after the
        // last run of the Mono (decision 4).
        self.approvals().withdraw_session(session);
        let Some(link) = (self.link)().filter(LinkHandle::is_open) else {
            return;
        };
        let mono = self.mono_id.clone();
        std::thread::spawn(move || {
            let _ = link.request(
                Method::TaskEnd,
                Some(&mono),
                json!({"deadline_ms": 5000}),
                Duration::from_secs(10),
            );
        });
    }

    pub(super) fn flag_injection(&self, session: i64) {
        self.with_trust(session, |state, _| state.flag_injection());
    }

    pub(super) fn note_read(&self, url: &str, text: &str) {
        let session = RunContext::current().map_or(0, |c| c.session_id);
        self.with_trust(session, |state, _| state.note_read(url, text));
    }

    /// Runs `f` on the session's trust state, creating it on first use (grants come from the
    /// store, so a revoke made elsewhere shows at once).
    fn with_trust<R>(&self, session: i64, f: impl FnOnce(&mut TrustState, &ActConfig) -> R) -> R {
        let config = self.act_config();
        let mut state = lock(&self.act.state);
        let trust = state.trust.entry(session).or_insert_with(|| {
            TrustState::new(GrantBook::stored_or_memory(
                &config.store,
                &self.mono_id,
                session,
            ))
        });
        f(trust, &config)
    }

    /// An event of the extension (spec 5.6) that matters to the gate.
    #[allow(dead_code)] // reached through `dispatch_event`
    pub(crate) fn note_event(&self, envelope: &Envelope) {
        let Some(event) = envelope.method.as_deref() else {
            return;
        };
        let params = envelope.params.clone().unwrap_or(Value::Null);
        match event {
            e if e == Event::Stop.as_str() => {
                lock(&self.act.state).stopped = true;
                self.end_all_grants();
                self.approvals().withdraw_all();
                self.write_row(
                    0,
                    DecisionAudit {
                        ts_ms: crate::time::now_ms(),
                        session_id: 0,
                        tool: "stop".into(),
                        url: String::new(),
                        summary: "toolbar Stop".into(),
                        tab_id: None,
                        tab_owner: None,
                        tier: "A".into(),
                        categories: Vec::new(),
                        decision: AuditDecision::Stopped,
                        reason: "the user pressed Stop in the toolbar".into(),
                        approval_id: None,
                        screenshot: None,
                        actor: "user_stop".into(),
                        input: None,
                        profile: None,
                    },
                );
            }
            e if e == Event::DialogOpened.as_str() => {
                if let Some(tab) = params["tab"].as_u64() {
                    lock(&self.act.state).dialogs.insert(
                        tab,
                        copy::clean(params["message"].as_str().unwrap_or(""), 400),
                    );
                }
            }
            e if e == Event::ActLate.as_str() => {
                let ok = params["ok"].as_bool().unwrap_or(false);
                let reason = if ok {
                    "late result: the action ran".to_owned()
                } else {
                    format!(
                        "late result: failed ({})",
                        params["error"]["code"].as_str().unwrap_or("error")
                    )
                };
                self.write_row(
                    0,
                    DecisionAudit {
                        ts_ms: crate::time::now_ms(),
                        session_id: 0,
                        tool: "act_late".into(),
                        url: audit_url(params["url"].as_str().unwrap_or("")),
                        summary: "the page answered after a suspected dialog".into(),
                        tab_id: params["tab"].as_i64(),
                        tab_owner: Some(self.mono_id.clone()),
                        tier: "A".into(),
                        categories: Vec::new(),
                        decision: AuditDecision::Auto,
                        reason,
                        approval_id: None,
                        screenshot: None,
                        actor: "mono".into(),
                        input: params["effects"]["method"].as_str().map(str::to_owned),
                        profile: None,
                    },
                );
            }
            _ => {}
        }
    }

    // ---- audit ----

    /// Writes one row and removes the screenshots of rows retention deleted.
    fn write_row(&self, _session: i64, row: DecisionAudit) {
        let config = self.act_config();
        let Some(store) = config.store else {
            return;
        };
        let mut row = row;
        row.profile = row.profile.or(config.profile);
        if let Ok(gone) = store.record_decision(&self.mono_id, &row) {
            if !gone.is_empty() {
                if let Some(dir) = lock(&self.output_dir).clone() {
                    output::delete_files(&dir, &gone);
                }
            }
        }
    }
}

impl GrantBook {
    /// A stored book when there is a store, else an in-memory one.
    pub(crate) fn stored_or_memory(
        store: &Option<MonoBrowserStore>,
        mono_id: &str,
        conversation: i64,
    ) -> GrantBook {
        match store {
            Some(store) => GrantBook::stored(store.clone(), mono_id, conversation),
            None => GrantBook::in_memory(mono_id, conversation),
        }
    }
}

// ---- one action ----

/// What `act.prepare` answered.
struct Prepared {
    /// The tab the action was prepared on (set by `prepare`, the extension does not echo it).
    tab: u64,
    id: String,
    digest: String,
    url: String,
    title: String,
    element: ElementSignals,
    form: FormSignals,
    ext: BTreeSet<Category>,
    screenshot: Option<(String, String, u32, u32)>,
}

impl Prepared {
    fn parse(result: &Value) -> Result<Self, String> {
        let bad = || "The extension sent an action preparation that could not be read.".to_owned();
        let ext: BTreeSet<Category> = result["ext_risk"]
            .as_array()
            .ok_or_else(bad)?
            .iter()
            .filter_map(|v| v.as_str().and_then(Category::parse))
            .collect();
        Ok(Self {
            tab: 0,
            id: result["prepare_id"].as_str().ok_or_else(bad)?.to_owned(),
            digest: result["digest"].as_str().ok_or_else(bad)?.to_owned(),
            url: str_field(result, "url"),
            title: str_field(result, "title"),
            element: serde_json::from_value(result["element"].clone()).map_err(|_| bad())?,
            form: serde_json::from_value(result["form"].clone()).unwrap_or_default(),
            ext,
            screenshot: result["screenshot"]["png_b64"].as_str().map(|b64| {
                (
                    b64.to_owned(),
                    str_field(&result["screenshot"], "method"),
                    result["screenshot"]["width"].as_u64().unwrap_or(0) as u32,
                    result["screenshot"]["height"].as_u64().unwrap_or(0) as u32,
                )
            }),
        })
    }

    /// The name of the element the user sees.
    fn label(&self) -> String {
        [
            self.element.label.as_deref().unwrap_or(""),
            &self.element.name,
            &self.element.text,
            self.element.placeholder.as_deref().unwrap_or(""),
        ]
        .into_iter()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_owned()
    }
}

/// How the call is shown and judged, built from the tool and its arguments.
struct Plan {
    tool: String,
    args: Value,
    action: Action,
    kind: Option<ActionKind>,
    /// Every value the call types (looked at for a card number, never stored).
    typed: Option<String>,
    /// For the audit and the result: a short description without typed values.
    summary: String,
    /// The tool shows `kind` as typing.
    types: bool,
    is_fill: bool,
    hover: bool,
}

fn s<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args[key].as_str().map(str::trim).filter(|v| !v.is_empty())
}

fn ref_arg(args: &Value) -> Result<String, String> {
    let target = s(args, "ref")
        .or_else(|| s(args, "target"))
        .ok_or("This needs `ref`: the [ref=eN] of an element from a browser_snapshot.")?;
    if !is_ref(target) {
        return Err(format!(
            "`ref` must be a ref from a snapshot, such as e12; this browser has no selectors (got `{target}`)."
        ));
    }
    Ok(target.to_owned())
}

fn build_plan(tool: &str, args: &Value) -> Result<Plan, String> {
    let base = |action: Action, summary: String| Plan {
        tool: tool.to_owned(),
        args: args.clone(),
        action,
        kind: Some(ActionKind::Click),
        typed: None,
        summary,
        types: false,
        is_fill: false,
        hover: false,
    };
    Ok(match tool {
        "browser_click" => {
            let target = ref_arg(args)?;
            let button = match s(args, "button") {
                None | Some("left") => None,
                Some(b @ ("right" | "middle")) => Some(b.to_owned()),
                Some(other) => {
                    return Err(format!("`button` is left, right or middle, not `{other}`."))
                }
            };
            base(
                Action::Click {
                    target,
                    button,
                    double: args["doubleClick"].as_bool().filter(|d| *d),
                    modifiers: args["modifiers"].as_array().map(|m| {
                        m.iter()
                            .filter_map(|v| v.as_str().map(|s| s.chars().take(16).collect()))
                            .collect()
                    }),
                },
                "click".into(),
            )
        }
        "browser_type" => {
            let target = ref_arg(args)?;
            let text = args["text"]
                .as_str()
                .ok_or("browser_type needs `text`.")?
                .to_owned();
            let mut plan = base(
                Action::Type {
                    target,
                    text: text.clone(),
                    submit: args["submit"].as_bool().filter(|v| *v),
                    slowly: args["slowly"].as_bool().filter(|v| *v),
                },
                String::new(),
            );
            plan.summary = typed_summary("field", &text);
            plan.typed = Some(text);
            plan.types = true;
            plan.kind = None; // set from the field kind after the prepare
            plan
        }
        "browser_fill_form" => {
            let fields: Vec<FillField> = args["fields"]
                .as_array()
                .filter(|f| !f.is_empty())
                .ok_or("browser_fill_form needs `fields`: a list of {ref, type, value}.")?
                .iter()
                .map(|f| {
                    let target = ref_arg(f)?;
                    let kind = match s(f, "type") {
                        None | Some("textbox") | Some("text") => "text",
                        Some(k @ ("checkbox" | "radio" | "combobox" | "slider")) => k,
                        Some(other) => {
                            return Err(format!("Field type `{other}` is not supported; use textbox, checkbox, radio, combobox or slider."))
                        }
                    };
                    let value = match &f["value"] {
                        Value::String(v) => v.clone(),
                        Value::Bool(b) => b.to_string(),
                        Value::Number(n) => n.to_string(),
                        _ => return Err("Every field needs a `value`.".into()),
                    };
                    Ok(FillField {
                        target,
                        value,
                        field_type: kind.into(),
                    })
                })
                .collect::<Result<_, String>>()?;
            let typed = fields
                .iter()
                .map(|f| f.value.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let mut plan = base(
                Action::Fill {
                    fields: fields.clone(),
                },
                String::new(),
            );
            plan.summary = format!("filled {} field(s)", fields.len());
            plan.typed = Some(typed);
            plan.types = true;
            plan.is_fill = true;
            plan.kind = None;
            plan
        }
        "browser_select_option" => {
            let target = ref_arg(args)?;
            let values: Vec<String> = args["values"]
                .as_array()
                .filter(|v| !v.is_empty())
                .ok_or("browser_select_option needs `values`: the options to select.")?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            let mut plan = base(Action::Select { target, values }, "select".into());
            plan.kind = Some(ActionKind::Select);
            plan
        }
        "browser_press_key" => {
            let key = s(args, "key")
                .ok_or("browser_press_key needs `key`.")?
                .to_owned();
            let target = match args.get("ref") {
                None | Some(Value::Null) => None,
                Some(_) => Some(ref_arg(args)?),
            };
            let scroll = crate::mono::browser::is_scroll_key(&key);
            let enter = matches!(key.as_str(), "Enter" | "NumpadEnter" | " " | "Space");
            let mut plan = base(
                Action::Press {
                    key: key.clone(),
                    target,
                },
                if key.chars().count() == 1 && !scroll && !enter {
                    "pressed a text key into field".to_owned()
                } else {
                    format!("press {key}")
                },
            );
            plan.kind = if scroll {
                None
            } else if enter {
                Some(ActionKind::Click)
            } else {
                None // text keys: the field kind decides after the prepare
            };
            plan.types = !scroll && !enter && key.chars().count() == 1;
            plan
        }
        "browser_hover" => {
            let mut plan = base(
                Action::Hover {
                    target: ref_arg(args)?,
                },
                "hover".into(),
            );
            plan.kind = None;
            plan.hover = true;
            plan
        }
        "browser_drag" => {
            let from = s(args, "startRef").ok_or("browser_drag needs `startRef` and `endRef`.")?;
            let to = s(args, "endRef").ok_or("browser_drag needs `startRef` and `endRef`.")?;
            if !is_ref(from) || !is_ref(to) {
                return Err(
                    "`startRef` and `endRef` must be refs from a snapshot, such as e12.".into(),
                );
            }
            base(
                Action::Drag {
                    from_ref: from.to_owned(),
                    to_ref: to.to_owned(),
                },
                "drag".into(),
            )
        }
        "browser_mouse_wheel" => {
            let dx = args["deltaX"].as_f64().unwrap_or(0.0).round() as i64;
            let dy = args["deltaY"].as_f64().unwrap_or(0.0).round() as i64;
            let mut plan = base(
                Action::Scroll {
                    target: None,
                    dx: dx.clamp(-1_000_000, 1_000_000),
                    dy: dy.clamp(-1_000_000, 1_000_000),
                },
                "scroll".into(),
            );
            plan.kind = None;
            plan
        }
        other => return Err(format!("{other} is not an element action.")),
    })
}

/// A sample of the sort of failure the agent gets, in words.
fn denied_by_user() -> String {
    "Denied by the user. Do not do this action, and do not try another way to do it.".into()
}

impl ExtensionUpstream {
    pub(super) fn act_tool(&self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "browser_navigate" | "browser_navigate_back" | "browser_navigate_forward" => {
                self.navigate_tool(name, args)
            }
            "browser_handle_dialog" => self.dialog_tool(args),
            "browser_file_upload" => self.upload_tool(args),
            n if n == super::super::APPROVAL_TOOL => self.request_approval_tool(args),
            _ => {
                let mut plan = build_plan(name, args)?;
                self.note_key_text(&mut plan);
                self.perform(plan)
            }
        }
    }

    /// Characters pressed one key at a time add up per tab, in memory only, so a card number typed
    /// digit by digit is still seen by the card check. Never written anywhere.
    fn note_key_text(&self, plan: &mut Plan) {
        let session = self.run_context().session_id;
        let tab = plan.args["tab"].as_u64().unwrap_or(0);
        let mut state = lock(&self.act.state);
        match &plan.action {
            Action::Press { key, .. } if plan.types => {
                let buffer = state.key_text.entry((session, tab)).or_default();
                buffer.push_str(key);
                let extra = buffer.chars().count().saturating_sub(64);
                if extra > 0 {
                    *buffer = buffer.chars().skip(extra).collect();
                }
                plan.typed = Some(buffer.clone());
            }
            Action::Press { .. } | Action::Type { .. } | Action::Fill { .. } => {
                state.key_text.remove(&(session, tab));
            }
            _ => {}
        }
    }

    fn run_context(&self) -> RunContext {
        RunContext::current().unwrap_or(RunContext {
            route: String::new(),
            session_id: 0,
        })
    }

    /// The tab an act tool works on, with the Mono's origin lists applied.
    fn act_tab(&self, args: &Value) -> Result<TabInfo, String> {
        self.resolve_tab(args)
    }

    // ---- prepare ----

    fn prepare(
        &self,
        tab: u64,
        action: &Action,
        screenshot: bool,
    ) -> Result<Prepared, PrepareFail> {
        let mut params = json!({"tab": tab, "action": action});
        if screenshot {
            params["needs_screenshot"] = json!(true);
        }
        match self.ask_raw(Method::ActPrepare, params, DEFAULT_DEADLINE_MS) {
            Ok(result) => Prepared::parse(&result)
                .map(|p| Prepared { tab, ..p })
                .map_err(PrepareFail::Text),
            Err(RequestError::Remote(body)) if body.code == ErrorCode::PasswordField => {
                Err(PrepareFail::Password(remote_text(&body)))
            }
            Err(RequestError::Remote(body)) if body.code == ErrorCode::Stopped => {
                self.note_stopped();
                Err(PrepareFail::Stopped(remote_text(&body)))
            }
            Err(e) => Err(PrepareFail::Text(request_error_text(e, Method::ActPrepare))),
        }
    }

    fn note_stopped(&self) {
        let was = std::mem::replace(&mut lock(&self.act.state).stopped, true);
        if !was {
            self.end_all_grants();
            self.approvals().withdraw_all();
        }
    }

    fn cancel(&self, prepare_id: &str) {
        let _ = self.ask_raw(
            Method::ActCancel,
            json!({"prepare_id": prepare_id}),
            DEFAULT_DEADLINE_MS,
        );
    }

    // ---- the decision ----

    /// Classifies a prepared action (spec 7.6).
    fn judge(&self, plan: &Plan, tab: &TabInfo, prepared: &Prepared, session: i64) -> Evaluation {
        let kind = plan_kind(plan, prepared);
        let page_text = format!(
            "{} {} {} {}",
            prepared.title,
            prepared.element.name,
            prepared.element.text,
            prepared.element.nearby_text
        );
        let heading = prepared
            .form
            .conversation_title
            .clone()
            .unwrap_or_else(|| prepared.title.clone());
        let mut action = TrustAction::new(&plan.tool, &plan.args, &prepared.url);
        action.kind = kind;
        action.element = prepared.element.clone();
        action.form = prepared.form.clone();
        action.page_title = &prepared.title;
        action.page_heading = &heading;
        action.page_vi = looks_vietnamese(&page_text);
        action.typed = plan.typed.as_deref();
        // Hovering never asks (7.1); what the extension saw is still sent back as approved.
        action.ext = if plan.hover {
            BTreeSet::new()
        } else {
            prepared.ext.clone()
        };
        let _ = tab;
        self.with_trust(session, |state, config| {
            state.evaluate(
                &Config {
                    level: config.level,
                    rules: &config.rules,
                },
                &action,
            )
        })
    }

    /// The facts of a card for a prepared action.
    fn facts(&self, plan: &Plan, prepared: &Prepared, ev: &Evaluation) -> CardFacts {
        let config = self.act_config();
        let label = prepared.label();
        let desc = match &plan.action {
            Action::Click { .. } => ActionDesc::Click { label },
            Action::Type { .. } | Action::Fill { .. } => ActionDesc::Type { label },
            Action::Select { .. } => ActionDesc::Select { label },
            Action::Press { key, .. } => ActionDesc::Press { key: key.clone() },
            Action::Hover { .. } => ActionDesc::Hover,
            Action::Drag { .. } => ActionDesc::Drag,
            Action::Scroll { .. } => ActionDesc::Scroll,
            Action::Upload { names, .. } => ActionDesc::Upload {
                names: names.clone(),
            },
        };
        let mut facts = CardFacts::new(
            &config.mono_label,
            &ev.site,
            desc,
            ev.categories.iter().copied().collect(),
        );
        facts.url = prepared.url.clone();
        facts.in_dialog = prepared.element.in_dialog;
        facts.dialog_text = prepared.element.dialog_text.clone();
        facts.recipients = prepared.form.recipients.clone();
        facts.amounts = prepared.element.amounts.clone();
        facts.field = plan_field(plan, prepared);
        facts.site_kind = site_category(&prepared.url);
        facts.rule = ev.reason.strip_prefix("soul rule: ").map(str::to_owned);
        facts.purpose = s(&plan.args, "purpose").map(str::to_owned);
        facts
    }

    /// Saves the prepare's picture next to the Mono's other browser files.
    fn save_card_screenshot(
        &self,
        shot: &(String, String, u32, u32),
        id_hint: &str,
    ) -> Option<CardScreenshot> {
        let dir = lock(&self.output_dir).clone()?;
        let bytes = B64.decode(&shot.0).ok()?;
        if bytes.len() as u64 > SCREENSHOT_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return None;
        }
        output::prepare(&dir).ok()?;
        let name = format!("approval-{id_hint}.png");
        let path = dir.join(&name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&path).ok()?;
        std::io::Write::write_all(&mut file, &bytes).ok()?;
        Some(CardScreenshot {
            file_name: name,
            path,
            width: shot.2,
            height: shot.3,
            method: shot.1.clone(),
        })
    }

    // ---- the whole action ----

    fn perform(&self, plan: Plan) -> Result<Value, String> {
        let ctx = self.run_context();
        let tab = self.act_tab(&plan.args)?;
        let level = self.act_config().level;
        if level == BrowserLevel::ReadOnly
            && !super::super::super::trust::read_only_ok(&plan.tool, &plan.args)
        {
            let reason = "read only".to_owned();
            self.audit_blocked(&ctx, &plan, &tab, &tab.url, reason);
            return Err(format!(
                "{} is not available: this Mono can only read the browser. Tell the user what to do, or that the browser level must be raised.",
                plan.tool
            ));
        }

        let mut prepared = self
            .prepare(tab.tab, &plan.action, false)
            .map_err(|f| self.prepare_failed(&ctx, &plan, &tab, f))?;
        let mut ev = self.judge(&plan, &tab, &prepared, ctx.session_id);
        // A card with a picture: the prepare did not carry one, so prepare again asking for it
        // (the first prepare is cancelled; the answer replaces it, signals included).
        if matches!(ev.decision, Decision::Ask(_)) && prepared.screenshot.is_none() {
            // An upload's bytes go with its prepare's cancel, and the second prepare needs them.
            if !matches!(plan.action, Action::Upload { .. }) {
                self.cancel(&prepared.id);
            }
            prepared = self
                .prepare(tab.tab, &plan.action, true)
                .map_err(|f| self.prepare_failed(&ctx, &plan, &tab, f))?;
            ev = self.judge(&plan, &tab, &prepared, ctx.session_id);
        }

        let mut approval_id: Option<String> = None;
        let mut screenshot_name: Option<String> = None;
        let mut decision = AuditDecision::Auto;
        let mut asked_answer: Option<Answer> = None;
        match ev.decision.clone() {
            Decision::Deny(reason) => {
                self.cancel(&prepared.id);
                let audit = if reason.contains("password") {
                    AuditDecision::Invariant
                } else {
                    AuditDecision::Blocked
                };
                self.write_decision(
                    &ctx, &plan, &tab, &prepared, &ev, audit, &reason, None, None, None,
                );
                return Err(format!("Not allowed: {reason}."));
            }
            Decision::Allow | Decision::AllowByGrant(_) => {
                if matches!(ev.decision, Decision::AllowByGrant(_)) {
                    approval_id = Some(format!("{:032x}", rand::random::<u128>()));
                }
            }
            Decision::Ask(_) => {
                let card_id = format!("{:032x}", rand::random::<u128>());
                let shot = prepared
                    .screenshot
                    .as_ref()
                    .and_then(|s| self.save_card_screenshot(s, &card_id[..16]));
                screenshot_name = shot.as_ref().map(|s| s.file_name.clone());
                let facts = self.facts(&plan, &prepared, &ev);
                let categories: Vec<Category> = ev.categories.iter().copied().collect();
                let request = Request {
                    mono_id: self.mono_id.clone(),
                    session_id: ctx.session_id,
                    tool: plan.tool.clone(),
                    tab: Some(tab.tab),
                    facts,
                    screenshot: shot,
                    offer_conversation: offers_conversation(&categories, ev.tier == Tier::Always),
                };
                let config = self.act_config();
                let prepare_id = prepared.id.clone();
                let (id, outcome) =
                    config
                        .queue
                        .ask(request, config.approval_timeout, KEEP_ALIVE, || {
                            self.keep_alive(&prepare_id)
                        });
                approval_id = Some(id);
                match outcome {
                    Outcome::Answered(UserAnswer::AllowOnce) => {
                        asked_answer = Some(Answer::AllowedOnce);
                        decision = AuditDecision::AskedAllowedOnce;
                    }
                    Outcome::Answered(UserAnswer::AllowConversation) => {
                        asked_answer = Some(Answer::AllowedTask);
                        decision = AuditDecision::AskedAllowedTask;
                    }
                    Outcome::Answered(UserAnswer::Deny) => {
                        self.cancel(&prepared.id);
                        self.write_decision(
                            &ctx,
                            &plan,
                            &tab,
                            &prepared,
                            &ev,
                            AuditDecision::AskedDenied,
                            "the user did not allow it",
                            approval_id,
                            screenshot_name,
                            None,
                        );
                        return Err(denied_by_user());
                    }
                    Outcome::TimedOut => {
                        self.cancel(&prepared.id);
                        self.write_decision(
                            &ctx,
                            &plan,
                            &tab,
                            &prepared,
                            &ev,
                            AuditDecision::AskedTimeout,
                            "the user did not answer in time",
                            approval_id,
                            screenshot_name,
                            None,
                        );
                        return Err("Denied: the user did not answer in time.".into());
                    }
                    Outcome::Withdrawn => {
                        self.cancel(&prepared.id);
                        self.write_decision(&ctx, &plan, &tab, &prepared, &ev, AuditDecision::AskedDenied, "nobody could answer (the run ended, was stopped, or has no one to ask)", approval_id, screenshot_name, None);
                        return Err("Denied: nobody could answer the question (the run ended or was stopped, or no one is there to ask).".into());
                    }
                }
            }
        }

        // Remember the answer; a conversation grant that could not be stored is only "once".
        if let Some(answer) = asked_answer {
            let granted = self.with_trust(ctx.session_id, |state, _| state.commit(&ev, answer));
            if answer == Answer::AllowedTask && granted.is_empty() {
                decision = AuditDecision::AskedAllowedOnce;
            }
        }

        // The extension cross-checks its own categories against what was approved. Hovering
        // sends the ones it saw: it never asks.
        // Hovering never asks, so it never claims an approval: the extension refuses a hover on
        // an element it finds risky (E_NEEDS_APPROVAL).
        let approved: BTreeSet<Category> = ev.categories.clone();
        let approval = (!approved.is_empty()).then(|| {
            json!({
                "approval_id": approval_id.clone().unwrap_or_else(|| format!("{:032x}", rand::random::<u128>())),
                "categories": approved.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
                "digest": prepared.digest,
            })
        });
        let committed = self
            .commit(&prepared, approval.as_ref(), None)
            .and_then(|first| {
                if first["effects"]["hint"].as_str().is_some()
                    && needs_trusted_retry(&first["effects"])
                {
                    // Nothing changed with the plain input: one retry through the debugger (6.4).
                    self.commit(&prepared, approval.as_ref(), Some("trusted"))
                        .or(Ok(first))
                } else {
                    Ok(first)
                }
            });
        let reason = ev.reason.clone();
        match committed {
            Ok(done) => {
                let method = done["effects"]["method"].as_str().map(str::to_owned);
                let after_url = str_field(&done, "url");
                self.write_decision(
                    &ctx,
                    &plan,
                    &tab,
                    &prepared,
                    &ev,
                    decision,
                    &reason,
                    approval_id,
                    screenshot_name,
                    method,
                );
                // A link that led to another site: reading there is free, acting asks (7.4).
                if registrable_domain_of(&after_url) != registrable_domain_of(&prepared.url) {
                    self.with_trust(ctx.session_id, |state, _| {
                        state.note_link_followed(&after_url)
                    });
                }
                // The page may have moved somewhere this Mono may not read: show nothing of it.
                self.check_reply_url(&after_url)?;
                Ok(text_result(act_result_text(&plan, &prepared, &done)))
            }
            Err(failure) => {
                let (text, code) = failure;
                if code == Some(ErrorCode::Stopped) {
                    self.note_stopped();
                }
                self.write_decision(
                    &ctx,
                    &plan,
                    &tab,
                    &prepared,
                    &ev,
                    decision,
                    &format!(
                        "{reason}; commit failed: {}",
                        code.map_or("error", |c| c.as_str())
                    ),
                    approval_id,
                    screenshot_name,
                    None,
                );
                Err(text)
            }
        }
    }

    /// `act.commit`; `Err` carries the agent's text and the extension's code.
    fn commit(
        &self,
        prepared: &Prepared,
        approval: Option<&Value>,
        input: Option<&str>,
    ) -> Result<Value, (String, Option<ErrorCode>)> {
        let mut params = json!({"prepare_id": prepared.id});
        if let Some(approval) = approval {
            params["approval"] = approval.clone();
        }
        if let Some(input) = input {
            params["input"] = json!(input);
        }
        let result = self.ask_raw(Method::ActCommit, params, DEFAULT_DEADLINE_MS);
        // The user pressed Stop on the tab while the action was in flight: that is the answer,
        // whatever the extension said about the cancelled prepare.
        let result = match (result, self.stopped_tab_error(prepared.tab)) {
            (Err(_), Some(stopped)) => Err(stopped),
            (other, _) => other,
        };
        result.map_err(|e| {
            let code = match &e {
                RequestError::Remote(body) => Some(body.code),
                _ => None,
            };
            (request_error_text(e, Method::ActCommit), code)
        })
    }

    fn keep_alive(&self, prepare_id: &str) {
        let until = crate::time::now_ms() + EXTEND_MS;
        let _ = self.ask_raw(
            Method::ActExtend,
            json!({"prepare_id": prepare_id, "until_ms": until}),
            DEFAULT_DEADLINE_MS,
        );
    }

    fn prepare_failed(
        &self,
        ctx: &RunContext,
        plan: &Plan,
        tab: &TabInfo,
        fail: PrepareFail,
    ) -> String {
        match fail {
            PrepareFail::Text(text) => text,
            PrepareFail::Stopped(text) => {
                self.audit_row_simple(
                    ctx,
                    plan,
                    tab,
                    AuditDecision::Stopped,
                    "zptah Browser is stopped",
                );
                text
            }
            PrepareFail::Password(text) => {
                self.audit_row_simple(
                    ctx,
                    plan,
                    tab,
                    AuditDecision::Invariant,
                    "typing into a password field is never allowed",
                );
                let site = registrable_domain_of(&tab.url).unwrap_or_default();
                self.approvals().post_notice(
                    super::super::super::trust::approval::NoticeKind::PasswordNeeded,
                    Some(tab.tab),
                    &self.act_config().mono_label,
                    &site,
                );
                text
            }
        }
    }

    fn audit_blocked(
        &self,
        ctx: &RunContext,
        plan: &Plan,
        tab: &TabInfo,
        url: &str,
        reason: String,
    ) {
        let _ = url;
        self.audit_row_simple(ctx, plan, tab, AuditDecision::Blocked, &reason);
    }

    fn audit_row_simple(
        &self,
        ctx: &RunContext,
        plan: &Plan,
        tab: &TabInfo,
        decision: AuditDecision,
        reason: &str,
    ) {
        self.write_row(
            ctx.session_id,
            DecisionAudit {
                ts_ms: crate::time::now_ms(),
                session_id: ctx.session_id,
                tool: plan.tool.clone(),
                url: audit_url(&tab.url),
                summary: plan.summary.clone(),
                tab_id: i64::try_from(tab.tab).ok(),
                tab_owner: tab.owner.clone(),
                tier: super::super::super::trust::tier_of(&plan.tool, &plan.args)
                    .letter()
                    .into(),
                categories: Vec::new(),
                decision,
                reason: reason.to_owned(),
                approval_id: None,
                screenshot: None,
                actor: "mono".into(),
                input: None,
                profile: None,
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn write_decision(
        &self,
        ctx: &RunContext,
        plan: &Plan,
        tab: &TabInfo,
        prepared: &Prepared,
        ev: &Evaluation,
        decision: AuditDecision,
        reason: &str,
        approval_id: Option<String>,
        screenshot: Option<String>,
        input: Option<String>,
    ) {
        let label = prepared.label();
        let summary = if plan.types {
            plan.summary.replace("field", &copy::clean(&label, 60))
        } else if label.is_empty() {
            plan.summary.clone()
        } else {
            format!("{} \"{}\"", plan.summary, copy::clean(&label, 80))
        };
        self.write_row(
            ctx.session_id,
            DecisionAudit {
                ts_ms: crate::time::now_ms(),
                session_id: ctx.session_id,
                tool: plan.tool.clone(),
                url: audit_url(&prepared.url),
                summary,
                tab_id: i64::try_from(tab.tab).ok(),
                tab_owner: tab.owner.clone(),
                tier: ev.tier.letter().into(),
                categories: ev
                    .categories
                    .iter()
                    .map(|c| c.as_str().to_owned())
                    .collect(),
                decision,
                reason: reason.to_owned(),
                approval_id,
                screenshot,
                actor: "mono".into(),
                input,
                profile: None,
            },
        );
    }

    // ---- navigation ----

    fn navigate_tool(&self, name: &str, args: &Value) -> Result<Value, String> {
        let ctx = self.run_context();
        let tab = self.act_tab(args)?;
        let (method, params, target) = match name {
            "browser_navigate" => {
                let url = s(args, "url").ok_or("browser_navigate needs `url`.")?;
                let parsed = url::Url::parse(url)
                    .ok()
                    .filter(|u| matches!(u.scheme(), "http" | "https"))
                    .ok_or("Only http and https addresses can be opened.")?;
                self.origins()
                    .check(parsed.as_str())
                    .map_err(|r| refusal_text(r).to_owned())?;
                (
                    Method::NavGo,
                    json!({"tab": tab.tab, "url": parsed.as_str()}),
                    Some(parsed.to_string()),
                )
            }
            "browser_navigate_back" => (
                Method::NavHistory,
                json!({"tab": tab.tab, "dir": "back"}),
                None,
            ),
            _ => (
                Method::NavHistory,
                json!({"tab": tab.tab, "dir": "forward"}),
                None,
            ),
        };
        let mut action = TrustAction::new(name, args, &tab.url);
        action.target_url = target.as_deref();
        action.page_title = &tab.title;
        let ev = self.with_trust(ctx.session_id, |state, config| {
            state.evaluate(
                &Config {
                    level: config.level,
                    rules: &config.rules,
                },
                &action,
            )
        });
        let summary = format!(
            "{} {}",
            name.trim_start_matches("browser_"),
            audit_url(target.as_deref().unwrap_or(""))
        );
        let (decision, approval_id) = match ev.decision.clone() {
            Decision::Deny(reason) => {
                self.write_nav_row(
                    &ctx,
                    name,
                    &tab,
                    &ev,
                    &summary,
                    AuditDecision::Blocked,
                    &reason,
                    None,
                );
                return Err(format!("Not allowed: {reason}."));
            }
            Decision::Allow => (AuditDecision::Auto, None),
            Decision::AllowByGrant(_) => (AuditDecision::Auto, None),
            Decision::Ask(_) => {
                let mut facts = CardFacts::new(
                    &self.act_config().mono_label,
                    &ev.site,
                    ActionDesc::Navigate,
                    ev.categories.iter().copied().collect(),
                );
                facts.url = target.clone().unwrap_or_default();
                facts.site_kind = target.as_deref().and_then(site_category);
                facts.rule = ev.reason.strip_prefix("soul rule: ").map(str::to_owned);
                facts.purpose = s(args, "purpose").map(str::to_owned);
                match self.ask_card(&ctx, name, Some(tab.tab), facts, &ev, None, None) {
                    Ok((answer, id)) => {
                        let granted =
                            self.with_trust(ctx.session_id, |state, _| state.commit(&ev, answer));
                        let decision = if answer == Answer::AllowedTask && !granted.is_empty() {
                            AuditDecision::AskedAllowedTask
                        } else {
                            AuditDecision::AskedAllowedOnce
                        };
                        (decision, Some(id))
                    }
                    Err((text, audit, id)) => {
                        self.write_nav_row(&ctx, name, &tab, &ev, &summary, audit, &text, Some(id));
                        return Err(text);
                    }
                }
            }
        };
        let result = self
            .ask_raw(method, params, DEFAULT_DEADLINE_MS)
            .map_err(|e| {
                if matches!(&e, RequestError::Remote(b) if b.code == ErrorCode::Stopped) {
                    self.note_stopped();
                }
                request_error_text(e, method)
            });
        let result = match result {
            Ok(r) => r,
            Err(text) => {
                self.write_nav_row(
                    &ctx,
                    name,
                    &tab,
                    &ev,
                    &summary,
                    decision,
                    &format!("{}; failed", ev.reason),
                    approval_id,
                );
                return Err(text);
            }
        };
        let (url, title) = (str_field(&result, "url"), str_field(&result, "title"));
        self.write_nav_row(
            &ctx,
            name,
            &tab,
            &ev,
            &summary,
            decision,
            &ev.reason,
            approval_id,
        );
        self.with_trust(ctx.session_id, |state, _| state.note_link_followed(&url));
        self.check_reply_url(&url)?;
        let mut text = page_header(&url, &title);
        text.push_str("### Result\n- Navigated. Take a browser_snapshot to see the page.\n");
        Ok(text_result(text))
    }

    #[allow(clippy::too_many_arguments)]
    fn write_nav_row(
        &self,
        ctx: &RunContext,
        tool: &str,
        tab: &TabInfo,
        ev: &Evaluation,
        summary: &str,
        decision: AuditDecision,
        reason: &str,
        approval_id: Option<String>,
    ) {
        self.write_row(
            ctx.session_id,
            DecisionAudit {
                ts_ms: crate::time::now_ms(),
                session_id: ctx.session_id,
                tool: tool.to_owned(),
                url: audit_url(&tab.url),
                summary: summary.to_owned(),
                tab_id: i64::try_from(tab.tab).ok(),
                tab_owner: tab.owner.clone(),
                tier: ev.tier.letter().into(),
                categories: ev
                    .categories
                    .iter()
                    .map(|c| c.as_str().to_owned())
                    .collect(),
                decision,
                reason: reason.to_owned(),
                approval_id,
                screenshot: None,
                actor: "mono".into(),
                input: None,
                profile: None,
            },
        );
    }

    /// Opens a card without a prepared action and waits. `Ok`: the answer to apply and the card
    /// id. `Err`: the agent's text, the audit decision and the card id.
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn ask_card(
        &self,
        ctx: &RunContext,
        tool: &str,
        tab: Option<u64>,
        facts: CardFacts,
        ev: &Evaluation,
        screenshot: Option<CardScreenshot>,
        keep_alive: Option<&str>,
    ) -> Result<(Answer, String), (String, AuditDecision, String)> {
        let categories: Vec<Category> = ev.categories.iter().copied().collect();
        let request = Request {
            mono_id: self.mono_id.clone(),
            session_id: ctx.session_id,
            tool: tool.to_owned(),
            tab,
            facts,
            screenshot,
            offer_conversation: offers_conversation(&categories, ev.tier == Tier::Always),
        };
        let config = self.act_config();
        let (id, outcome) = config
            .queue
            .ask(request, config.approval_timeout, KEEP_ALIVE, || {
                if let Some(prepare_id) = keep_alive {
                    self.keep_alive(prepare_id);
                }
            });
        match outcome {
            Outcome::Answered(UserAnswer::AllowOnce) => Ok((Answer::AllowedOnce, id)),
            Outcome::Answered(UserAnswer::AllowConversation) => Ok((Answer::AllowedTask, id)),
            Outcome::Answered(UserAnswer::Deny) => Err((denied_by_user(), AuditDecision::AskedDenied, id)),
            Outcome::TimedOut => Err((
                "Denied: the user did not answer in time.".into(),
                AuditDecision::AskedTimeout,
                id,
            )),
            Outcome::Withdrawn => Err((
                "Denied: nobody could answer the question (the run ended or was stopped, or no one is there to ask).".into(),
                AuditDecision::AskedDenied,
                id,
            )),
        }
    }

    // ---- the agent's own approval request (tier K) ----

    fn request_approval_tool(&self, args: &Value) -> Result<Value, String> {
        let ctx = self.run_context();
        let action_text = s(args, "action").ok_or("Describe the action in `action`.")?;
        s(args, "category")
            .ok_or("Give a category: payment, send_message, post_publish or other.")?;
        let selected = {
            let state = self.state();
            state.selected.and_then(|id| state.known.get(&id).cloned())
        };
        let url = s(args, "url")
            .map(str::to_owned)
            .or_else(|| selected.as_ref().map(|t| t.url.clone()))
            .unwrap_or_default();
        let mut action = TrustAction::new(super::super::APPROVAL_TOOL, args, &url);
        action.page_title = selected.as_ref().map_or("", |t| t.title.as_str());
        let ev = self.with_trust(ctx.session_id, |state, config| {
            state.evaluate(
                &Config {
                    level: config.level,
                    rules: &config.rules,
                },
                &action,
            )
        });
        if let Decision::Deny(reason) = &ev.decision {
            return Err(format!("Not allowed: {reason}."));
        }
        let mut facts = CardFacts::new(
            &self.act_config().mono_label,
            &ev.site,
            ActionDesc::Other,
            ev.categories.iter().copied().collect(),
        );
        facts.url = url.clone();
        facts.agent_action = Some(action_text.to_owned());
        facts.agent_details = s(args, "details").map(str::to_owned);
        facts.site_kind = site_category(&url);
        let tab = selected.as_ref().map(|t| t.tab);
        let row_tab = selected.unwrap_or_else(|| TabInfo {
            tab: 0,
            window: 0,
            url: url.clone(),
            title: String::new(),
            owner: None,
            group_title: None,
            active_in_window: false,
            audible: false,
            loading: false,
        });
        let summary = format!("approval request: {}", copy::clean(action_text, 120));
        match self.ask_card(
            &ctx,
            super::super::APPROVAL_TOOL,
            tab,
            facts,
            &ev,
            None,
            None,
        ) {
            Ok((answer, id)) => {
                self.with_trust(ctx.session_id, |state, _| state.commit(&ev, answer));
                let decision = if answer == Answer::AllowedTask {
                    AuditDecision::AskedAllowedTask
                } else {
                    AuditDecision::AskedAllowedOnce
                };
                self.write_nav_row(
                    &ctx,
                    super::super::APPROVAL_TOOL,
                    &row_tab,
                    &ev,
                    &summary,
                    decision,
                    &ev.reason,
                    Some(id),
                );
                Ok(text_result(
                    "Approved by the user. You may do this action now.".into(),
                ))
            }
            Err((text, audit, id)) => {
                self.write_nav_row(
                    &ctx,
                    super::super::APPROVAL_TOOL,
                    &row_tab,
                    &ev,
                    &summary,
                    audit,
                    &text,
                    Some(id),
                );
                Err(text)
            }
        }
    }

    // ---- dialogs ----

    fn dialog_tool(&self, args: &Value) -> Result<Value, String> {
        let ctx = self.run_context();
        let tab = self.act_tab(args)?;
        let accept = args["accept"]
            .as_bool()
            .ok_or("browser_handle_dialog needs `accept` (true or false).")?;
        let message = lock(&self.act.state)
            .dialogs
            .get(&tab.tab)
            .cloned()
            .unwrap_or_default();
        let mut action = TrustAction::new("browser_handle_dialog", args, &tab.url);
        action.kind = accept.then_some(ActionKind::Click);
        if accept && message.is_empty() {
            // The dialog's text is unknown (no debugger event): accepting it blind asks.
            action.declared.insert(Category::Destructive);
        }
        action.element = ElementSignals {
            role: "button".into(),
            name: message.clone(),
            text: message.clone(),
            tag: "button".into(),
            in_dialog: true,
            dialog_text: Some(message.clone()),
            ..Default::default()
        };
        action.page_title = &tab.title;
        let ev = self.with_trust(ctx.session_id, |state, config| {
            state.evaluate(
                &Config {
                    level: config.level,
                    rules: &config.rules,
                },
                &action,
            )
        });
        let summary = format!("dialog {}", if accept { "accept" } else { "dismiss" });
        let mut decision = AuditDecision::Auto;
        let mut approval_id = None;
        match ev.decision.clone() {
            Decision::Deny(reason) => {
                self.write_nav_row(
                    &ctx,
                    "browser_handle_dialog",
                    &tab,
                    &ev,
                    &summary,
                    AuditDecision::Blocked,
                    &reason,
                    None,
                );
                return Err(format!("Not allowed: {reason}."));
            }
            Decision::Ask(_) => {
                let mut facts = CardFacts::new(
                    &self.act_config().mono_label,
                    &ev.site,
                    ActionDesc::Dialog { accept },
                    ev.categories.iter().copied().collect(),
                );
                facts.in_dialog = true;
                facts.dialog_text = Some(message.clone());
                match self.ask_card(
                    &ctx,
                    "browser_handle_dialog",
                    Some(tab.tab),
                    facts,
                    &ev,
                    None,
                    None,
                ) {
                    Ok((answer, id)) => {
                        self.with_trust(ctx.session_id, |state, _| state.commit(&ev, answer));
                        decision = if answer == Answer::AllowedTask {
                            AuditDecision::AskedAllowedTask
                        } else {
                            AuditDecision::AskedAllowedOnce
                        };
                        approval_id = Some(id);
                    }
                    Err((text, audit, id)) => {
                        self.write_nav_row(
                            &ctx,
                            "browser_handle_dialog",
                            &tab,
                            &ev,
                            &summary,
                            audit,
                            &text,
                            Some(id),
                        );
                        return Err(text);
                    }
                }
            }
            _ => {}
        }
        let mut params = json!({"tab": tab.tab, "accept": accept});
        if let Some(text) = args["promptText"].as_str() {
            params["prompt_text"] = json!(text.chars().take(65_000).collect::<String>());
        }
        let outcome = self.ask(Method::DialogRespond, params, DEFAULT_DEADLINE_MS);
        self.write_nav_row(
            &ctx,
            "browser_handle_dialog",
            &tab,
            &ev,
            &summary,
            decision,
            &ev.reason,
            approval_id,
        );
        outcome?;
        lock(&self.act.state).dialogs.remove(&tab.tab);
        Ok(text_result(format!(
            "### Result\n- The dialog was {}.\n",
            if accept { "accepted" } else { "dismissed" }
        )))
    }

    // ---- uploads ----

    fn upload_tool(&self, args: &Value) -> Result<Value, String> {
        let _tab = self.act_tab(args)?;
        if self.act_config().level == BrowserLevel::ReadOnly {
            return Err(
                "browser_file_upload is not available: this Mono can only read the browser.".into(),
            );
        }
        let home = upload_home().ok_or("Uploads are not available: the home folder is unknown, so credential files cannot be ruled out.")?;
        let target = ref_arg(args)
            .map_err(|_| "browser_file_upload needs `ref`: the file input (or the button that opens it) from a browser_snapshot, and `paths`: the files to upload.".to_owned())?;
        let paths: Vec<PathBuf> = args["paths"]
            .as_array()
            .filter(|p| !p.is_empty() && p.len() <= 20)
            .ok_or("browser_file_upload needs `paths`: one to twenty files.")?
            .iter()
            .filter_map(|p| p.as_str().map(PathBuf::from))
            .collect();
        let files = read_upload_files(&paths, Some(&home))?;
        let names: Vec<String> = files.iter().map(|(name, _)| name.clone()).collect();
        let upload_id = format!("{:032x}", rand::random::<u128>());
        // The extension holds the bytes (in memory, for 10 minutes at most) until the commit: the
        // prepare of an upload needs them sent first. Nothing reaches a page before the answer.
        let mut stream: Vec<u8> = Vec::new();
        for (_, bytes) in &files {
            stream.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            stream.extend_from_slice(bytes);
        }
        let chunks: Vec<&[u8]> = stream.chunks(UPLOAD_CHUNK).collect();
        for (seq, chunk) in chunks.iter().enumerate() {
            let last = seq + 1 == chunks.len();
            self.ask(
                Method::UploadPut,
                json!({"upload_id": upload_id, "seq": seq, "bytes_b64": B64.encode(chunk), "last": last}),
                DEFAULT_DEADLINE_MS,
            )?;
        }
        let plan = build_plan_for_upload(args, target, upload_id, names);
        self.perform(plan)
    }
}

fn build_plan_for_upload(
    args: &Value,
    target: String,
    upload_id: String,
    names: Vec<String>,
) -> Plan {
    Plan {
        tool: "browser_file_upload".into(),
        args: args.clone(),
        summary: format!("upload {} file(s)", names.len()),
        action: Action::Upload {
            target,
            upload_id,
            names,
        },
        kind: Some(ActionKind::Upload),
        typed: None,
        types: false,
        is_fill: false,
        hover: false,
    }
}

/// Reads the files of an upload: regular files only, nothing under the places that hold
/// credentials, at most 32 MiB in all.
pub(in crate::mono::browser::gate) fn read_upload_files(
    paths: &[PathBuf],
    home: Option<&Path>,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut total = 0u64;
    let mut out = Vec::new();
    for path in paths {
        if !path.is_absolute() {
            return Err(format!(
                "`paths` must be absolute file paths (got `{}`).",
                path.display()
            ));
        }
        let canonical = std::fs::canonicalize(path).map_err(|_| {
            format!(
                "The file `{}` does not exist or cannot be read.",
                path.display()
            )
        })?;
        if upload_path_refused(&canonical, home) {
            return Err(format!(
                "`{}` holds credentials or the app's own data, so it is never uploaded.",
                path.display()
            ));
        }
        let meta = std::fs::metadata(&canonical)
            .map_err(|_| format!("The file `{}` cannot be read.", path.display()))?;
        if !meta.is_file() {
            return Err(format!("`{}` is not a regular file.", path.display()));
        }
        total += meta.len();
        if total > MAX_UPLOAD_BYTES {
            return Err(
                "The files together are larger than 32 MiB, the most one upload can carry.".into(),
            );
        }
        let bytes = std::fs::read(&canonical)
            .map_err(|_| format!("The file `{}` cannot be read.", path.display()))?;
        let name = canonical
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_owned();
        out.push((name, bytes));
    }
    Ok(out)
}

/// The user's home folder, where the credential places are.
fn upload_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Credential files and the stores of other programs (the same lists the remote read refuses).
fn upload_path_refused(canonical: &Path, home: Option<&Path>) -> bool {
    use crate::credential_guard::{
        all_app_data_dirs, INDEXED_HOME_PATHS, MORE_HOME_CREDENTIALS, SECRET_STORE_DIRS,
    };
    let Some(home) = home else {
        return false;
    };
    let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    INDEXED_HOME_PATHS
        .iter()
        .chain(MORE_HOME_CREDENTIALS.iter())
        .chain(SECRET_STORE_DIRS.iter())
        .map(|tail| home.join(tail))
        .chain(all_app_data_dirs(&home))
        .any(|dir| canonical.starts_with(&dir))
}

// ---- small helpers ----

enum PrepareFail {
    Text(String),
    Stopped(String),
    Password(String),
}

/// The kind of the action as the classifier takes it, knowing what the prepare found.
fn plan_kind(plan: &Plan, prepared: &Prepared) -> Option<ActionKind> {
    if plan.kind.is_some() || !plan.types {
        return plan.kind;
    }
    Some(ActionKind::Type(field_kind_of(plan, prepared)))
}

fn field_kind_of(plan: &Plan, prepared: &Prepared) -> FieldKind {
    let el = &prepared.element;
    let own = classify_field(&FieldAttrs {
        input_type: el.input_type.clone().unwrap_or_default(),
        autocomplete: el.autocomplete.clone().unwrap_or_default(),
        name: el.input_name.clone().unwrap_or_default(),
        id: el.input_id.clone().unwrap_or_default(),
        label: el.label.clone().unwrap_or_else(|| el.name.clone()),
        placeholder: el.placeholder.clone().unwrap_or_default(),
        inputmode: String::new(),
        maxlength: None,
        nearby_text: el.nearby_text.clone(),
    });
    if own != FieldKind::Other || !plan.is_fill {
        return own;
    }
    // A form fill: the most sensitive field the form holds decides (a password is the
    // extension's to refuse, so it is not taken from the form).
    prepared
        .form
        .fields
        .iter()
        .map(|f| f.kind)
        .find(|k| {
            matches!(
                k,
                FieldKind::Otp
                    | FieldKind::CardNumber
                    | FieldKind::CardCvc
                    | FieldKind::CardExpiry
                    | FieldKind::NationalId
                    | FieldKind::BankAccount
            )
        })
        .unwrap_or(FieldKind::Other)
}

fn plan_field(plan: &Plan, prepared: &Prepared) -> Option<FieldKind> {
    plan.types.then(|| field_kind_of(plan, prepared))
}

fn needs_trusted_retry(effects: &Value) -> bool {
    effects["method"] == "content"
        && effects["dom_mutations"].as_u64() == Some(0)
        && effects["url_changed"] != true
        && effects["focus_changed"] != true
        && effects["submitted"] != true
}

/// Whether a text is Vietnamese (a letter only Vietnamese uses): the lexicon reads some words
/// differently on a Vietnamese page.
pub(crate) fn looks_vietnamese(text: &str) -> bool {
    const ONLY_VI: &str = "ăâđêôơưĂÂĐÊÔƠƯạảãấầẩẫậắằẳẵặẹẻẽếềểễệỉịọỏốồổỗộớờởỡợụủứừửữựỳỵỷỹẠẢÃẤẦẨẪẬẮẰẲẴẶẸẺẼẾỀỂỄỆỈỊỌỎỐỒỔỖỘỚỜỞỠỢỤỦỨỪỬỮỰỲỴỶỸ";
    text.chars().any(|c| ONLY_VI.contains(c))
}

/// What the agent reads after an action ran.
fn act_result_text(plan: &Plan, prepared: &Prepared, done: &Value) -> String {
    let url = {
        let after = str_field(done, "url");
        if after.is_empty() {
            prepared.url.clone()
        } else {
            after
        }
    };
    let title = {
        let after = str_field(done, "title");
        if after.is_empty() {
            prepared.title.clone()
        } else {
            after
        }
    };
    let effects = &done["effects"];
    let mut text = page_header(&url, &title);
    text.push_str("### Result\n");
    let label = prepared.label();
    if label.is_empty() {
        text.push_str(&format!(
            "- Ran {}.\n",
            plan.tool.trim_start_matches("browser_")
        ));
    } else {
        text.push_str(&format!(
            "- Ran {} on \"{}\".\n",
            plan.tool.trim_start_matches("browser_"),
            clean_line(&label).chars().take(80).collect::<String>()
        ));
    }
    text.push_str(&format!(
        "- Page changes seen: {} (address changed: {}).\n",
        effects["dom_mutations"].as_u64().unwrap_or(0),
        effects["url_changed"] == true
    ));
    if effects["submitted"] == true {
        text.push_str("- The form was submitted.\n");
    }
    if effects["value_matches"] == false {
        text.push_str(
            "- The field does not hold the value that was typed; check with a browser_snapshot.\n",
        );
    }
    if let Some(hint) = effects["hint"].as_str() {
        text.push_str(&format!("- Note: {}\n", clean_line(hint)));
    }
    text.push_str("Take a browser_snapshot to see the page now.\n");
    text
}
