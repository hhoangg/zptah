//! The "Available variables" list of the inspector: what a step's prompt or command may use,
//! each a row that types the variable at the caret of that field.
use super::super::*;
use super::board::BoardField;
use super::review::node_title;
use super::view::section;
use gpui::{Div, FontWeight};
use monocode_core::mono::workflows::{
    model::Workflow,
    variables::{Variable, VariableGroup, available_variables},
};

/// A stable name for a group, for selectors.
fn group_key(group: &VariableGroup) -> String {
    match group {
        VariableGroup::Trigger => "trigger".into(),
        VariableGroup::Inputs => "inputs".into(),
        VariableGroup::Run => "run".into(),
        VariableGroup::Item => "item".into(),
        VariableGroup::Step(id) => format!("step:{id}"),
    }
}

impl Monos {
    /// The section listing the variables node `id` may use, or None when it has none. A click on
    /// a row inserts the variable into the field `target` names.
    pub(super) fn board_variables(
        &mut self,
        w: &Workflow,
        id: &str,
        target: BoardField,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let variables = available_variables(w, id);
        if variables.is_empty() {
            return None;
        }
        let mut groups: Vec<(VariableGroup, Vec<Variable>)> = Vec::new();
        for variable in variables {
            match groups.iter_mut().find(|(g, _)| *g == variable.group) {
                Some((_, rows)) => rows.push(variable),
                None => groups.push((variable.group.clone(), vec![variable])),
            }
        }
        let mut body = div()
            .id("wf-variables")
            .debug_selector(|| "wf-variables".into())
            .px_1()
            .flex()
            .flex_col()
            .gap_2();
        for (group, rows) in groups {
            let title = match &group {
                VariableGroup::Trigger => "Trigger".to_string(),
                VariableGroup::Inputs => "Inputs".to_string(),
                VariableGroup::Run => "Run".to_string(),
                VariableGroup::Item => "Item".to_string(),
                VariableGroup::Step(step) => {
                    w.node(step).map(node_title).unwrap_or_else(|| step.clone())
                }
            };
            let key = group_key(&group);
            let mut list = div().flex().flex_col();
            for variable in rows {
                list = list.child(self.variable_row(&target, variable, cx));
            }
            body = body.child(
                div()
                    .id(SharedString::from(format!("wf-variables-group:{key}")))
                    .debug_selector(move || format!("wf-variables-group:{key}"))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_1()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.4))
                            .child(title),
                    )
                    .child(list),
            );
        }
        Some(section("Available variables", None).child(body))
    }

    fn variable_row(
        &mut self,
        target: &BoardField,
        variable: Variable,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selector = format!("wf-var:{}", variable.path);
        let insert = variable.path.clone();
        let field = target.key();
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .role(gpui::Role::Button)
            .aria_label(format!("Insert {}", variable.path))
            .cursor_pointer()
            .rounded_sm()
            .px_1()
            .py_0p5()
            .flex()
            .items_baseline()
            .gap_2()
            .text_size(px(11.))
            .line_height(px(16.))
            .hover(|s| s.bg(content(0.08)))
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(crate::fonts::mono())
                    .text_color(content(0.85))
                    .child(variable.path),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(content(0.4))
                    .child(format!("{} - {}", variable.kind.name(), variable.label)),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                let Some(input) = this.fields.get(&field).cloned() else {
                    return;
                };
                input.update(cx, |input, cx| input.insert_at_cursor(&insert, window, cx));
                window.focus(&input.focus_handle(cx), cx);
            }))
    }
}
