//! The act tools of the Ardia upstream (design spec sections 4, 6 and 9): click, type, fill,
//! press, select, check, hover and upload, the user's own tab, and the Mono's own approval
//! request.
//!
//! Every act tool is the same sequence:
//!
//! 1. validate the arguments (no process runs for a malformed call);
//! 2. a **fresh read** of the page right before the action: the tab URL and title, the snapshot
//!    refs, and for the target its `type`, `autocomplete`, link `href` and button `formaction`,
//!    the page's `form` and the actions of its forms. The call fails with "the element changed,
//!    take a new snapshot" when the ref was never in a snapshot the Mono took, no longer
//!    resolves, has another role or name, or the tab is on another URL than at that snapshot;
//! 3. checks that no approval can override: the page and the destination (a link's `href`, a
//!    form's `action`) against the Mono's origin lists, a password or one-time code field, a
//!    character key whose focus is not known to be harmless, an upload outside the allowed places;
//! 4. the shared classifier ([`TrustState::evaluate`]) on the fresh read, and when it says ask,
//!    a card through the approval queue (a grant, a soul rule and the level work as for the
//!    extension backend);
//! 5. the command, then the URL after it, the origin check of the page it left the tab on
//!    ([`ArdiaUpstream::after`]) and one audit row.
//!
//! An act call is never repeated: when its outcome is unknown (Ardia went away after the command
//! was sent, a timeout) the Mono is told it may or may not have run.
use super::fresh::{self, Attrs, ElementInput, FormActions, FormRead, CHANGED};
use super::uploads;
use super::*;
use crate::mono::browser::ardia::runner::literal_value;
use crate::mono::browser::copy::{ActionDesc, CardFacts};
use crate::mono::browser::gate::extension::act::looks_vietnamese;
use crate::mono::browser::risk::decide::Decision;
use crate::mono::browser::risk::signals::{registrable_domain_of, site_category, ActionKind};
use crate::mono::browser::risk::Category;
use crate::mono::browser::trust::approval::{
    offers_conversation, NoticeKind, Outcome, Request, UserAnswer,
};
use crate::mono::browser::trust::audit::{typed_summary, AuditDecision, DecisionAudit};
use crate::mono::browser::trust::{
    Action as TrustAction, Answer, BrowserLevel, Config as TrustConfig, Evaluation, Tier,
};
use std::collections::BTreeSet;

/// How often a card's wait renews the holds on the conversation's tabs.
const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// The longest text one `browser_type` or `browser_fill` takes.
const MAX_TYPED: usize = 20_000;
/// The added elements an act reply lists (their refs become usable without a new snapshot).
const MAX_ADDED: usize = 20;
/// What the Mono is told for a ref it never got from a snapshot.
const NEVER_SEEN: &str = "That ref was not in any snapshot you took of this page, so it is not used. Take a new snapshot and use a ref from it (the element changed, take a new snapshot).";

/// The tool name the shared classifier knows (its tier table, the soul rules and the deny list
/// use the extension backend's names). The audit and the Mono keep the Ardia names.
fn classifier_name(tool: &str) -> &'static str {
    match tool {
        "browser_click" | "browser_check" => "browser_click",
        "browser_type" | "browser_fill" => "browser_type",
        "browser_press" => "browser_press_key",
        "browser_select" => "browser_select_option",
        "browser_hover" => "browser_hover",
        "browser_upload" => "browser_file_upload",
        _ => APPROVAL_TOOL,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Click,
    Check,
    Hover,
    Type,
    Fill,
    Select,
    Press,
    Upload,
}

/// How the call is run and shown, built from the tool and its arguments.
struct Plan {
    tool: String,
    op: Op,
    args: Value,
    /// `e12`: the ref the call acts on (none for a key).
    ref_: Option<String>,
    /// The command and its flags.
    cmd: Vec<&'static str>,
    /// The values after `--`, the ref first (`@e12`).
    values: Vec<String>,
    /// Every value the call types (looked at for a card number, never stored).
    typed: Option<String>,
    key: Option<String>,
    paths: Vec<String>,
    /// For the audit: what was done, never a typed value.
    summary: String,
}

fn build_plan(tool: &str, args: &Value) -> Result<Plan, String> {
    let ref_ = || -> Result<String, String> {
        let raw = args["ref"]
            .as_str()
            .or_else(|| args["target"].as_str())
            .ok_or("This needs `ref`: the [ref=eN] of an element from a browser_snapshot.")?;
        fresh::parse_ref(raw)
    };
    let mut plan = Plan {
        tool: tool.to_owned(),
        op: Op::Click,
        args: args.clone(),
        ref_: None,
        cmd: Vec::new(),
        values: Vec::new(),
        typed: None,
        key: None,
        paths: Vec::new(),
        summary: String::new(),
    };
    let with_ref = |plan: &mut Plan, r: String| {
        plan.values.push(format!("@{r}"));
        plan.ref_ = Some(r);
    };
    match tool {
        "browser_click" => {
            plan.op = Op::Click;
            plan.cmd = vec!["click"];
            with_ref(&mut plan, ref_()?);
            plan.summary = "click".into();
        }
        "browser_check" => {
            plan.op = Op::Check;
            let on = args["checked"].as_bool().unwrap_or(true);
            plan.cmd = vec![if on { "check" } else { "uncheck" }];
            with_ref(&mut plan, ref_()?);
            plan.summary = if on { "check" } else { "uncheck" }.into();
        }
        "browser_hover" => {
            plan.op = Op::Hover;
            plan.cmd = vec!["hover"];
            with_ref(&mut plan, ref_()?);
            plan.summary = "hover".into();
        }
        "browser_type" | "browser_fill" => {
            let fill = tool == "browser_fill";
            plan.op = if fill { Op::Fill } else { Op::Type };
            plan.cmd = vec![if fill { "fill" } else { "type" }];
            with_ref(&mut plan, ref_()?);
            let text = args["text"]
                .as_str()
                .or_else(|| args["value"].as_str())
                .ok_or_else(|| format!("{tool} needs `text`."))?;
            literal_value("text", text, fill, MAX_TYPED)?;
            plan.values.push(text.to_owned());
            plan.typed = Some(text.to_owned());
            plan.summary = typed_summary("field", text);
        }
        "browser_select" => {
            plan.op = Op::Select;
            plan.cmd = vec!["select"];
            with_ref(&mut plan, ref_()?);
            let value = args["value"]
                .as_str()
                .or_else(|| args["values"][0].as_str())
                .ok_or("browser_select needs `value`: the option to choose.")?;
            literal_value("value", value, false, 2000)?;
            plan.values.push(value.to_owned());
            plan.typed = Some(value.to_owned());
            plan.summary = "select an option (value masked)".into();
        }
        "browser_press" => {
            plan.op = Op::Press;
            plan.cmd = vec!["press"];
            let key = args["key"].as_str().ok_or("browser_press needs `key`.")?;
            literal_value("key", key, false, 40)?;
            plan.values.push(key.to_owned());
            plan.key = Some(key.to_owned());
            plan.summary = if fresh::is_printable_key(key) {
                "press a character key".into()
            } else {
                format!("press {}", key.chars().take(20).collect::<String>())
            };
        }
        "browser_upload" => {
            plan.op = Op::Upload;
            plan.cmd = vec!["upload"];
            with_ref(&mut plan, ref_()?);
            let paths: Vec<String> = args["paths"]
                .as_array()
                .ok_or("browser_upload needs `paths`: the files to attach, as absolute paths.")?
                .iter()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect();
            if paths.is_empty() || paths.len() > uploads::MAX_FILES {
                return Err(format!(
                    "browser_upload takes one to {} paths.",
                    uploads::MAX_FILES
                ));
            }
            plan.summary = format!("upload {} file(s)", paths.len());
            plan.paths = paths;
        }
        other => return Err(format!("{other} is not an act tool.")),
    }
    Ok(plan)
}

