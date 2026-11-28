//! The Chief in the app: its hidden chat, the dispatcher for the app calls that chat makes, and
//! the reads it is allowed (`chief.overview`, `chief.readSession`, `chief.commands`, its own
//! soul and memory). The rules of what a proposal is live in `monocode_core::mono::chief`.
//!
//! A call from the Chief's chat reaches [`Monos::chief_call`] and nothing else: it never goes
//! through `app_caller`, so no Mono action is reachable from it, and `chief.*` is refused to
//! every other caller (`app_caller`, the Operator's `validate_operator`).
use super::{Monos, host::text};
use crate::{conversation::Conversation, runtime::locked};
use gpui::{AppContext as _, Context, Entity};
use monocode_core::{
    agent::{AccessMode, Settings},
    mono::{
        self, Message, Roster,
        bridge::Call,
        browser::defense,
        chief::{self, Owners},
        files, memory,
    },
    provider::Kind,
    transcript::Row,
};
use serde_json::{Value, json};
use std::collections::HashMap;

/// What a Mono or the Operator is told when it calls a `chief.*` action.
pub(super) const NOT_A_CHIEF: &str = "chief.* actions are only available in the Chief's own chat";
pub(super) const CHIEF_CHAT_REFUSED: &str = "The Chief's chat is not a session you can use";
pub(super) const MONO_CHAT_REFUSED: &str =
    "A Mono's chat cannot be sent to or stopped through sessions.*";
const OFF: &str = "The Chief is turned off";

/// The only actions the Chief's chat may call.
pub(crate) const ALLOWED: [&str; 13] = [
    "chief.overview",
    "chief.readSession",
    "chief.commands",
    "chief.propose",
    "chief.cancel",
    "chief.proposeStart",
    "chief.digest",
    "memory.read",
    "memory.search",
    "memory.add",
    "memory.replace",
    "memory.remove",
    "soul.read",
];

/// How far back the overview looks unless a `query` is given.
const WINDOW_DAYS: u64 = 7;
const DAY_MS: u64 = 86_400_000;
const EXCHANGES: usize = 3;
const EXCHANGE_CHARS: usize = 4000;
const LAST_LINE_CHARS: usize = 200;
/// The most sessions one `chief.digest` lists, and how long a session's last line may be.
pub(super) const DIGEST_SESSIONS: usize = 60;
const DIGEST_LINE_CHARS: usize = 400;

// ---- What the shell tells the overview ----

/// One session of the shell's catalog (see `Shell::chief_catalog`).
#[derive(Clone)]
pub(crate) struct SessionFact {
    pub id: i64,
    pub title: String,
    pub provider: String,
    pub cwd: String,
    pub updated_at: u64,
    /// The project's path, and the name the sidebar shows for it.
    pub project: String,
    pub project_name: String,
    pub live: Option<crate::mobile::catalog::LiveSession>,
    /// The `target` and `cloudState` the phone's card would carry, for a cloud session.
    pub cloud: Option<Value>,
}

pub(crate) struct Catalog {
    /// The session open in the main window.
    pub focused: Option<i64>,
    pub projects: Vec<(String, String)>,
    pub sessions: Vec<SessionFact>,
}

/// A Mono as the overview lists it.
#[derive(Clone)]
pub(super) struct MonoFact {
    pub id: String,
    pub name: String,
    pub projects: Vec<String>,
    pub status: String,
    pub last_line: String,
    /// The Mono's chat session, read from saved history when its chat is not open.
    pub session: Option<i64>,
    pub open: bool,
}

/// A session with the facts only the store and git know.
struct Detail {
    fact: SessionFact,
    model: Option<String>,
    branch: String,
}

fn fenced(text: &str, nonce: &str) -> Value {
    json!(chief::fence_untrusted(text, nonce))
}

/// A project path or name, fenced but never masked (see `chief::fence_path`).
fn fenced_path(text: &str, nonce: &str) -> Value {
    json!(chief::fence_path(text, nonce))
}

fn last_line(rows: &[Row]) -> String {
    last_prose_line(rows, LAST_LINE_CHARS)
}

/// The first line of the last assistant prose, clipped to `max` characters.
fn last_prose_line(rows: &[Row], max: usize) -> String {
    rows.iter()
        .rev()
        .find(|row| row.role == "assistant" && !row.text.trim().is_empty())
        .and_then(|row| row.text.lines().find(|line| !line.trim().is_empty()))
        .map(|line| line.trim().chars().take(max).collect())
        .unwrap_or_default()
}

