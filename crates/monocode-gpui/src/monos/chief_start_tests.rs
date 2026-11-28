//! Starting new work through the Chief: `chief.proposeStart`, the owner's click, the Mono's
//! proposal-bound `sessions.start` and the app's own start for a project no Mono covers.
use super::chief_command_tests::{
    TEXT, bound_send, confirm, end_relay, open_mono_chat, outbox, proposal, propose, second_mono,
};
use super::chief_tests::{World, call_world, world, world_with};
use super::*;
use gpui::TestAppContext;
use monocode_core::{
    agent::AccessMode,
    mono::chief::{self as core_chief, ProposalKind, ProposalState, Route, Surface},
};
use serde_json::{Value, json};

pub(super) const START: &str = "Add a rate limit to the login endpoint";
const ELSEWHERE: &str = "/elsewhere/other";

fn propose_start(
    w: &World,
    project: &str,
    mono: Option<&str>,
    cx: &mut TestAppContext,
) -> Result<Value, String> {
    let mut input = json!({"project":project,"instruction":START,"title":"Rate limit"});
    if let Some(mono) = mono {
        input["mono"] = json!(mono);
    }
    call_world(w, w.chief_chat, "chief.proposeStart", input, cx)
}

fn id_of(reply: &Value) -> String {
    reply["proposalId"].as_str().unwrap().to_owned()
}

/// A start proposal through the Mono, confirmed.
pub(super) fn confirmed_start(w: &World, cx: &mut TestAppContext) -> String {
    open_mono_chat(w, cx);
    let id = id_of(&propose_start(w, &w.project, None, cx).unwrap());
    confirm(w, &id, cx).unwrap();
    id
}

/// The Mono's `sessions.start` for `prompt` in `project`, bound to `id`. The session it creates
/// runs no provider in the test (a bound start is never a draft).
pub(super) fn bound_start(
    w: &World,
    chat: i64,
    project: &str,
    prompt: &str,
    id: &str,
    cx: &mut TestAppContext,
) -> Result<Value, String> {
    crate::shell::Shell::test_hold_chief_starts(true);
    call_world(
        w,
        chat,
        "sessions.start",
        json!({"project":project,"prompt":prompt,"proposalId":id}),
        cx,
    )
}

fn no_mono_covers(roster: &mut Roster, _: i64) {
    roster.monos[0].projects = vec![ELSEWHERE.into()];
}

fn launches(w: &World, cx: &mut TestAppContext) -> Vec<mono::Launch> {
    cx.update(|cx| {
        w.manager
            .read(cx)
            .roster
            .get("mono-one")
            .unwrap()
            .launches
            .clone()
    })
}

// ---- chief.proposeStart ----

#[gpui::test]
fn one_covering_mono_takes_the_start_and_the_card_names_the_project(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let reply = propose_start(&w, &w.project, None, cx).unwrap();
    assert_eq!(reply["status"], "awaitingOwner");
    assert_eq!(reply["route"], json!({"type":"viaMono","monoId":w.mono}));
    let p = proposal(&w, &id_of(&reply), cx);
    assert_eq!(p.state, ProposalState::AwaitingOwner);
    assert_eq!(p.session_id, None);
    assert_eq!(p.instruction, START);
    assert!(!p.project.is_empty() && !p.project.contains('/'));
    assert!(matches!(&p.kind, ProposalKind::Start { project, title }
        if *project == w.project && title.as_deref() == Some("Rate limit")));
    cx.update(|cx| assert_eq!(w.manager.read(cx).chief_open_count(), 1));
    // By the project's name too.
    let by_name = propose_start(&w, &p.project, None, cx).unwrap();
    assert_eq!(by_name["route"], json!({"type":"viaMono","monoId":w.mono}));
}

