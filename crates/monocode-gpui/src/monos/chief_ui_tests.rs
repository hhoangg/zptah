//! The Chief on screen: what it may read, its settings, the rail and the panel, the proposal
//! cards, the relay label in a Mono's chat and the Chief page of Settings.
use super::chief_command_tests::{
    TEXT, confirm, open_mono_chat, outbox, proposal, propose, session_view,
};
use super::chief_tests::{World, call_world, world};
use super::chief_ui::CHIEF_ID;
use super::*;
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use monocode_core::{
    mono::chief::{self as core_chief, ProposalState},
    tool::{ToolCall, ToolState},
    transcript::Row,
};
use serde_json::{Value, json};

fn row(role: &str, text: &str) -> Row {
    Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: None,
    }
}

fn proposal_id(reply: &Value) -> String {
    reply["proposalId"].as_str().unwrap().to_owned()
}

/// The rows a Chief turn leaves when it called the app tool (`chief.propose`, how the Chief
/// really calls it) and got `id` back.
fn propose_turn(id: &str) -> Vec<Row> {
    turn_with_call(
        "mcp__monocode_app__app",
        r#"{"action":"chief.propose","input":{}}"#,
        id,
    )
}

/// The same turn through the shell CLI.
fn propose_turn_cli(id: &str) -> Vec<Row> {
    turn_with_call(
        "Bash",
        r#"{"command":"monocode app chief.propose --json {}"}"#,
        id,
    )
}

fn turn_with_call(name: &str, input: &str, id: &str) -> Vec<Row> {
    let call = serde_json::to_string(&ToolCall {
        id: "t".into(),
        name: name.into(),
        input: input.into(),
        output: format!(
            r#"{{"ok":true,"result":{{"proposalId":"{id}","status":"awaitingOwner"}}}}"#
        ),
        state: ToolState::Completed,
        panel: None,
        display: None,
    })
    .unwrap();
    vec![
        row("user", "Ask the session to add tests"),
        row("tool", &call),
        row("assistant", "I drafted a command for you to check."),
    ]
}

#[test]
fn a_proposal_is_found_in_the_app_tool_shape_and_in_the_cli_shape() {
    for rows in [propose_turn("p_0000000a"), propose_turn_cli("p_0000000a")] {
        let found = crate::transcript::chief_card::run_proposals(&rows, 0, rows.len(), |_| true);
        assert_eq!(found, vec!["p_0000000a".to_owned()]);
    }
    // Another action of the app tool is not a proposal.
    let mut other = propose_turn("p_0000000a");
    other[1].text = other[1].text.replace("chief.propose", "memory.add");
    assert!(
        crate::transcript::chief_card::run_proposals(&other, 0, other.len(), |_| true).is_empty()
    );
}

#[gpui::test]
fn an_open_proposal_no_turn_placed_is_drawn_after_the_last_turn(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    open_mono_chat(&w, cx);
    let id = proposal_id(&propose(&w, w.owned, None, cx).unwrap());
    // A turn that never shows the call.
    post(
        &chief_chat(&w, cx),
        vec![row("user", "Hello"), row("assistant", "Hi")],
        cx,
    );
    sync(&w, cx);
    let mut visual = panel(&w, CHIEF_ID, cx);
    for button in ["send", "edit", "cancel"] {
        assert!(
            bounds(&mut visual, format!("chief-{button}-{id}")).is_some(),
            "{button}"
        );
    }
    // Once the turn that made it shows up, it is drawn once, in place.
    post(&chief_chat(&w, cx), propose_turn(&id), cx);
    sync(&w, cx);
    let chat = chief_chat(&w, cx);
    let count = cx.update(|cx| chat.read(cx).transcript.read(cx).proposal_rows(&id));
    assert_eq!(count, 1);
}

/// Adds `rows` after what the chat already shows.
fn post(chat: &Entity<Conversation>, rows: Vec<Row>, cx: &mut TestAppContext) {
    let transcript = cx.update(|cx| chat.read(cx).transcript.clone());
    transcript.update(cx, |t, cx| {
        let at = t.source_rows().len();
        t.patch(at, rows, cx);
    });
}

fn chief_chat(w: &World, cx: &mut TestAppContext) -> Entity<Conversation> {
    cx.update(|cx| w.manager.read(cx).chief_chat().cloned().unwrap())
}