/// The last `max` exchanges of prose: what the owner wrote and what the session answered.
pub(super) fn last_exchanges(rows: &[Row], max: usize) -> Vec<(String, String)> {
    let mut exchanges: Vec<(String, String)> = Vec::new();
    for row in rows {
        if row.is_prompt() {
            if row.first || exchanges.is_empty() {
                exchanges.push((row.text.clone(), String::new()));
            } else if let Some(last) = exchanges.last_mut() {
                last.0.push_str(&row.text);
            }
        } else if row.role == "assistant"
            && let Some(last) = exchanges.last_mut()
        {
            if row.first {
                last.1 = row.text.clone();
            } else {
                last.1.push_str(&row.text);
            }
        }
    }
    let skip = exchanges.len().saturating_sub(max);
    exchanges.split_off(skip)
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

// ---- The overview ----

/// The overview of `input`, shaped from the facts. Pure: the caller read the shell, the store
/// and git. Every string that came from a session or a Mono is fenced under `nonce`.
pub(super) fn overview(
    input: &Value,
    catalog: Catalog,
    details: Vec<(SessionFact, Option<String>, String)>,
    monos: &[MonoFact],
    roster: &Roster,
    now: u64,
    nonce: &str,
) -> Result<Value, String> {
    let details: Vec<Detail> = details
        .into_iter()
        .map(|(fact, model, branch)| Detail {
            fact,
            model,
            branch,
        })
        .collect();
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(chief::OVERVIEW_DEFAULT, |n| n as usize)
        .clamp(1, chief::OVERVIEW_MAX);
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    // The project the owner named, as a path: a rail project's path or name, else the text.
    let project = input.get("project").and_then(Value::as_str).map(|wanted| {
        let wanted = &chief::unfence(wanted);
        let known = catalog.projects.iter().find(|(path, name)| {
            monocode_core::files::path_key(path) == monocode_core::files::path_key(wanted)
                || name.eq_ignore_ascii_case(wanted)
        });
        monocode_core::files::path_key(known.map_or(wanted, |(path, _)| path.as_str()))
    });
    let mono_filter = input.get("mono").and_then(Value::as_str);
    let status = input.get("status").and_then(Value::as_str);
    if let Some(status) = status
        && !matches!(status, "busy" | "idle" | "waiting")
    {
        return Err("status is busy, idle or waiting".into());
    }
    if let Some(id) = mono_filter
        && roster.get(id).is_none()
    {
        return Err("Unknown Mono".into());
    }
    // A search reaches back as far as it needs; a plain look covers the week.
    let window = query
        .is_none()
        .then(|| now.saturating_sub(WINDOW_DAYS * DAY_MS));
    let mut matched: Vec<&Detail> = details
        .iter()
        .filter(|d| window.is_none_or(|from| d.fact.updated_at >= from))
        .filter(|d| {
            project
                .as_deref()
                .is_none_or(|wanted| monocode_core::files::path_key(&d.fact.project) == wanted)
        })
        .filter(|d| {
            mono_filter.is_none_or(|id| match chief::owners_of(roster, d.fact.id) {
                Owners::None => false,
                Owners::One(owner) => owner == id,
                Owners::Many(owners) => owners.iter().any(|owner| owner == id),
            })
        })
        .filter(|d| match status {
            None => true,
            Some("busy") => d.fact.live.as_ref().is_some_and(|l| l.busy),
            Some("waiting") => d.fact.live.as_ref().is_some_and(|l| l.waiting.is_some()),
            Some(_) => d
                .fact
                .live
                .as_ref()
                .is_none_or(|l| !l.busy && l.waiting.is_none()),
        })
        .filter(|d| {
            query.as_deref().is_none_or(|q| {
                [
                    &d.fact.title,
                    &d.branch,
                    &d.fact.project_name,
                    &d.fact.project,
                ]
                .iter()
                .any(|field| field.to_lowercase().contains(q))
            })
        })
        .collect();
    matched.sort_by_key(|d| std::cmp::Reverse(d.fact.updated_at));
    let total = matched.len();
    matched.truncate(limit);
    let mut projects: Vec<Value> = Vec::new();
    let order: Vec<&String> = catalog.projects.iter().map(|(path, _)| path).collect();
    let mut paths: Vec<&str> = Vec::new();
    for detail in &matched {
        if !paths.contains(&detail.fact.project.as_str()) {
            paths.push(&detail.fact.project);
        }
    }
    paths.sort_by_key(|path| order.iter().position(|known| known.as_str() == *path));
    for path in paths {
        let sessions: Vec<Value> = matched
            .iter()
            .filter(|d| d.fact.project == path)
            .map(|d| session_value(d, roster, nonce))
            .collect();
        let name = matched
            .iter()
            .find(|d| d.fact.project == path)
            .map(|d| d.fact.project_name.as_str())
            .unwrap_or_default();
        projects.push(
            json!({"path":fenced_path(path, nonce),"name":fenced(name, nonce),"sessions":sessions}),
        );
    }
    let monos: Vec<Value> = monos
        .iter()
        .filter(|m| mono_filter.is_none_or(|id| id == m.id))
        .filter(|m| {
            project.as_deref().is_none_or(|wanted| {
                m.projects
                    .iter()
                    .any(|mp| monocode_core::files::path_key(mp) == wanted)
            })
        })
        .map(|m| {
            json!({
                "id":m.id,
                "name":fenced(&m.name, nonce),
                "status":m.status,
                "projects":m.projects.iter().map(|p| fenced_path(p, nonce)).collect::<Vec<_>>(),
                "lastLine":fenced(&m.last_line, nonce),
            })
        })
        .collect();
    Ok(json!({
        "focusedSessionId":catalog.focused,
        "windowDays":window.map(|_| WINDOW_DAYS),
        "projects":projects,
        "monos":monos,
        "total":total,
        "truncated":total.saturating_sub(limit),
        "busyNote":"busy is exact only where busyKnown is true; a session not open in the app may still be working",
    }))
}

fn session_value(d: &Detail, roster: &Roster, nonce: &str) -> Value {
    let fact = &d.fact;
    let kind = Kind::from_id(&fact.provider);
    let live = fact.live.as_ref();
    let mut value = json!({
        "id":fact.id,
        "title":fenced(monocode_core::session_title::display_title(&fact.title, &fact.provider), nonce),
        "harness":fact.provider,
        "model":crate::mobile::catalog::model_id(kind, d.model.as_deref()),
        "busy":live.is_some_and(|l| l.busy),
        "busyKnown":live.is_some_and(|l| l.loaded),
        "waiting":live.and_then(|l| l.waiting),
        "updatedAt":fact.updated_at,
    });
    if !d.branch.is_empty() {
        value["branch"] = fenced(&d.branch, nonce);
    }
    if let Some(cloud) = &fact.cloud {
        for key in ["target", "cloudState"] {
            if let Some(field) = cloud.get(key) {
                value[key] = field.clone();
            }
        }
    }
    let owners: Vec<String> = match chief::owners_of(roster, fact.id) {
        Owners::None => Vec::new(),
        Owners::One(id) => vec![id],
        Owners::Many(ids) => ids,
    };
    value["owners"] = owners
        .iter()
        .filter_map(|id| roster.get(id))
        .map(|m| json!({"id":m.id,"name":fenced(&m.label(), nonce)}))
        .collect();
    value
}

// ---- The digest ----

/// The start of today in local time, in epoch ms.
fn start_of_today(now: u64) -> u64 {
    use chrono::{Local, TimeZone};
    let local = chrono::DateTime::from_timestamp_millis(now as i64)
        .map(|t| t.with_timezone(&Local))
        .unwrap_or_else(Local::now);
    local
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|midnight| Local.from_local_datetime(&midnight).earliest())
        .map_or(now.saturating_sub(DAY_MS), |t| t.timestamp_millis() as u64)
}

