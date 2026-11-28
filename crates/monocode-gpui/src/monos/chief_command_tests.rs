//! Commanding a session through the Chief: proposals, the owner's click, the Mono's bound send
//! and how a proposal's state follows the turn and the queue.
use super::chief_tests::{World, call_world, world, world_with};
use super::*;
use crate::conversation::FollowUpStatus;
use gpui::TestAppContext;
use monocode_core::mono::chief::{self as core_chief, Proposal, ProposalState, Route, Surface};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub(super) const TEXT: &str = "Add tests for the expired-token case";

pub(super) fn propose(
    w: &World,
    session: i64,
    mono: Option<&str>,
    cx: &mut TestAppContext,
) -> Result<Value, String> {
    let mut input = json!({"sessionId":session,"instruction":TEXT});
    if let Some(mono) = mono {
        input["mono"] = json!(mono);
    }
    call_world(w, w.chief_chat, "chief.propose", input, cx)
}

pub(super) fn proposal(w: &World, id: &str, cx: &mut TestAppContext) -> Proposal {
    cx.update(|cx| {
        w.manager
            .read(cx)
            .chief_proposals()
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .expect("the proposal exists")
    })
}

pub(super) fn confirm(w: &World, id: &str, cx: &mut TestAppContext) -> Result<(), String> {
    w.manager
        .update(cx, |m, cx| m.chief_confirm(id, Surface::Desktop, None, cx))
}