fn sync(w: &World, cx: &mut TestAppContext) {
    w.manager.update(cx, |m, cx| m.chief_sync_cards_now(cx));
    cx.run_until_parked();
}

/// Opens the floating panel on `id` and returns a context drawing it.
fn panel(w: &World, id: &str, cx: &mut TestAppContext) -> VisualTestContext {
    w.manager
        .update(cx, |m, cx| m.open_floating(id.to_owned(), cx));
    cx.run_until_parked();
    let handle: AnyWindowHandle = cx.update(|cx| *w.manager.read(cx).floating.last().unwrap());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual
}

fn bounds(visual: &mut VisualTestContext, selector: String) -> Option<gpui::Bounds<gpui::Pixels>> {
    visual.debug_bounds(Box::leak(selector.into_boxed_str()))
}

fn click(visual: &mut VisualTestContext, selector: &'static str) {
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is drawn"));
    visual.simulate_click(bounds.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
}

fn card_of(w: &World, id: &str, cx: &mut TestAppContext) -> crate::transcript::ChiefCard {
    cx.update(|cx| {
        let chat = w.manager.read(cx).chief_chat().unwrap().read(cx);
        chat.transcript
            .read(cx)
            .chief_cards()
            .cards
            .get(id)
            .cloned()
            .expect("the card is known")
    })
}

// ---- What the Chief may read ----

#[gpui::test]
fn the_chief_reads_a_session_a_mono_launched_but_not_chats_or_habit_runs(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    // The Mono's own launch is hidden from the sidebar, and still the Chief's to report on.
    cx.update(|cx| assert!(w.manager.read(cx).hidden_session(w.owned)));
    let read = |session: i64, cx: &mut TestAppContext| {
        call_world(
            &w,
            w.chief_chat,
            "chief.readSession",
            json!({"sessionId":session}),
            cx,
        )
    };
    assert_eq!(read(w.owned, cx).unwrap()["sessionId"], json!(w.owned));
    // A Mono's chat and a habit run are refused.
    let run = locked(&w.store)
        .unwrap()
        .create("codex", &w.project)
        .unwrap()
        .id;
    w.manager.update(cx, |m, _| {
        m.roster
            .get_mut("mono-one")
            .unwrap()
            .habit_sessions
            .push(run)
    });
    for refused in [w.mono_chat, run] {
        assert_eq!(
            read(refused, cx).unwrap_err(),
            "This session is not one the Chief can read"
        );
    }
}

#[gpui::test]
fn the_overview_lists_a_session_a_mono_launched_and_hid(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    cx.update(|cx| assert!(w.manager.read(cx).hidden_session(w.owned)));
    let overview = call_world(&w, w.chief_chat, "chief.overview", json!({}), cx).unwrap();
    let listed: Vec<i64> = overview["projects"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["sessions"].as_array().unwrap().clone())
        .map(|s| s["id"].as_i64().unwrap())
        .collect();
    assert!(listed.contains(&w.owned), "{overview}");
    assert!(!listed.contains(&w.mono_chat) && !listed.contains(&w.chief_chat));
}

#[gpui::test]
fn a_card_notes_a_paused_queue(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let view = session_view(&w, w.free, cx);
    view.update(cx, |v, _| v.test_pause_queue());
    let reply = propose(&w, w.free, None, cx).unwrap();
    assert_eq!(reply["notes"], json!(["queuePaused"]));
    let p = proposal(&w, &proposal_id(&reply), cx);
    cx.update(|cx| {
        assert_eq!(
            w.manager.read(cx).chief_card_notes(&p, cx),
            vec!["queuePaused"]
        )
    });
    // The card says it in words.
    sync(&w, cx);
    assert_eq!(
        card_of(&w, &p.id, cx).notes,
        vec!["The session's queue is paused, so it waits until you resume it".to_owned()]
    );
}

// ---- Settings reconfigure the open chat ----

#[gpui::test]
fn changing_the_settings_reconfigures_the_open_chat(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let chat = chief_chat(&w, cx);
    cx.update(|cx| assert_eq!(chat.read(cx).settings.effort, None));
    w.manager.update(cx, |m, cx| {
        m.update_chief(
            |c| {
                c.name = "Skipper".into();
                c.model = Some("claude-opus-9".into());
                c.effort = Some("high".into());
            },
            cx,
        )
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let chat = chat.read(cx);
        assert_eq!(chat.settings.effort.as_deref(), Some("high"));
        assert_eq!(chat.mono_model().as_deref(), Some("claude-opus-9"));
        let context = chat.settings.mono.as_ref().unwrap();
        assert_eq!(context.mono.name, "Skipper");
        assert_eq!(chat.summary.provider, "claude");
    });
    // Clearing the effort goes back to the chat's quick default.
    w.manager
        .update(cx, |m, cx| m.update_chief(|c| c.effort = None, cx));
    cx.update(|cx| assert_eq!(chat.read(cx).settings.effort.as_deref(), Some("low")));
}

#[gpui::test]
fn another_harness_keeps_the_chat_closed_to_the_owner_with_a_status(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    w.manager.update(cx, |m, cx| {
        m.update_chief(|c| c.harness = Some("codex".into()), cx)
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let m = w.manager.read(cx);
        assert_eq!(
            m.chief_status.as_deref(),
            Some(monocode_core::provider::CHIEF_HARNESS_REFUSAL)
        );
        // The chat it has keeps its own harness.
        assert_eq!(m.chief_chat().unwrap().read(cx).summary.provider, "codex");
    });
    let error = w
        .manager
        .update(cx, |m, cx| m.chief_send("hello".into(), cx))
        .unwrap_err();
    assert!(error.contains("Claude Code only"), "{error}");
    // Back on Claude, the status goes.
    w.manager.update(cx, |m, cx| {
        m.update_chief(|c| c.harness = Some("claude".into()), cx)
    });
    cx.update(|cx| assert!(w.manager.read(cx).chief_status.is_none()));
}

#[gpui::test]
fn the_chief_defaults_to_claude_without_a_harness(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    w.manager
        .update(cx, |m, cx| m.update_chief(|c| c.harness = None, cx));
    cx.update(|cx| assert_eq!(w.manager.read(cx).chief_kind(), Kind::Claude));
}

// ---- The panel and the rail ----

#[gpui::test]
fn the_chief_is_pinned_first_with_a_tag_and_a_badge_while_commands_wait(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let visual = &mut panel(&w, CHIEF_ID, cx);
    let chief = visual
        .debug_bounds("floating-mono-button-chief")
        .expect("the Chief is in the rail");
    let mono = visual
        .debug_bounds(Box::leak(
            format!("floating-mono-button-{}", w.mono).into_boxed_str(),
        ))
        .expect("the Mono is in the rail");
    assert!(chief.top() < mono.top(), "the Chief is first");
    assert!(visual.debug_bounds("chief-tag").is_some());
    assert!(visual.debug_bounds("floating-chief-badge").is_none());
    // Selecting it opens the Chief's chat in the panel.
    let selected = cx.update(|cx| {
        let handle = *w.manager.read(cx).floating.last().unwrap();
        handle
            .downcast::<floating::Floating>()
            .unwrap()
            .read(cx)
            .unwrap()
            .selected
            .clone()
    });
    assert_eq!(selected, CHIEF_ID);
    // One command waits for Send.
    propose(&w, w.free, None, cx).unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    cx.update(|cx| assert_eq!(w.manager.read(cx).chief_open_count(), 1));
    assert!(visual.debug_bounds("floating-chief-badge").is_some());
    assert!(
        visual.debug_bounds("floating-status").is_some(),
        "its status label shows"
    );
}

#[gpui::test]
fn picking_the_chief_in_the_rail_opens_its_chat(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let visual = &mut panel(&w, &w.mono.clone(), cx);
    click(visual, "floating-mono-button-chief");
    let (selected, chief_chat_id) = cx.update(|cx| {
        let m = w.manager.read(cx);
        let handle = *m.floating.last().unwrap();
        let view = handle.downcast::<floating::Floating>().unwrap();
        (
            view.read(cx).unwrap().selected.clone(),
            m.chief_chat().unwrap().read(cx).summary.id,
        )
    });
    assert_eq!(selected, CHIEF_ID);
    assert_eq!(chief_chat_id, w.chief_chat);
    // A Chief that is off is not in the rail and cannot be opened.
    w.manager
        .update(cx, |m, cx| m.update_chief(|c| c.enabled = false, cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("floating-mono-button-chief").is_none());
}

#[gpui::test]
fn the_sidebar_rail_pins_the_chief_above_the_monos(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let visual = &mut VisualTestContext::from_window(w.window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let Some(chief) = visual.debug_bounds("chief-rail") else {
        // The layout does not list the Monos here; the panel test covers the pinning.
        return;
    };
    let mono = visual
        .debug_bounds(Box::leak(format!("mono-rail-{}", w.mono).into_boxed_str()))
        .unwrap();
    assert!(chief.top() < mono.top());
    assert!(visual.debug_bounds("chief-tag").is_some());
}

#[gpui::test]
fn picking_the_chief_in_the_main_window_shows_its_chat_without_mono_panels(
    cx: &mut TestAppContext,
) {
    let w = world(cx, "claude");
    let chat = chief_chat(&w, cx);
    w.window
        .update(cx, |shell, window, cx| {
            shell
                .mono_manager()
                .update(cx, |m, cx| m.open(CHIEF_ID, window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let m = w.manager.read(cx);
        assert_eq!(m.active.as_deref(), Some(CHIEF_ID));
        assert!(m.chief_active());
        // No Mono is current, so no Mono panel can draw; the chat is the Chief's.
        assert!(m.current().is_none());
        assert_eq!(m.chat().map(|c| c.entity_id()), Some(chat.entity_id()));
        assert!(m.identity_for(CHIEF_ID).is_some());
    });
    let visual = &mut VisualTestContext::from_window(w.window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("chief-main").is_some());
    assert!(visual.debug_bounds("chief-main-settings").is_some());
    assert!(visual.debug_bounds("chief-main-history").is_some());
    assert!(visual.debug_bounds("mono-settings").is_none());
    // History asks for Settings > Chief with the history section first.
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = asked.clone();
    let _watch = cx.update(|cx| {
        cx.subscribe(&w.manager, move |_, event: &super::ShowChiefSettings, _| {
            seen.borrow_mut().push(event.history)
        })
    });
    click(visual, "chief-main-history");
    assert_eq!(*asked.borrow(), vec![true]);
    // Turning the Chief off leaves the view.
    w.manager
        .update(visual, |m, cx| m.update_chief(|c| c.enabled = false, cx));
    visual.run_until_parked();
    cx.update(|cx| assert!(!w.manager.read(cx).chief_active()));
}

// ---- The proposal cards ----

fn chief_panel_with_card(
    w: &World,
    session: i64,
    cx: &mut TestAppContext,
) -> (VisualTestContext, String) {
    let id = proposal_id(&propose(w, session, None, cx).unwrap());
    post(&chief_chat(w, cx), propose_turn(&id), cx);
    sync(w, cx);
    let mut visual = panel(w, CHIEF_ID, cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    (visual, id)
}

#[gpui::test]
fn a_card_shows_the_route_the_full_text_and_the_notes(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    open_mono_chat(&w, cx);
    let (mut visual, id) = {
        super::chief_command_tests::set_busy(&w, w.owned, true, cx);
        chief_panel_with_card(&w, w.owned, cx)
    };
    let card = card_of(&w, &id, cx);
    assert!(card.route.starts_with("Atlas \u{2192} #"), "{}", card.route);
    assert!(card.route.contains("owned session"), "{}", card.route);
    assert_eq!(card.instruction, TEXT);
    assert_eq!(
        card.notes,
        vec![
            "Will wait in the session queue".to_owned(),
            "The Mono is busy and will take it after its current turn".to_owned()
        ]
    );
    assert!(card.open && card.state == "Waiting for your Send");
    assert!(bounds(&mut visual, format!("chief-card-{id}")).is_some());
    for button in ["send", "edit", "cancel"] {
        assert!(
            bounds(&mut visual, format!("chief-{button}-{id}")).is_some(),
            "{button}"
        );
    }
    // A direct command names no Mono.
    let direct = proposal_id(&propose(&w, w.free, None, cx).unwrap());
    sync(&w, cx);
    assert!(
        card_of(&w, &direct, cx)
            .route
            .starts_with("Directly \u{2192} #")
    );
}

#[gpui::test]
fn send_on_a_card_confirms_it_and_the_chip_follows(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    open_mono_chat(&w, cx);
    let (mut visual, id) = chief_panel_with_card(&w, w.owned, cx);
    click(
        &mut visual,
        Box::leak(format!("chief-send-{id}").into_boxed_str()),
    );
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::QueuedAtMono);
    assert_eq!(outbox(&w, &w.mono, cx).len(), 1);
    let card = card_of(&w, &id, cx);
    assert_eq!(card.state, "Waiting for Atlas");
    assert!(!card.open);
    // The buttons are gone once it left the owner's hands.
    assert!(bounds(&mut visual, format!("chief-send-{id}")).is_none());
}

#[gpui::test]
fn send_on_a_direct_card_to_an_idle_session_marks_it_sent(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let view = session_view(&w, w.free, cx);
    view.update(cx, |v, cx| v.load(cx));
    cx.run_until_parked();
    let (mut visual, id) = chief_panel_with_card(&w, w.free, cx);
    click(
        &mut visual,
        Box::leak(format!("chief-send-{id}").into_boxed_str()),
    );
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::Sent);
    assert_eq!(card_of(&w, &id, cx).state, "Sent");
}

#[gpui::test]
fn cancel_on_a_card_cancels_it(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let (mut visual, id) = chief_panel_with_card(&w, w.free, cx);
    click(
        &mut visual,
        Box::leak(format!("chief-cancel-{id}").into_boxed_str()),
    );
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::Cancelled);
    assert_eq!(card_of(&w, &id, cx).state, "Cancelled");
    cx.update(|cx| assert_eq!(w.manager.read(cx).chief_open_count(), 0));
}

#[gpui::test]
fn edit_on_a_card_changes_the_text_that_is_sent(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let (mut visual, id) = chief_panel_with_card(&w, w.free, cx);
    click(
        &mut visual,
        Box::leak(format!("chief-edit-{id}").into_boxed_str()),
    );
    assert!(bounds(&mut visual, format!("chief-editor-{id}")).is_some());
    assert!(bounds(&mut visual, format!("chief-send-{id}")).is_none());
    let editor = card_of(&w, &id, cx)
        .editor
        .expect("an inline editor opened");
    editor.update(&mut visual, |input, cx| {
        input.set_text("Only run the auth tests".into(), cx)
    });
    click(
        &mut visual,
        Box::leak(format!("chief-save-{id}").into_boxed_str()),
    );
    assert_eq!(proposal(&w, &id, cx).instruction, "Only run the auth tests");
    let card = card_of(&w, &id, cx);
    assert_eq!(card.instruction, "Only run the auth tests");
    assert!(card.editor.is_none() && card.open);
    // Discard leaves the text alone.
    click(
        &mut visual,
        Box::leak(format!("chief-edit-{id}").into_boxed_str()),
    );
    click(
        &mut visual,
        Box::leak(format!("chief-discard-{id}").into_boxed_str()),
    );
    assert_eq!(proposal(&w, &id, cx).instruction, "Only run the auth tests");
    assert!(card_of(&w, &id, cx).editor.is_none());
}

#[gpui::test]
fn a_refused_send_shows_its_reason_on_the_card(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let (mut visual, id) = chief_panel_with_card(&w, w.owned, cx);
    // The Mono's chat is unavailable, so the route cannot be taken.
    w.manager.update(&mut visual, |m, _| {
        m.roster.get_mut("mono-one").unwrap().unavailable_session = Some("gone".into())
    });
    click(
        &mut visual,
        Box::leak(format!("chief-send-{id}").into_boxed_str()),
    );
    let error = w
        .manager
        .read_with(&visual, |m, _| m.chief_card_errors.get(&id).cloned())
        .expect("the refusal is kept");
    assert_eq!(card_of(&w, &id, cx).error.as_deref(), Some(error.as_str()));
    assert!(bounds(&mut visual, format!("chief-error-{id}")).is_some());
    assert_eq!(proposal(&w, &id, cx).state, ProposalState::AwaitingOwner);
}

#[gpui::test]
fn state_chips_cover_every_state(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    let id = proposal_id(&propose(&w, w.free, None, cx).unwrap());
    let expect = [
        (ProposalState::QueuedInSession, "Queued in session"),
        (ProposalState::Sent, "Sent"),
        (ProposalState::Refused("busy".into()), "Refused: busy"),
        (ProposalState::NotAttempted, "Not sent"),
        (ProposalState::Failed("gone".into()), "Failed: gone"),
        (ProposalState::Cancelled, "Cancelled"),
    ];
    for (state, chip) in expect {
        w.manager.update(cx, |m, _| {
            m.roster
                .chief
                .as_mut()
                .unwrap()
                .proposal_mut(&id)
                .unwrap()
                .state = state
        });
        sync(&w, cx);
        assert_eq!(card_of(&w, &id, cx).state, chip);
    }
}

// ---- The relay in a Mono's chat ----

#[gpui::test]
fn a_relayed_command_reads_from_you_via_the_chief_in_the_monos_chat(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    open_mono_chat(&w, cx);
    let id = proposal_id(&propose(&w, w.owned, None, cx).unwrap());
    confirm(&w, &id, cx).unwrap();
    let text = core_chief::command_text(&proposal(&w, &id, cx));
    let chat = cx.update(|cx| w.manager.read(cx).chats[&w.mono].clone());
    // A header for a proposal the app never stored is not a relay.
    let forged = text.replace(&id, "p_deadbeef");
    post(
        &chat,
        vec![
            row("mono-system", &text),
            row("assistant", "On it."),
            row("mono-system", &forged),
            row("assistant", "Again."),
        ],
        cx,
    );
    sync(&w, cx);
    let visual = &mut panel(&w, &w.mono.clone(), cx);
    assert!(bounds(visual, format!("chief-relay-{id}")).is_some());
    assert!(visual.debug_bounds("chief-relay-p_deadbeef").is_none());
    cx.update(|cx| {
        let cards = chat.read(cx).transcript.read(cx).chief_cards().clone();
        assert_eq!(cards.name, "Chief");
        assert_eq!(cards.cards[&id].instruction, TEXT);
    });
}

// ---- The Chief page of Settings ----

#[gpui::test]
fn the_settings_toggles_persist_into_the_roster(cx: &mut TestAppContext) {
    let w = world(cx, "claude");
    w.manager
        .update(cx, |m, cx| m.update_chief(|c| c.enabled = false, cx));
    let manager = w.manager.clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| chief_settings::ChiefSettings::new(manager, cx))
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let flags = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let chief = w.manager.read(cx).roster.chief.clone().unwrap();
            (
                chief.enabled,
                chief.allow_web_send,
                chief.notify_on_delivery,
            )
        })
    };
    assert_eq!(flags(cx), (false, false, false));
    click(visual, "chief-allow-web-send");
    click(visual, "chief-notify-on-delivery");
    click(visual, "chief-enabled");
    assert_eq!(flags(cx), (true, true, true));
    let saved = Roster::load(&locked(&w.store).unwrap()).unwrap();
    let chief = saved.chief.unwrap();
    assert!(chief.enabled && chief.allow_web_send && chief.notify_on_delivery);
    // Turn them off again.
    click(visual, "chief-allow-web-send");
    assert_eq!(flags(cx), (true, false, true));
    assert!(visual.debug_bounds("chief-soul-editor").is_some());
    assert!(visual.debug_bounds("chief-memory").is_some());
    assert!(visual.debug_bounds("chief-name-field").is_some());
}

// ---- The shortcut ----

#[test]
fn the_ask_chief_shortcut_registers_only_when_it_is_set() {
    let mut prefs = crate::settings::Preferences::default();
    assert_eq!(crate::quick_composer::chief_shortcut(&prefs), None);
    prefs
        .keybindings
        .insert("App: Ask Chief".into(), Some("cmd-shift-k".into()));
    assert_eq!(
        crate::quick_composer::chief_shortcut(&prefs).as_deref(),
        Some("cmd-shift-k")
    );
    // Disabled or cleared is the same as not set.
    prefs.keybindings.insert("App: Ask Chief".into(), None);
    assert_eq!(crate::quick_composer::chief_shortcut(&prefs), None);
    prefs
        .keybindings
        .insert("App: Ask Chief".into(), Some("  ".into()));
    assert_eq!(crate::quick_composer::chief_shortcut(&prefs), None);
    // The binding is listed with no keys by default.
    assert!(
        crate::settings::command_shortcut(
            &crate::settings::Preferences::default(),
            "App: Ask Chief"
        )
        .is_some_and(|keys| keys.is_empty())
    );
    let mut controller = crate::quick_composer::Controller::default();
    assert!(controller.configure_chief(&prefs).is_none());
}