/// The window start of `chief.digest`: `since` as epoch ms, an ISO time (a zone, else local
/// time) or a date, or the start of today in local time when it is absent.
pub(super) fn digest_since(input: &Value, now: u64) -> Result<u64, String> {
    use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
    let Some(text) = input.get("since").and_then(Value::as_str) else {
        return Ok(start_of_today(now));
    };
    let text = text.trim();
    let bad = || "since is an ISO time (2026-11-27T09:00:00) or epoch milliseconds".to_owned();
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return text.parse().map_err(|_| bad());
    }
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
        return u64::try_from(time.timestamp_millis()).map_err(|_| bad());
    }
    let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M"))
        .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S"))
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
        })
        .ok_or_else(bad)?;
    let local = Local
        .from_local_datetime(&naive)
        .earliest()
        .ok_or_else(bad)?;
    u64::try_from(local.timestamp_millis()).map_err(|_| bad())
}

/// The sessions a digest lists: updated since `since`, newest first, at most
/// [`DIGEST_SESSIONS`]. The second value is how many more matched.
pub(super) fn digest_sessions(sessions: &[SessionFact], since: u64) -> (Vec<SessionFact>, usize) {
    let mut matched: Vec<SessionFact> = sessions
        .iter()
        .filter(|s| s.updated_at >= since)
        .cloned()
        .collect();
    matched.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    let truncated = matched.len().saturating_sub(DIGEST_SESSIONS);
    matched.truncate(DIGEST_SESSIONS);
    (matched, truncated)
}

fn state_name(live: Option<&crate::mobile::catalog::LiveSession>) -> &'static str {
    match live {
        Some(l) if l.waiting.is_some() => "waiting",
        Some(l) if l.busy => "busy",
        _ => "idle",
    }
}

fn counts(states: &[&str]) -> Value {
    let of = |name: &str| states.iter().filter(|s| **s == name).count();
    json!({"sessions":states.len(),"busy":of("busy"),"waiting":of("waiting"),"idle":of("idle")})
}