fn audit(w: &World, cx: &mut TestAppContext) -> Vec<Value> {
    let dir = cx.update(|cx| core_chief::dir(&w.manager.read(cx).chief_root()));
    std::fs::read_to_string(dir.join(core_chief::AUDIT_FILE))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

pub(super) fn second_mono(roster: &mut Roster, owned: i64) {
    let project = roster.monos[0].projects.clone();
    roster.monos.push(Mono {
        id: "mono-two".into(),
        name: "Borealis".into(),
        projects: project.clone(),
        launches: vec![super::tests::launch_of(owned, &project[0])],
        ..Default::default()
    });
}

pub(super) fn outbox(w: &World, mono: &str, cx: &mut TestAppContext) -> Vec<Message> {
    cx.update(|cx| w.manager.read(cx).roster.get(mono).unwrap().outbox.clone())
}

/// A bound send of `prompt` to `session` from `mono_chat`'s Mono.
pub(super) fn bound_send(
    w: &World,
    chat: i64,
    session: i64,
    prompt: &str,
    id: &str,
    cx: &mut TestAppContext,
) -> Result<Value, String> {
    call_world(
        w,
        chat,
        "sessions.send",
        json!({"sessionId":session,"prompt":prompt,"proposalId":id,"notifyOnComplete":true}),
        cx,
    )
}

pub(super) fn set_busy(w: &World, session: i64, busy: bool, cx: &mut TestAppContext) {
    cx.update(|cx| {
        let view = w
            .window
            .update(cx, |shell, _, cx| shell.mono_ensure_session(session, cx))
            .unwrap()
            .unwrap();
        view.update(cx, |v, _| v.busy = busy);
    });
}

/// Opens the Mono's chat and holds it busy, so no queued message is started in the test.
pub(super) fn open_mono_chat(w: &World, cx: &mut TestAppContext) -> Entity<Conversation> {
    w.manager.update(cx, |m, cx| m.ensure_chat("mono-one", cx));
    cx.run_until_parked();
    let chat = cx.update(|cx| w.manager.read(cx).chats["mono-one"].clone());
    chat.update(cx, |v, _| {
        v.busy = true;
        v.test_hold_launches();
    });
    chat
}

pub(super) fn session_view(
    w: &World,
    session: i64,
    cx: &mut TestAppContext,
) -> Entity<Conversation> {
    let view = cx.update(|cx| {
        w.window
            .update(cx, |shell, _, cx| shell.mono_ensure_session(session, cx))
            .unwrap()
            .unwrap()
    });
    // A send is recorded, never run by a real provider.
    view.update(cx, |v, _| v.test_hold_launches());
    view
}

/// A proposal through the Mono, confirmed: queued at the Mono, with its relay message.
pub(super) fn confirmed(w: &World, cx: &mut TestAppContext) -> (String, String) {
    open_mono_chat(w, cx);
    let id = propose(w, w.owned, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    confirm(w, &id, cx).unwrap();
    let outbox_id = proposal(w, &id, cx).outbox_id.unwrap();
    (id, outbox_id)
}

// ---- chief.propose ----

#[gpui::test]
fn one_owner_routes_through_its_mono_and_the_record_shows_the_session(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let reply = propose(&w, w.owned, None, cx).unwrap();
    assert_eq!(reply["status"], "awaitingOwner");
    assert_eq!(reply["route"], json!({"type":"viaMono","monoId":w.mono}));
    assert!(reply["notes"].as_array().unwrap().is_empty());
    let p = proposal(&w, reply["proposalId"].as_str().unwrap(), cx);
    assert_eq!(p.state, ProposalState::AwaitingOwner);
    assert_eq!(p.session_id, Some(w.owned));
    assert_eq!(p.session_title, "owned session");
    assert_eq!(p.instruction, TEXT);
    assert!(!p.project.is_empty() && !p.project.contains('/'));
    cx.update(|cx| assert_eq!(w.manager.read(cx).chief_open_count(), 1));
    // The record is on disk and in the audit log.
    let saved = Roster::load(&locked(&w.store).unwrap()).unwrap();
    assert_eq!(saved.chief.unwrap().proposals.len(), 1);
    assert_eq!(audit(&w, cx).len(), 1);
}

#[gpui::test]
fn no_owner_routes_directly_and_a_named_mono_is_refused(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let reply = propose(&w, w.free, None, cx).unwrap();
    assert_eq!(reply["route"], json!({"type":"direct"}));
    let error = propose(&w, w.free, Some(&w.mono), cx).unwrap_err();
    assert!(error.contains("No Mono owns"), "{error}");
}

#[gpui::test]
fn several_owners_need_the_owner_to_pick_one(cx: &mut TestAppContext) {
    let w = world_with(cx, "claude", second_mono);
    let reply = propose(&w, w.owned, None, cx).unwrap();
    assert_eq!(reply["status"], "needsMono");
    let ids: Vec<&str> = reply["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["mono-one", "mono-two"]);
    // The names are fenced: they are a Mono's words.
    assert!(
        reply["candidates"][0]["name"]
            .as_str()
            .unwrap()
            .contains("Atlas")
    );
    cx.update(|cx| assert_eq!(w.manager.read(cx).chief_open_count(), 0));
    let picked = propose(&w, w.owned, Some("mono-two"), cx).unwrap();
    assert_eq!(
        picked["route"],
        json!({"type":"viaMono","monoId":"mono-two"})
    );
    let error = propose(&w, w.owned, Some("mono-nope"), cx).unwrap_err();
    assert!(error.contains("not one of the owners"), "{error}");
}

#[gpui::test]
fn only_five_proposals_wait_for_the_owner(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    for _ in 0..5 {
        propose(&w, w.free, None, cx).unwrap();
    }
    let error = propose(&w, w.free, None, cx).unwrap_err();
    assert!(error.contains("5 proposals"), "{error}");
    // Cancelling one makes room.
    let first = cx.update(|cx| w.manager.read(cx).chief_proposals()[0].id.clone());
    w.manager
        .update(cx, |m, cx| m.chief_cancel(&first, cx))
        .unwrap();
    propose(&w, w.free, None, cx).unwrap();
}

#[gpui::test]
fn hidden_mono_chat_and_chief_chat_targets_are_refused(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    w.manager
        .update(cx, |m, _| m.roster.hidden_sessions.push(w.free));
    for session in [w.free, w.mono_chat, w.chief_chat, 999_999] {
        assert!(propose(&w, session, None, cx).is_err(), "{session}");
    }
    assert_eq!(
        propose(&w, w.chief_chat, None, cx).unwrap_err(),
        super::chief::CHIEF_CHAT_REFUSED
    );
    cx.update(|cx| assert!(w.manager.read(cx).chief_proposals().is_empty()));
}

#[gpui::test]
fn a_mono_whose_chat_is_unavailable_cannot_take_a_proposal(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    w.manager.update(cx, |m, _| {
        m.roster.get_mut("mono-one").unwrap().unavailable_session = Some("gone".into())
    });
    let error = propose(&w, w.owned, None, cx).unwrap_err();
    assert!(error.contains("unavailable"), "{error}");
}

#[gpui::test]
fn the_card_warns_when_the_session_or_the_mono_is_busy(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    set_busy(&w, w.owned, true, cx);
    open_mono_chat(&w, cx);
    let reply = propose(&w, w.owned, None, cx).unwrap();
    assert_eq!(reply["notes"], json!(["sessionBusy", "monoBusy"]));
    let p = proposal(&w, reply["proposalId"].as_str().unwrap(), cx);
    cx.update(|cx| {
        assert_eq!(
            w.manager.read(cx).chief_card_notes(&p, cx),
            vec!["sessionBusy", "monoBusy"]
        )
    });
}

#[gpui::test]
fn chief_cancel_cancels_an_open_proposal_once(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    let reply = call_world(
        &w,
        w.chief_chat,
        "chief.cancel",
        json!({"proposalId":id}),
        cx,
    )
    .unwrap();
    assert_eq!(reply["status"], "cancelled");
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::Cancelled);
    assert!(
        call_world(
            &w,
            w.chief_chat,
            "chief.cancel",
            json!({"proposalId":id}),
            cx
        )
        .is_err()
    );
    assert_eq!(audit(&w, cx).len(), 2);
}

// ---- The owner's click ----

#[gpui::test]
fn confirming_through_a_mono_enqueues_the_exact_command(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let (id, outbox_id) = confirmed(&w, cx);
    let p = proposal(&w, &id, cx);
    assert_eq!(p.state, ProposalState::QueuedAtMono);
    assert!(p.confirmed_at.is_some());
    let queue = outbox(&w, "mono-one", cx);
    let message = queue.iter().find(|m| m.id == outbox_id).unwrap();
    assert_eq!(message.text, core_chief::command_text(&p));
    assert!(message.internal);
    assert!(message.text.contains(TEXT) && message.text.contains(&id));
    assert_eq!(audit(&w, cx).len(), 2);
}

#[gpui::test]
fn an_edited_text_is_what_the_mono_receives(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = propose(&w, w.owned, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    w.manager
        .update(cx, |m, cx| m.chief_edit(&id, "Only add one test", cx))
        .unwrap();
    assert_eq!(proposal(&w, &id, cx).instruction, "Only add one test");
    w.manager
        .update(cx, |m, cx| {
            m.chief_confirm(&id, Surface::Desktop, Some("Add two tests".into()), cx)
        })
        .unwrap();
    let p = proposal(&w, &id, cx);
    let queue = outbox(&w, "mono-one", cx);
    assert!(queue.last().unwrap().text.contains("Add two tests"));
    assert!(!queue.last().unwrap().text.contains("Only add one test"));
    // A handled proposal cannot be edited, cancelled or sent again.
    assert!(
        w.manager
            .update(cx, |m, cx| m.chief_edit(&id, "x", cx))
            .is_err()
    );
    assert!(
        w.manager
            .update(cx, |m, cx| m.chief_cancel(&id, cx))
            .is_err()
    );
    assert!(confirm(&w, &id, cx).is_err());
    assert_eq!(p.state, ProposalState::QueuedAtMono);
}

#[gpui::test]
fn a_missing_mono_chat_is_refused_at_confirm(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = propose(&w, w.owned, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    w.manager.update(cx, |m, _| {
        m.roster.get_mut("mono-one").unwrap().unavailable_session = Some("gone".into())
    });
    let error = confirm(&w, &id, cx).unwrap_err();
    assert!(error.contains("unavailable"), "{error}");
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::AwaitingOwner);
    assert!(outbox(&w, "mono-one", cx).is_empty());
}

#[gpui::test]
fn a_changed_route_is_refused_not_rerouted(cx: &mut TestAppContext) {
    // Through a Mono that dismissed the launch meanwhile.
    let w = world(cx, "claude");
    let id = propose(&w, w.owned, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    w.manager.update(cx, |m, _| {
        m.roster.get_mut("mono-one").unwrap().launches[0].dismissed = true
    });
    let error = confirm(&w, &id, cx).unwrap_err();
    assert!(error.contains("owns this session changed"), "{error}");
    assert!(outbox(&w, "mono-one", cx).is_empty());
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::AwaitingOwner);
    // A direct proposal whose session a Mono took over meanwhile.
    let id = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    w.manager.update(cx, |m, _| {
        let launch = super::tests::launch_of(w.free, &w.project);
        m.roster.get_mut("mono-one").unwrap().launches.push(launch)
    });
    assert!(confirm(&w, &id, cx).is_err());
    assert!(proposal(&w, &id, cx).is_open());
}

#[gpui::test]
fn a_session_that_became_a_chat_is_refused_at_confirm(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    w.manager
        .update(cx, |m, _| m.roster.hidden_sessions.push(w.free));
    assert!(confirm(&w, &id, cx).is_err());
}

#[gpui::test]
fn web_send_needs_the_desktop_toggle(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = propose(&w, w.owned, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    let denied = w
        .manager
        .update(cx, |m, cx| m.chief_confirm(&id, Surface::Web, None, cx));
    assert!(denied.unwrap_err().contains("turned off"));
    w.manager.update(cx, |m, cx| {
        m.update_chief(|c| c.allow_web_send = true, cx);
        m.chief_confirm(&id, Surface::Web, None, cx).unwrap();
    });
    assert_eq!(proposal(&w, &id, cx).surface, Surface::Web);
}

#[gpui::test]
fn a_direct_command_to_an_idle_session_is_sent(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let view = session_view(&w, w.free, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    let id = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    confirm(&w, &id, cx).unwrap();
    let p = proposal(&w, &id, cx);
    assert_eq!(p.state, ProposalState::Sent, "{p:?}");
    assert_eq!(p.follow_up_id, None);
    assert!(outbox(&w, "mono-one", cx).is_empty());
    cx.update(|cx| assert_eq!(view.read(cx).test_launched(), vec![TEXT.to_owned()]));
}

#[gpui::test]
fn a_direct_command_to_a_busy_or_drafted_session_waits_and_leaves_the_draft(
    cx: &mut TestAppContext,
) {
    let w = world(cx, "claude");
    let view = session_view(&w, w.free, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    set_busy(&w, w.free, true, cx);
    view.update(cx, |v, cx| {
        v.input
            .update(cx, |i, cx| i.set_text("my draft".into(), cx))
    });
    let id = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    confirm(&w, &id, cx).unwrap();
    let p = proposal(&w, &id, cx);
    assert_eq!(p.state, ProposalState::QueuedInSession);
    let queued = p.follow_up_id.clone().unwrap();
    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.input.read(cx).content.to_string(), "my draft");
        assert_eq!(v.test_queued_follow_ups(), vec![queued.clone()]);
        assert_eq!(v.follow_up_status(&queued), FollowUpStatus::Queued);
    });
    // The queue delivers it: the proposal follows.
    view.update(cx, |v, cx| v.test_follow_up_delivered(&queued, cx));
    w.manager.update(cx, |m, cx| m.chief_sync_follow_ups(cx));
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::Sent);
}

#[gpui::test]
fn a_command_the_owner_removes_from_the_queue_fails(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let view = session_view(&w, w.free, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    set_busy(&w, w.free, true, cx);
    let id = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    confirm(&w, &id, cx).unwrap();
    let queued = proposal(&w, &id, cx).follow_up_id.unwrap();
    // Still queued: nothing moves.
    w.manager.update(cx, |m, cx| m.chief_sync_follow_ups(cx));
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::QueuedInSession);
    view.update(cx, |v, _| v.test_remove_follow_up(&queued));
    w.manager.update(cx, |m, cx| m.chief_sync_follow_ups(cx));
    assert_eq!(
        proposal(&w, &id, cx).state,
        ProposalState::Failed("removed from the session queue".into())
    );
}

// ---- The Mono's bound send ----

#[gpui::test]
fn a_bound_send_is_accepted_once_and_marks_the_launch(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let view = session_view(&w, w.owned, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    let (id, _) = confirmed(&w, cx);
    let reply = bound_send(
        &w,
        w.mono_chat,
        w.owned,
        &format!("Please: {TEXT}"),
        &id,
        cx,
    )
    .unwrap();
    assert_eq!(reply, json!({"accepted":true}));
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::Sent);
    cx.update(|cx| {
        assert_eq!(
            view.read(cx).test_launched(),
            vec![format!("Please: {TEXT}")]
        )
    });
    cx.update(|cx| {
        let launch = &w.manager.read(cx).roster.get("mono-one").unwrap().launches[0];
        assert_eq!(
            launch.origin,
            Some(mono::LaunchOrigin {
                proposal_id: id.clone(),
                follow_up_id: None
            })
        );
        assert!(launch.notify_on_complete);
    });
    // A second send with the same proposal is refused.
    let error = bound_send(&w, w.mono_chat, w.owned, TEXT, &id, cx).unwrap_err();
    assert!(error.contains("already used"), "{error}");
}

#[gpui::test]
fn a_bound_send_that_does_not_match_is_refused_and_recorded(cx: &mut TestAppContext) {
    // Altered text.
    let w = world(cx, "claude");
    let (id, _) = confirmed(&w, cx);
    let error = bound_send(&w, w.mono_chat, w.owned, "Add some tests", &id, cx).unwrap_err();
    assert!(error.contains("unchanged"), "{error}");
    assert!(matches!(
        proposal(&w, &id, cx).state,
        ProposalState::Refused(_)
    ));
    // The refusal used it up.
    assert!(bound_send(&w, w.mono_chat, w.owned, TEXT, &id, cx).is_err());
    // Another session.
    let (id, _) = confirmed(&w, cx);
    let error = bound_send(&w, w.mono_chat, w.free, TEXT, &id, cx).unwrap_err();
    assert!(error.contains("another session"), "{error}");
    assert!(matches!(
        proposal(&w, &id, cx).state,
        ProposalState::Refused(_)
    ));
    // Never confirmed, and one that does not exist.
    let open = propose(&w, w.owned, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    let error = bound_send(&w, w.mono_chat, w.owned, TEXT, &open, cx).unwrap_err();
    assert!(error.contains("not confirmed"), "{error}");
    assert!(proposal(&w, &open, cx).is_open());
    assert!(bound_send(&w, w.mono_chat, w.owned, TEXT, "p_00000000", cx).is_err());
}

#[gpui::test]
fn another_mono_cannot_use_a_proposal_and_does_not_burn_it(cx: &mut TestAppContext) {
    let w = world_with(cx, "claude", |roster, owned| {
        let project = roster.monos[0].projects.clone();
        roster.monos.push(Mono {
            id: "mono-two".into(),
            name: "Borealis".into(),
            session_id: None,
            projects: project.clone(),
            launches: vec![super::tests::launch_of(owned, &project[0])],
            ..Default::default()
        });
        // Only the second Mono will own it: the first dismissed it.
        roster.monos[0].launches[0].dismissed = true;
    });
    // The proposal is routed through mono-two; mono-one's chat tries to use it.
    let id = propose(&w, w.owned, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    confirm(&w, &id, cx).unwrap();
    let error = bound_send(&w, w.mono_chat, w.owned, TEXT, &id, cx).unwrap_err();
    assert!(error.contains("another Mono"), "{error}");
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::QueuedAtMono);
}

#[gpui::test]
fn a_direct_proposal_cannot_be_sent_by_a_mono(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    let error = bound_send(&w, w.mono_chat, w.free, TEXT, &id, cx).unwrap_err();
    assert!(error.contains("not confirmed"), "{error}");
    assert!(proposal(&w, &id, cx).is_open());
}

#[gpui::test]
fn a_bound_send_to_a_busy_session_queues_while_a_plain_one_is_refused(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let view = session_view(&w, w.owned, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    set_busy(&w, w.owned, true, cx);
    view.update(cx, |v, cx| {
        v.input
            .update(cx, |i, cx| i.set_text("my draft".into(), cx))
    });
    let plain = call_world(
        &w,
        w.mono_chat,
        "sessions.send",
        json!({"sessionId":w.owned,"prompt":"hi"}),
        cx,
    );
    assert!(plain.unwrap_err().contains("busy or has an unsent draft"));
    let (id, _) = confirmed(&w, cx);
    let reply = bound_send(&w, w.mono_chat, w.owned, TEXT, &id, cx).unwrap();
    assert_eq!(reply, json!({"accepted":true,"queued":true}));
    let p = proposal(&w, &id, cx);
    assert_eq!(p.state, ProposalState::QueuedInSession);
    let queued = p.follow_up_id.unwrap();
    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.test_queued_follow_ups(), vec![queued.clone()]);
        assert_eq!(v.input.read(cx).content.to_string(), "my draft");
        let launch = &w.manager.read(cx).roster.get("mono-one").unwrap().launches[0];
        assert_eq!(
            launch.origin.as_ref().unwrap().follow_up_id.as_deref(),
            Some(queued.as_str())
        );
    });
}

// ---- Completion ----

fn origin_launch(session: i64, follow_up: Option<&str>) -> mono::Launch {
    let mut launch = super::tests::launch_of(session, "/p");
    launch.notify_on_complete = true;
    launch.each = true;
    launch.origin = Some(mono::LaunchOrigin {
        proposal_id: "p_1".into(),
        follow_up_id: follow_up.map(str::to_owned),
    });
    launch
}

#[test]
fn a_chief_launch_is_not_reported_until_its_queued_message_was_delivered() {
    let launches = [origin_launch(1, Some("queued")), origin_launch(2, None)];
    let held = |l: &mono::Launch| l.session_id == 1;
    let ready = host::ready_batches(&launches, |_| false, |_| true, held);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].0, host::Batch::Each(2));
    // Delivered, and its turn over: reported.
    let ready = host::ready_batches(&launches, |_| false, |_| true, |_| false);
    assert_eq!(ready.len(), 2);
    // Delivered but its turn still runs: not yet.
    let ready = host::ready_batches(&launches, |_| false, |id| id != 1, |_| false);
    assert_eq!(ready.len(), 1);
    // In a group, one held child holds the whole report.
    let mut group = [origin_launch(1, Some("q")), origin_launch(2, None)];
    for l in &mut group {
        l.each = false;
    }
    assert!(host::ready_batches(&group, |_| false, |_| true, |l| l.session_id == 1).is_empty());
}