/// Everything the gate read from the page right before the action.
#[derive(Default)]
struct Read {
    tab: String,
    url: String,
    title: String,
    /// The element the action lands on: the target, or the focused element of a key.
    ref_: Option<String>,
    role: String,
    name: String,
    attrs: Attrs,
    form: FormRead,
    actions: FormActions,
    /// For a key: the focus is known (a snapshot line marks it).
    focus_known: bool,
}

fn is_stale_code(code: Option<&str>) -> bool {
    matches!(code, Some("stale_ref" | "vocab_superseded" | "unknown_ref"))
}

fn is_field_role(role: &str) -> bool {
    matches!(
        role,
        "textbox" | "searchbox" | "combobox" | "spinbutton" | "textarea"
    )
}

impl ArdiaUpstream {
    // ---- trust state, per conversation ----

    fn trust_map(&self) -> MutexGuard<'_, HashMap<i64, TrustState>> {
        self.trust.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on the conversation's trust state, creating it on first use. `None` when this
    /// Mono has no act settings.
    fn with_trust<R>(
        &self,
        session: i64,
        f: impl FnOnce(&mut TrustState, &ActConfig) -> R,
    ) -> Option<R> {
        let act = self.config().act?;
        let mut map = self.trust_map();
        let state = map.entry(session).or_insert_with(|| {
            TrustState::new(GrantBook::stored_or_memory(
                &act.store,
                &self.mono_id,
                session,
            ))
        });
        Some(f(state, &act))
    }

    pub(super) fn begin_run_trust(&self, session: i64, message: &str) {
        self.with_trust(session, |state, _| state.begin_run(message));
    }
    pub(super) fn note_read(&self, session: i64, url: &str, text: &str) {
        self.with_trust(session, |state, _| state.note_read(url, text));
    }
    pub(super) fn flag_injection(&self, session: i64) {
        self.with_trust(session, |state, _| state.flag_injection());
    }
    /// The conversation ended: its grants and what the gate remembered end, its cards are
    /// withdrawn.
    pub(super) fn end_trust(&self, conversation: i64) {
        let removed = self.trust_map().remove(&conversation);
        let act = self.config().act;
        match (removed, &act) {
            (Some(mut state), _) => state.end_conversation(),
            (None, Some(act)) => {
                GrantBook::stored_or_memory(&act.store, &self.mono_id, conversation)
                    .end_conversation()
            }
            _ => {}
        }
        if let Some(act) = act {
            act.queue.withdraw_session(conversation);
        }
    }
    /// The run is over: its open cards are withdrawn.
    pub(super) fn end_run_cards(&self, session: i64) {
        if let Some(act) = self.config().act {
            act.queue.withdraw_session(session);
        }
    }
    /// Stop: the open cards of the conversation are withdrawn and one row says so.
    pub(super) fn stopped_by_user(&self, conversation: i64) {
        self.end_run_cards(conversation);
        let url = self.conv(conversation, |c| c.page.clone());
        self.write_row(DecisionAudit {
            ts_ms: crate::time::now_ms(),
            session_id: conversation,
            tool: "browser_stop".into(),
            url: audit_url(&url),
            summary: "the user pressed Stop".into(),
            tab_id: None,
            tab_owner: None,
            tier: "A".into(),
            categories: Vec::new(),
            decision: AuditDecision::Stopped,
            reason: "the user stopped the browser; the call in flight was killed".into(),
            approval_id: None,
            screenshot: None,
            actor: "user_stop".into(),
            input: None,
            profile: None,
        });
    }

    // ---- audit ----

    /// Writes one decision row (never a typed value).
    fn write_row(&self, mut row: DecisionAudit) {
        let config = self.config();
        let store = config
            .act
            .as_ref()
            .and_then(|a| a.store.clone())
            .or_else(|| config.database.clone().map(MonoBrowserStore::at));
        let Some(store) = store else {
            return;
        };
        row.profile = row
            .profile
            .or_else(|| config.act.as_ref().and_then(|a| a.profile.clone()));
        row.summary = row.summary.chars().take(300).collect();
        let _ = store.record_decision(&self.mono_id, &row);
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        id: i64,
        tool: &str,
        read_url: &str,
        summary: &str,
        user_tab: bool,
        ev: Option<&Evaluation>,
        decision: AuditDecision,
        reason: &str,
        approval_id: Option<String>,
    ) {
        self.write_row(DecisionAudit {
            ts_ms: crate::time::now_ms(),
            session_id: id,
            tool: tool.to_owned(),
            url: audit_url(read_url),
            summary: summary.to_owned(),
            tab_id: None,
            tab_owner: Some(if user_tab {
                "user".to_owned()
            } else {
                self.mono_id.clone()
            }),
            tier: ev.map_or("A", |e| e.tier.letter()).to_owned(),
            categories: ev
                .map(|e| e.categories.iter().map(|c| c.as_str().to_owned()).collect())
                .unwrap_or_default(),
            decision,
            reason: reason.to_owned(),
            approval_id,
            screenshot: None,
            actor: "mono".into(),
            input: None,
            profile: None,
        });
    }

    /// A refusal: one row, and the text for the Mono.
    fn refuse(
        &self,
        id: i64,
        plan: &Plan,
        read: &Read,
        decision: AuditDecision,
        reason: &str,
        text: String,
    ) -> String {
        let user_tab = self.is_user_tab(id, &read.tab);
        self.row(
            id,
            &plan.tool,
            &read.url,
            &plan.summary,
            user_tab,
            None,
            decision,
            reason,
            None,
        );
        text
    }

