//! The act tools of the Ardia upstream (design spec sections 4, 6 and 9): click, type, fill,
//! press, select, check, hover and upload, the user's own tab, and the Mono's own approval
//! request.
//!
//! Every act tool is the same sequence:
//!
//! 1. validate the arguments (no process runs for a malformed call);
//! 2. a **fresh read** of the page right before the action ([`super::read`]): the tab URL and
//!    title, the snapshot refs, the target's attributes, the page's `form` and the actions of its
//!    forms, the text around a bare button, and for a key with no ref the focused element. The
//!    call fails with "the element changed, take a new snapshot" when the ref was never in a
//!    snapshot the Mono took, no longer resolves, has another role or name, or the tab is on
//!    another URL than at that snapshot;
//! 3. checks that no approval can override, then the shared classifier on the fresh read
//!    ([`super::judge`]); when it says ask, a card through the approval queue (a grant, a soul
//!    rule and the level work as for the extension backend). No card is posted for a turn that
//!    was stopped;
//! 4. after the user answered, the fresh read and the decision again: the same destinations and
//!    categories and no new refusal, or the call fails as changed (one audit row for the act, not
//!    one per read). The level is read again too: a level lowered while the card was open stops
//!    the act. An upload's files are copied into the gate's private folder, checked on the open
//!    file (the same file, not swapped for a link), and `ardia` gets the copies;
//! 5. the command (a link that opens a window of its own is loaded in the session's tab instead),
//!    then the URL after it, the origin check of the page it left the tab on
//!    ([`ArdiaUpstream::after`]), the closing of any window the page opened ([`super::popups`])
//!    and one audit row.
//!
//! The risk inputs are the shared classifier's: the extension backend's Tainted (data read on one
//! site carried to another), Injection and read-only domains. There is no per-turn intent guard.
//!
//! An act call is never repeated: when its outcome is unknown (Ardia went away after the command
//! was sent, a timeout) the Mono is told it may or may not have run.
use super::fresh::{self, CHANGED};
use super::judge::Judged;
use super::read::{press_effect, PressEffect, Read};
use super::uploads;
use super::user_tab;
use super::*;
use crate::mono::browser::ardia::runner::literal_value;
use crate::mono::browser::copy::{ActionDesc, CardFacts};
use crate::mono::browser::risk::decide::Decision;
use crate::mono::browser::risk::signals::{registrable_domain_of, site_category};
use crate::mono::browser::risk::Category;
use crate::mono::browser::trust::approval::{
    offers_conversation, NoticeKind, Outcome, Request, UserAnswer,
};
use crate::mono::browser::trust::audit::{typed_summary, AuditDecision, DecisionAudit};
use crate::mono::browser::trust::{
    Action as TrustAction, Answer, BrowserLevel, Config as TrustConfig, Evaluation, Tier,
};
use std::collections::BTreeSet;
use std::time::Instant;
/// How often a card's wait renews the holds on the conversation's tabs.
/// What a Mono whose browser has no act settings loaded is told: nothing can be judged, so no page
/// is opened or read.
pub(super) const NO_ACT: &str =
    "This browser has no settings loaded, so zptah can not check the page and nothing was done.";
const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// How often a card's wait looks at whether the turn was stopped.
const STOP_POLL: Duration = Duration::from_millis(200);
/// The longest text one `browser_type` or `browser_fill` takes.
const MAX_TYPED: usize = 20_000;
/// The added elements an act reply lists (their refs become usable without a new snapshot).
const MAX_ADDED: usize = 20;
/// What the Mono is told for a ref it never got from a snapshot.
pub(super) const NEVER_SEEN: &str = "That ref was not in any snapshot you took of this page, so it is not used. Take a new snapshot and use a ref from it (the element changed, take a new snapshot).";

