//! Commanding a session through the Chief: the proposals the Chief makes, the owner's click that
//! confirms, edits or cancels one, the check on the Mono's proposal-bound `sessions.send`, and
//! how a proposal's state follows the Mono's turn and the session's queue afterwards.
//!
//! The authority is the app's: the proposal record decides, never model text. The rules of a
//! proposal (states, the bound-send check, the audit line) live in `monocode_core::mono::chief`.
use super::{Monos, host};
use crate::conversation::FollowUpStatus;
use crate::runtime::locked;
use gpui::{Context, EventEmitter};
use monocode_core::mono::{
    self, Mono,
    browser::defense,
    chief::{self, Owners, Proposal, ProposalKind, ProposalState, Route, Surface},
    cloud_api,
};
use serde_json::{Value, json};
use std::collections::HashSet;

/// The Chief's proposals changed (a new card, a click, a state that moved on). The desktop
/// card list and the web push observe it; `Changed` and a notify follow every save as well.
pub(crate) struct ChiefProposalsChanged;
impl EventEmitter<ChiefProposalsChanged> for Monos {}

/// What the owner's Mono reply and a failure reason keep, before the proposal clips them.
const REPLY_KEPT: usize = 2000;

/// The facts about a target session a proposal and its confirmation both check.
struct Target {
    title: String,
    cwd: String,
    /// The local project the session belongs to (a cloud session's twin).
    project: String,
    branch: Option<String>,
    busy: bool,
    cloud: bool,
}

/// What the owning Monos say about the route.
enum Routing {
    Route(Route),
    NeedsMono(Vec<String>),
}

fn basename(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_owned()
}

/// The known project `wanted` names: its path, else its name (the sidebar's or the folder's),
/// when exactly one project has it. Returns the project's path and the name the card shows.
fn resolve_known_project(
    known: &[(String, String)],
    wanted: &str,
) -> Result<(String, String), String> {
    let key = monocode_core::files::path_key(wanted);
    if let Some((path, name)) = known
        .iter()
        .find(|(path, _)| monocode_core::files::path_key(path) == key)
    {
        return Ok((path.clone(), name.clone()));
    }
    let named: Vec<&(String, String)> = known
        .iter()
        .filter(|(path, name)| {
            name.eq_ignore_ascii_case(wanted) || basename(path).eq_ignore_ascii_case(wanted)
        })
        .collect();
    match named.as_slice() {
        [(path, name)] => Ok((path.clone(), name.clone())),
        [] => Err(
            "Unknown project. Use chief.overview to see the projects, or give its full path".into(),
        ),
        _ => Err("Several projects have that name. Give the project's full path".into()),
    }
}

