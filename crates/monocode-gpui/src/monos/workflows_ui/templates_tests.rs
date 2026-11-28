use super::board_tests::{on_board, rename};
use super::tests::{click, open_tab, put, route, saved, settle};
use super::*;
use crate::monos::tests::setup;
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::model::NodeKind;
use monocode_core::mono::workflows::templates::templates;

const PROJECT: &str = "/work/app";

fn open_gallery(s: &crate::monos::tests::Setup, cx: &mut TestAppContext) -> VisualTestContext {
    put(s, vec![]);
    open_tab(s, cx);
    let mut visual = VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.manager.update(&mut visual, |m, _| {
        m.wf.scope.project = PROJECT.into();
        m.wf.scope.projects = vec![PROJECT.into()];
    });
    visual
}

#[gpui::test]
fn the_gallery_lists_every_template_grouped_by_trigger(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut open_gallery(&s, cx);
    click(visual, "mono-workflow-templates");
    settle(&s, visual);
    assert_eq!(route(&s, visual), Some(WfRoute::Templates));
    assert!(visual.debug_bounds("mono-templates-page").is_some());
    let scheduled = visual.debug_bounds("mono-templates-scheduled").unwrap();
    let events = visual.debug_bounds("mono-templates-events").unwrap();
    assert!(scheduled.top() < events.top());
    for t in templates() {
        let selector: &'static str = Box::leak(format!("mono-template-{}", t.id).into_boxed_str());
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not listed"));
        let group = if t.is_scheduled() { scheduled } else { events };
        assert!(
            bounds.top() >= group.top() && bounds.bottom() <= group.bottom(),
            "{selector} sits in its group"
        );
    }
}

#[gpui::test]
fn the_empty_list_offers_the_gallery(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut open_gallery(&s, cx);
    click(visual, "mono-workflow-templates-empty");
    assert_eq!(route(&s, visual), Some(WfRoute::Templates));
}

#[gpui::test]
fn picking_a_template_opens_a_prefilled_board_saved_paused(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut open_gallery(&s, cx);
    click(visual, "mono-workflow-templates");
    settle(&s, visual);
    click(visual, "mono-template-find-critical-bugs");
    settle(&s, visual);
    assert_eq!(route(&s, visual), Some(WfRoute::New));
    let (name, project, kinds) = on_board(&s, visual, |b| {
        let w = b.current();
        let kinds: Vec<&str> = w.nodes.iter().map(|n| n.kind.type_name()).collect();
        (w.name.clone(), w.workspace.project.clone(), kinds)
    });
    assert_eq!(name, "Find critical bugs");
    assert_eq!(project, PROJECT);
    assert_eq!(kinds, vec!["schedule", "session"]);
    assert!(on_board(&s, visual, |b| b.is_new));
    rename(&s, visual, "Bugs weekly");
    click(visual, "wf-board-save");
    visual.run_until_parked();
    let list = saved(&s);
    assert_eq!(list.len(), 1);
    let w = &list[0];
    assert!(!w.enabled);
    assert_eq!(
        (w.name.as_str(), w.workspace.project.as_str()),
        ("Bugs weekly", PROJECT)
    );
    assert!(
        matches!(&w.nodes[1].kind, NodeKind::Session { prompt, .. } if prompt.starts_with("Review recent git history"))
    );
}

#[gpui::test]
fn an_event_template_opens_with_its_event_and_the_plain_new_button_stays_blank(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let visual = &mut open_gallery(&s, cx);
    s.manager.update(visual, |m, cx| {
        m.start_from_template("review-pull-requests", cx)
    });
    visual.run_until_parked();
    settle(&s, visual);
    let (project, event, access) = on_board(&s, visual, |b| {
        let w = b.current();
        let event = match &w.nodes[0].kind {
            NodeKind::Event(e) => format!("{}/{}", e.source, e.event),
            _ => String::new(),
        };
        (
            w.workspace.project.clone(),
            event,
            w.nodes[1].kind.session_access().to_string(),
        )
    });
    assert_eq!(project, PROJECT);
    assert_eq!(event, "github/pull_request_opened");
    assert_eq!(access, "auto-accept-edits");
    // Back to the list, then the plain New: no template is left over.
    s.manager.update(visual, |m, cx| {
        m.wf.route = None;
        cx.notify();
    });
    visual.run_until_parked();
    click(visual, "mono-workflow-new");
    settle(&s, visual);
    let names = on_board(&s, visual, |b| b.current().name.clone());
    assert_eq!(names, "New workflow");
}