#[test]
fn the_chief_lead_replaces_the_normal_lead_only_when_every_session_is_the_chiefs() {
    let results = [(7, "t".to_owned(), "done".to_owned())];
    let plain = host::completion_message_for(host::Batch::Each(7), &results, &[]);
    let chief = host::completion_message_for(host::Batch::Each(7), &results, &[7]);
    assert!(chief.contains(core_chief::COMPLETION_LEAD));
    assert!(!plain.contains(core_chief::COMPLETION_LEAD));
    assert!(chief.contains("Session 7: t\ndone"));
    // A group with one Chief child keeps the group's lead and names the Chief's session.
    let group = [
        (7, "a".to_owned(), "done".to_owned()),
        (8, "b".to_owned(), "done".to_owned()),
    ];
    let mixed = host::completion_message_for(host::Batch::Group(1), &group, &[8]);
    assert!(!mixed.contains(core_chief::COMPLETION_LEAD), "{mixed}");
    assert!(
        mixed.contains("All sessions in this group have stopped"),
        "{mixed}"
    );
    assert!(mixed.contains("#8"), "{mixed}");
    assert!(!mixed.contains("#7"), "{mixed}");
    let all = host::completion_message_for(host::Batch::Group(1), &group, &[7, 8]);
    assert!(all.contains(core_chief::COMPLETION_LEAD), "{all}");
}