/// A session title the Chief suggested, as one clean line.
fn clean_title(text: &str) -> Option<String> {
    let line = defense::strip_invisible(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let line: String = line.chars().take(160).collect();
    (!line.is_empty()).then_some(line)
}

impl Monos {
    // ---- What the UI reads ----

    /// Every proposal, oldest first.
    pub(crate) fn chief_proposals(&self) -> &[Proposal] {
        self.roster
            .chief
            .as_ref()
            .map_or(&[][..], |c| c.proposals.as_slice())
    }
    /// How many proposals wait for the owner's click.
    pub(crate) fn chief_open_count(&self) -> usize {
        self.chief_proposals()
            .iter()
            .filter(|p| p.is_open())
            .count()
    }
    /// What a card warns about, read live: `sessionBusy` (the command waits in the session's
    /// queue), `monoBusy` (it waits for the Mono's turn), `queuePaused` (the session's queue is
    /// paused, so a queued command waits for the owner) and `cloudAsleep` (the sandbox is not
    /// connected; a send wakes it, which costs money).
    pub(crate) fn chief_card_notes(
        &self,
        proposal: &Proposal,
        cx: &gpui::App,
    ) -> Vec<&'static str> {
        if matches!(proposal.kind, ProposalKind::Start { .. }) {
            return self.chief_notes_for(proposal, None, cx);
        }
        match proposal
            .session_id
            .and_then(|s| self.chief_target(s, cx).ok())
        {
            Some(target) => self.chief_notes_for(proposal, Some(&target), cx),
            None => Vec::new(),
        }
    }

    // ---- Recording ----

    /// Applies `change` to a copy of the proposal and keeps it only when it succeeds; then the
    /// record goes to the audit log, the roster is saved and the UI is told.
    fn chief_commit(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut Proposal) -> Result<(), String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let proposal = self
            .roster
            .chief
            .as_mut()
            .and_then(|c| c.proposal_mut(id))
            .ok_or("Unknown proposal")?;
        let mut next = proposal.clone();
        change(&mut next)?;
        *proposal = next.clone();
        self.chief_recorded(&next, cx);
        Ok(())
    }
    fn chief_recorded(&mut self, proposal: &Proposal, cx: &mut Context<Self>) {
        if let Err(error) = chief::append_audit(&chief::dir(&self.chief_root()), proposal) {
            self.error = Some(format!("Could not write the Chief's command log: {error}"));
        }
        self.save(cx);
        cx.emit(ChiefProposalsChanged);
        self.chief_sync_cards(cx);
        self.chief_delivered(proposal, cx);
    }

    /// A proposal reached `Sent` (on either route, at once or from a queue): with "notify me when
    /// a command is delivered" on, post one notice, once per proposal. The shell posts it, in a
    /// deferred task, since this can run inside a shell update.
    fn chief_delivered(&mut self, proposal: &Proposal, cx: &mut Context<Self>) {
        if proposal.state != ProposalState::Sent {
            return;
        }
        let Some(chief) = self.roster.chief.as_ref().filter(|c| c.notify_on_delivery) else {
            return;
        };
        let Some(session) = proposal.session_id else {
            return;
        };
        if !self.chief_notified.insert(proposal.id.clone()) {
            return;
        }
        let name = chief.label();
        let body = format!("Sent to #{session} {}", proposal.session_title.trim());
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |shell, cx| {
                shell.deliver_chief_notice(session, &name, body.trim(), cx)
            });
        });
    }

    // ---- Checking a target and its route ----

    /// The session, if the Chief may command it: it exists, is not archived and is neither a
    /// Mono's chat, the Chief's chat, a habit run, a workflow question nor a worker.
    fn chief_target(&self, session: i64, cx: &gpui::App) -> Result<Target, String> {
        if self.is_chief_session(session) {
            return Err(super::chief::CHIEF_CHAT_REFUSED.into());
        }
        if self.is_mono_session(session) || self.unmanaged_session(session, 0, cx) {
            return Err("The Chief cannot command this session".into());
        }
        let owner = self.owner.upgrade().ok_or("The app is closing")?;
        let views = owner.read(cx).mono_session_views(cx);
        let view = views
            .iter()
            .find(|v| v.id == session)
            .ok_or("Session not found")?;
        if view.archived {
            return Err("This session is archived".into());
        }
        Ok(Target {
            title: view.title.clone(),
            cwd: view.cwd.clone(),
            project: host::launch_project(&self.store, session, &view.cwd),
            branch: view.branch.clone(),
            busy: view.busy,
            cloud: view.cloud,
        })
    }

    /// Where a command for `session` goes, from the Monos that own it. `wanted` is the Mono the
    /// owner picked when several own it.
    fn chief_routing(&self, session: i64, wanted: Option<&str>) -> Result<Routing, String> {
        match chief::owners_of(&self.roster, session) {
            Owners::One(owner) => match wanted {
                Some(w) if w != owner => Err("That Mono does not own this session".into()),
                _ => Ok(Routing::Route(Route::ViaMono { mono_id: owner })),
            },
            Owners::Many(owners) => match wanted {
                Some(w) if owners.iter().any(|o| o == w) => {
                    Ok(Routing::Route(Route::ViaMono { mono_id: w.into() }))
                }
                Some(_) => Err("That Mono is not one of the owners of this session".into()),
                None => Ok(Routing::NeedsMono(owners)),
            },
            Owners::None => match wanted {
                Some(_) => Err("No Mono owns this session; leave mono out".into()),
                None => Ok(Routing::Route(Route::Direct)),
            },
        }
    }

    /// The Mono can take the command: its chat is available and the session is in its projects
    /// (or is the cloud twin of one).
    fn chief_check_mono(&self, mono_id: &str, session: i64, cwd: &str) -> Result<(), String> {
        let mono: &Mono = self
            .roster
            .get(mono_id)
            .ok_or("That Mono no longer exists")?;
        if mono.unavailable_session.is_some() {
            return Err(format!(
                "{}'s chat is unavailable, so it cannot take a command",
                mono.label()
            ));
        }
        let key = monocode_core::files::path_key(cwd);
        let inside = mono.projects.iter().any(|project| {
            monocode_core::files::path_key(project) == key
                || (cwd.starts_with("remote://")
                    && locked(&self.store).is_ok_and(|store| {
                        cloud_api::is_cloud_session_of(&store, session, project)
                    }))
        });
        if !inside {
            return Err(format!(
                "This session is outside {}'s projects",
                mono.label()
            ));
        }
        Ok(())
    }

    // ---- chief.propose and chief.cancel ----

    pub(super) fn chief_propose(&mut self, call: mono::bridge::Call, cx: &mut Context<Self>) {
        let result = self.chief_propose_now(&call.input, cx);
        self.finish_call(call, result, cx);
    }

    fn chief_propose_now(
        &mut self,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let session = host::id(input, "sessionId")?;
        let instruction = input
            .get("instruction")
            .and_then(Value::as_str)
            .ok_or("Provide instruction")?;
        let wanted = input.get("mono").and_then(Value::as_str);
        chief::can_open(self.chief_proposals())?;
        let target = self.chief_target(session, cx)?;
        let route = match self.chief_routing(session, wanted)? {
            Routing::NeedsMono(owners) => {
                let nonce = defense::new_nonce();
                let candidates: Vec<Value> = owners
                    .iter()
                    .filter_map(|id| self.roster.get(id))
                    .map(|m| json!({"id":m.id,"name":chief::fence_untrusted(&m.label(), &nonce)}))
                    .collect();
                return Ok(json!({"status":"needsMono","candidates":candidates}));
            }
            Routing::Route(route) => route,
        };
        if let Route::ViaMono { mono_id } = &route {
            self.chief_check_mono(mono_id, session, &target.cwd)?;
        }
        let proposal = Proposal::new(
            monocode_core::time::now_ms(),
            session,
            &target.title,
            &basename(&target.project),
            target.branch.clone().filter(|b| !b.is_empty()),
            route.clone(),
            instruction,
            Surface::Desktop,
        )?;
        let notes = self.chief_notes_for(&proposal, Some(&target), cx);
        let id = proposal.id.clone();
        self.roster
            .chief
            .as_mut()
            .ok_or("The Chief is turned off")?
            .add_proposal(proposal.clone())?;
        self.chief_recorded(&proposal, cx);
        Ok(json!({"proposalId":id,"status":"awaitingOwner","route":route,"notes":notes}))
    }

    // ---- chief.proposeStart ----

    /// The project `wanted` names among the projects the app knows. Reads the shell.
    fn chief_resolve_project(
        &self,
        wanted: &str,
        cx: &gpui::App,
    ) -> Result<(String, String), String> {
        // The value may come back as a list fenced it.
        let wanted = chief::unfence(wanted);
        let wanted = wanted.as_str();
        let owner = self.owner.upgrade().ok_or("The app is closing")?;
        let monos = self
            .roster
            .monos
            .iter()
            .flat_map(|m| m.projects.clone())
            .collect();
        resolve_known_project(&owner.read(cx).chief_known_projects(monos), wanted)
    }

    /// Where new work in `project` goes, from the Monos whose projects cover it. `wanted` is the
    /// Mono the owner (or the Chief) picked when several cover it.
    fn chief_start_routing(&self, project: &str, wanted: Option<&str>) -> Result<Routing, String> {
        let covering: Vec<String> = self
            .roster
            .monos
            .iter()
            .filter(|m| m.works_on(project))
            .map(|m| m.id.clone())
            .collect();
        match (covering.len(), wanted) {
            (0, Some(_)) => Err("No Mono covers this project; leave mono out".into()),
            (0, None) => Ok(Routing::Route(Route::Direct)),
            (_, Some(w)) if covering.iter().any(|id| id == w) => {
                Ok(Routing::Route(Route::ViaMono { mono_id: w.into() }))
            }
            (_, Some(_)) => Err("That Mono does not cover this project".into()),
            (1, None) => Ok(Routing::Route(Route::ViaMono {
                mono_id: covering[0].clone(),
            })),
            (_, None) => Ok(Routing::NeedsMono(covering)),
        }
    }

    /// The Mono can take a start: it exists, its chat is available and `project` is one of its
    /// projects.
    fn chief_check_start_mono(&self, mono_id: &str, project: &str) -> Result<(), String> {
        let mono: &Mono = self
            .roster
            .get(mono_id)
            .ok_or("That Mono no longer exists")?;
        if mono.unavailable_session.is_some() {
            return Err(format!(
                "{}'s chat is unavailable, so it cannot take a command",
                mono.label()
            ));
        }
        if !mono.works_on(project) {
            return Err(format!(
                "This project is outside {}'s projects",
                mono.label()
            ));
        }
        Ok(())
    }

    pub(super) fn chief_propose_start(&mut self, call: mono::bridge::Call, cx: &mut Context<Self>) {
        let result = self.chief_propose_start_now(&call.input, cx);
        self.finish_call(call, result, cx);
    }

    fn chief_propose_start_now(
        &mut self,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let project = input
            .get("project")
            .and_then(Value::as_str)
            .ok_or("Provide project")?;
        let instruction = input
            .get("instruction")
            .and_then(Value::as_str)
            .ok_or("Provide instruction")?;
        let wanted = input.get("mono").and_then(Value::as_str);
        let title = input
            .get("title")
            .and_then(Value::as_str)
            .and_then(clean_title);
        chief::can_open(self.chief_proposals())?;
        let (path, name) = self.chief_resolve_project(project, cx)?;
        let route = match self.chief_start_routing(&path, wanted)? {
            Routing::NeedsMono(covering) => {
                let nonce = defense::new_nonce();
                let candidates: Vec<Value> = covering
                    .iter()
                    .filter_map(|id| self.roster.get(id))
                    .map(|m| json!({"id":m.id,"name":chief::fence_untrusted(&m.label(), &nonce)}))
                    .collect();
                return Ok(json!({"status":"needsMono","candidates":candidates}));
            }
            Routing::Route(route) => route,
        };
        if let Route::ViaMono { mono_id } = &route {
            self.chief_check_start_mono(mono_id, &path)?;
        }
        let proposal = Proposal::new_start(
            monocode_core::time::now_ms(),
            &path,
            &name,
            title,
            route.clone(),
            instruction,
            Surface::Desktop,
        )?;
        let notes = self.chief_notes_for(&proposal, None, cx);
        let id = proposal.id.clone();
        self.roster
            .chief
            .as_mut()
            .ok_or("The Chief is turned off")?
            .add_proposal(proposal.clone())?;
        self.chief_recorded(&proposal, cx);
        Ok(json!({"proposalId":id,"status":"awaitingOwner","route":route,"notes":notes}))
    }

    fn chief_notes_for(
        &self,
        proposal: &Proposal,
        target: Option<&Target>,
        cx: &gpui::App,
    ) -> Vec<&'static str> {
        let mut notes = Vec::new();
        if target.is_some_and(|t| t.busy) {
            notes.push("sessionBusy");
        }
        if let Route::ViaMono { mono_id } = &proposal.route
            && self.chats.get(mono_id).is_some_and(|c| c.read(cx).busy)
        {
            notes.push("monoBusy");
        }
        if target.is_some_and(|t| t.cloud)
            && proposal
                .session_id
                .and_then(|s| self.owner.upgrade()?.read(cx).mono_session_entity(s))
                .is_none_or(|view| view.read(cx).chief_cloud_asleep())
        {
            notes.push("cloudAsleep");
        }
        if proposal
            .session_id
            .and_then(|s| self.owner.upgrade()?.read(cx).mono_session_entity(s))
            .is_some_and(|view| view.read(cx).chief_queue_paused())
        {
            notes.push("queuePaused");
        }
        notes
    }

    pub(super) fn chief_cancel_call(&mut self, call: mono::bridge::Call, cx: &mut Context<Self>) {
        let result = (|| {
            let id = call
                .input
                .get("proposalId")
                .and_then(Value::as_str)
                .filter(|id| chief::valid_proposal_id(id))
                .ok_or("proposalId must be a proposal id (p_...)")?
                .to_owned();
            self.chief_cancel(&id, cx)?;
            Ok(json!({"proposalId":id,"status":"cancelled"}))
        })();
        self.finish_call(call, result, cx);
    }

    // ---- The owner's click ----

    /// Cancel on a card.
    pub(crate) fn chief_cancel(&mut self, id: &str, cx: &mut Context<Self>) -> Result<(), String> {
        self.chief_commit(id, |p| p.cancel(), cx)
    }
    /// Edit on a card: the owner changes the text before sending.
    pub(crate) fn chief_edit(
        &mut self,
        id: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.chief_commit(id, |p| p.edit(text), cx)
    }

    /// Send on a card, from `surface`. The target and the route are checked again (the world may
    /// have moved since the proposal): a changed route is refused with its reason and nothing is
    /// sent, the card stays for the owner to cancel. Then:
    /// - through a Mono: the command goes into that Mono's outbox (state `QueuedAtMono`);
    /// - direct: the text goes into the session as a web send does, now when it is idle, else
    ///   into its follow-up queue (`Sent` or `QueuedInSession`). A cloud session that is not
    ///   connected is woken, as any send wakes it.
    ///
    /// `edited_instruction` replaces the text first, as Edit then Send would. Call it from a
    /// click handler or a deferred task, never from inside an update of the `Shell`.
    pub(crate) fn chief_confirm(
        &mut self,
        id: &str,
        surface: Surface,
        edited_instruction: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let chief = self
            .roster
            .chief
            .as_ref()
            .ok_or("The Chief is turned off")?;
        let mut proposal = chief.proposal(id).cloned().ok_or("Unknown proposal")?;
        if !proposal.is_open() {
            return Err("This command was already handled".into());
        }
        if surface == Surface::Web && !chief.allow_web_send {
            return Err("Sending from the web is turned off in the app".into());
        }
        if let Some(text) = &edited_instruction {
            proposal.edit(text)?;
        }
        if let ProposalKind::Start { project, title } = proposal.kind.clone() {
            return self.chief_confirm_start(proposal, &project, title, surface, cx);
        }
        let session = proposal.session_id.ok_or("This proposal has no session")?;
        let target = self.chief_target(session, cx)?;
        let routing = self.chief_routing(
            session,
            match &proposal.route {
                Route::ViaMono { mono_id } => Some(mono_id.as_str()),
                Route::Direct => None,
            },
        );
        // Same owner as when it was proposed, or the proposal no longer describes the world.
        let changed = "Who owns this session changed. Cancel this card and ask the Chief again";
        match (&proposal.route, routing) {
            (Route::Direct, Ok(Routing::Route(Route::Direct))) => {}
            (Route::ViaMono { mono_id }, Ok(Routing::Route(Route::ViaMono { mono_id: now })))
                if *mono_id == now => {}
            _ => return Err(changed.into()),
        }
        proposal.surface = surface;
        let now = monocode_core::time::now_ms();
        match proposal.route.clone() {
            Route::ViaMono { mono_id } => {
                self.chief_check_mono(&mono_id, session, &target.cwd)?;
                let outbox = self
                    .enqueue(
                        &mono_id,
                        chief::command_text(&proposal),
                        Vec::new(),
                        true,
                        cx,
                    )
                    .ok_or("That Mono no longer exists")?;
                proposal.confirm_via_mono(now, outbox)?;
            }
            Route::Direct => {
                let view = self
                    .owner
                    .upgrade()
                    .and_then(|o| o.update(cx, |o, cx| o.mono_ensure_session(session, cx)))
                    .ok_or("Session is no longer available")?;
                let text = proposal.instruction.clone();
                let queued = view.update(cx, |v, cx| v.chief_direct(text, cx))?;
                // A stopped workspace is woken as any send wakes it (the card's note warned it
                // costs money); the queued command flushes once it is back.
                if queued.is_some() && view.read(cx).chief_cloud_asleep() {
                    let store = self.store.clone();
                    let view = view.clone();
                    cx.spawn(async move |_, cx| {
                        crate::cloud_wake::wake_for_send(store, session, view, cx).await;
                    })
                    .detach();
                }
                proposal.confirm_direct(now, queued)?;
            }
        }
        self.chief_store(proposal, cx);
        Ok(())
    }

    /// Keeps a proposal the owner's click changed, records it and tells the UI.
    fn chief_store(&mut self, proposal: Proposal, cx: &mut Context<Self>) {
        let kept = proposal.clone();
        if let Some(slot) = self
            .roster
            .chief
            .as_mut()
            .and_then(|c| c.proposal_mut(&proposal.id))
        {
            *slot = proposal;
        }
        self.chief_recorded(&kept, cx);
    }

    /// Send on a [`ProposalKind::Start`] card. The project and the route are checked again:
    /// through a Mono the start text goes into its outbox (the Mono starts the session with a
    /// proposal-bound `sessions.start`); with no Mono covering the project the app starts the
    /// session itself, with the owner's defaults and never with full access, and sends the text
    /// as its first prompt.
    fn chief_confirm_start(
        &mut self,
        mut proposal: Proposal,
        project: &str,
        title: Option<String>,
        surface: Surface,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let (path, _) = self.chief_resolve_project(project, cx)?;
        let routing = self.chief_start_routing(
            &path,
            match &proposal.route {
                Route::ViaMono { mono_id } => Some(mono_id.as_str()),
                Route::Direct => None,
            },
        );
        let changed =
            "Which Mono covers this project changed. Cancel this card and ask the Chief again";
        match (&proposal.route, routing) {
            (Route::Direct, Ok(Routing::Route(Route::Direct))) => {}
            (Route::ViaMono { mono_id }, Ok(Routing::Route(Route::ViaMono { mono_id: now })))
                if *mono_id == now => {}
            _ => return Err(changed.into()),
        }
        proposal.surface = surface;
        let now = monocode_core::time::now_ms();
        match proposal.route.clone() {
            Route::ViaMono { mono_id } => {
                self.chief_check_start_mono(&mono_id, &path)?;
                let outbox = self
                    .enqueue(&mono_id, chief::start_text(&proposal), Vec::new(), true, cx)
                    .ok_or("That Mono no longer exists")?;
                proposal.confirm_via_mono(now, outbox)?;
            }
            Route::Direct => {
                let owner = self.owner.upgrade().ok_or("The app is closing")?;
                let text = proposal.instruction.clone();
                let (session, started) = owner.update(cx, |o, cx| {
                    o.chief_start_session(&path, title.as_deref(), text, cx)
                })?;
                proposal.confirm_direct(now, None)?;
                proposal.record_started(session, &started);
            }
        }
        self.chief_store(proposal, cx);
        Ok(())
    }

    // ---- The Mono's proposal-bound send ----

    /// The Mono's bound call failed after the proposal was matched to it: the proposal becomes
    /// `Refused(reason)`, so the turn does not end as `NotAttempted` and the Mono cannot retry
    /// in the same turn. Only the Mono the proposal is routed through can refuse it; another
    /// caller's attempt leaves it alone.
    pub(super) fn chief_refuse_bound(
        &mut self,
        proposal_id: &str,
        mono_id: &str,
        reason: &str,
        cx: &mut Context<Self>,
    ) {
        let own = self
            .roster
            .chief
            .as_ref()
            .and_then(|c| c.proposal(proposal_id))
            .is_some_and(|p| {
                p.state == ProposalState::QueuedAtMono
                    && matches!(&p.route, Route::ViaMono { mono_id: routed } if routed == mono_id)
            });
        if own {
            let _ = self.chief_commit(proposal_id, |p| p.refuse(reason), cx);
        }
    }

    /// The check on a `sessions.send` that carries `proposal_id`. A failure of the Mono the
    /// proposal is routed through is recorded on the proposal as `Refused`; another caller's
    /// attempt is refused without touching it.
    pub(super) fn chief_check_bound(
        &mut self,
        proposal_id: &str,
        mono_id: &str,
        session: i64,
        prompt: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let proposal = self
            .roster
            .chief
            .as_ref()
            .and_then(|c| c.proposal(proposal_id))
            .cloned()
            .ok_or("Unknown proposal")?;
        let Err(reason) = chief::check_bound_send(&proposal, mono_id, session, prompt) else {
            return Ok(());
        };
        self.chief_refuse_bound(proposal_id, mono_id, &reason, cx);
        Err(reason)
    }

    /// The check on a `sessions.start` that carries `proposal_id`, for `project` (the path the
    /// Mono's start resolved). Like [`Monos::chief_check_bound`], a failure of the Mono the
    /// proposal is routed through is recorded as `Refused`. A draft is refused too: a draft saves
    /// the text without sending it, so the command would be marked sent although nothing ran. A
    /// pass reserves the proposal until the start ends ([`Monos::chief_start_ended`]), so a
    /// second start cannot use it too.
    pub(super) fn chief_check_bound_start(
        &mut self,
        proposal_id: &str,
        mono_id: &str,
        project: &str,
        prompt: &str,
        draft: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let proposal = self
            .roster
            .chief
            .as_ref()
            .and_then(|c| c.proposal(proposal_id))
            .cloned()
            .ok_or("Unknown proposal")?;
        let checked =
            chief::check_bound_start(&proposal, mono_id, project, prompt).and_then(|()| {
                if draft {
                    Err(
                        "An owner command starts a session that runs: it cannot be a draft"
                            .to_owned(),
                    )
                } else {
                    Ok(())
                }
            });
        let Err(reason) = checked else {
            if !self.chief_starting.insert(proposal_id.to_owned()) {
                return Err("This proposal was already used".into());
            }
            return Ok(());
        };
        self.chief_refuse_bound(proposal_id, mono_id, &reason, cx);
        Err(reason)
    }

    /// The bound start created session `session`: the proposal is used and records it, with the
    /// prompt the session started with. False when the proposal is no longer the Mono's to use
    /// (the launch then carries no origin). A turn stopped while the start was still running
    /// ended `NotAttempted`; the session now exists, so the proposal is `Sent`.
    pub(super) fn chief_start_accepted(
        &mut self,
        proposal_id: &str,
        mono_id: &str,
        session: i64,
        title: &str,
        prompt: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        self.chief_starting.remove(proposal_id);
        self.chief_commit(
            proposal_id,
            |p| match &p.route {
                Route::ViaMono { mono_id: routed } if routed == mono_id => {
                    p.accept_start(session, title)?;
                    p.record_sent_prompt(prompt);
                    Ok(())
                }
                _ => Err("Not routed through this Mono".into()),
            },
            cx,
        )
        .is_ok()
    }

    /// A bound start ended (its session exists, or it failed): the proposal is no longer held.
    pub(super) fn chief_start_ended(&mut self, proposal_id: &str) {
        self.chief_starting.remove(proposal_id);
    }

    /// A bound start failed after its check passed: the proposal is no longer held and becomes
    /// `Refused(reason)`.
    pub(super) fn chief_start_failed(
        &mut self,
        proposal_id: &str,
        mono_id: &str,
        reason: &str,
        cx: &mut Context<Self>,
    ) {
        self.chief_starting.remove(proposal_id);
        self.chief_refuse_bound(proposal_id, mono_id, reason, cx);
    }

    /// The Mono's bound send went through: the proposal is used, queued in the session
    /// (`follow_up` is the queued message) or sent.
    pub(super) fn chief_bound_accepted(
        &mut self,
        proposal_id: &str,
        follow_up: Option<String>,
        prompt: &str,
        cx: &mut Context<Self>,
    ) {
        let _ = self.chief_commit(
            proposal_id,
            |p| {
                p.accept_send(follow_up)?;
                p.record_sent_prompt(prompt);
                Ok(())
            },
            cx,
        );
    }

    // ---- Following a proposal afterwards ----

    /// The proposal whose relay message is the turn this Mono is running now, if any. Its
    /// turn is the owner's command, not the owner's own, so it sends no notification.
    pub(super) fn chief_relay_of(&self, mono_id: &str) -> Option<String> {
        let message = self.inflight.get(mono_id)?;
        self.roster
            .chief
            .as_ref()?
            .proposals
            .iter()
            .find(|p| {
                p.outbox_id.as_deref() == Some(message.id.as_str())
                    && matches!(&p.route, Route::ViaMono { mono_id: m } if m == mono_id)
            })
            .map(|p| p.id.clone())
    }

    /// The Mono's relay turn ended. The Mono's last words are kept on the proposal. A command it
    /// never sent becomes `NotAttempted`; a turn that failed, `Failed`.
    pub(super) fn chief_relay_ended(
        &mut self,
        proposal_id: &str,
        ok: bool,
        status: &str,
        reply: &str,
        cx: &mut Context<Self>,
    ) {
        let reply = monocode_core::response::limit(reply, REPLY_KEPT);
        let _ = self.chief_commit(
            proposal_id,
            |p| {
                p.set_reply(&reply);
                if p.state != ProposalState::QueuedAtMono {
                    return Ok(());
                }
                if ok || status.starts_with("Stopped") {
                    p.not_attempted()
                } else {
                    p.fail(&format!("The Mono's turn failed: {status}"))
                }
            },
            cx,
        );
    }

    /// An outbox message can no longer be delivered (it errored or was discarded): the command it
    /// carried failed.
    pub(super) fn chief_outbox_lost(
        &mut self,
        outbox_id: &str,
        reason: &str,
        cx: &mut Context<Self>,
    ) {
        let found = self.chief_proposals().iter().find_map(|p| {
            (p.state == ProposalState::QueuedAtMono && p.outbox_id.as_deref() == Some(outbox_id))
                .then(|| p.id.clone())
        });
        if let Some(id) = found {
            let _ = self.chief_commit(&id, |p| p.fail(reason), cx);
        }
    }

    /// A Mono was reset or deleted. The commands waiting at it fail: `only_gone` keeps those whose
    /// message is still in its outbox (a reset keeps the queue for the fresh chat).
    pub(super) fn chief_mono_lost(
        &mut self,
        mono_id: &str,
        only_gone: bool,
        reason: &str,
        cx: &mut Context<Self>,
    ) {
        let queued: HashSet<String> = self
            .roster
            .get(mono_id)
            .map(|m| m.outbox.iter().map(|q| q.id.clone()).collect())
            .unwrap_or_default();
        let lost: Vec<String> = self
            .chief_proposals()
            .iter()
            .filter(|p| {
                p.state == ProposalState::QueuedAtMono
                    && matches!(&p.route, Route::ViaMono { mono_id: m } if m == mono_id)
                    && !(only_gone && p.outbox_id.as_ref().is_some_and(|id| queued.contains(id)))
            })
            .map(|p| p.id.clone())
            .collect();
        for id in lost {
            let _ = self.chief_commit(&id, |p| p.fail(reason), cx);
        }
    }

    /// On load: a command queued at a Mono whose message is gone can never be sent.
    pub(super) fn chief_fix_interrupted(&mut self, cx: &mut Context<Self>) {
        self.chief_resolve_queued(cx);
        let live: HashSet<String> = self
            .roster
            .monos
            .iter()
            .flat_map(|m| m.outbox.iter().map(|q| q.id.clone()))
            .collect();
        let waiting: Vec<String> = self
            .chief_proposals()
            .iter()
            .filter(|p| p.state == ProposalState::QueuedAtMono)
            .map(|p| p.id.clone())
            .collect();
        if waiting.is_empty() {
            return;
        }
        if let Some(chief) = self.roster.chief.as_mut() {
            chief.fix_interrupted(&live);
        }
        let failed: Vec<Proposal> = waiting
            .iter()
            .filter_map(|id| self.roster.chief.as_ref()?.proposal(id))
            .filter(|p| p.state != ProposalState::QueuedAtMono)
            .cloned()
            .collect();
        for proposal in failed {
            self.chief_recorded(&proposal, cx);
        }
    }

    /// On load: a command that was in a session's queue when the app quit is looked up in that
    /// session's saved queue. Still queued: it keeps waiting. Delivered (the saved queue
    /// recorded it): `Sent`. Neither: it was removed, `Failed`. Without this a proposal could
    /// stay `QueuedInSession` forever for a session nobody opens.
    fn chief_resolve_queued(&mut self, cx: &mut Context<Self>) {
        let mut moves: Vec<(String, FollowUpStatus)> = Vec::new();
        for p in self.chief_proposals() {
            let (ProposalState::QueuedInSession, Some(message), Some(session)) =
                (&p.state, p.follow_up_id.as_deref(), p.session_id)
            else {
                continue;
            };
            let Some((state, recorded)) = locked(&self.store).ok().and_then(|store| {
                let state = store.follow_ups(session).ok()?;
                // The store marks a message the provider took when its turn was written.
                let recorded = store
                    .setting(&format!("mono-delivered:{session}:{message}"))
                    .ok()
                    .flatten()
                    .is_some();
                Some((state, recorded))
            }) else {
                continue;
            };
            let queued = state.messages.iter().any(|m| m.id == message)
                || state.draft.as_ref().is_some_and(|m| m.id == message);
            if queued {
                continue;
            }
            let status = if recorded || state.delivered.iter().any(|d| d == message) {
                FollowUpStatus::Delivered
            } else {
                FollowUpStatus::Gone
            };
            moves.push((p.id.clone(), status));
        }
        for (id, status) in moves {
            let _ = self.chief_commit(
                &id,
                |p| match status {
                    FollowUpStatus::Delivered => p.delivered(),
                    _ => p.fail("removed from the session queue"),
                },
                cx,
            );
        }
    }

    /// Follows the commands in sessions' queues: delivered becomes `Sent`; a message the owner
    /// removed from the queue becomes `Failed`. Run with the completion checks.
    pub(super) fn chief_sync_follow_ups(&mut self, cx: &mut Context<Self>) {
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        let mut moves: Vec<(String, FollowUpStatus)> = Vec::new();
        for p in self.chief_proposals() {
            let (ProposalState::QueuedInSession, Some(message), Some(session)) =
                (&p.state, p.follow_up_id.as_deref(), p.session_id)
            else {
                continue;
            };
            let Some(view) = owner.read(cx).mono_session_entity(session) else {
                continue;
            };
            // A queue still loading answers `Queued`, so only a loaded queue that neither holds
            // nor recorded the message (the owner removed it) moves a proposal to `Failed`.
            let status = view.read(cx).follow_up_status(message);
            if status != FollowUpStatus::Queued {
                moves.push((p.id.clone(), status));
            }
        }
        for (id, status) in moves {
            let _ = self.chief_commit(
                &id,
                |p| match status {
                    FollowUpStatus::Delivered => p.delivered(),
                    _ => p.fail("removed from the session queue"),
                },
                cx,
            );
        }
    }
}
