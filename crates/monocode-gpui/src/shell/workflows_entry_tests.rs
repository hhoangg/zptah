//! The Workflows app view: its sidebar entries, and the zap badge of the sessions it starts.
use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};
use monocode_core::mono::workflows::{
    engine::start_run,
    model::{CallerKind, NodeRun, RunCaller, Workflow},
    store::{update_runs, update_workflows},
};

/// `classic` + `rail`: full rail, compact rail; `!classic`: the new layout.
fn setup(cx: &mut TestAppContext, classic: bool, rail: bool) -> (WindowHandle<Shell>, i64) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (window, _, session) = super::tests::window(cx);
    cx.simulate_window_resize(window.into(), gpui::size(px(1500.), px(900.)));
    window
        .update(cx, |shell, window, cx| {
            shell.workspace.classic_layout = classic;
            shell.workspace.sidebar_visible = true;
            shell.workspace.project_rail_open = rail;
            shell.workspace.right_panel_open = false;
            shell.open(session, window, cx);
            shell.layout_changed(cx);
        })
        .unwrap();
    cx.run_until_parked();
    (window, session)
}

const LAYOUTS: [(&str, bool, bool, &str); 3] = [
    ("new layout", false, true, "sidebar-navigation-Workflows"),
    ("full rail", true, true, "sidebar-navigation-Workflows"),
    ("compact rail", true, false, "rail-workflows"),
];

fn shown(window: WindowHandle<Shell>, visual: &mut VisualTestContext) -> bool {
    window
        .read_with(visual, |shell, _| {
            shell.active_app_view == Some(app_views::AppView::Workflows)
        })
        .unwrap()
}

#[gpui::test]
fn the_sidebar_entry_opens_the_workflows_page_in_every_layout(cx: &mut TestAppContext) {
    for (name, classic, rail, selector) in LAYOUTS {
        let (window, _) = setup(cx, classic, rail);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        assert!(!shown(window, visual), "{name}");
        let entry = visual.debug_bounds(selector).expect(name);
        visual.simulate_click(entry.center(), Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(shown(window, visual), "{name}");
        assert!(visual.debug_bounds("workflows-view").is_some(), "{name}");
        assert_eq!(app_views::AppView::Workflows.label(), "Workflows");
        assert_eq!(app_views::AppView::Workflows.icon(), "zap");
        // Automations stays where it was, with its own icon.
        assert_eq!(app_views::AppView::Automations.icon(), "refresh-cw");
    }
}

#[gpui::test]
fn the_workflows_entry_has_no_visibility_toggle_of_its_own(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    window
        .read_with(visual, |shell, _| {
            let labels: Vec<String> = shell
                .sidebar_options_menu_items()
                .into_iter()
                .filter_map(|item| match item {
                    MenuItem::Action(action) => Some(action.label.to_string()),
                    _ => None,
                })
                .collect();
            assert!(
                !labels.iter().any(|label| label == "Workflows"),
                "{labels:?}"
            );
            assert!(
                labels.iter().any(|label| label == "Automations"),
                "{labels:?}"
            );
        })
        .unwrap();
}

fn flagged_workflow() -> Workflow {
    let mut w = Workflow::new("wf-aaaaaaaa", "Nightly checks");
    w.nodes = vec![
        serde_json::from_value(serde_json::json!({"id": "start", "type": "manual"}))
            .expect("a trigger node"),
    ];
    w
}

#[gpui::test]
fn a_session_a_workflow_started_gets_the_zap_badge(cx: &mut TestAppContext) {
    let (window, session) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(
        !window
            .read_with(visual, |s, _| s.session_zap(session))
            .unwrap()
    );
    window
        .update(visual, |shell, _, cx| {
            shell.workflow_session_started(session, cx)
        })
        .unwrap();
    assert!(
        window
            .read_with(visual, |s, _| s.session_zap(session))
            .unwrap()
    );
    // An automation's session still counts.
    window
        .update(visual, |shell, _, _| {
            shell
                .automation_state
                .session_automations
                .insert(session + 1000, "automation".into());
        })
        .unwrap();
    assert!(
        window
            .read_with(visual, |s, _| s.session_zap(session + 1000))
            .unwrap()
    );
}

#[gpui::test]
fn the_zap_badge_is_read_back_from_the_run_files_at_startup(cx: &mut TestAppContext) {
    let (window, session) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let (dir, persisted) = window
        .read_with(visual, |shell, cx| {
            let dir = shell.workflows().expect("workflows").read(cx).dir().clone();
            let persisted = locked(&shell.store)
                .unwrap()
                .persisted_session_id(session)
                .unwrap();
            (dir, persisted)
        })
        .unwrap();
    let w = flagged_workflow();
    update_workflows(&dir, |mut list| {
        list.push(w.clone());
        Ok((list, ()))
    })
    .unwrap();
    let mut run = start_run(&w, RunCaller::new(CallerKind::Manual, "start"), 1, "run-1");
    let mut node = NodeRun::new("start", "");
    node.session_id = Some(persisted);
    run.nodes.push(node);
    update_runs(&dir, &w.id, |_| Ok((vec![run.clone()], ()))).unwrap();
    assert!(
        !window
            .read_with(visual, |s, _| s.session_zap(session))
            .unwrap()
    );
    window
        .update(visual, |shell, _, cx| shell.load_workflow_sessions(cx))
        .unwrap();
    visual.run_until_parked();
    assert!(
        window
            .read_with(visual, |s, _| s.session_zap(session))
            .unwrap()
    );
}
