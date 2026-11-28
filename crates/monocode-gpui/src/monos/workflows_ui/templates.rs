//! "New from template": the gallery of starter workflows. Picking one opens the board of a new
//! workflow filled from the template and the open project. It is saved paused like any new one.
use super::super::*;
use super::view::header_button;
use crate::template_gallery::{card_grid, template_card, template_icon, trigger_mark};
use gpui::FontWeight;
use monocode_core::mono::workflows::templates::{TemplateTrigger, WorkflowTemplate, templates};

/// The mark at the foot of a card: a clock for a schedule, the provider for an event.
fn mark_of(template: &WorkflowTemplate) -> AnyElement {
    match &template.trigger {
        TemplateTrigger::Schedule { .. } => trigger_mark("time", 12.),
        TemplateTrigger::Event { source, .. } => trigger_mark(source, 12.),
    }
}

impl Monos {
    /// Opens the gallery (below the workflow limit; the buttons refuse at it).
    pub(in crate::monos) fn open_templates_page(&mut self, cx: &mut Context<Self>) {
        self.open_workflow_route(WfRoute::Templates, cx);
    }

    /// A new workflow from the template: its board opens unsaved, for the open project.
    pub(in crate::monos) fn start_from_template(&mut self, id: &str, cx: &mut Context<Self>) {
        self.wf.template = Some(id.to_string());
        self.open_workflow_route(WfRoute::New, cx);
    }

    /// The workflow a new board starts from: the template's, or the blank one.
    pub(super) fn new_board_workflow(
        &mut self,
        scope: &Scope,
        now: u64,
    ) -> monocode_core::mono::workflows::model::Workflow {
        let template = self
            .wf
            .template
            .take()
            .and_then(|id| monocode_core::mono::workflows::templates::template(&id));
        match template {
            Some(t) => t.instantiate(&scope.project, now),
            None => monocode_core::mono::workflows::board_model::blank_workflow(now),
        }
    }

    pub(in crate::monos) fn workflow_templates_page(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let full = self
            .workflow_count()
            .is_some_and(|n| n >= monocode_core::mono::workflows::model::WORKFLOWS_MAX);
        let mut body = div()
            .id("mono-templates-page")
            .debug_selector(|| "mono-templates-page".into())
            .w_full()
            .min_w_0()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_4()
            .flex()
            .flex_col()
            .gap_4();
        for (key, title, scheduled) in [
            ("scheduled", "Scheduled", true),
            ("events", "On events", false),
        ] {
            let cards: Vec<AnyElement> = templates()
                .iter()
                .filter(|t| t.is_scheduled() == scheduled)
                .map(|t| {
                    let id = t.id.clone();
                    template_card(
                        SharedString::from(format!("mono-template-{}", t.id)),
                        template_icon(&t.icon),
                        t.name.clone(),
                        t.description.clone(),
                        Some((mark_of(t), t.trigger_label().into())),
                    )
                    .debug_selector({
                        let id = id.clone();
                        move || format!("mono-template-{id}")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !full {
                            this.start_from_template(&id, cx);
                        }
                    }))
                    .into_any_element()
                })
                .collect();
            body = body.child(
                div()
                    .id(SharedString::from(format!("mono-templates-{key}")))
                    .debug_selector(move || format!("mono-templates-{key}"))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.55))
                            .child(title),
                    )
                    .child(card_grid(cards)),
            );
        }
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(self.workflow_header("New from template", None, cx))
            .children(full.then(|| {
                div()
                    .px_4()
                    .pt_3()
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child("At most 100 workflows. Remove one to add another.")
            }))
            .child(body)
            .into_any_element()
    }
}

/// The empty list's button next to the message.
pub(super) fn empty_templates_button(cx: &mut Context<Monos>, enabled: bool) -> AnyElement {
    header_button(
        "mono-workflow-templates-empty",
        "New from template",
        true,
        enabled,
    )
    .on_click(cx.listener(move |this, _, _, cx| {
        if enabled {
            this.open_templates_page(cx);
        }
    }))
    .into_any_element()
}
