//! The global Workflows page: a view of the app (like Today or Automations) that hosts the
//! workflow pages of `Monos`. It owns nothing but its focus and what the shell tells it about the
//! app; the state and the pages are in `WorkflowsUi` and the sibling files, so no Mono is needed
//! to show them.
use super::super::*;
use super::{Scope, WfRoute};
use gpui::FontWeight;
use monocode_core::provider::Kind;

pub(crate) struct WorkflowsPage {
    monos: Entity<Monos>,
    focus: gpui::FocusHandle,
    _observation: Subscription,
}

impl WorkflowsPage {
    pub(crate) fn new(monos: Entity<Monos>, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            _observation: cx.observe(&monos, |_, _, cx| cx.notify()),
            monos,
        }
    }

    /// What the app has for the pages: its projects, the open one, and the harness and model a
    /// new session starts with when a step sets none.
    pub(crate) fn set_context(
        &mut self,
        projects: Vec<String>,
        project: String,
        default: (Kind, Option<String>),
        cx: &mut Context<Self>,
    ) {
        let scope = Scope {
            projects,
            project,
            default_kind: Some(default.0),
            default_model: default.1,
            ..Scope::default()
        };
        self.monos.update(cx, |monos, cx| {
            if monos.wf.scope != scope {
                monos.wf.scope = scope;
                cx.notify();
            }
        });
    }

    /// What the Inbox knows, for the event inspector: which sources are connected, and its items.
    pub(crate) fn set_event_sources(
        &mut self,
        connections: std::collections::HashMap<String, (bool, String)>,
        items: Vec<monocode_core::inbox::Item>,
        cx: &mut Context<Self>,
    ) {
        self.monos.update(cx, |monos, cx| {
            monos.set_event_sources(connections, items, cx)
        });
    }

    /// The session folders of each app project, as (id, name).
    pub(crate) fn set_session_folders(
        &mut self,
        folders: std::collections::HashMap<String, Vec<(String, String)>>,
        cx: &mut Context<Self>,
    ) {
        self.monos.update(cx, |monos, cx| {
            if monos.wf.folders != folders {
                monos.wf.folders = folders;
                cx.notify();
            }
        });
    }

    /// The page is on screen (or no longer is).
    pub(crate) fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.monos
            .update(cx, |monos, cx| monos.workflows_view_shown(shown, cx));
    }
}

impl Focusable for WorkflowsPage {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

impl Render for WorkflowsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self
            .monos
            .update(cx, |monos, cx| monos.workflows_view(window, cx));
        div()
            .id("workflows-view")
            .debug_selector(|| "workflows-view".into())
            .role(Role::Region)
            .aria_label("Workflows")
            .track_focus(&self.focus)
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .child(body)
    }
}

impl Monos {
    /// The page in view: the list with its header, or the open workflow page.
    fn workflows_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let scope = self.wf.scope.clone();
        let body = match self.wf.route.clone() {
            None => self.workflows_home(&scope, window, cx),
            Some(WfRoute::Review(id, intent)) => self.workflow_review_page(&id, intent, cx),
            Some(WfRoute::Import) => self.import_workflow_page(&scope, cx),
            Some(WfRoute::Templates) => self.workflow_templates_page(cx),
            Some(
                route
                @ (WfRoute::New | WfRoute::Board(_) | WfRoute::Workflow(_) | WfRoute::Run(..)),
            ) => self.workflow_shell(&scope, &route, window, cx),
        };
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(body)
            .children(self.picker_surface(cx))
            .into_any_element()
    }

    /// The list under its header: the title and the buttons that make, import or ask for one.
    fn workflows_home(
        &mut self,
        scope: &Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let actions = self.workflow_header_actions(cx);
        let count = self.workflow_count().filter(|n| *n > 0);
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(40.))
                    .flex_shrink_0()
                    .min_w_0()
                    .border_b_1()
                    .border_color(content(0.1))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Workflows"),
                    )
                    .when_some(count, |d, count| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(content(0.4))
                                .child(count.to_string()),
                        )
                    })
                    .children(actions),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.workflows_list(scope, window, cx)),
            )
            .into_any_element()
    }
}