#[gpui::test]
fn several_covering_monos_need_the_owner_to_pick_one(cx: &mut TestAppContext) {
    let w = world_with(cx, "claude", second_mono);
    let reply = propose_start(&w, &w.project, None, cx).unwrap();
    assert_eq!(reply["status"], "needsMono");
    let ids: Vec<&str> = reply["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["mono-one", "mono-two"]);
    assert!(
        reply["candidates"][0]["name"]
            .as_str()
            .unwrap()
            .contains("Atlas")
    );
    cx.update(|cx| assert_eq!(w.manager.read(cx).chief_open_count(), 0));
    let picked = propose_start(&w, &w.project, Some("mono-two"), cx).unwrap();
    assert_eq!(
        picked["route"],
        json!({"type":"viaMono","monoId":"mono-two"})
    );
    assert!(propose_start(&w, &w.project, Some("mono-nope"), cx).is_err());
}

#[gpui::test]
fn no_covering_mono_means_the_app_starts_it_directly(cx: &mut TestAppContext) {
    let w = world_with(cx, "claude", no_mono_covers);
    let reply = propose_start(&w, &w.project, None, cx).unwrap();
    assert_eq!(reply["route"], json!({"type":"direct"}));
    let error = propose_start(&w, &w.project, Some("mono-one"), cx).unwrap_err();
    assert!(error.contains("No Mono covers"), "{error}");
}

#[gpui::test]
fn an_unknown_project_is_refused_and_the_cap_is_shared(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let error = propose_start(&w, "/no/such/project", None, cx).unwrap_err();
    assert!(error.contains("Unknown project"), "{error}");
    let error = propose_start(&w, "nope", None, cx).unwrap_err();
    assert!(error.contains("Unknown project"), "{error}");
    cx.update(|cx| assert!(w.manager.read(cx).chief_proposals().is_empty()));
    // Starts and commands count toward the same five.
    for _ in 0..3 {
        propose_start(&w, &w.project, None, cx).unwrap();
    }
    for _ in 0..2 {
        propose(&w, w.owned, None, cx).unwrap();
    }
    assert!(
        propose_start(&w, &w.project, None, cx)
            .unwrap_err()
            .contains("5 proposals")
    );
}

#[gpui::test]
fn a_start_card_warns_only_when_the_mono_is_busy(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    open_mono_chat(&w, cx);
    let reply = propose_start(&w, &w.project, None, cx).unwrap();
    assert_eq!(reply["notes"], json!(["monoBusy"]));
}

// ---- The owner's click ----

#[gpui::test]
fn confirming_a_start_through_a_mono_enqueues_the_exact_start_text(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = confirmed_start(&w, cx);
    let p = proposal(&w, &id, cx);
    assert_eq!(p.state, ProposalState::QueuedAtMono);
    let queue = outbox(&w, "mono-one", cx);
    let message = queue
        .iter()
        .find(|m| Some(&m.id) == p.outbox_id.as_ref())
        .unwrap();
    assert_eq!(message.text, core_chief::start_text(&p));
    assert!(message.internal);
    for needle in [
        core_chief::HEADER,
        "Start new work in project",
        START,
        &format!("proposalId: \"{id}\""),
        "notifyOnComplete: true",
    ] {
        assert!(message.text.contains(needle), "{needle}");
    }
    // The edited text is what the Mono receives.
    let other = id_of(&propose_start(&w, &w.project, None, cx).unwrap());
    w.manager
        .update(cx, |m, cx| {
            m.chief_confirm(&other, Surface::Desktop, Some("Only add a test".into()), cx)
        })
        .unwrap();
    assert!(
        outbox(&w, "mono-one", cx)
            .last()
            .unwrap()
            .text
            .contains("Only add a test")
    );
}

#[gpui::test]
fn a_changed_route_is_refused_at_confirm(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = id_of(&propose_start(&w, &w.project, None, cx).unwrap());
    w.manager.update(cx, |m, _| {
        m.roster.get_mut("mono-one").unwrap().projects = vec![ELSEWHERE.into()]
    });
    let error = confirm(&w, &id, cx).unwrap_err();
    assert!(error.contains("changed"), "{error}");
    assert!(proposal(&w, &id, cx).is_open());
}

#[gpui::test]
fn a_direct_start_creates_a_session_with_safe_access_and_sends_the_text(cx: &mut TestAppContext) {
    let w = world_with(cx, "claude", no_mono_covers);
    crate::shell::Shell::test_hold_chief_starts(true);
    cx.update(|cx| crate::conversation::remember_access(AccessMode::FullAccess, cx));
    let before = locked(&w.store).unwrap().sessions().unwrap().len();
    let id = id_of(&propose_start(&w, &w.project, None, cx).unwrap());
    confirm(&w, &id, cx).unwrap();
    cx.run_until_parked();
    let p = proposal(&w, &id, cx);
    assert_eq!(p.state, ProposalState::Sent);
    let session = p.session_id.expect("the new session is recorded");
    assert_eq!(p.session_title, "Rate limit");
    let saved = locked(&w.store).unwrap().sessions().unwrap();
    assert_eq!(saved.len(), before + 1);
    assert_eq!(
        saved.iter().find(|s| s.id == session).unwrap().cwd,
        w.project
    );
    let view = cx.update(|cx| {
        w.window
            .update(cx, |shell, _, _| shell.mono_session_entity(session))
            .unwrap()
            .expect("the session is open in the app")
    });
    cx.update(|cx| {
        let view = view.read(cx);
        assert_ne!(view.settings.access, AccessMode::FullAccess);
        assert_eq!(view.test_launched(), vec![START.to_owned()]);
    });
    // Nothing follows it: no Mono launch, no outbox message.
    assert_eq!(launches(&w, cx).len(), 1);
    assert!(outbox(&w, "mono-one", cx).is_empty());
    // A second click does nothing more.
    assert!(confirm(&w, &id, cx).is_err());
    assert_eq!(
        locked(&w.store).unwrap().sessions().unwrap().len(),
        before + 1
    );
}

// ---- The Mono's proposal-bound start ----

#[gpui::test]
fn a_bound_start_creates_the_session_and_marks_the_proposal_and_launch(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = confirmed_start(&w, cx);
    let reply = bound_start(
        &w,
        w.mono_chat,
        &w.project,
        &format!("Please: {START}"),
        &id,
        cx,
    )
    .unwrap();
    let session = reply["sessionId"].as_i64().expect("a session was started");
    let p = proposal(&w, &id, cx);
    assert_eq!(p.state, ProposalState::Sent);
    assert_eq!(p.session_id, Some(session));
    let launch = launches(&w, cx)
        .into_iter()
        .find(|l| l.session_id == session)
        .unwrap();
    assert_eq!(
        launch.origin,
        Some(mono::LaunchOrigin {
            proposal_id: id.clone(),
            follow_up_id: None
        })
    );
    // Its completion report leads with the owner-command lead.
    let results = [(session, "t".to_owned(), "done".to_owned())];
    let text = host::completion_message_for(host::Batch::Each(session), &results, &[session]);
    assert!(text.contains(core_chief::COMPLETION_LEAD));
    // A second start with the same proposal is refused.
    let error = bound_start(&w, w.mono_chat, &w.project, START, &id, cx).unwrap_err();
    assert!(error.contains("already used"), "{error}");
    assert_eq!(launches(&w, cx).len(), 2);
}

#[gpui::test]
fn a_start_without_a_proposal_has_no_origin(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let reply = call_world(
        &w,
        w.mono_chat,
        "sessions.start",
        json!({"project":w.project,"prompt":"Plain","draft":true}),
        cx,
    )
    .unwrap();
    let session = reply["sessionId"].as_i64().unwrap();
    let launch = launches(&w, cx)
        .into_iter()
        .find(|l| l.session_id == session)
        .unwrap();
    assert_eq!(launch.origin, None);
}

#[gpui::test]
fn a_bound_start_that_does_not_match_is_refused_and_recorded(cx: &mut TestAppContext) {
    let w = world_with(cx, "claude", |roster, _| {
        roster.monos[0].projects.push(ELSEWHERE.into())
    });
    let before = launches(&w, cx).len();
    // Altered text.
    let id = confirmed_start(&w, cx);
    let error = bound_start(&w, w.mono_chat, &w.project, "Add something", &id, cx).unwrap_err();
    assert!(error.contains("unchanged"), "{error}");
    assert!(matches!(
        proposal(&w, &id, cx).state,
        ProposalState::Refused(_)
    ));
    // The refusal used it up.
    assert!(bound_start(&w, w.mono_chat, &w.project, START, &id, cx).is_err());
    // Another project.
    let id = confirmed_start(&w, cx);
    let error = bound_start(&w, w.mono_chat, ELSEWHERE, START, &id, cx).unwrap_err();
    assert!(error.contains("another project"), "{error}");
    assert!(matches!(
        proposal(&w, &id, cx).state,
        ProposalState::Refused(_)
    ));
    // Never confirmed, and one that does not exist.
    let open = id_of(&propose_start(&w, &w.project, None, cx).unwrap());
    let error = bound_start(&w, w.mono_chat, &w.project, START, &open, cx).unwrap_err();
    assert!(error.contains("not confirmed"), "{error}");
    assert!(proposal(&w, &open, cx).is_open());
    assert!(bound_start(&w, w.mono_chat, &w.project, START, "p_00000000", cx).is_err());
    assert_eq!(launches(&w, cx).len(), before, "nothing was started");
}

#[gpui::test]
fn another_mono_cannot_use_a_start_proposal_and_does_not_burn_it(cx: &mut TestAppContext) {
    let w = world_with(cx, "claude", second_mono);
    open_mono_chat(&w, cx);
    let id = id_of(&propose_start(&w, &w.project, Some("mono-two"), cx).unwrap());
    confirm(&w, &id, cx).unwrap();
    let error = bound_start(&w, w.mono_chat, &w.project, START, &id, cx).unwrap_err();
    assert!(error.contains("another Mono"), "{error}");
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::QueuedAtMono);
}

