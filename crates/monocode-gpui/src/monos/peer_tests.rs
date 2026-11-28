//! `monos.list` and `monos.send`: a message to another Mono and its reply coming back.
use super::tests::{call, setup};
use super::*;
use gpui::TestAppContext;
use serde_json::json;

/// Adds a second Mono, MonoXiaoyue, on its own project. Its conversation is marked unavailable so
/// the test never starts a provider for it: queued messages stay in its outbox.
fn add_peer(s: &super::tests::Setup, accept: bool, cx: &mut TestAppContext) -> String {
    s.manager.update(cx, |m, _| {
        let mut peer = Mono {
            name: "MonoXiaoyue".into(),
            projects: vec!["/work/xiaoyue".into()],
            accept_peer_messages: accept,
            unavailable_session: Some("kept".into()),
            ..Mono::default()
        };
        peer.normalize();
        let id = peer.id.clone();
        m.roster.monos.push(peer);
        id
    })
}

#[gpui::test]
fn list_shows_other_monos_without_the_caller(cx: &mut TestAppContext) {
    let s = setup(cx);
    let peer = add_peer(&s, true, cx);
    let listed = call(&s, "list", "monos.list", json!({}), cx)
        .try_recv()
        .unwrap()
        .unwrap();
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], json!(peer));
    assert_eq!(listed[0]["name"], "MonoXiaoyue");
    assert_eq!(listed[0]["projects"], json!(["/work/xiaoyue"]));
    assert_eq!(listed[0]["acceptsMessages"], true);
}

