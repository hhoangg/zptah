//! The access, folder and work item link controls of a session step.
use super::board::BoardPick;
use super::board_tests::*;
use super::graph::GraphEvent;
use super::inspector_event_tests::{flow, open_event};
use super::tests::{click, settle};
use crate::monos::tests::setup;
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::model::NodeKind;
use serde_json::json;

fn session(
    s: &crate::monos::tests::Setup,
    visual: &mut VisualTestContext,
) -> (Option<String>, Option<String>, Option<bool>) {
    let NodeKind::Session {
        access,
        folder,
        link_work_item,
        ..
    } = node_of(s, visual, "dev").kind
    else {
        panic!("a session")
    };
    (access, folder, link_work_item)
}

#[gpui::test]
fn the_access_picker_writes_the_mode_and_leaves_the_default_out(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["github"], Vec::new(), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("dev".into()));
    settle(&s, visual);
    assert!(visual.debug_bounds("wf-pick:Access(\"dev\")").is_some());
    pick(&s, visual, BoardPick::Access("dev".into()), "supervised");
    assert_eq!(session(&s, visual).0.as_deref(), Some("supervised"));
    pick(
        &s,
        visual,
        BoardPick::Access("dev".into()),
        "auto-accept-edits",
    );
    assert_eq!(session(&s, visual).0, None);
}

#[gpui::test]
fn full_access_after_an_event_trigger_warns_before_saving(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["github"], Vec::new(), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("dev".into()));
    settle(&s, visual);
    assert!(visual.debug_bounds("wf-node-warning").is_none());
    pick(&s, visual, BoardPick::Access("dev".into()), "full-access");
    settle(&s, visual);
    assert!(visual.debug_bounds("wf-node-warning").is_some());
    assert!(on_board(&s, visual, |b| b.changed()), "not saved yet");
    pick(&s, visual, BoardPick::Access("dev".into()), "auto");
    settle(&s, visual);
    assert!(visual.debug_bounds("wf-node-warning").is_none());
}

#[gpui::test]
fn full_access_with_no_event_trigger_has_no_warning(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("dev".into()));
    pick(&s, visual, BoardPick::Access("dev".into()), "full-access");
    settle(&s, visual);
    // Other advice about the step may show (parallel writers), but not the event one.
    let event_warning = on_board(&s, visual, |b| {
        b.warnings
            .iter()
            .any(|w| w.message.contains("after an event trigger"))
    });
    assert!(!event_warning);
}

#[gpui::test]
fn the_folder_picker_offers_the_projects_folders(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["github"], Vec::new(), cx);
    let visual = &mut visual;
    let folders = std::collections::HashMap::from([(
        s.project.clone(),
        vec![("fld-1".to_string(), "Reviews".to_string())],
    )]);
    do_with(&s, visual, |m, cx| {
        m.wf.folders = folders;
        cx.notify();
    });
    event(&s, visual, GraphEvent::Click("dev".into()));
    settle(&s, visual);
    assert!(visual.debug_bounds("wf-pick:Folder(\"dev\")").is_some());
    pick(&s, visual, BoardPick::Folder("dev".into()), "fld-1");
    assert_eq!(session(&s, visual).1.as_deref(), Some("fld-1"));
    pick(&s, visual, BoardPick::Folder("dev".into()), "");
    assert_eq!(session(&s, visual).1, None);
}

#[gpui::test]
fn the_link_switch_shows_only_after_an_event_trigger(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["github"], Vec::new(), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("dev".into()));
    settle(&s, visual);
    click(visual, "wf-link-work-item");
    assert_eq!(session(&s, visual).2, Some(true));
    click(visual, "wf-link-work-item");
    assert_eq!(session(&s, visual).2, None);
    // A workflow with no event trigger has nothing to link.
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    event(&s, &mut visual, GraphEvent::Click("dev".into()));
    settle(&s, &mut visual);
    assert!(visual.debug_bounds("wf-link-work-item").is_none());
}