/// The tool name the shared classifier knows (its tier table, the soul rules and the deny list
/// use the extension backend's names). The audit and the Mono keep the Ardia names.
pub(super) fn classifier_name(tool: &str) -> &'static str {
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
pub(super) enum Op {
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
pub(super) struct Plan {
    pub tool: String,
    pub op: Op,
    pub args: Value,
    /// `e12`: the ref the call acts on (a key may have none).
    pub ref_: Option<String>,
    /// The command and its flags.
    pub cmd: Vec<&'static str>,
    /// The values after `--`, the ref first (`@e12`), then the text or key.
    pub values: Vec<String>,
    /// Every value the call types (looked at for a card number, never stored).
    pub typed: Option<String>,
    pub key: Option<String>,
    pub paths: Vec<String>,
    /// For the audit: what was done, never a typed value.
    pub summary: String,
    /// `browser_check` with `checked: false`.
    pub uncheck: bool,
    /// The text of a type or fill has a line break (which can press Enter).
    pub newline: bool,
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
        uncheck: false,
        newline: false,
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
            plan.uncheck = !on;
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
            plan.newline = text.contains(['\n', '\r']);
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
            // With a `ref` the key is pressed in that element (`press @e3 Enter`), so the gate
            // judges exactly it and its form; without one the gate reads the focus itself.
            let has_ref = ["ref", "target"]
                .iter()
                .any(|k| args.get(*k).is_some_and(|v| !v.is_null()));
            if has_ref {
                with_ref(&mut plan, ref_()?);
            }
            plan.values.push(key.to_owned());
            plan.key = Some(key.to_owned());
            plan.summary = if fresh::is_printable_key(key) && !fresh::is_space_key(key) {
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

pub(super) fn is_stale_code(code: Option<&str>) -> bool {
    matches!(code, Some("stale_ref" | "vocab_superseded" | "unknown_ref"))
}

/// Roles of a control that holds typed text.
pub(super) fn is_field_role(role: &str) -> bool {
    matches!(
        role,
        "textbox" | "searchbox" | "combobox" | "spinbutton" | "textarea"
    )
}

/// Roles `check` and `uncheck` work on. `check` on anything else CLICKS it (a page script runs)
/// and only then fails, so the gate refuses it first.
pub(super) fn is_checkable_role(role: &str) -> bool {
    matches!(
        role,
        "checkbox" | "radio" | "switch" | "menuitemcheckbox" | "menuitemradio"
    )
}

/// How an act was decided.
enum Decided {
    /// Allowed (no card).
    Go(AuditDecision),
    /// The user answered a card.
    Asked { card: String, answer: Answer },
}

impl ArdiaUpstream {
    // ---- trust state, per conversation ----

    pub(super) fn trust_map(&self) -> MutexGuard<'_, HashMap<i64, TrustState>> {
        self.trust.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on the conversation's trust state, creating it on first use. `None` when this
    /// Mono has no act settings.
    pub(super) fn with_trust<R>(
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

    /// `message` is what the user typed (empty for a run no user message started).
    pub(super) fn begin_run_trust(&self, session: i64, message: &str) {
        self.with_trust(session, |state, _| state.begin_run(message));
    }
    /// What the Mono read is remembered (in memory), to spot it being sent to another site.
    pub(super) fn note_read(&self, session: i64, url: &str, text: &str) {
        self.with_trust(session, |state, _| state.note_read(url, text));
    }

    /// The trust level in force now: the gate's policy once it told this upstream
    /// ([`Upstream::policy_changed`]), else the level of the launch-time config.
    pub(super) fn level_now(&self, act: &ActConfig) -> BrowserLevel {
        self.level
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or(act.level)
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
    pub(super) fn write_row(&self, mut row: DecisionAudit) {
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
        row.summary = row.summary.chars().take(1200).collect();
        let _ = store.record_decision(&self.mono_id, &row);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn row(
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
    pub(super) fn refuse(
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
    pub(super) fn act_allowed(&self, id: i64, tool: &str) -> Result<ActConfig, String> {
        let Some(act) = self.config().act else {
            return Err(format!(
                "{tool} is not available: this browser has no act settings loaded."
            ));
        };
        if self.level_now(&act) != BrowserLevel::Act {
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

    /// `Err` when the turn was stopped: nothing more runs and no card is posted.
    pub(super) fn not_stopped(&self, id: i64) -> Result<(), String> {
        let (stopped, cancelled) = self.conv(id, |c| (c.stopped, c.cancel.is_cancelled()));
        if stopped || cancelled {
            Err(STOPPED.into())
        } else {
            Ok(())
        }
    }

    pub(super) fn act_tool(&self, id: i64, name: &str, args: &Value) -> Result<String, String> {
        let act = self.act_allowed(id, name)?;
        let mut plan = build_plan(name, args)?;
        self.not_stopped(id)?;
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
        self.note_key_text(id, &tab, &mut plan);
        let judged = self.evaluate(id, &plan, &read, files.as_deref())?;
        let (approval_id, decision) =
            match self.decide(id, &act, &plan, &read, &judged.ev, files.as_deref())? {
                Decided::Go(how) => (None, how),
                Decided::Asked { card, answer } => {
                    // The card may have waited minutes: the page can have changed meanwhile.
                    self.hold(id)?;
                    self.after_card(id, &tab, &plan, &read, &judged, &card, files.as_deref())?;
                    let granted = self
                        .with_trust(id, |state, _| state.commit(&judged.ev, answer))
                        .unwrap_or_default();
                    let how = if answer == Answer::AllowedTask && !granted.is_empty() {
                        AuditDecision::AskedAllowedTask
                    } else {
                        AuditDecision::AskedAllowedOnce
                    };
                    (Some(card), how)
                }
            };
        // Right before the command the approved files are copied into the gate's private folder
        // (the same files as were approved, checked on the open file) and `ardia` gets the copies,
        // never the originals: a file swapped after the approval does not reach the page.
        let staged = match &files {
            Some(files) => match self.stage_files(id, files) {
                Ok(staged) => Some(staged),
                Err(why) => {
                    self.row(
                        id,
                        &plan.tool,
                        &read.url,
                        &plan.summary,
                        self.is_user_tab(id, &tab),
                        Some(&judged.ev),
                        AuditDecision::Blocked,
                        &why,
                        approval_id.clone(),
                    );
                    return Err(format!("Not done: {why}. Nothing was uploaded."));
                }
            },
            None => None,
        };
        self.run_act(
            id,
            &plan,
            &read,
            &judged.ev,
            decision,
            approval_id,
            files.as_deref(),
            staged.as_deref(),
        )
    }

    // ---- the private copies of uploaded files ----

    /// The folder of a conversation's copies: `<staging>/<hash of the session name>`.
    fn staging_folder(&self, conversation: i64) -> Option<PathBuf> {
        let root = self.config().staging_dir?;
        Some(root.join(staging_name(&self.session(conversation))))
    }

    /// Copies each approved file into its own folder, `<staging>/<session hash>/<n>/<basename>`.
    fn stage_files(&self, id: i64, files: &[uploads::UploadFile]) -> Result<Vec<PathBuf>, String> {
        let folder = self
            .staging_folder(id)
            .ok_or("uploads are not available: this Mono has no private folder for the copies")?;
        let mut staged = Vec::new();
        for file in files {
            let n = self.conv(id, |c| {
                c.upload_seq += 1;
                c.upload_seq
            });
            // A Stop of the turn ends the copy of a large file.
            let stopped = || self.not_stopped(id).is_err();
            staged.push(file.stage(&folder.join(n.to_string()), &stopped)?);
        }
        Ok(staged)
    }

    /// The conversation ended: its private copies are deleted.
    pub(super) fn remove_staged(&self, conversation: i64) {
        if let Some(folder) = self.staging_folder(conversation) {
            let _ = std::fs::remove_dir_all(folder);
        }
    }

    /// After a card: the fresh read and the decision once more. The same destinations and
    /// categories and no new refusal, or the call fails as changed.
    #[allow(clippy::too_many_arguments)]
    fn after_card(
        &self,
        id: i64,
        tab: &str,
        plan: &Plan,
        read: &Read,
        judged: &Judged,
        card: &str,
        files: Option<&[uploads::UploadFile]>,
    ) -> Result<(), String> {
        let row = |why: &str| {
            self.row(
                id,
                &plan.tool,
                &read.url,
                &plan.summary,
                self.is_user_tab(id, tab),
                Some(&judged.ev),
                AuditDecision::Blocked,
                why,
                Some(card.to_owned()),
            )
        };
        self.not_stopped(id)
            .inspect_err(|_| row("stopped while the user was asked"))?;
        // The level may have been lowered while the card was open.
        if self
            .config()
            .act
            .is_none_or(|act| self.level_now(&act) != BrowserLevel::Act)
        {
            row("the browser level was changed to read only while the user was asked");
            return Err(format!(
                "{} is not available any more: the browser level was changed to read only while the user was asked, so nothing was done.",
                plan.tool
            ));
        }
        // Exactly one audit row for this act: the reads below stay quiet and the row above or
        // below is the one that is written.
        let again = self
            .fresh_read_rows(id, tab, plan, false)
            .inspect_err(|_| {
                row("the page could not be read again after the user answered");
            })?;
        let second = match self.evaluate_rows(id, plan, &again, files, false) {
            Ok(second) => second,
            Err(_) => {
                row("a new refusal appeared while the user was asked");
                return Err(CHANGED.to_owned());
            }
        };
        if second.sig != judged.sig {
            row("the page changed while the user was asked");
            return Err(CHANGED.to_owned());
        }
        Ok(())
    }

    /// Characters pressed one key at a time add up per tab, in memory only, so a card number typed
    /// digit by digit is still seen by the card check. Never written anywhere.
    fn note_key_text(&self, id: i64, tab: &str, plan: &mut Plan) {
        if !matches!(plan.op, Op::Press | Op::Type | Op::Fill) {
            return;
        }
        let text = (plan.op == Op::Press)
            .then(|| plan.key.as_deref().and_then(fresh::key_text))
            .flatten();
        let buffer = self.conv(id, |c| match text {
            Some(text) => {
                let buffer = c.key_text.entry(tab.to_owned()).or_default();
                buffer.push_str(&text);
                let extra = buffer.chars().count().saturating_sub(64);
                if extra > 0 {
                    *buffer = buffer.chars().skip(extra).collect();
                }
                Some(buffer.clone())
            }
            None => {
                c.key_text.remove(tab);
                None
            }
        });
        if buffer.is_some() {
            plan.typed = buffer;
        }
    }

    fn upload_files(&self, id: i64, plan: &Plan) -> Result<Vec<uploads::UploadFile>, String> {
        let config = self.config();
        if config.staging_dir.is_none() {
            let text = "browser_upload is not available: this Mono has no private folder for the copies of uploaded files.".to_owned();
            let page = self.conv(id, |c| c.page.clone());
            self.row(
                id,
                &plan.tool,
                &page,
                &plan.summary,
                false,
                None,
                AuditDecision::Blocked,
                &text,
                None,
            );
            return Err(text);
        }
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

    pub(super) fn url_now(&self, id: i64) -> Result<String, String> {
        let data = self
            .browse(id, &["get", "url"], &[])
            .map_err(|f| self.act_fail(id, f))?;
        Ok(data["url"].as_str().unwrap_or("").to_owned())
    }

    /// A failure before the command ran: a ref Ardia no longer knows is "the element changed".
    pub(super) fn act_fail(&self, id: i64, failure: Failure) -> String {
        if is_stale_code(failure.code()) {
            return CHANGED.to_owned();
        }
        self.fail(id, failure)
    }

    // ---- the decision ----

    pub(super) fn post_password_notice(&self, page: &str) {
        if let Some(act) = self.config().act {
            let site = registrable_domain_of(page).unwrap_or_default();
            act.queue
                .post_notice(NoticeKind::PasswordNeeded, None, &act.mono_label, &site);
        }
    }

    /// Allow, allow by grant, or ask. `Asked`: the card id and the answer (nothing is committed
    /// until the page was checked again).
    fn decide(
        &self,
        id: i64,
        act: &ActConfig,
        plan: &Plan,
        read: &Read,
        ev: &Evaluation,
        files: Option<&[uploads::UploadFile]>,
    ) -> Result<Decided, String> {
        match &ev.decision {
            Decision::Allow | Decision::AllowByGrant(_) => Ok(Decided::Go(AuditDecision::Auto)),
            Decision::Deny(reason) => Err(format!("Not allowed: {reason}.")),
            Decision::Ask(_) => {
                let facts = self.facts(act, plan, read, ev, files);
                let user_tab = self.is_user_tab(id, &read.tab);
                match self.ask_card(act, id, &plan.tool, facts, ev) {
                    Ok((answer, card)) => Ok(Decided::Asked { card, answer }),
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
                            Some(card).filter(|c| !c.is_empty()),
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
            Op::Click => ActionDesc::Click { label },
            Op::Check => ActionDesc::Check {
                label,
                on: !plan.uncheck,
            },
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
        facts.in_dialog = read.ctx.in_dialog || read.modal;
        facts.dialog_text = Some(clean(&read.ctx.dialog_text)).filter(|t| !t.is_empty());
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

    /// Opens a card and waits for the answer. Nothing is posted for a turn that was stopped, and
    /// the wait looks at the stop about five times a second. `Err`: the text for the Mono, how to
    /// audit it and the card id (empty when no card was posted).
    #[allow(clippy::type_complexity)]
    pub(super) fn ask_card(
        &self,
        act: &ActConfig,
        id: i64,
        tool: &str,
        facts: CardFacts,
        ev: &Evaluation,
    ) -> Result<(Answer, String), (String, AuditDecision, String)> {
        if self.not_stopped(id).is_err() {
            return Err((STOPPED.into(), AuditDecision::Stopped, String::new()));
        }
        // A card was shown in this call: a call that lost Ardia is not repeated (see `call_tool`).
        self.conv(id, |c| c.carded = true);
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
        let mut last_hold = Instant::now();
        let (card, outcome) = act.queue.ask(request, act.approval_timeout, STOP_POLL, || {
            if self.not_stopped(id).is_err() {
                // The Stop came after the card was posted: take the card back.
                act.queue.withdraw_session(id);
                return;
            }
            // The holds on the tabs last 2 minutes: a card can wait for longer.
            if last_hold.elapsed() >= KEEP_ALIVE {
                last_hold = Instant::now();
                let _ = self.hold(id);
            }
        });
        let stopped = self.not_stopped(id).is_err();
        match outcome {
            Outcome::Answered(_) if stopped => {
                Err((STOPPED.into(), AuditDecision::Stopped, card))
            }
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
            Outcome::Withdrawn if stopped => Err((STOPPED.into(), AuditDecision::Stopped, card)),
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
        decision: AuditDecision,
        approval_id: Option<String>,
        files: Option<&[uploads::UploadFile]>,
        staged: Option<&[PathBuf]>,
    ) -> Result<String, String> {
        let user_tab = self.is_user_tab(id, &read.tab);
        // An upload sends the private copies the gate made, never the paths the Mono named or
        // the originals.
        let mut values = plan.values.clone();
        if let Some(staged) = staged {
            values.truncate(1);
            values.extend(staged.iter().map(|p| p.to_string_lossy().into_owned()));
        }
        let mut cmd = plan.cmd.clone();
        // A link that opens a window of its own is not clicked: its address is loaded in the
        // session's own tab (the origin check of the address was part of the decision).
        let in_this_tab = read
            .new_window
            .as_ref()
            .filter(|_| plan.op == Op::Click || press_effect(plan, read) == PressEffect::Presses);
        if let Some(url) = in_this_tab {
            cmd = vec!["open"];
            values = vec![url.clone()];
        }
        let value_refs: Vec<&str> = values.iter().map(String::as_str).collect();
        let reason = ev.reason.clone();
        // What the act may do outside the tab (a new window, the window's profile).
        let watch = (in_this_tab.is_none()
            && matches!(
                plan.op,
                Op::Click | Op::Press | Op::Select | Op::Check | Op::Type | Op::Fill | Op::Hover
            ))
        .then(|| self.watch_before(id, &read.tab));
        let data = match self.browse(id, &cmd, &value_refs) {
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
                // A window the act may have opened is closed all the same, also after a Stop
                // (the cleanup ignores it), and what could not be undone is told.
                let cleanup = watch
                    .map(|watch| self.watch_after(id, &read.tab, watch))
                    .unwrap_or_default()
                    .lines();
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
                    format!("{STOPPED} The last action may or may not have run: look at the page before trying anything again.\n{cleanup}")
                } else {
                    format!("The result of this action is unknown: Ardia stopped answering after the command was sent, so it may or may not have run. It was NOT repeated. Look at the page (browser_snapshot) before trying again.\n{cleanup}")
                });
            }
        };

        let effects = watch
            .map(|watch| self.watch_after(id, &read.tab, watch))
            .unwrap_or_default();
        let changes = &data["changes"];
        // The URL after the action. A page that can not be read is a hit: where the tab is now is
        // not known, so it is stopped like a blocked page. When `get url` fails but the act's own
        // reply says where the tab went (`changes.url.to`), that address is used.
        let after = match self.url_now(id) {
            Ok(url) if !url.is_empty() => url,
            _ => match changes["url"]["to"].as_str().filter(|u| !u.is_empty()) {
                Some(reported) if self.not_stopped(id).is_ok() => reported.to_owned(),
                _ => {
                    self.row(
                        id,
                        &plan.tool,
                        &read.url,
                        &format!("{}; url after: unknown", plan.summary),
                        user_tab,
                        Some(ev),
                        decision,
                        &format!("{reason}; the page after the action could not be read"),
                        approval_id,
                    );
                    return Err(self.halt_unknown(id, &read.tab, &read.url, &plan.tool));
                }
            },
        };
        let title = changes["title"]["to"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| read.title.clone());
        let writes = fresh::writes_of(changes);
        // A reload of a page an act posted to may send the form again.
        self.conv(id, |c| {
            if writes.is_empty() {
                c.posted.remove(&read.tab);
            } else {
                c.posted.insert(read.tab.clone());
            }
        });
        let mut head = plan.summary.clone();
        if matches!(plan.op, Op::Type | Op::Fill) {
            head = head.replace(
                "field",
                &clean(&read.name).chars().take(60).collect::<String>(),
            );
        } else if !read.name.is_empty() {
            head = format!(
                "{head} \"{}\"",
                clean(&read.name).chars().take(80).collect::<String>()
            );
        }
        if let Some(files) = files {
            head = format!(
                "{head}: {}",
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
        // The free text is cut, never the list of requests the action caused.
        let mut summary = format!(
            "{}; url after {}",
            head.chars().take(100).collect::<String>(),
            audit_url(&after)
        );
        if !writes.is_empty() {
            summary = format!("{summary}; wrote: {}", writes.join(", "));
        }
        if in_this_tab.is_some() {
            summary = format!("{summary}; the link opens a new window: loaded in this tab");
        }
        if !effects.closed.is_empty() {
            summary = format!(
                "{summary}; closed {} new window(s) the page opened",
                effects.closed.len()
            );
        }
        if effects.profile_restored {
            summary = format!("{summary}; put the window back on its profile");
        }
        if effects.needs_attention() {
            summary = format!("{summary}; a window the page opened may have been left open");
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
            // A name that came from the focus eval is page text the Mono was never shown: it is
            // not echoed.
            if read.name.is_empty() || plan.ref_.is_none() {
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
        if let Some(url) = in_this_tab {
            body.push_str(&format!(
                "- That link opens in a new window, so zptah did not click it: it loaded {} in this same tab.\n",
                defense::redact_url_secrets(&one_line(url))
            ));
        }
        body.push_str(&effects.lines());
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
    /// time for that tab id in this conversation). It joins the conversation's set: never closed,
    /// only let go of.
    ///
    /// The tab is found without binding anything (`ardia call windows`: the active tab of the
    /// focused window), refused when it is not in the Mono's Ardia profile or shows a page this
    /// Mono may not use, shown to the user with its address, and only after the approval is that
    /// exact id bound (`tab <tab-id>`, checked against the id Ardia says it bound). The approval
    /// is recorded before the bind, so a retry after a failed bind asks nothing twice.
    pub(super) fn adopt_active(&self, id: i64, args: &Value) -> Result<String, String> {
        let act = self.act_allowed(id, "browser_tabs adopt_active")?;
        self.not_stopped(id)?;
        self.ready(id)?;
        let wanted = args["tab"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let windows = self
            .controller
            .cli()
            .call_without_params("windows")
            .map_err(|f| self.fail(id, f))?;
        let Some(active) = user_tab::active_tab(&windows) else {
            return Err("The user has no tab open in Ardia that can be used.".into());
        };
        let tab = active.id.clone();
        let url = active.url.clone();
        let title = clean(&active.title);
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
        // A tab zptah opened (for any conversation) is never the user's own.
        let zptah_tab = self
            .controller
            .sessions()
            .iter()
            .any(|s| self.controller.session_tabs(s).contains(&tab));
        if zptah_tab {
            self.audit(
                id,
                "tab_rejected",
                "",
                "adopt_active of a tab zptah opened for a conversation",
            );
            return Err(
                "The tab in front is one zptah opened for a conversation, not the user's own tab."
                    .into(),
            );
        }
        let profile = self.config().profile;
        if active.profile != profile {
            self.row(
                id,
                "browser_tabs",
                &url,
                "adopt the user's active tab",
                true,
                None,
                AuditDecision::Blocked,
                "the user's tab is not in this Mono's Ardia profile",
                None,
            );
            return Err("The tab the user has open now is not in the Ardia profile of this Mono, so it was not taken. Ask the user to open the page in that profile, or use browser_open.".into());
        }
        if url.trim().is_empty() {
            return Err("The user's tab shows no page yet, so it was not taken.".into());
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
        // Approved by tab id and the site it shows: the same tab on another site asks again.
        let approved_key = (
            tab.clone(),
            registrable_domain_of(&url).unwrap_or_else(|| url.clone()),
        );
        if !self.conv(id, |c| c.approved_user_tabs.contains(&approved_key)) {
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
                Ok((_, card)) => {
                    // Recorded before anything else can fail, so a retry does not ask again.
                    self.conv(id, |c| c.approved_user_tabs.insert(approved_key.clone()));
                    approval = Some(card);
                }
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
                        Some(card).filter(|c| !c.is_empty()),
                    );
                    return Err(text);
                }
            }
        }
        self.not_stopped(id)?;
        // Only now is that exact id bound, and the reply must say it is the one that was bound.
        let bound = self.bind(id, &tab)?;
        // The user handed this tab over: its site is part of the task.
        self.note_user_domain(id, &url);
        self.conv(id, |c| {
            c.user_tab = Some(tab.clone());
            c.approved_user_tabs.insert(approved_key.clone());
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
    /// classifier (an address outside the task, a sensitive site) and asked when it says so. Only
    /// at the act level.
    pub(super) fn user_tab_navigation(&self, id: i64, url: &str, tab: &str) -> Result<(), String> {
        self.act_allowed(id, "browser_navigate")?;
        let _ = tab;
        self.judge_navigation(id, "browser_navigate", &json!({"url": url}), url, true)
    }

    /// The decision for an address the Mono moves a tab to (`browser_navigate`, `browser_open`
    /// as the extension's new tab): the shared classifier with the target address, so an address
    /// outside the task (UnrelatedDomain), one that carries what the Mono read on another site
    /// (Tainted) and the first entry to a sensitive site ask, at every level, exactly as the
    /// extension backend did. A Mono with no act settings loaded has no card to show and nothing
    /// to judge with: the move is refused. `Err` is the text for the Mono (audited).
    pub(super) fn judge_navigation(
        &self,
        id: i64,
        tool: &str,
        args: &Value,
        url: &str,
        user_tab: bool,
    ) -> Result<(), String> {
        let Some(act) = self.config().act else {
            return Err(NO_ACT.into());
        };
        let page = self.conv(id, |c| c.page.clone());
        let mut action = TrustAction::new(tool, args, &page);
        action.target_url = Some(url);
        let Some(ev) = self.with_trust(id, |state, cfg| {
            state.evaluate(
                &TrustConfig {
                    level: self.level_now(cfg),
                    rules: &cfg.rules,
                },
                &action,
            )
        }) else {
            return Err(NO_ACT.into());
        };
        let summary = if user_tab {
            format!("navigate the user's tab to {}", audit_url(url))
        } else {
            format!("{} {}", tool.trim_start_matches("browser_"), audit_url(url))
        };
        match &ev.decision {
            Decision::Deny(reason) => {
                self.row(
                    id,
                    tool,
                    &page,
                    &summary,
                    user_tab,
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
                facts.purpose = args["purpose"]
                    .as_str()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned);
                match self.ask_card(&act, id, tool, facts, &ev) {
                    Ok((answer, card)) => {
                        let granted = self
                            .with_trust(id, |state, _| state.commit(&ev, answer))
                            .unwrap_or_default();
                        let how = if answer == Answer::AllowedTask && !granted.is_empty() {
                            AuditDecision::AskedAllowedTask
                        } else {
                            AuditDecision::AskedAllowedOnce
                        };
                        self.row(
                            id,
                            tool,
                            &page,
                            &summary,
                            user_tab,
                            Some(&ev),
                            how,
                            &ev.reason,
                            Some(card),
                        );
                        Ok(())
                    }
                    Err((text, audit, card)) => {
                        self.row(
                            id,
                            tool,
                            &page,
                            &summary,
                            user_tab,
                            Some(&ev),
                            audit,
                            &text,
                            Some(card).filter(|c| !c.is_empty()),
                        );
                        Err(text)
                    }
                }
            }
            Decision::Allow | Decision::AllowByGrant(_) => Ok(()),
        }
    }

    /// The first read of a sensitive site (a bank, a mail or payment page ...) asks once, as for
    /// the extension. `tool` is the read tool (`browser_snapshot`, `browser_read`,
    /// `browser_take_screenshot`) and `url` the page it read; `Err` withholds what was read.
    pub(super) fn guard_first_read(&self, id: i64, tool: &str, url: &str) -> Result<(), String> {
        let Some(act) = self.config().act else {
            return Err(format!("{NO_ACT} What was read was withheld."));
        };
        let args = json!({});
        let action = TrustAction::new(tool, &args, url);
        let Some(ev) = self.with_trust(id, |state, cfg| {
            state.evaluate(
                &TrustConfig {
                    level: self.level_now(cfg),
                    rules: &cfg.rules,
                },
                &action,
            )
        }) else {
            return Err(format!("{NO_ACT} What was read was withheld."));
        };
        let user_tab = self.conv(id, |c| c.bound.is_some() && c.user_tab == c.bound);
        let summary = format!("first read of {}", audit_url(url));
        match &ev.decision {
            Decision::Allow | Decision::AllowByGrant(_) => Ok(()),
            Decision::Deny(reason) => {
                self.row(
                    id,
                    tool,
                    url,
                    &summary,
                    user_tab,
                    Some(&ev),
                    AuditDecision::Blocked,
                    reason,
                    None,
                );
                Err(format!(
                    "Not allowed: {reason}. What was read was withheld."
                ))
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
                match self.ask_card(&act, id, tool, facts, &ev) {
                    Ok((answer, card)) => {
                        self.with_trust(id, |state, _| state.commit(&ev, answer));
                        self.row(
                            id,
                            tool,
                            url,
                            &summary,
                            user_tab,
                            Some(&ev),
                            AuditDecision::AskedAllowedOnce,
                            &ev.reason,
                            Some(card),
                        );
                        Ok(())
                    }
                    Err((text, audit, card)) => {
                        self.row(
                            id,
                            tool,
                            url,
                            &summary,
                            user_tab,
                            Some(&ev),
                            audit,
                            &text,
                            Some(card).filter(|c| !c.is_empty()),
                        );
                        Err(format!("{text} What was read was withheld."))
                    }
                }
            }
        }
    }

    /// A reload of a tab whose last act sent a POST may send the form again: it asks first (an
    /// unclear step, never granted for the conversation).
    pub(super) fn ask_before_reload(&self, id: i64) -> Result<(), String> {
        let Some(tab) = self.conv(id, |c| c.bound.clone()) else {
            return Ok(());
        };
        if !self.conv(id, |c| c.posted.contains(&tab)) {
            return Ok(());
        }
        let Some(act) = self.config().act else {
            return Err(NO_ACT.into());
        };
        let page = self.conv(id, |c| c.page.clone());
        let user_tab = self.is_user_tab(id, &tab);
        let ev = Evaluation {
            decision: Decision::Ask(vec![Category::Unclear]),
            tier: Tier::Act,
            categories: [Category::Unclear].into_iter().collect(),
            site: registrable_domain_of(&page).unwrap_or_default(),
            reason: "reloading may send the form again".into(),
        };
        let mut facts = CardFacts::new(
            &act.mono_label,
            &ev.site,
            ActionDesc::Reload,
            vec![Category::Unclear],
        );
        facts.url = page.clone();
        facts.site_kind = site_category(&page);
        let summary = format!("reload {}", audit_url(&page));
        match self.ask_card(&act, id, "browser_reload", facts, &ev) {
            Ok((_, card)) => {
                self.conv(id, |c| c.posted.remove(&tab));
                self.row(
                    id,
                    "browser_reload",
                    &page,
                    &summary,
                    user_tab,
                    Some(&ev),
                    AuditDecision::AskedAllowedOnce,
                    &ev.reason,
                    Some(card),
                );
                Ok(())
            }
            Err((text, audit, card)) => {
                self.row(
                    id,
                    "browser_reload",
                    &page,
                    &summary,
                    user_tab,
                    Some(&ev),
                    audit,
                    &text,
                    Some(card).filter(|c| !c.is_empty()),
                );
                Err(text)
            }
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
                        level: self.level_now(cfg),
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
                    Some(card).filter(|c| !c.is_empty()),
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