/// `chief.digest`: what happened since `since`, per project and per owning Mono. `details` are
/// the facts of the sessions [`digest_sessions`] picked, with each one's last prose line (read
/// from its saved history). Pure: every string that came from a session or a Mono is fenced
/// under `nonce`, and the Chief's proposals in the window carry their state only.
#[allow(clippy::too_many_arguments)]
pub(super) fn digest(
    since: u64,
    now: u64,
    catalog: &Catalog,
    details: Vec<(SessionFact, Option<String>, String, String)>,
    truncated: usize,
    roster: &Roster,
    nonce: &str,
) -> Value {
    let order: Vec<&String> = catalog.projects.iter().map(|(path, _)| path).collect();
    let mut details = details;
    details.sort_by_key(|(fact, ..)| std::cmp::Reverse(fact.updated_at));
    let mut paths: Vec<&str> = Vec::new();
    for (fact, ..) in &details {
        if !paths.contains(&fact.project.as_str()) {
            paths.push(&fact.project);
        }
    }
    paths.sort_by_key(|path| order.iter().position(|known| known.as_str() == *path));
    let listed = |fact: &SessionFact, model: &Option<String>, branch: &String, line: &String| {
        let detail = Detail {
            fact: fact.clone(),
            model: model.clone(),
            branch: branch.clone(),
        };
        let mut value = session_value(&detail, roster, nonce);
        value["state"] = json!(state_name(fact.live.as_ref()));
        value["lastLine"] = fenced(&clip(line, DIGEST_LINE_CHARS), nonce);
        value
    };
    let projects: Vec<Value> = paths
        .iter()
        .map(|path| {
            let rows: Vec<&(SessionFact, Option<String>, String, String)> = details
                .iter()
                .filter(|(f, ..)| f.project == *path)
                .collect();
            let states: Vec<&str> = rows
                .iter()
                .map(|(f, ..)| state_name(f.live.as_ref()))
                .collect();
            let name = rows.first().map(|(f, ..)| f.project_name.as_str());
            json!({
                "path":fenced_path(path, nonce),
                "name":fenced(name.unwrap_or_default(), nonce),
                "counts":counts(&states),
                "sessions":rows.iter().map(|(f, m, b, l)| listed(f, m, b, l)).collect::<Vec<_>>(),
            })
        })
        .collect();
    let owned_by = |mono: &str, fact: &SessionFact| match chief::owners_of(roster, fact.id) {
        Owners::None => false,
        Owners::One(owner) => owner == mono,
        Owners::Many(owners) => owners.iter().any(|o| o == mono),
    };
    let mut monos: Vec<Value> = roster
        .monos
        .iter()
        .filter_map(|mono| {
            let rows: Vec<&SessionFact> = details
                .iter()
                .map(|(f, ..)| f)
                .filter(|f| owned_by(&mono.id, f))
                .collect();
            if rows.is_empty() {
                return None;
            }
            let states: Vec<&str> = rows.iter().map(|f| state_name(f.live.as_ref())).collect();
            Some(json!({
                "id":mono.id,
                "name":fenced(&mono.label(), nonce),
                "counts":counts(&states),
                "sessionIds":rows.iter().map(|f| f.id).collect::<Vec<_>>(),
            }))
        })
        .collect();
    let unowned: Vec<&SessionFact> = details
        .iter()
        .map(|(f, ..)| f)
        .filter(|f| matches!(chief::owners_of(roster, f.id), Owners::None))
        .collect();
    if !unowned.is_empty() {
        let states: Vec<&str> = unowned
            .iter()
            .map(|f| state_name(f.live.as_ref()))
            .collect();
        monos.push(json!({
            "id":Value::Null,
            "name":"No Mono",
            "counts":counts(&states),
            "sessionIds":unowned.iter().map(|f| f.id).collect::<Vec<_>>(),
        }));
    }
    let states: Vec<&str> = details
        .iter()
        .map(|(f, ..)| state_name(f.live.as_ref()))
        .collect();
    let proposals: Vec<Value> = roster
        .chief
        .as_ref()
        .map(|c| c.proposals.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|p| p.confirmed_at.unwrap_or(p.created_at) >= since || p.created_at >= since)
        .map(|p| {
            json!({
                "id":p.id,
                "kind":p.kind,
                "state":proposal_state_name(&p.state),
                "createdAt":p.created_at,
            })
        })
        .collect();
    json!({
        "since":since,
        "now":now,
        "counts":counts(&states),
        "projects":projects,
        "monos":monos,
        "chiefProposals":proposals,
        "truncated":truncated,
        "busyNote":"busy is exact only where busyKnown is true; a session not open in the app may still be working",
    })
}