#[gpui::test]
fn a_direct_start_cannot_be_used_by_a_mono(cx: &mut TestAppContext) {
    let w = world_with(cx, "claude", no_mono_covers);
    let id = id_of(&propose_start(&w, &w.project, None, cx).unwrap());
    let error = bound_start(&w, w.mono_chat, ELSEWHERE, START, &id, cx).unwrap_err();
    assert!(error.contains("not confirmed"), "{error}");
    assert!(proposal(&w, &id, cx).is_open());
}

#[gpui::test]
fn a_command_proposal_cannot_start_and_a_start_cannot_send(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let (command, _) = super::chief_command_tests::confirmed(&w, cx);
    let error = bound_start(&w, w.mono_chat, &w.project, TEXT, &command, cx).unwrap_err();
    assert!(error.contains("not a command to start"), "{error}");
    let start = confirmed_start(&w, cx);
    let error = bound_send(&w, w.mono_chat, w.owned, START, &start, cx).unwrap_err();
    assert!(error.contains("not a command for a session"), "{error}");
}

#[gpui::test]
fn a_proposal_is_held_while_its_start_runs(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = confirmed_start(&w, cx);
    let check = |cx: &mut TestAppContext| {
        w.manager.update(cx, |m, cx| {
            m.chief_check_bound_start(&id, "mono-one", &w.project, START, false, cx)
        })
    };
    assert_eq!(check(cx), Ok(()));
    assert!(check(cx).unwrap_err().contains("already used"));
    w.manager.update(cx, |m, _| m.chief_start_ended(&id));
    assert_eq!(check(cx), Ok(()));
}