#[gpui::test]
fn send_is_refused_while_the_target_does_not_accept(cx: &mut TestAppContext) {
    let s = setup(cx);
    let peer = add_peer(&s, false, cx);
    let error = call(
        &s,
        "closed",
        "monos.send",
        json!({"mono":"MonoXiaoyue","message":"Run the tests"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap_err();
    assert!(error.contains("does not accept"), "{error}");
    s.manager.update(cx, |m, _| {
        assert!(m.roster.get(&peer).unwrap().outbox.is_empty());
    });
    let error = call(&s, "bad", "monos.send", json!({"mono":"x","text":"y"}), cx)
        .try_recv()
        .unwrap()
        .unwrap_err();
    assert!(error.contains("does not take text"), "{error}");
}

#[gpui::test]
fn send_queues_a_labelled_peer_message_and_the_reply_comes_back(cx: &mut TestAppContext) {
    let s = setup(cx);
    let peer = add_peer(&s, true, cx);
    let sent = call(
        &s,
        "send",
        "monos.send",
        json!({"mono":"monoxiaoyue","message":"Run the xiaoyue tests"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    assert_eq!(sent["to"]["id"], json!(peer));
    assert_eq!(sent["depth"], 1);
    assert_eq!(sent["notifyOnReply"], true);
    let message_id = sent["messageId"].as_str().unwrap().to_owned();
    let sender = s.id.clone();
    s.manager.update(cx, |m, cx| {
        let target = m.roster.get(&peer).unwrap();
        let queued = target
            .outbox
            .iter()
            .find(|q| q.id == message_id)
            .expect("the message waits in the target's chat");
        assert!(queued.internal);
        assert!(queued.text.starts_with(mono::peers::REQUEST_HEADER));
        assert!(queued.text.contains("not the user"));
        assert!(queued.text.ends_with("Run the xiaoyue tests"));
        assert_eq!(target.peer_inbox[0].from, sender);
        // The sender's scope is not handed over.
        assert_eq!(target.projects, ["/work/xiaoyue"]);
        // The turn that handled it ends: its reply goes to the sender's chat.
        let before = m.roster.get(&sender).unwrap().outbox.len();
        m.peer_turn_finished(&peer, &message_id, None, mono::peers::Outcome::Stopped, cx);
        let outbox = &m.roster.get(&sender).unwrap().outbox;
        assert_eq!(outbox.len(), before + 1);
        let reply = outbox.last().unwrap();
        assert!(reply.internal && reply.text.starts_with(mono::peers::REPLY_HEADER));
        assert!(reply.text.contains("From: MonoXiaoyue"));
        assert!(m.roster.get(&peer).unwrap().peer_inbox.is_empty());
        let back = &m.roster.get(&sender).unwrap().peer_inbox;
        assert_eq!((back[0].depth, back[0].notify_on_reply), (2, false));
    });
    // A retry with the same request ID replays the receipt instead of sending twice.
    let again = call(
        &s,
        "send",
        "monos.send",
        json!({"mono":"monoxiaoyue","message":"Run the xiaoyue tests"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    assert_eq!(again["messageId"], json!(message_id));
}

#[gpui::test]
fn a_turn_started_by_a_peer_message_cannot_go_past_the_hop_limit(cx: &mut TestAppContext) {
    let s = setup(cx);
    add_peer(&s, true, cx);
    let sender = s.id.clone();
    s.manager.update(cx, |m, _| {
        // The sender's running turn handles a message at the last allowed hop.
        m.roster
            .get_mut(&sender)
            .unwrap()
            .peer_inbox
            .push(mono::peers::PeerMessage {
                message_id: "deep".into(),
                from: "other".into(),
                depth: mono::peers::MAX_DEPTH,
                notify_on_reply: false,
            });
        m.inflight.insert(
            sender.clone(),
            Message {
                id: "deep".into(),
                text: String::new(),
                sent_at: None,
                internal: true,
                error: None,
                attachments: Vec::new(),
            },
        );
    });
    let error = call(
        &s,
        "deep",
        "monos.send",
        json!({"mono":"MonoXiaoyue","message":"And again"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap_err();
    assert!(error.contains("hops"), "{error}");
}

#[test]
fn operator_threads_cannot_message_monos() {
    for action in ["monos.list", "monos.send"] {
        assert_eq!(
            monocode_core::app_access::validate_operator(action, &json!({})).unwrap_err(),
            mono::peers::MONO_ONLY
        );
    }
}

/// A second Mono with a real chat, MonoXiaoyue, so its turns can end through `chat_changed`. A
/// usage limit keeps the chat from starting a provider.
fn add_live_peer(s: &super::tests::Setup, cx: &mut TestAppContext) -> String {
    let project = std::env::temp_dir().join(format!("native-peer-{}", mono::new_id()));
    std::fs::create_dir_all(&project).unwrap();
    let project = project.to_string_lossy().into_owned();
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| {
            s.manager.update(cx, |m, cx| m.create(vec![project], w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    s.manager.update(cx, |m, cx| {
        let id = m
            .roster
            .monos
            .iter()
            .find(|x| x.id != s.id)
            .unwrap()
            .id
            .clone();
        m.roster.get_mut(&id).unwrap().name = "MonoXiaoyue".into();
        m.chats[&id].update(cx, |chat, _| {
            chat.settings.usage_limit = Some(Default::default());
        });
        id
    })
}

/// Sends a peer message and moves it the way `dispatch` does: out of the target's outbox and into
/// `inflight`. Returns the message id.
fn deliver(s: &super::tests::Setup, peer: &str, cx: &mut TestAppContext) -> String {
    let sent = call(
        s,
        "deliver",
        "monos.send",
        json!({"mono":"MonoXiaoyue","message":"Run the tests"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    let id = sent["messageId"].as_str().unwrap().to_owned();
    s.manager.update(cx, |m, _| {
        let target = m.roster.get_mut(peer).unwrap();
        let at = target.outbox.iter().position(|q| q.id == id).unwrap();
        let message = target.outbox.remove(at);
        m.inflight.insert(peer.to_owned(), message);
    });
    id
}

/// Ends the target's turn with `status` through the chat's own change notification.
fn end_turn(s: &super::tests::Setup, peer: &str, status: &str, cx: &mut TestAppContext) {
    s.manager.update(cx, |m, cx| {
        m.chats[peer].update(cx, |v, _| {
            v.set_busy_for_test(false);
            v.set_status_for_test(status);
        });
        m.chat_changed(peer, cx);
    });
}

#[gpui::test]
fn a_finished_turn_replies_to_the_sender_through_chat_changed(cx: &mut TestAppContext) {
    let s = setup(cx);
    let peer = add_live_peer(&s, cx);
    let id = deliver(&s, &peer, cx);
    end_turn(&s, &peer, "Ready", cx);
    s.manager.update(cx, |m, _| {
        assert!(!m.inflight.contains_key(&peer));
        assert!(m.roster.get(&peer).unwrap().peer_inbox.is_empty());
        let reply = m.roster.get(&s.id).unwrap().outbox.last().unwrap();
        assert!(reply.text.starts_with(mono::peers::REPLY_HEADER), "{id}");
        assert!(reply.text.contains("From: MonoXiaoyue"));
        assert!(m.roster.get(&peer).unwrap().outbox.is_empty());
    });
}

#[gpui::test]
fn a_stopped_turn_tells_the_sender_it_was_stopped(cx: &mut TestAppContext) {
    let s = setup(cx);
    let peer = add_live_peer(&s, cx);
    deliver(&s, &peer, cx);
    end_turn(&s, &peer, "Stopped by the user", cx);
    s.manager.update(cx, |m, _| {
        let reply = m.roster.get(&s.id).unwrap().outbox.last().unwrap();
        assert!(
            reply.text.contains("The user stopped its turn"),
            "{}",
            reply.text
        );
        assert!(m.roster.get(&peer).unwrap().peer_inbox.is_empty());
        assert!(m.roster.get(&peer).unwrap().outbox.is_empty());
    });
}

#[gpui::test]
fn a_failed_turn_keeps_waiting_and_the_retry_replies(cx: &mut TestAppContext) {
    let s = setup(cx);
    let peer = add_live_peer(&s, cx);
    let id = deliver(&s, &peer, cx);
    let sender_before = s
        .manager
        .update(cx, |m, _| m.roster.get(&s.id).unwrap().outbox.len());
    // The message left the outbox when it was delivered: the failure queues it again at the front
    // under a fresh id, and the peer entry follows it.
    end_turn(&s, &peer, "Error: the provider failed", cx);
    let retry = s.manager.update(cx, |m, _| {
        let target = m.roster.get(&peer).unwrap();
        assert_eq!(target.outbox.len(), 1);
        let queued = &target.outbox[0];
        assert_ne!(queued.id, id, "a fresh id");
        assert!(queued.error.is_some());
        assert_eq!(target.peer_inbox.len(), 1);
        assert_eq!(target.peer_inbox[0].message_id, queued.id);
        assert_eq!(
            m.roster.get(&s.id).unwrap().outbox.len(),
            sender_before,
            "no reply yet: the retry will answer"
        );
        queued.id.clone()
    });
    // The user retries and the turn succeeds: the reply reaches the sender.
    s.manager.update(cx, |m, _| {
        let target = m.roster.get_mut(&peer).unwrap();
        let message = target.outbox.remove(0);
        m.inflight.insert(peer.clone(), message);
    });
    end_turn(&s, &peer, "Ready", cx);
    s.manager.update(cx, |m, _| {
        assert!(m.roster.get(&peer).unwrap().peer_inbox.is_empty());
        let reply = m.roster.get(&s.id).unwrap().outbox.last().unwrap();
        assert!(reply.text.starts_with(mono::peers::REPLY_HEADER), "{retry}");
    });
}

#[gpui::test]
fn a_failed_turn_whose_message_is_still_queued_keeps_its_id(cx: &mut TestAppContext) {
    let s = setup(cx);
    let peer = add_live_peer(&s, cx);
    let id = deliver(&s, &peer, cx);
    // The message is queued again (a retry in flight) before the failed turn ends.
    s.manager.update(cx, |m, _| {
        let message = m.inflight[&peer].clone();
        m.roster.get_mut(&peer).unwrap().outbox.push(message);
    });
    end_turn(&s, &peer, "Error: the provider failed", cx);
    s.manager.update(cx, |m, _| {
        let target = m.roster.get(&peer).unwrap();
        assert_eq!(target.outbox.len(), 1);
        assert_eq!(target.outbox[0].id, id);
        assert!(target.outbox[0].error.is_some());
        assert_eq!(target.peer_inbox[0].message_id, id);
    });
}

#[gpui::test]
fn a_turn_without_a_peer_entry_routes_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    let peer = add_live_peer(&s, cx);
    let before = s
        .manager
        .update(cx, |m, _| m.roster.get(&s.id).unwrap().outbox.len());
    s.manager.update(cx, |m, _| {
        m.inflight.insert(
            peer.clone(),
            Message {
                id: "typed".into(),
                text: String::new(),
                sent_at: None,
                internal: false,
                error: None,
                attachments: Vec::new(),
            },
        );
    });
    end_turn(&s, &peer, "Ready", cx);
    s.manager.update(cx, |m, _| {
        assert_eq!(m.roster.get(&s.id).unwrap().outbox.len(), before);
    });
}

#[gpui::test]
fn habit_runs_heartbeats_and_questions_cannot_message_other_monos(cx: &mut TestAppContext) {
    let s = setup(cx);
    let peer = add_peer(&s, true, cx);
    let run = super::tests::fake_run(&s, cx);
    let heartbeat = super::tests::fake_run(&s, cx);
    s.manager.update(cx, |m, _| {
        m.running.get_mut(&heartbeat).unwrap().habit = mono::heartbeat::RUN_ID.into();
    });
    for session in [run, heartbeat] {
        let refused = super::tests::call_from(
            &s,
            session,
            &format!("{session}-send"),
            "monos.send",
            json!({"mono":"MonoXiaoyue","message":"Run the tests"}),
            cx,
        )
        .try_recv()
        .unwrap();
        assert_eq!(
            refused,
            Err("A habit run, heartbeat or workflow question cannot message other Monos".into())
        );
        // Listing stays allowed.
        let listed = super::tests::call_from(
            &s,
            session,
            &format!("{session}-list"),
            "monos.list",
            json!({}),
            cx,
        )
        .try_recv()
        .unwrap();
        assert!(listed.is_ok(), "{listed:?}");
    }
    s.manager.update(cx, |m, _| {
        assert!(m.roster.get(&peer).unwrap().outbox.is_empty());
        assert!(m.roster.get(&peer).unwrap().peer_inbox.is_empty());
    });
}