/// A proposal's state as a name, without the reason a refusal or failure carries.
fn proposal_state_name(state: &chief::ProposalState) -> String {
    match serde_json::to_value(state) {
        Ok(Value::String(name)) => name,
        Ok(Value::Object(map)) => map.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

/// A proposal's state, with the reason of a refusal or failure fenced: it can hold a Mono's or a
/// session's words.
fn state_value(state: &chief::ProposalState, nonce: &str) -> Value {
    match state {
        chief::ProposalState::Refused(reason) => json!({"refused":fenced(reason, nonce)}),
        chief::ProposalState::Failed(reason) => json!({"failed":fenced(reason, nonce)}),
        other => json!(other),
    }
}

/// A proposal's kind, with the project path of a start fenced.
fn kind_value(kind: &chief::ProposalKind, nonce: &str) -> Value {
    match kind {
        chief::ProposalKind::Start { project, title } => json!({
            "type":"start",
            "project":fenced_path(project, nonce),
            "title":title.as_deref().map(|t| fenced(t, nonce)),
        }),
        other => json!(other),
    }
}

/// `chief.commands`: the proposals, newest first.
pub(super) fn commands(input: &Value, chief: Option<&chief::Chief>, nonce: &str) -> Value {
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(20, |n| n as usize);
    let proposals: Vec<Value> = chief
        .map(|c| c.proposals.iter().rev().take(limit).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            json!({
                "id":p.id,
                "state":state_value(&p.state, nonce),
                "route":p.route,
                "kind":kind_value(&p.kind, nonce),
                "sessionId":p.session_id,
                "sessionTitle":fenced(&p.session_title, nonce),
                "project":fenced_path(&p.project, nonce),
                "branch":p.branch.as_deref().map(|b| fenced(b, nonce)),
                "surface":p.surface,
                "createdAt":p.created_at,
                "confirmedAt":p.confirmed_at,
                // The owner's own confirmed text.
                "instruction":p.instruction,
                // A Mono's words, never the Chief's to obey.
                "monoReply":p.mono_reply.as_deref().map(|r| fenced(r, nonce)),
            })
        })
        .collect();
    json!({"proposals":proposals})
}

/// `chief.readSession`: the last exchanges of one session, clipped and fenced.
pub(super) fn read_value(
    session: i64,
    title: &str,
    provider: &str,
    rows: &[Row],
    nonce: &str,
) -> Value {
    let exchanges: Vec<Value> = last_exchanges(rows, EXCHANGES)
        .into_iter()
        .map(|(user, assistant)| {
            json!({
                "user":fenced(&clip(&user, EXCHANGE_CHARS), nonce),
                "assistant":fenced(&clip(&assistant, EXCHANGE_CHARS), nonce),
            })
        })
        .collect();
    json!({
        "sessionId":session,
        "title":fenced(monocode_core::session_title::display_title(title, provider), nonce),
        "harness":provider,
        "exchanges":exchanges,
    })
}

impl Monos {
    pub(crate) fn chief_session(&self) -> Option<i64> {
        self.roster.chief.as_ref().and_then(|c| c.session_id)
    }
    pub(crate) fn is_chief_session(&self, id: i64) -> bool {
        self.chief_session() == Some(id)
    }
    /// The folder the Chief's own folder lives in: the app data folder, next to `monos/`.
    pub(crate) fn chief_root(&self) -> std::path::PathBuf {
        self.root
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone())
    }
    /// The harness the Chief's chat runs on: its own choice, else Claude Code, the only harness
    /// it can run on (`provider::CHIEF_HARNESS_REFUSAL`).
    pub(crate) fn chief_kind(&self) -> Kind {
        self.roster
            .chief
            .as_ref()
            .and_then(|c| c.harness.as_deref())
            .and_then(Kind::parse)
            .unwrap_or(Kind::Claude)
    }
    pub(crate) fn chief_chat(&self) -> Option<&Entity<Conversation>> {
        self.chief_chat.as_ref()
    }
    /// Changes the Chief's settings, saves them and makes sure its chat follows.
    pub(crate) fn update_chief(
        &mut self,
        change: impl FnOnce(&mut chief::Chief),
        cx: &mut Context<Self>,
    ) {
        let chief = self.roster.chief.get_or_insert_with(Default::default);
        change(chief);
        chief.normalize();
        self.save(cx);
        self.chief_changed(cx);
    }
    /// The Chief's settings changed: open its chat when it is on, and show its current look.
    pub(crate) fn chief_changed(&mut self, cx: &mut Context<Self>) {
        self.chief_status = None;
        self.chief_notice = None;
        // The Monos' rules carry the Chief section only while it is on.
        if self.chief_rules_applied != self.chief_enabled() {
            self.chief_rules_applied = self.chief_enabled();
            let open: Vec<String> = self.chats.keys().cloned().collect();
            for id in open {
                self.configure_chat_with(&id, false, cx);
            }
        }
        if !self.roster.chief.as_ref().is_some_and(|c| c.enabled) {
            if self.chief_active() {
                self.close(cx);
            }
            self.changed(cx);
            self.chief_sync_cards(cx);
            return;
        }
        // A chat that already runs stays as it is, but the owner is told why the Chief cannot
        // answer on a harness it cannot be limited on.
        if self.chief_kind() != Kind::Claude {
            self.chief_status = Some(monocode_core::provider::CHIEF_HARNESS_REFUSAL.into());
        }
        self.ensure_chief_chat(cx);
        self.configure_chief_chat(cx);
        self.chief_sync_cards(cx);
        self.changed(cx);
    }
    /// Applies the settings to the open chat as a Mono's settings changes are applied: its look,
    /// model and effort now (the next turn reads them), its account once the chat is idle.
    fn configure_chief_chat(&mut self, cx: &mut Context<Self>) {
        let (Some(chief), Some(view)) = (self.roster.chief.clone(), self.chief_chat.clone()) else {
            return;
        };
        let env = self
            .bridge
            .as_ref()
            .map(|b| b.environment(view.read(cx).summary.id))
            .unwrap_or_default();
        let root = self.chief_root();
        let default_model = self.prefs.default_models.get(Kind::Claude.id()).cloned();
        let claude = self.chief_kind() == Kind::Claude;
        view.update(cx, |view, cx| {
            view.configure_chief(&chief, root, env, cx);
            if claude {
                view.chief_apply_settings(&chief, default_model, cx);
            }
        });
        self.chief_reconcile_account(cx);
    }
    /// Moves the chat to the account the Chief names, when it is idle. A busy chat is moved by
    /// the next `chief_chat_changed` that finds it settled.
    fn chief_reconcile_account(&mut self, cx: &mut Context<Self>) {
        let (Some(wanted), Some(view)) = (
            self.roster.chief.as_ref().and_then(|c| c.account.clone()),
            self.chief_chat.clone(),
        ) else {
            return;
        };
        let (current, settled) = {
            let view = view.read(cx);
            (view.settings.account.id.clone(), view.mono_settled())
        };
        if current == wanted || !settled {
            return;
        }
        if let Some(center) = crate::accounts::get(cx) {
            let profile = center.read(cx).profile(&wanted);
            view.update(cx, |view, cx| view.switch_account(profile, cx));
        }
    }
    /// Opens the Chief's hidden chat, creating it on first use. Does nothing while it is off,
    /// already open or opening. A harness the Chief cannot be limited on is refused here with
    /// a status instead of a chat (`provider::CHIEF_HARNESS_REFUSAL`).
    pub(crate) fn ensure_chief_chat(&mut self, cx: &mut Context<Self>) {
        let Some(chief) = self.roster.chief.clone().filter(|c| c.enabled) else {
            return;
        };
        if self.chief_chat.is_some() || self.chief_loading {
            return;
        }
        let kind = self.chief_kind();
        if kind != Kind::Claude {
            self.chief_status = Some(monocode_core::provider::CHIEF_HARNESS_REFUSAL.into());
            self.changed(cx);
            return;
        }
        if let Some(missing) = &chief.unavailable_session {
            self.chief_status = Some(format!(
                "The Chief's saved conversation ({missing}) is unavailable. Its settings and history are kept."
            ));
            self.changed(cx);
            return;
        }
        self.chief_loading = true;
        let store = self.store.clone();
        let default_model = self.prefs.default_models.get(kind.id()).cloned();
        let account = chief
            .account
            .as_deref()
            .and_then(|id| crate::accounts::get(cx).map(|center| center.read(cx).profile(id)));
        let work = cx.background_executor().spawn(async move {
            let db = locked(&store)?;
            if let Some(session) = chief.session_id {
                let summary = db.mono_session(session)?.ok_or(
                    "The Chief's saved conversation is unavailable. Turn the Chief off and on to start a new one.",
                )?;
                return Ok::<_, String>((summary, None));
            }
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_else(|_| ".".into());
            let mut summary = db.create(kind.id(), &home)?;
            summary.title = chief.label();
            db.title(summary.id, &summary.title)?;
            // A chat that answers at once: low effort unless the owner chose otherwise. No
            // tool needs approval: the run has only the app tool, and anything else asks the owner.
            let mut settings = Settings {
                access: AccessMode::Supervised,
                model: chief.model.clone().or(default_model),
                effort: Some(chief.effort.clone().unwrap_or_else(|| "low".into())),
                ..Default::default()
            };
            if let Some(account) = account {
                settings.account = account;
            }
            db.configure_agent(summary.id, kind.id(), &settings)?;
            Ok((summary, Some(settings)))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.chief_loading = false;
                let (summary, settings) = match result {
                    Ok(value) => value,
                    Err(error) => {
                        this.chief_status = Some(error);
                        this.changed(cx);
                        return;
                    }
                };
                if !this.roster.chief.as_ref().is_some_and(|c| c.enabled) {
                    return;
                }
                if !this.roster.hidden_sessions.contains(&summary.id) {
                    this.roster.hidden_sessions.push(summary.id);
                }
                if let Some(chief) = this.roster.chief.as_mut() {
                    chief.session_id = Some(summary.id);
                }
                let chief = this.roster.chief.clone().unwrap_or_default();
                let view = cx.new(|cx| Conversation::new(summary, this.store.clone(), cx));
                let root = this.chief_root();
                let env = this
                    .bridge
                    .as_ref()
                    .map(|b| b.environment(view.read(cx).summary.id))
                    .unwrap_or_default();
                view.update(cx, |view, cx| {
                    view.configure_chief(&chief, root, env, cx);
                    if let Some(mut settings) = settings {
                        // A new chat was saved without an account; keep the one
                        // `Conversation::new` chose unless the Chief names its own.
                        if settings.account.id == monocode_core::accounts::DEFAULT {
                            settings.account = view.settings.account.clone();
                        }
                        // Whatever an older chat saved: the Chief always asks.
                        settings.access = AccessMode::Supervised;
                        let config = view.settings.mono.take();
                        let env = std::mem::take(&mut view.settings.environment);
                        view.install_transfer(settings, None, cx);
                        view.settings.mono = config;
                        view.settings.environment = env;
                    }
                    view.load(cx);
                });
                this.observations.push(cx.subscribe(
                    &view,
                    move |this, _, _: &crate::conversation::SessionChanged, cx| {
                        this.chief_chat_changed(cx);
                    },
                ));
                this.chief_attach(&view, cx);
                this.chief_chat = Some(view);
                this.save(cx);
                this.chief_sync_cards(cx);
                this.changed(cx);
            });
        })
        .detach();
    }
    /// The Chief's chat changed (a turn started, streamed or finished). The cards are not
    /// recomputed here: their notes read every session, which a streaming reply must not pay
    /// for on each update. They follow the proposals, the owner's clicks and the 10 s clock.
    pub(crate) fn chief_chat_changed(&mut self, cx: &mut Context<Self>) {
        self.chief_reconcile_account(cx);
        cx.notify();
    }
    /// Sends the owner's text to the Chief's chat: it starts a turn when the chat is idle.
    pub(crate) fn chief_send(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.roster.chief.as_ref().is_some_and(|c| c.enabled) {
            return Err(OFF.into());
        }
        if self.chief_kind() != Kind::Claude {
            return Err(monocode_core::provider::CHIEF_HARNESS_REFUSAL.into());
        }
        let Some(view) = self.chief_chat.clone() else {
            return Err(self
                .chief_status
                .clone()
                .unwrap_or_else(|| "The Chief's chat is not open yet".into()));
        };
        let message = Message {
            id: mono::new_id(),
            text,
            sent_at: Some(monocode_core::time::now_ms()),
            internal: false,
            error: None,
            attachments: Vec::new(),
        };
        if view.update(cx, |view, cx| view.mono_dispatch(&message, cx)) {
            Ok(())
        } else {
            Err("The Chief is busy or not ready; try again in a moment".into())
        }
    }

    // ---- The dispatcher ----

    /// Whether a call from the Chief's chat may run: only its own actions, and only while it
    /// is on. Everything else, every Mono action included, is refused here.
    pub(super) fn chief_caller(&self, call: &Call) -> Result<(), String> {
        if !self.roster.chief.as_ref().is_some_and(|c| c.enabled) {
            return Err(OFF.into());
        }
        if !ALLOWED.contains(&call.action.as_str()) {
            return Err(format!(
                "The Chief cannot use {}. It can use chief.overview, chief.readSession, chief.commands, chief.digest, chief.propose, chief.proposeStart, chief.cancel, memory.* and soul.read",
                call.action
            ));
        }
        Ok(())
    }

    /// Runs a call that passed [`Monos::chief_caller`] and the receipt checks.
    pub(super) fn chief_call(
        &mut self,
        call: Call,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if !call.input.is_object() {
            self.finish_call(call, Err("Input must be a JSON object".into()), cx);
            return;
        }
        let nonce = defense::new_nonce();
        match call.action.as_str() {
            "chief.overview" => self.chief_overview(call, nonce, cx),
            "chief.readSession" => self.chief_read_session(call, nonce, cx),
            "chief.commands" => {
                let value = commands(&call.input, self.roster.chief.as_ref(), &nonce);
                self.finish_call(call, Ok(value), cx);
            }
            "chief.propose" => self.chief_propose(call, cx),
            "chief.cancel" => self.chief_cancel_call(call, cx),
            "chief.proposeStart" => self.chief_propose_start(call, cx),
            "chief.digest" => self.chief_overview(call, nonce, cx),
            "soul.read" | "memory.read" | "memory.search" | "memory.add" | "memory.replace"
            | "memory.remove" => self.chief_files_call(call, cx),
            _ => {
                let error = format!("The Chief cannot use {}", call.action);
                self.finish_call(call, Err(error), cx);
            }
        }
    }

    /// The Chief's own soul and memory, in its folder next to `monos/`.
    fn chief_files_call(&mut self, call: Call, cx: &mut Context<Self>) {
        let root = self.chief_root();
        let action = call.action.clone();
        let input = call.input.clone();
        self.call_work(
            call,
            chief::FILES_ID.into(),
            move |_| {
                let target = chief::FILES_ID;
                let value = match action.as_str() {
                    "soul.read" => json!(files::read(&root, target, None, "SOUL.md")?),
                    "memory.read" => json!(memory::read(
                        &root,
                        target,
                        input.get("topic").and_then(Value::as_str)
                    )?),
                    "memory.search" => json!(memory::search(
                        &root,
                        target,
                        &text(&input, "query", 4000)?,
                        input.get("since").and_then(Value::as_str)
                    )?),
                    _ => {
                        let entry = if action == "memory.remove" {
                            None
                        } else {
                            Some(memory::entry(
                                &text(&input, "fact", 4000)?,
                                &memory::date(),
                                input.get("until").and_then(Value::as_str),
                            )?)
                        };
                        let find = input
                            .get("find")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let hash = memory::edit(
                            &root,
                            target,
                            input.get("topic").and_then(Value::as_str),
                            |old| {
                                if action == "memory.add" {
                                    Ok(memory::add(old, entry.as_deref().unwrap()))
                                } else {
                                    memory::replace(old, find, entry.as_deref(), &memory::date())
                                }
                            },
                        )?;
                        json!({"hash":hash})
                    }
                };
                Ok((value, super::host::Effect::None))
            },
            cx,
        );
    }

    /// The shell is read outside this update: its catalog reads the Monos, which are being
    /// updated while a call runs.
    fn chief_overview(&mut self, call: Call, nonce: String, cx: &mut Context<Self>) {
        let owner = self.owner.clone();
        cx.spawn(async move |this, cx| {
            let catalog = owner.read_with(cx, |shell, cx| shell.chief_catalog(cx));
            let _ = this.update(cx, |this, cx| match catalog {
                Ok(catalog) if call.action == "chief.digest" => {
                    this.chief_digest_with(call, nonce, catalog, cx)
                }
                Ok(catalog) => this.chief_overview_with(call, nonce, catalog, cx),
                Err(_) => this.finish_call(call, Err("The app is closing".into()), cx),
            });
        })
        .detach();
    }

    fn chief_overview_with(
        &mut self,
        call: Call,
        nonce: String,
        catalog: Catalog,
        cx: &mut Context<Self>,
    ) {
        let mut monos: Vec<MonoFact> = self
            .roster
            .monos
            .iter()
            .map(|mono| {
                let chat = self.chats.get(&mono.id).map(|c| c.read(cx));
                MonoFact {
                    id: mono.id.clone(),
                    name: mono.label(),
                    projects: mono.projects.clone(),
                    status: chat.map_or("idle", |c| c.mono_status()).to_owned(),
                    last_line: chat.map(|c| last_line(c.mono_rows(cx))).unwrap_or_default(),
                    session: mono.session_id,
                    open: chat.is_some(),
                }
            })
            .collect();
        let roster = self.roster.clone();
        let store = self.store.clone();
        let input = call.input.clone();
        let work = cx.background_executor().spawn(async move {
            let (saved, mut monos) = {
                let db = locked(&store)?;
                let saved = db.mobile_session_metadata()?;
                // A Mono whose chat is not open still has a last line in its saved history.
                for mono in monos.iter_mut().filter(|m| !m.open) {
                    if let Some(session) = mono.session
                        && let Ok(history) = db.load(session)
                    {
                        mono.last_line = last_line(&history.rows);
                    }
                }
                (saved, monos)
            };
            let mut branches = HashMap::new();
            let now = monocode_core::time::now_ms();
            let details = catalog
                .sessions
                .iter()
                .map(|fact| {
                    let metadata = crate::mobile::catalog::metadata(
                        saved.get(&fact.id).unwrap_or(&Default::default()),
                        fact.live.as_ref(),
                    );
                    let branch = metadata
                        .branch
                        .clone()
                        .filter(|b| !b.is_empty() && !metadata.worktree_removed)
                        .unwrap_or_else(|| {
                            if metadata.worktree_removed {
                                String::new()
                            } else {
                                crate::mobile::catalog::checkout_branch(
                                    metadata.worktree_cwd.as_deref().unwrap_or(&fact.cwd),
                                    &mut branches,
                                )
                            }
                        });
                    (fact.clone(), metadata.model.clone(), branch)
                })
                .collect();
            monos.truncate(64);
            overview(&input, catalog, details, &monos, &roster, now, &nonce)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| this.finish_call(call, result, cx));
        })
        .detach();
    }

    /// `chief.digest`: the sessions updated in the window are picked first, then their branch
    /// and last prose line are read off the UI thread.
    fn chief_digest_with(
        &mut self,
        call: Call,
        nonce: String,
        catalog: Catalog,
        cx: &mut Context<Self>,
    ) {
        let now = monocode_core::time::now_ms();
        let since = match digest_since(&call.input, now) {
            Ok(since) => since,
            Err(error) => {
                self.finish_call(call, Err(error), cx);
                return;
            }
        };
        let roster = self.roster.clone();
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let (picked, truncated) = digest_sessions(&catalog.sessions, since);
            let (saved, mut lines) = {
                let db = locked(&store)?;
                let lines: HashMap<i64, String> = picked
                    .iter()
                    .filter_map(|fact| {
                        let history = db.load(fact.id).ok()?;
                        Some((fact.id, last_prose_line(&history.rows, DIGEST_LINE_CHARS)))
                    })
                    .collect();
                (db.mobile_session_metadata()?, lines)
            };
            let mut branches = HashMap::new();
            let details = picked
                .into_iter()
                .map(|fact| {
                    let metadata = crate::mobile::catalog::metadata(
                        saved.get(&fact.id).unwrap_or(&Default::default()),
                        fact.live.as_ref(),
                    );
                    let branch = metadata
                        .branch
                        .clone()
                        .filter(|b| !b.is_empty() && !metadata.worktree_removed)
                        .unwrap_or_else(|| {
                            if metadata.worktree_removed {
                                String::new()
                            } else {
                                crate::mobile::catalog::checkout_branch(
                                    metadata.worktree_cwd.as_deref().unwrap_or(&fact.cwd),
                                    &mut branches,
                                )
                            }
                        });
                    let line = lines.remove(&fact.id).unwrap_or_default();
                    (fact, metadata.model.clone(), branch, line)
                })
                .collect();
            Ok::<_, String>(digest(
                since, now, &catalog, details, truncated, &roster, &nonce,
            ))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| this.finish_call(call, result, cx));
        })
        .detach();
    }

    fn chief_read_session(&mut self, call: Call, nonce: String, cx: &mut Context<Self>) {
        let session = match super::host::id(&call.input, "sessionId") {
            Ok(id) => id,
            Err(error) => {
                self.finish_call(call, Err(error), cx);
                return;
            }
        };
        // Never the Chief's own chat, a Mono's chat, a habit run, a workflow question or a worker.
        if self.is_chief_session(session) {
            self.finish_call(call, Err(CHIEF_CHAT_REFUSED.into()), cx);
            return;
        }
        // The sessions a Mono launched are hidden from the sidebar but are exactly what the Chief
        // reports on, so only `unmanaged_session` (Mono chats, habit runs, workflow questions and
        // workers) refuses here, as the propose check does.
        if self.is_mono_session(session) || self.unmanaged_session(session, 0, cx) {
            self.finish_call(
                call,
                Err("This session is not one the Chief can read".into()),
                cx,
            );
            return;
        }
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let db = locked(&store)?;
            let summary = db.mono_session(session)?.ok_or("Session not found")?;
            let history = db.load(session)?;
            Ok::<_, String>(read_value(
                session,
                &summary.title,
                &summary.provider,
                &history.rows,
                &nonce,
            ))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| this.finish_call(call, result, cx));
        })
        .detach();
    }
}