    // ---- levels ----

    /// The act settings, when this Mono may act.
    fn act_allowed(&self, id: i64, tool: &str) -> Result<ActConfig, String> {
        let Some(act) = self.config().act else {
            return Err(format!(
                "{tool} is not available: this browser has no act settings loaded."
            ));
        };
        if act.level != BrowserLevel::Act {
            let page = self.conv(id, |c| c.page.clone());
            self.row(
                id,
                tool,
                &page,
                "refused: read only",
                false,
                None,
                AuditDecision::Blocked,
                "read only",
                None,
            );
            return Err(format!(
                "{tool} is not available: this Mono can only read the browser. Tell the user what to do, or that the browser level must be raised."
            ));
        }
        Ok(act)
    }

    // ---- the act tools ----

    pub(super) fn act_tool(&self, id: i64, name: &str, args: &Value) -> Result<String, String> {
        let act = self.act_allowed(id, name)?;
        let plan = build_plan(name, args)?;
        let files = if plan.op == Op::Upload {
            Some(self.upload_files(id, &plan)?)
        } else {
            None
        };
        self.ready(id)?;
        let tab = self.bound_tab(id)?;
        if self.conv(id, |c| c.halted.contains(&tab)) {
            return Err("This tab was stopped for the rest of the turn because it reached a page this Mono may not use. Do not go back to it.".into());
        }
        self.hold(id)?;
        let read = self.fresh_read(id, &tab, &plan)?;
        let (ev, extra) = self.evaluate(id, &plan, &read, &act, files.as_deref())?;
        let (approval_id, decision) = self.decide(id, &act, &plan, &read, &ev, files.as_deref())?;
        if approval_id.is_some() {
            // The card may have waited minutes: the page can have changed meanwhile.
            self.hold(id)?;
            self.still_same(id, &plan, &read).inspect_err(|text| {
                self.row(
                    id,
                    &plan.tool,
                    &read.url,
                    &plan.summary,
                    self.is_user_tab(id, &tab),
                    Some(&ev),
                    AuditDecision::Blocked,
                    "the page changed while the user was asked",
                    approval_id.clone(),
                );
                let _ = text;
            })?;
        }
        self.run_act(
            id,
            &plan,
            &read,
            &ev,
            &extra,
            decision,
            approval_id,
            files.as_deref(),
        )
    }

    fn upload_files(&self, id: i64, plan: &Plan) -> Result<Vec<uploads::UploadFile>, String> {
        let config = self.config();
        let mut allowed = config.project_dirs.clone();
        allowed.extend(config.output_dir.clone());
        let rules = uploads::Rules {
            allowed,
            output_dir: config.output_dir.clone(),
            home: config.home.clone().or_else(uploads::home_dir),
            user_messages: self.conv(id, |c| c.user_messages.clone()),
        };
        uploads::check(&plan.paths, &rules).inspect_err(|text| {
            let page = self.conv(id, |c| c.page.clone());
            self.row(
                id,
                &plan.tool,
                &page,
                &plan.summary,
                false,
                None,
                AuditDecision::Blocked,
                text,
                None,
            );
        })
    }

    // ---- the fresh read ----

    fn url_now(&self, id: i64) -> Result<String, String> {
        let data = self
            .browse(id, &["get", "url"], &[])
            .map_err(|f| self.act_fail(id, f))?;
        Ok(data["url"].as_str().unwrap_or("").to_owned())
    }

    /// A failure before the command ran: a ref Ardia no longer knows is "the element changed".
    fn act_fail(&self, id: i64, failure: Failure) -> String {
        if is_stale_code(failure.code()) {
            return CHANGED.to_owned();
        }
        self.fail(id, failure)
    }

    /// One attribute: `Some(Some(v))` a value, `Some(None)` present but empty, `None` not
    /// readable (absent, or the element has no such attribute).
    fn attr_probe(
        &self,
        id: i64,
        target: &str,
        name: &str,
    ) -> Result<Option<Option<String>>, String> {
        match self.browse(id, &["get", "attr"], &[target, name]) {
            Ok(data) => Ok(Some(fresh::attr_value(&data))),
            Err(Failure::Refused { code, .. }) if is_stale_code(code.as_deref()) => {
                Err(CHANGED.to_owned())
            }
            Err(Failure::Refused { .. }) => Ok(None),
            Err(other) => Err(self.fail(id, other)),
        }
    }
    fn attr(&self, id: i64, target: &str, name: &str) -> Result<Option<String>, String> {
        Ok(self.attr_probe(id, target, name)?.flatten())
    }

    fn form_now(&self, id: i64) -> Result<FormRead, String> {
        match self.browse(id, &["form"], &[]) {
            Ok(data) => Ok(FormRead::parse(&data)),
            Err(Failure::Refused { .. }) => Ok(FormRead::default()),
            Err(other) => Err(self.fail(id, other)),
        }
    }

    /// The actions of the page's forms, each resolved against the page address. The first form
    /// is read with `get attr form action` (measured); further forms are probed one by one
    /// (`form:nth-of-type(N)`, not measured) until one is not found. A form with no `action`
    /// posts to the page itself.
    fn form_actions_now(
        &self,
        id: i64,
        page: &str,
        form: &FormRead,
    ) -> Result<FormActions, String> {
        let mut out = FormActions {
            urls: Vec::new(),
            complete: true,
        };
        let mut push = |raw: Option<String>| match raw {
            Some(raw) => match fresh::resolve(page, &raw) {
                Some(url) => out.urls.push(url),
                None => out.complete = false,
            },
            None => out.urls.push(page.to_owned()),
        };
        match self.attr_probe(id, "form", "action")? {
            Some(raw) => push(raw),
            None => {
                // No form found. A page whose `form` lists fields has one: its action is unknown.
                if form.known && (!form.fields.is_empty() || !form.submits.is_empty()) {
                    out.complete = false;
                }
                return Ok(out);
            }
        }
        for n in 2..=6 {
            match self.attr_probe(id, &format!("form:nth-of-type({n})"), "action")? {
                Some(raw) => push(raw),
                None => break,
            }
        }
        Ok(out)
    }