#[gpui::test]
fn a_launch_waits_for_its_queued_message_through_check_completions(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let view = session_view(&w, w.owned, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    set_busy(&w, w.owned, true, cx);
    let (id, _) = confirmed(&w, cx);
    bound_send(&w, w.mono_chat, w.owned, TEXT, &id, cx).unwrap();
    let queued = proposal(&w, &id, cx).follow_up_id.unwrap();
    // The session stopped, but the message still waits behind a paused queue.
    set_busy(&w, w.owned, false, cx);
    w.manager.update(cx, |m, cx| m.check_completions(cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let launch = &w.manager.read(cx).roster.get("mono-one").unwrap().launches[0];
        assert!(!launch.completed, "reported the earlier turn's result");
        assert_eq!(
            view.read(cx).follow_up_status(&queued),
            FollowUpStatus::Queued
        );
    });
}

// ---- Following the Mono's turn ----

/// The Mono's relay turn ended with `reply` as its last words.
pub(super) fn end_relay(w: &World, outbox_id: &str, reply: &str, cx: &mut TestAppContext) {
    let chat = open_mono_chat(w, cx);
    // The Mono took the message: it is no longer in its outbox.
    w.manager.update(cx, |m, _| {
        m.roster
            .get_mut("mono-one")
            .unwrap()
            .outbox
            .retain(|q| q.id != outbox_id)
    });
    chat.update(cx, |v, cx| {
        v.busy = false;
        v.mono_post("assistant", reply.into(), cx).unwrap();
    });
    w.manager.update(cx, |m, cx| {
        m.inflight.insert(
            "mono-one".into(),
            Message {
                id: outbox_id.into(),
                text: "relay".into(),
                sent_at: None,
                internal: true,
                error: None,
                attachments: Vec::new(),
            },
        );
        m.chat_changed("mono-one", cx);
    });
}

#[gpui::test]
fn a_relay_turn_without_a_bound_send_is_not_attempted_and_keeps_the_reply(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let (id, outbox_id) = confirmed(&w, cx);
    end_relay(&w, &outbox_id, "I could not find that session.", cx);
    let p = proposal(&w, &id, cx);
    assert_eq!(p.state, ProposalState::NotAttempted);
    assert_eq!(
        p.mono_reply.as_deref(),
        Some("I could not find that session.")
    );
    assert!(audit(&w, cx).len() >= 3);
}

#[gpui::test]
fn the_reply_of_a_turn_that_did_send_is_kept_without_changing_the_state(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let view = session_view(&w, w.owned, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    let (id, outbox_id) = confirmed(&w, cx);
    bound_send(&w, w.mono_chat, w.owned, TEXT, &id, cx).unwrap();
    end_relay(&w, &outbox_id, "Sent it.", cx);
    let p = proposal(&w, &id, cx);
    assert_eq!(p.state, ProposalState::Sent);
    assert_eq!(p.mono_reply.as_deref(), Some("Sent it."));
}

#[gpui::test]
fn a_relay_turn_sends_no_notification_but_other_turns_do(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let finished = Arc::new(AtomicUsize::new(0));
    let count = finished.clone();
    cx.update(|cx| {
        cx.subscribe(&w.manager, move |_, _: &NotificationFinished, _| {
            count.fetch_add(1, Ordering::SeqCst);
        })
        .detach();
    });
    let (_, outbox_id) = confirmed(&w, cx);
    end_relay(&w, &outbox_id, "ok", cx);
    assert_eq!(finished.load(Ordering::SeqCst), 0);
    // A turn on anything else notifies as before.
    end_relay(&w, "some-other-message", "ok", cx);
    assert_eq!(finished.load(Ordering::SeqCst), 1);
}

#[gpui::test]
fn a_discarded_or_failed_relay_message_fails_the_command(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let (id, outbox_id) = confirmed(&w, cx);
    w.manager.update(cx, |m, cx| {
        m.queue_changed("mono-one", &outbox_id, true, cx)
    });
    assert!(matches!(
        proposal(&w, &id, cx).state,
        ProposalState::Failed(_)
    ));
    // A relay turn that errored.
    let (id, outbox_id) = confirmed(&w, cx);
    w.manager.update(cx, |m, cx| {
        m.chief_relay_ended(&id, false, "The provider crashed", "", cx)
    });
    let _ = outbox_id;
    match proposal(&w, &id, cx).state {
        ProposalState::Failed(reason) => assert!(reason.contains("provider crashed")),
        other => panic!("{other:?}"),
    }
}

#[gpui::test]
fn deleting_or_resetting_a_mono_fails_the_commands_waiting_at_it(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let (id, _) = confirmed(&w, cx);
    w.manager.update(cx, |m, cx| m.remove_mono("mono-one", cx));
    assert_eq!(
        proposal(&w, &id, cx).state,
        ProposalState::Failed("The Mono was deleted".into())
    );
    // A reset keeps the messages still in the outbox: only taken ones fail.
    let w = world(cx, "claude");
    let (kept, _) = confirmed(&w, cx);
    let (taken, taken_id) = confirmed(&w, cx);
    w.manager.update(cx, |m, cx| {
        m.roster
            .get_mut("mono-one")
            .unwrap()
            .outbox
            .retain(|q| q.id != taken_id);
        m.chief_mono_lost("mono-one", true, "The Mono's conversation was reset", cx);
    });
    assert_eq!(proposal(&w, &kept, cx).state, ProposalState::QueuedAtMono);
    assert!(matches!(
        proposal(&w, &taken, cx).state,
        ProposalState::Failed(_)
    ));
}

#[gpui::test]
fn a_command_queued_at_a_mono_whose_message_is_gone_is_interrupted_on_load(
    cx: &mut TestAppContext,
) {
    let w = world_with(cx, "claude", |roster, owned| {
        let mut waiting = Proposal::new(
            1,
            owned,
            "owned session",
            "web",
            None,
            Route::ViaMono {
                mono_id: "mono-one".into(),
            },
            TEXT,
            Surface::Desktop,
        )
        .unwrap();
        waiting.confirm_via_mono(2, "kept".into()).unwrap();
        let mut gone = waiting.clone();
        gone.id = "p_gone".into();
        gone.outbox_id = Some("gone".into());
        roster.monos[0].outbox.push(Message {
            id: "kept".into(),
            text: "x".into(),
            sent_at: None,
            internal: true,
            error: None,
            attachments: Vec::new(),
        });
        roster.chief.as_mut().unwrap().proposals = vec![waiting, gone];
    });
    cx.update(|cx| {
        let proposals = w.manager.read(cx).chief_proposals();
        assert_eq!(proposals[0].state, ProposalState::QueuedAtMono);
        assert_eq!(
            proposals[1].state,
            ProposalState::Failed("interrupted".into())
        );
    });
    assert!(
        audit(&w, cx)
            .iter()
            .any(|line| line["id"] == "p_gone" && line["state"] == json!({"failed":"interrupted"}))
    );
}

// ---- The log and the delivery notice ----

fn notices(cx: &mut TestAppContext) -> Vec<(String, String)> {
    cx.update(|cx| cx.default_global::<crate::shell::ChiefNotices>().0.clone())
}

fn notify_on_delivery(w: &World, on: bool, cx: &mut TestAppContext) {
    w.manager.update(cx, |m, cx| {
        m.update_chief(|c| c.notify_on_delivery = on, cx)
    });
}

#[gpui::test]
fn every_state_change_appends_one_line_to_the_log(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let view = session_view(&w, w.free, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    set_busy(&w, w.free, true, cx);
    let id = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    let states = |w: &World, cx: &mut TestAppContext| -> Vec<String> {
        audit(w, cx)
            .iter()
            .map(|line| {
                assert_eq!(line["id"], id.as_str());
                line["state"].as_str().unwrap_or("?").to_owned()
            })
            .collect()
    };
    assert_eq!(states(&w, cx).len(), 1, "proposed");
    w.manager
        .update(cx, |m, cx| m.chief_edit(&id, "Another text", cx))
        .unwrap();
    assert_eq!(states(&w, cx).len(), 2, "edited");
    confirm(&w, &id, cx).unwrap();
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::QueuedInSession);
    assert_eq!(states(&w, cx).len(), 3, "confirmed into the queue");
    // A refused change (it is not open any more) writes nothing.
    assert!(
        w.manager
            .update(cx, |m, cx| m.chief_cancel(&id, cx))
            .is_err()
    );
    assert_eq!(states(&w, cx).len(), 3);
    let queued = proposal(&w, &id, cx).follow_up_id.unwrap();
    view.update(cx, |v, cx| v.test_follow_up_delivered(&queued, cx));
    w.manager.update(cx, |m, cx| m.chief_sync_follow_ups(cx));
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::Sent);
    // Delivered: one more line, and a second sync adds none.
    w.manager.update(cx, |m, cx| m.chief_sync_follow_ups(cx));
    let lines = audit(&w, cx);
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert_eq!(lines[3]["instruction"], "Another text");
}

#[gpui::test]
fn a_delivery_notice_is_posted_once_when_it_is_turned_on(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    std::fs::create_dir_all(&w.project).unwrap();
    notify_on_delivery(&w, true, cx);
    let view = session_view(&w, w.free, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    set_busy(&w, w.free, true, cx);
    let id = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    confirm(&w, &id, cx).unwrap();
    cx.run_until_parked();
    // Queued, not delivered yet: no notice.
    assert!(notices(cx).is_empty());
    let queued = proposal(&w, &id, cx).follow_up_id.unwrap();
    view.update(cx, |v, cx| v.test_follow_up_delivered(&queued, cx));
    for _ in 0..3 {
        w.manager.update(cx, |m, cx| m.chief_sync_follow_ups(cx));
        cx.run_until_parked();
    }
    let sent = notices(cx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "Chief");
    assert_eq!(sent[0].1, format!("Sent to #{} free session", w.free));
}

#[gpui::test]
fn a_direct_send_posts_the_notice_and_the_setting_off_posts_none(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let view = session_view(&w, w.free, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    // Off (the default): delivered, nothing posted.
    let off = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    confirm(&w, &off, cx).unwrap();
    cx.run_until_parked();
    assert_eq!(proposal(&w, &off, cx).state, ProposalState::Sent);
    assert!(notices(cx).is_empty());
    // On: the next delivery posts one.
    notify_on_delivery(&w, true, cx);
    let on = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    confirm(&w, &on, cx).unwrap();
    cx.run_until_parked();
    assert_eq!(notices(cx).len(), 1);
}

#[gpui::test]
fn a_muted_project_gets_no_delivery_notice(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    notify_on_delivery(&w, true, cx);
    let project = w.project.clone();
    w.window
        .update(cx, |shell, _, _| {
            shell
                .workspace
                .preferences
                .mute_notification_project(&project, None);
        })
        .unwrap();
    let view = session_view(&w, w.free, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    let id = propose(&w, w.free, None, cx).unwrap()["proposalId"]
        .as_str()
        .unwrap()
        .to_owned();
    confirm(&w, &id, cx).unwrap();
    cx.run_until_parked();
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::Sent);
    assert!(notices(cx).is_empty());
}