// ---- Following a start ----

#[gpui::test]
fn a_relay_turn_that_never_started_the_session_is_not_attempted(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = confirmed_start(&w, cx);
    let outbox_id = proposal(&w, &id, cx).outbox_id.unwrap();
    end_relay(&w, &outbox_id, "I could not do that", cx);
    let p = proposal(&w, &id, cx);
    assert_eq!(p.state, ProposalState::NotAttempted);
    assert_eq!(p.mono_reply.as_deref(), Some("I could not do that"));
}

#[gpui::test]
fn a_discarded_start_message_or_a_deleted_mono_fails_the_start(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = confirmed_start(&w, cx);
    let outbox_id = proposal(&w, &id, cx).outbox_id.unwrap();
    w.manager
        .update(cx, |m, cx| m.chief_outbox_lost(&outbox_id, "discarded", cx));
    assert!(matches!(
        proposal(&w, &id, cx).state,
        ProposalState::Failed(_)
    ));
    let id = confirmed_start(&w, cx);
    w.manager.update(cx, |m, cx| {
        m.chief_mono_lost("mono-one", false, "Mono was reset", cx)
    });
    assert!(matches!(
        proposal(&w, &id, cx).state,
        ProposalState::Failed(_)
    ));
}

// ---- What the owner sees ----

#[gpui::test]
fn the_card_and_the_history_row_say_new_session_and_the_project(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = id_of(&propose_start(&w, &w.project, None, cx).unwrap());
    w.manager.update(cx, |m, cx| m.chief_sync_cards_now(cx));
    cx.run_until_parked();
    let p = proposal(&w, &id, cx);
    let card = cx.update(|cx| {
        let chat = w.manager.read(cx).chief_chat().unwrap().read(cx);
        chat.transcript
            .read(cx)
            .chief_cards()
            .cards
            .get(&id)
            .cloned()
            .expect("the card is known")
    });
    assert_eq!(
        card.route,
        format!("Atlas \u{2192} new session \u{b7} {}", p.project)
    );
    assert_eq!(card.instruction, START);
    assert!(card.open);
    assert_eq!(card.state, "Waiting for your Send");
    let rows = cx.update(|cx| {
        let m = w.manager.read(cx);
        super::chief_history::history_rows(m.chief_proposals(), &m.roster)
    });
    assert_eq!(
        rows[0].session,
        format!("New session \u{b7} {} Rate limit", p.project)
    );
    assert_eq!(rows[0].instruction, START);
    // The route names the Mono, never the owner's session.
    assert!(matches!(p.route, Route::ViaMono { .. }));
}

#[gpui::test]
fn the_web_payload_of_a_start_has_its_kind_and_project_and_no_session(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = id_of(&propose_start(&w, &w.project, None, cx).unwrap());
    let p = proposal(&w, &id, cx);
    let value = cx.update(|cx| {
        let m = w.manager.read(cx);
        m.chief_web_proposal(&p, &m.chief_card_notes(&p, cx))
    });
    assert_eq!(value["kind"], "start");
    assert_eq!(value["project"], p.project.as_str());
    assert_eq!(value["route"]["type"], "viaMono");
    assert_eq!(value["state"], "awaitingOwner");
    assert!(value.get("sessionNumber").is_none());
    assert_eq!(value["instruction"], START);
}