    fn fresh_read(&self, id: i64, tab: &str, plan: &Plan) -> Result<Read, String> {
        let mut read = Read {
            tab: tab.to_owned(),
            url: self.url_now(id)?,
            ..Read::default()
        };
        let config = self.config();
        // The page the action would happen on must be one this Mono may use.
        if let Err(refusal) = config.origins.check(&read.url) {
            let text = format!(
                "{} Nothing was done: the tab is on a page this Mono may not use.",
                tabs_origin_refusal(refusal)
            );
            return Err(self.refuse(
                id,
                plan,
                &read,
                AuditDecision::Blocked,
                "the page the action would happen on is not allowed",
                text,
            ));
        }
        read.title = match self.browse(id, &["get", "title"], &[]) {
            Ok(data) => data["title"].as_str().unwrap_or("").to_owned(),
            Err(Failure::Refused { .. }) => String::new(),
            Err(other) => return Err(self.fail(id, other)),
        };
        let key = plan.key.as_deref();
        let key_prints = key.is_some_and(fresh::is_printable_key);
        let key_submits = key.is_some_and(fresh::is_submit_key);
        let needs_refs = plan.ref_.is_some() || key_prints || key_submits;
        if !needs_refs {
            return Ok(read);
        }
        let changed = |this: &Self, read: &Read, why: &str, text: &str| {
            this.refuse(id, plan, read, AuditDecision::Blocked, why, text.to_owned())
        };
        let seen = self
            .conv(id, |c| c.seen.get(tab).cloned())
            .unwrap_or_default();
        if let Some(r) = &plan.ref_ {
            if !seen.refs.contains_key(r) {
                return Err(changed(
                    self,
                    &read,
                    "the ref was never in a snapshot",
                    NEVER_SEEN,
                ));
            }
            if seen.url != read.url {
                return Err(changed(
                    self,
                    &read,
                    "the tab URL changed since the snapshot",
                    CHANGED,
                ));
            }
        }
        let snapshot = self
            .browse(id, &["snapshot", "-i"], &[])
            .map_err(|f| self.act_fail(id, f))?;
        let refs = fresh::refs_of(&snapshot);
        if let Some(r) = &plan.ref_ {
            match (refs.get(r), seen.refs.get(r)) {
                (Some(now), Some(then)) if now == then => {
                    read.role = now.0.clone();
                    read.name = now.1.clone();
                    read.ref_ = Some(r.clone());
                }
                _ => {
                    return Err(changed(
                        self,
                        &read,
                        "the ref no longer resolves to what the snapshot showed",
                        CHANGED,
                    ))
                }
            }
        } else if let Some(focus) = fresh::focused_ref(snapshot["snapshot"].as_str().unwrap_or(""))
        {
            // A key: the focused element, when the snapshot marks one.
            if let Some((role, name)) = refs.get(&focus) {
                read.focus_known = true;
                read.role = role.clone();
                read.name = name.clone();
                read.ref_ = Some(focus);
            }
        }
        self.read_attrs(id, plan, &mut read)?;
        Ok(read)
    }

    /// The attributes, the form and the form actions the action's risk depends on.
    fn read_attrs(&self, id: i64, plan: &Plan, read: &mut Read) -> Result<(), String> {
        let at = read.ref_.as_ref().map(|r| format!("@{r}"));
        let link = read.role == "link";
        let needs_form = match plan.op {
            Op::Click => !link,
            Op::Type | Op::Fill => true,
            Op::Press => {
                read.ref_.is_some() || plan.key.as_deref().is_some_and(fresh::is_submit_key)
            }
            _ => false,
        };
        if let Some(at) = &at {
            match plan.op {
                Op::Click if link => read.attrs.href = self.attr(id, at, "href")?,
                Op::Click => {
                    read.attrs.input_type = self.attr(id, at, "type")?;
                    read.attrs.formaction = self.attr(id, at, "formaction")?;
                }
                Op::Check | Op::Upload => read.attrs.input_type = self.attr(id, at, "type")?,
                Op::Type | Op::Fill | Op::Press => {
                    read.attrs.input_type = self.attr(id, at, "type")?;
                    read.attrs.autocomplete = self.attr(id, at, "autocomplete")?;
                }
                _ => {}
            }
        }
        if needs_form {
            read.form = self.form_now(id)?;
            if read.attrs.input_type.is_none() {
                // `form` lists the type of every field: it fills a `get attr` that failed.
                read.attrs.input_type = read
                    .ref_
                    .as_deref()
                    .and_then(|r| read.form.field(r))
                    .map(|f| f.input_type.clone())
                    .filter(|t| !t.is_empty());
            }
        }
        let submit_like = match plan.op {
            Op::Click => {
                !link
                    && (read.ref_.as_deref().is_some_and(|r| read.form.is_submit(r))
                        || read.attrs.input_type.as_deref() == Some("submit")
                        || !read.form.known
                        || read.attrs.input_type.is_none())
            }
            Op::Press => plan.key.as_deref().is_some_and(fresh::is_submit_key),
            _ => false,
        };
        if submit_like {
            read.actions = self.form_actions_now(id, &read.url, &read.form)?;
        } else {
            read.actions.complete = true;
        }
        Ok(())
    }

    /// After a card: the same url and the same element as before.
    fn still_same(&self, id: i64, plan: &Plan, read: &Read) -> Result<(), String> {
        if self.url_now(id)? != read.url {
            return Err(CHANGED.to_owned());
        }
        if let Some(r) = &plan.ref_ {
            let snapshot = self
                .browse(id, &["snapshot", "-i"], &[])
                .map_err(|f| self.act_fail(id, f))?;
            let refs = fresh::refs_of(&snapshot);
            if refs
                .get(r)
                .is_none_or(|(role, name)| *role != read.role || *name != read.name)
            {
                return Err(CHANGED.to_owned());
            }
        }
        Ok(())
    }

    // ---- the decision ----

