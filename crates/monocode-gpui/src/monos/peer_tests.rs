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