    /// Checks no approval overrides, then the shared classifier. `Err` is the refusal (already
    /// audited); `Ok` the evaluation and the categories the read added itself.
    fn evaluate(
        &self,
        id: i64,
        plan: &Plan,
        read: &Read,
        _act: &ActConfig,
        files: Option<&[uploads::UploadFile]>,
    ) -> Result<(Evaluation, BTreeSet<Category>), String> {
        let page = &read.url;
        let name = clean(&read.name);
        let origins = self.config().origins;
        let deny = |decision: AuditDecision, reason: &str, text: String| {
            self.refuse(id, plan, read, decision, reason, text)
        };

        // Passwords and one-time codes, and character keys that could land in one.
        let types = matches!(plan.op, Op::Type | Op::Fill);
        let key = plan.key.as_deref();
        let key_prints = key.is_some_and(fresh::is_printable_key);
        if key_prints && !read.focus_known {
            return Err(deny(
                AuditDecision::Blocked,
                "a character key with a focus that could not be read",
                "A character key is not sent: zptah could not tell which element has the focus, and the key could land in a password field. Use browser_type or browser_fill with the ref of the field.".into(),
            ));
        }
        if types || key_prints {
            if let Some(why) = fresh::secret_field(&name, &read.attrs) {
                self.post_password_notice(page);
                return Err(deny(
                    AuditDecision::Invariant,
                    why,
                    format!(
                        "Not allowed: {why}. Ask the user to enter it themselves, then continue."
                    ),
                ));
            }
        }

        // The destinations: a link's address, a form's action, the button's own `formaction`.
        let signals_input = ElementInput {
            role: &read.role,
            name: &name,
            attrs: &read.attrs,
            form: &read.form,
            ref_: read.ref_.as_deref().unwrap_or(""),
            page_url: page,
        };
        let mut element = fresh::element_signals(&signals_input);
        let submits = match plan.op {
            Op::Click => element.is_submit || !read.actions.urls.is_empty(),
            Op::Press => key.is_some_and(fresh::is_submit_key),
            _ => false,
        };
        let link = plan.op == Op::Click && read.role == "link";
        let mut destinations: Vec<(&str, String)> = Vec::new();
        if link {
            if let Some(raw) = read.attrs.href.as_deref() {
                match fresh::resolve(page, raw) {
                    Some(url) => destinations.push(("the link", url)),
                    // A link whose address can not be read is not followed blind.
                    None => destinations.push(("the link", raw.to_owned())),
                }
            }
        }
        if submits {
            for url in &read.actions.urls {
                destinations.push(("the form", url.clone()));
            }
            if let Some(url) = &element.form_action {
                destinations.push(("the button", url.clone()));
            }
        }
        for (what, url) in &destinations {
            if let Err(refusal) = origins.check(url) {
                let shown = audit_url(url);
                return Err(deny(
                    AuditDecision::Blocked,
                    &format!("{what} leads to {shown}, which this Mono may not use"),
                    format!(
                        "Not done: {what} leads to {}. {} The page was left as it is.",
                        if shown.is_empty() {
                            "an address that is not a web page".to_owned()
                        } else {
                            shown
                        },
                        tabs_origin_refusal(refusal)
                    ),
                ));
            }
        }

        // The classifier's inputs.
        let mut ext: BTreeSet<Category> = BTreeSet::new();
        let mut declared: BTreeSet<Category> = BTreeSet::new();
        // A submit whose destination or form could not be read in full: ask.
        if submits
            && (!read.actions.complete || (!read.form.known && !read.actions.urls.is_empty()))
        {
            ext.insert(Category::SoulRule);
        }
        let kind = match plan.op {
            Op::Click | Op::Check => Some(ActionKind::Click),
            Op::Select => Some(ActionKind::Select),
            Op::Upload => Some(ActionKind::Upload),
            Op::Hover => None,
            Op::Type | Op::Fill => Some(ActionKind::Type(fresh::field_kind(&name, &read.attrs))),
            Op::Press if key_prints => {
                Some(ActionKind::Type(fresh::field_kind(&name, &read.attrs)))
            }
            Op::Press if key.is_some_and(fresh::is_submit_key) => {
                // Enter presses the focused control or submits its form: judged as a click on the
                // form's submit button.
                element.is_submit = true;
                if !(read.focus_known && read.role == "button") {
                    element.role = "button".into();
                    element.name = read
                        .form
                        .submits
                        .first()
                        .map(|(_, label)| clean(label))
                        .unwrap_or_default();
                    element.label = Some(element.name.clone()).filter(|n| !n.is_empty());
                }
                Some(ActionKind::Click)
            }
            Op::Press => None,
        };
        // A textbox whose kind could not be read, on a sign-in page: asks first.
        if types
            && read.attrs.input_type.is_none()
            && (is_field_role(&read.role) || read.role.is_empty())
            && fresh::login_like(page, &read.title, &read.form)
        {
            declared.insert(Category::CredentialEntry);
        }
        let heading = read.title.clone();
        let vi = looks_vietnamese(&format!("{} {} {}", read.title, name, element.nearby_text));
        let legacy = classifier_name(&plan.tool);
        let mut action = TrustAction::new(legacy, &plan.args, page);
        action.kind = kind;
        action.form = read.form.signals();
        action.element = element;
        action.page_title = &read.title;
        action.page_heading = &heading;
        action.page_vi = vi;
        action.typed = plan.typed.as_deref();
        action.declared.extend(declared);
        action.ext = if plan.op == Op::Hover {
            BTreeSet::new()
        } else {
            ext.clone()
        };
        let ev = self
            .with_trust(id, |state, cfg| {
                state.evaluate(
                    &TrustConfig {
                        level: cfg.level,
                        rules: &cfg.rules,
                    },
                    &action,
                )
            })
            .ok_or("This browser has no act settings loaded.")?;
        let _ = files;
        match &ev.decision {
            Decision::Deny(reason) => {
                let audit = if reason.contains("password") {
                    AuditDecision::Invariant
                } else {
                    AuditDecision::Blocked
                };
                self.row(
                    id,
                    &plan.tool,
                    page,
                    &plan.summary,
                    self.is_user_tab(id, &read.tab),
                    Some(&ev),
                    audit,
                    reason,
                    None,
                );
                Err(format!("Not allowed: {reason}."))
            }
            _ => Ok((ev, ext)),
        }
    }

    fn post_password_notice(&self, page: &str) {
        if let Some(act) = self.config().act {
            let site = registrable_domain_of(page).unwrap_or_default();
            act.queue
                .post_notice(NoticeKind::PasswordNeeded, None, &act.mono_label, &site);
        }
    }

    /// Allow, allow by grant, or ask. `Ok`: the card id when one was asked, and how it was
    /// decided for the audit row.
    fn decide(
        &self,
        id: i64,
        act: &ActConfig,
        plan: &Plan,
        read: &Read,
        ev: &Evaluation,
        files: Option<&[uploads::UploadFile]>,
    ) -> Result<(Option<String>, AuditDecision), String> {
        match &ev.decision {
            Decision::Allow => Ok((None, AuditDecision::Auto)),
            Decision::AllowByGrant(_) => Ok((None, AuditDecision::Auto)),
            Decision::Deny(reason) => Err(format!("Not allowed: {reason}.")),
            Decision::Ask(_) => {
                let facts = self.facts(act, plan, read, ev, files);
                let user_tab = self.is_user_tab(id, &read.tab);
                match self.ask_card(act, id, &plan.tool, facts, ev) {
                    Ok((answer, card)) => {
                        let granted = self
                            .with_trust(id, |state, _| state.commit(ev, answer))
                            .unwrap_or_default();
                        let how = if answer == Answer::AllowedTask && !granted.is_empty() {
                            AuditDecision::AskedAllowedTask
                        } else {
                            AuditDecision::AskedAllowedOnce
                        };
                        Ok((Some(card), how))
                    }
                    Err((text, audit, card)) => {
                        self.row(
                            id,
                            &plan.tool,
                            &read.url,
                            &plan.summary,
                            user_tab,
                            Some(ev),
                            audit,
                            &text,
                            Some(card),
                        );
                        Err(text)
                    }
                }
            }
        }
    }

    /// The facts of a card.
    fn facts(
        &self,
        act: &ActConfig,
        plan: &Plan,
        read: &Read,
        ev: &Evaluation,
        files: Option<&[uploads::UploadFile]>,
    ) -> CardFacts {
        let label = clean(&read.name);
        let desc = match plan.op {
            Op::Click | Op::Check => ActionDesc::Click { label },
            Op::Type | Op::Fill => ActionDesc::Type { label },
            Op::Select => ActionDesc::Select { label },
            Op::Press => ActionDesc::Press {
                key: plan.key.clone().unwrap_or_default(),
            },
            Op::Hover => ActionDesc::Hover,
            Op::Upload => ActionDesc::Upload {
                names: files
                    .into_iter()
                    .flatten()
                    .map(|f| {
                        f.path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    })
                    .collect(),
            },
        };
        let mut facts = CardFacts::new(
            &act.mono_label,
            &ev.site,
            desc,
            ev.categories.iter().copied().collect(),
        );
        facts.url = read.url.clone();
        facts.recipients = read.form.signals().recipients;
        facts.amounts = crate::mono::browser::risk::signals::extract_amounts(&read.name);
        facts.field = matches!(plan.op, Op::Type | Op::Fill)
            .then(|| fresh::field_kind(&read.name, &read.attrs));
        facts.site_kind = site_category(&read.url);
        facts.rule = ev.reason.strip_prefix("soul rule: ").map(str::to_owned);
        facts.purpose = plan.args["purpose"]
            .as_str()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned);
        if let Some(files) = files {
            // Each exact path and its size (the Mono named them; the user decides on them).
            facts.agent_details = Some(
                files
                    .iter()
                    .map(uploads::UploadFile::describe)
                    .collect::<Vec<_>>()
                    .join("; "),
            );
        }
        facts
    }

    /// Opens a card and waits for the answer. `Err`: the text for the Mono, how to audit it and
    /// the card id.
    #[allow(clippy::type_complexity)]
    fn ask_card(
        &self,
        act: &ActConfig,
        id: i64,
        tool: &str,
        facts: CardFacts,
        ev: &Evaluation,
    ) -> Result<(Answer, String), (String, AuditDecision, String)> {
        let categories: Vec<Category> = ev.categories.iter().copied().collect();
        let request = Request {
            mono_id: self.mono_id.clone(),
            session_id: id,
            tool: tool.to_owned(),
            tab: None,
            facts,
            screenshot: None,
            offer_conversation: offers_conversation(&categories, ev.tier == Tier::Always),
        };
        let (card, outcome) = act
            .queue
            .ask(request, act.approval_timeout, KEEP_ALIVE, || {
                // The holds on the tabs last 2 minutes: a card can wait for longer.
                let _ = self.hold(id);
            });
        match outcome {
            Outcome::Answered(UserAnswer::AllowOnce) => Ok((Answer::AllowedOnce, card)),
            Outcome::Answered(UserAnswer::AllowConversation) => Ok((Answer::AllowedTask, card)),
            Outcome::Answered(UserAnswer::Deny) => Err((
                "Denied by the user. Do not do this action, and do not try another way to do it."
                    .into(),
                AuditDecision::AskedDenied,
                card,
            )),
            Outcome::TimedOut => Err((
                "Denied: the user did not answer in time.".into(),
                AuditDecision::AskedTimeout,
                card,
            )),
            Outcome::Withdrawn => Err((
                "Denied: nobody could answer the question (the run ended or was stopped, or no one is there to ask).".into(),
                AuditDecision::AskedDenied,
                card,
            )),
        }
    }

    // ---- running it ----

    #[allow(clippy::too_many_arguments)]
    fn run_act(
        &self,
        id: i64,
        plan: &Plan,
        read: &Read,
        ev: &Evaluation,
        _extra: &BTreeSet<Category>,
        decision: AuditDecision,
        approval_id: Option<String>,
        files: Option<&[uploads::UploadFile]>,
    ) -> Result<String, String> {
        let user_tab = self.is_user_tab(id, &read.tab);
        // An upload sends the paths as they were resolved and checked, never the ones given.
        let mut values = plan.values.clone();
        if let Some(files) = files {
            values.truncate(1);
            values.extend(files.iter().map(|f| f.path.to_string_lossy().into_owned()));
        }
        let value_refs: Vec<&str> = values.iter().map(String::as_str).collect();
        let reason = ev.reason.clone();
        let data = match self.browse(id, &plan.cmd, &value_refs) {
            Ok(data) => data,
            Err(Failure::Refused { code, message }) => {
                // The command refused: it did not run.
                let text = if is_stale_code(code.as_deref()) {
                    CHANGED.to_owned()
                } else if code.as_deref() == Some("not_visible") {
                    format!(
                        "{} The element is not visible: scroll to it, or take a new snapshot.",
                        clean(&message)
                    )
                } else {
                    clean(&message)
                };
                self.row(
                    id,
                    &plan.tool,
                    &read.url,
                    &plan.summary,
                    user_tab,
                    Some(ev),
                    decision,
                    &format!(
                        "{reason}; the command did not run: {}",
                        code.as_deref().unwrap_or("failed")
                    ),
                    approval_id,
                );
                return Err(text);
            }
            Err(other) => {
                // Unreachable after the command was sent, a timeout, an unreadable reply, a
                // Stop: nobody knows whether it ran. Never repeated (see `call_tool`).
                let stopped = matches!(other, Failure::Cancelled);
                let unreachable = matches!(other, Failure::Unreachable(_));
                let _ = self.fail(id, other);
                if unreachable {
                    // `fail` marked the call for a repeat; an act call is never repeated.
                    self.conv(id, |c| c.lost = false);
                }
                self.row(
                    id,
                    &plan.tool,
                    &read.url,
                    &plan.summary,
                    user_tab,
                    Some(ev),
                    decision,
                    &format!("{reason}; the outcome is unknown"),
                    approval_id,
                );
                return Err(if stopped {
                    format!("{STOPPED} The last action may or may not have run: look at the page before trying anything again.")
                } else {
                    "The result of this action is unknown: Ardia stopped answering after the command was sent, so it may or may not have run. It was NOT repeated. Look at the page (browser_snapshot) before trying again.".into()
                });
            }
        };

        let changes = &data["changes"];
        let after = self
            .url_now(id)
            .ok()
            .filter(|u| !u.is_empty())
            .or_else(|| changes["url"]["to"].as_str().map(str::to_owned))
            .unwrap_or_else(|| read.url.clone());
        let title = changes["title"]["to"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| read.title.clone());
        let writes = fresh::writes_of(changes);
        let mut summary = plan.summary.clone();
        if matches!(plan.op, Op::Type | Op::Fill) {
            summary = summary.replace(
                "field",
                &clean(&read.name).chars().take(60).collect::<String>(),
            );
        } else if !read.name.is_empty() {
            summary = format!(
                "{summary} \"{}\"",
                clean(&read.name).chars().take(80).collect::<String>()
            );
        }
        if let Some(files) = files {
            summary = format!(
                "{summary}: {}",
                files
                    .iter()
                    .map(|f| f
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        summary = format!("{summary}; url after {}", audit_url(&after));
        if !writes.is_empty() {
            summary = format!("{summary}; wrote: {}", writes.join(", "));
        }
        self.row(
            id,
            &plan.tool,
            &read.url,
            &summary,
            user_tab,
            Some(ev),
            decision,
            &reason,
            approval_id,
        );
        // A password field that took the text anyway (its type could not be read): Ardia says so
        // after the fact.
        let password_note = data["verify"]["kind"] == "password";
        if password_note {
            self.row(
                id,
                &plan.tool,
                &read.url,
                "text went into a password field",
                user_tab,
                Some(ev),
                AuditDecision::Invariant,
                "the field turned out to be a password field after the text was entered",
                None,
            );
            self.post_password_notice(&read.url);
        }

        // A link that led to another site: reading there is free, acting asks.
        if registrable_domain_of(&after) != registrable_domain_of(&read.url) {
            self.with_trust(id, |state, _| state.note_link_followed(&after));
        }
        let navigated = changes["document"] == "new" || after != read.url;
        let (listed, shown) = added_elements(changes);
        self.conv(id, |c| {
            let seen = c.seen.entry(read.tab.clone()).or_default();
            if navigated {
                *seen = fresh::SeenRefs {
                    url: after.clone(),
                    epoch: None,
                    refs: HashMap::new(),
                };
            }
            seen.refs.extend(listed);
        });
        // The tab must not be left on a page this Mono may not use (page scripts can navigate).
        self.after(id, &read.tab, &after, &plan.tool)?;

        let mut body = String::from("### Result\n");
        body.push_str(&format!(
            "- Ran {}{}.\n",
            plan.tool.trim_start_matches("browser_"),
            if read.name.is_empty() {
                String::new()
            } else {
                format!(
                    " on \"{}\"",
                    clean(&read.name).chars().take(80).collect::<String>()
                )
            }
        ));
        if navigated {
            body.push_str("- The page changed: the refs of the old page no longer work.\n");
        }
        if let Some(files) = files {
            body.push_str(&format!("- Attached {} file(s).\n", files.len()));
        }
        if !writes.is_empty() {
            body.push_str(&format!("- Requests sent: {}\n", writes.join(", ")));
        }
        if password_note {
            body.push_str("- WARNING: that field is a password field. Do not enter anything else into it; tell the user.\n");
        }
        let count = |key: &str| changes[key].as_array().map_or(0, Vec::len);
        body.push_str(&format!(
            "- Changes: {} added, {} removed, {} changed.\n",
            count("added"),
            count("removed"),
            count("changed")
        ));
        if !shown.is_empty() {
            body.push_str("### Added\n");
            for line in &shown {
                body.push_str(&format!("- {line}\n"));
            }
        }
        body.push_str("Take a browser_snapshot to see the page now.\n");
        Ok(format!(
            "{}{}",
            header(&after, &clean(&title)),
            clean(&body)
        ))
    }

    // ---- the user's own tab ----

    /// `browser_tabs adopt_active`: the tab the user has open now, once they approve (the first
    /// time for that tab in this conversation). It joins the conversation's set: never closed,
    /// only let go of.
    pub(super) fn adopt_active(&self, id: i64, args: &Value) -> Result<String, String> {
        let act = self.act_allowed(id, "browser_tabs adopt_active")?;
        self.ready(id)?;
        let wanted = args["tab"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let data = self
            .browse(id, &["tab", "list"], &[])
            .map_err(|f| self.fail(id, f))?;
        let active = data["tabs"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| {
                t["active"] == true
                    && t["appTabId"].as_str().is_some()
                    && t["type"].as_str().is_none_or(|k| k == "page")
            })
            .cloned();
        let Some(active) = active else {
            return Err("The user has no tab open in Ardia that can be used.".into());
        };
        let tab = active["appTabId"].as_str().unwrap_or("").to_owned();
        let url = active["url"].as_str().unwrap_or("").to_owned();
        let title = clean(active["title"].as_str().unwrap_or(""));
        if wanted.is_some_and(|w| w != tab) {
            self.audit(
                id,
                "tab_rejected",
                "",
                "adopt_active of a tab that is not the user's active tab",
            );
            return Err("That is not the tab the user has open now. adopt_active takes the user's active tab only; leave `tab` out.".into());
        }
        if self.usable(id, &tab) {
            return Err(format!(
                "Tab {tab} is already one of this conversation's tabs."
            ));
        }
        if let Err(refusal) = self.config().origins.check(&url) {
            self.row(
                id,
                "browser_tabs",
                &url,
                "adopt the user's active tab",
                true,
                None,
                AuditDecision::Blocked,
                "the user's tab is on a page this Mono may not use",
                None,
            );
            return Err(format!(
                "{} The user's tab was not taken.",
                tabs_origin_refusal(refusal)
            ));
        }
        let mut approval = None;
        if !self.conv(id, |c| c.approved_user_tabs.contains(&tab)) {
            let ev = Evaluation {
                decision: Decision::Ask(Vec::new()),
                tier: Tier::Always,
                categories: BTreeSet::new(),
                site: registrable_domain_of(&url).unwrap_or_default(),
                reason: "take over the user's tab".into(),
            };
            let mut facts = CardFacts::new(
                &act.mono_label,
                &ev.site,
                ActionDesc::TabTransfer {
                    title: title.clone(),
                    from: "your browser".into(),
                },
                Vec::new(),
            );
            facts.url = url.clone();
            match self.ask_card(&act, id, "browser_tabs", facts, &ev) {
                Ok((_, card)) => approval = Some(card),
                Err((text, audit, card)) => {
                    self.row(
                        id,
                        "browser_tabs",
                        &url,
                        "adopt the user's active tab",
                        true,
                        Some(&ev),
                        audit,
                        &text,
                        Some(card),
                    );
                    return Err(text);
                }
            }
        }
        let bound = self.bind(id, &tab)?;
        self.conv(id, |c| {
            c.user_tab = Some(tab.clone());
            c.approved_user_tabs.insert(tab.clone());
        });
        self.row(
            id,
            "browser_tabs",
            &url,
            "adopt the user's active tab",
            true,
            None,
            if approval.is_some() {
                AuditDecision::AskedAllowedOnce
            } else {
                AuditDecision::Auto
            },
            "the user's tab joined the conversation (it is never closed)",
            approval,
        );
        let shown = bound["url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .unwrap_or(&url);
        let page = bound["title"].as_str().unwrap_or(&title);
        self.page_reply(
            id,
            &tab,
            "browser_tabs",
            shown,
            page,
            format!("### Result\nTook the user's tab {tab} and made it this conversation's tab. It stays open when you are done.\n"),
        )
    }

    /// `browser_navigate` in the user's own tab: judged like any navigation by the shared
    /// classifier (an address outside the task, a sensitive site) and asked when it says so.
    pub(super) fn user_tab_navigation(&self, id: i64, url: &str, tab: &str) -> Result<(), String> {
        let act = self.act_allowed(id, "browser_navigate")?;
        let page = self.conv(id, |c| c.page.clone());
        let args = json!({"url": url});
        let mut action = TrustAction::new("browser_navigate", &args, &page);
        action.target_url = Some(url);
        let ev = self
            .with_trust(id, |state, cfg| {
                state.evaluate(
                    &TrustConfig {
                        level: cfg.level,
                        rules: &cfg.rules,
                    },
                    &action,
                )
            })
            .ok_or("This browser has no act settings loaded.")?;
        let summary = format!("navigate the user's tab to {}", audit_url(url));
        match &ev.decision {
            Decision::Deny(reason) => {
                self.row(
                    id,
                    "browser_navigate",
                    &page,
                    &summary,
                    true,
                    Some(&ev),
                    AuditDecision::Blocked,
                    reason,
                    None,
                );
                Err(format!("Not allowed: {reason}."))
            }
            Decision::Ask(_) => {
                let mut facts = CardFacts::new(
                    &act.mono_label,
                    &ev.site,
                    ActionDesc::Navigate,
                    ev.categories.iter().copied().collect(),
                );
                facts.url = url.to_owned();
                facts.site_kind = site_category(url);
                facts.rule = ev.reason.strip_prefix("soul rule: ").map(str::to_owned);
                match self.ask_card(&act, id, "browser_navigate", facts, &ev) {
                    Ok((answer, card)) => {
                        self.with_trust(id, |state, _| state.commit(&ev, answer));
                        self.row(
                            id,
                            "browser_navigate",
                            &page,
                            &summary,
                            true,
                            Some(&ev),
                            AuditDecision::AskedAllowedOnce,
                            &ev.reason,
                            Some(card),
                        );
                        let _ = tab;
                        Ok(())
                    }
                    Err((text, audit, card)) => {
                        self.row(
                            id,
                            "browser_navigate",
                            &page,
                            &summary,
                            true,
                            Some(&ev),
                            audit,
                            &text,
                            Some(card),
                        );
                        Err(text)
                    }
                }
            }
            Decision::Allow | Decision::AllowByGrant(_) => Ok(()),
        }
    }

    // ---- the Mono's own approval request ----

    /// `browser_request_approval`: always asks (a grant never covers it).
    pub(super) fn request_approval(&self, id: i64, args: &Value) -> Result<String, String> {
        let act = self.act_allowed(id, APPROVAL_TOOL)?;
        let text = |key: &str| args[key].as_str().map(str::trim).filter(|v| !v.is_empty());
        let action_text = text("action").ok_or("Describe the action in `action`.")?;
        text("category").ok_or("Give a category: payment, send_message, post_publish or other.")?;
        let url = text("url")
            .map(str::to_owned)
            .unwrap_or_else(|| self.conv(id, |c| c.page.clone()));
        let action = TrustAction::new(APPROVAL_TOOL, args, &url);
        let ev = self
            .with_trust(id, |state, cfg| {
                state.evaluate(
                    &TrustConfig {
                        level: cfg.level,
                        rules: &cfg.rules,
                    },
                    &action,
                )
            })
            .ok_or("This browser has no act settings loaded.")?;
        if let Decision::Deny(reason) = &ev.decision {
            return Err(format!("Not allowed: {reason}."));
        }
        let mut facts = CardFacts::new(
            &act.mono_label,
            &ev.site,
            ActionDesc::Other,
            ev.categories.iter().copied().collect(),
        );
        facts.url = url.clone();
        facts.agent_action = Some(action_text.to_owned());
        facts.agent_details = text("details").map(str::to_owned);
        facts.site_kind = site_category(&url);
        let summary = format!(
            "approval request: {}",
            clean(action_text).chars().take(120).collect::<String>()
        );
        match self.ask_card(&act, id, APPROVAL_TOOL, facts, &ev) {
            Ok((answer, card)) => {
                self.with_trust(id, |state, _| state.commit(&ev, answer));
                let how = if answer == Answer::AllowedTask {
                    AuditDecision::AskedAllowedTask
                } else {
                    AuditDecision::AskedAllowedOnce
                };
                self.row(
                    id,
                    APPROVAL_TOOL,
                    &url,
                    &summary,
                    false,
                    Some(&ev),
                    how,
                    &ev.reason,
                    Some(card),
                );
                Ok("Approved by the user. You may do this action now.".into())
            }
            Err((text, audit, card)) => {
                self.row(
                    id,
                    APPROVAL_TOOL,
                    &url,
                    &summary,
                    false,
                    Some(&ev),
                    audit,
                    &text,
                    Some(card),
                );
                Err(text)
            }
        }
    }
}

/// The elements the action added, as lines for the Mono (their refs become usable without a new
/// snapshot, because the Mono has now seen them), and the refs to remember.
#[allow(clippy::type_complexity)]
fn added_elements(changes: &Value) -> (HashMap<String, (String, String)>, Vec<String>) {
    let mut refs = HashMap::new();
    let mut lines = Vec::new();
    for node in changes["added"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_ADDED)
    {
        let (Some(r), role, name) = (
            node["ref"].as_str(),
            node["role"].as_str().unwrap_or(""),
            node["name"].as_str().unwrap_or(""),
        ) else {
            continue;
        };
        let shown = clean(name).chars().take(80).collect::<String>();
        lines.push(format!("{role} \"{shown}\" [ref={r}]"));
        refs.insert(r.to_owned(), (role.to_owned(), name.to_owned()));
    }
    (refs, lines)
}
