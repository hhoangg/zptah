//! The inspector's model, schedule, output and settings parts (`NodeInspector.tsx`).
use super::super::panels::OpenPicker;
use super::super::scheduler::{habit_field, habit_icon_button};
use super::super::*;
use super::Pick;
use super::board::{BoardField, BoardPick};
use super::page::{basename, value_text};
use super::view::{hint, property, section};
use crate::pickers::{ModelList, ModelPicked, cached_model_name};
use crate::popover::Align;
use gpui::{Div, Stateful};
use monocode_core::mono::workflows::{
    board_edit::*,
    model::{
        DecisionModel, InputType, InputValue, Node, NodeKind, OutputField, OutputFieldType,
        SessionMode, Workflow, WorkflowInput,
    },
    models::{model_efforts, session_node_problem, stored_model_id},
    output::session_builtins,
    report::field_in_report,
    validate::terminal_problems,
};
use monocode_core::provider::Kind;
use monocode_core::store::native_model;

use super::inspector::*;

impl Monos {
    // ---- how a session step runs ------------------------------------------------------------------

    /// The `surface` (chat or terminal) and `mode` (auto or wait for me) of a session step, and
    /// the built-in outputs it gets. Terminal is dimmed, with the reason under the control, when
    /// the step cannot run in the provider's own CLI.
    pub(super) fn board_session_how(
        &mut self,
        w: &Workflow,
        node: &Node,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = node.id.clone();
        let NodeKind::Session {
            harness,
            model,
            target,
            ..
        } = &node.kind
        else {
            return div();
        };
        let surface = node.kind.session_surface();
        let mode = node.kind.session_mode();
        let cloud = target.is_some();
        let project = project_path(w, node);
        let mut problems = terminal_problems(
            project.as_deref(),
            harness.as_deref(),
            model.as_deref(),
            cloud,
            cfg!(windows),
        );
        // The harness the model rows show counts too: a step that names none runs on it.
        let shown = self.board_model_state(node, cx).kind;
        if harness.as_deref().is_none_or(str::is_empty)
            && model.as_deref().is_none_or(str::is_empty)
            && shown != Kind::Claude
        {
            problems.push(format!(
                "A terminal step runs Claude only, but this step uses {}",
                shown.label()
            ));
        }
        let reason = problems.into_iter().next();
        let blocked: &[&str] = if reason.is_some() { &["terminal"] } else { &[] };
        let surface_id = id.clone();
        let surface_control = crate::segmented::segmented_with(
            "segmented",
            "wf-surface",
            "How the step runs",
            if surface.is_terminal() {
                "terminal"
            } else {
                "chat"
            },
            &[("chat", "Chat"), ("terminal", "Terminal")],
            blocked,
            cx,
            move |this: &mut Self, picked, cx| {
                this.board_node_edit(
                    &surface_id,
                    |n| {
                        if let NodeKind::Session { surface, .. } = &mut n.kind {
                            *surface = (picked == "terminal").then(|| "terminal".to_string());
                        }
                    },
                    cx,
                )
            },
        );
        let mode_id = id.clone();
        let mode_control = crate::segmented::segmented(
            "segmented",
            "wf-mode-of-step",
            "When the step ends",
            if mode == SessionMode::WaitForMe {
                "waitForMe"
            } else {
                "auto"
            },
            &[("auto", "Auto"), ("waitForMe", "Wait for me")],
            cx,
            move |this: &mut Self, picked, cx| {
                this.board_node_edit(
                    &mode_id,
                    |n| {
                        if let NodeKind::Session { mode, .. } = &mut n.kind {
                            *mode = (picked == "waitForMe").then(|| "waitForMe".to_string());
                        }
                    },
                    cx,
                )
            },
        );
        let builtins = session_builtins(surface, cloud).join(", ");
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .pt_3()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(property("Opens as", surface_control))
                    .when_some(reason, |d, reason| {
                        d.child(
                            div()
                                .id("wf-surface-reason")
                                .debug_selector(|| "wf-surface-reason".into())
                                .text_size(px(11.))
                                .line_height(px(16.))
                                .text_color(content(0.45))
                                .child(format!("Terminal is not available: {reason}")),
                        )
                    })
                    .child(property("Ends", mode_control))
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(content(0.35))
                            .child(if mode == SessionMode::WaitForMe {
                                "After the first turn you chat on in the session, then press Continue or Fail step."
                            } else {
                                "The step ends when the first turn ends."
                            }),
                    ),
            )
            .child(
                div()
                    .id("wf-builtin-outputs")
                    .debug_selector(|| "wf-builtin-outputs".into())
                    .px_4()
                    .pt_2()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(content(0.35))
                    .child(format!("Built-in outputs: {builtins}")),
            )
    }

    // ---- the model ------------------------------------------------------------------------------

    pub(super) fn board_model_rows(
        &mut self,
        node: &Node,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let state = self.board_model_state(node, cx);
        let Some(b) = self.wf.board.as_ref() else {
            return Vec::new();
        };
        let problem = session_node_problem(node, &b.catalog);
        let efforts = state
            .stored
            .as_deref()
            .and_then(|stored| model_efforts(&b.catalog, state.kind.id(), stored))
            .unwrap_or_default();
        let anchor = b.model_anchor.clone();
        let label = cached_model_name(state.kind, state.native.as_deref());
        let id = node.id.clone();
        let picker = div()
            .id("wf-model")
            .debug_selector(|| "wf-model".into())
            .role(Role::Button)
            .aria_label(format!("Model: {label}"))
            .relative()
            .cursor_pointer()
            .h(px(28.))
            .max_w_full()
            .px_2()
            .rounded_md()
            .flex()
            .items_center()
            .gap_1p5()
            .text_size(px(12.))
            .hover(|s| s.bg(content(0.08)))
            .child(div().absolute().size_full().child(anchor.probe()))
            .child(provider_icon(state.kind.id(), 14.))
            .child(div().min_w_0().truncate().child(label))
            .child(icon("chevron-down", 12.))
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, window, cx| this.board_open_model_picker(&id, window, cx)
            }));
        let reset_id = id.clone();
        let under = if state.unset {
            div()
                .pb_0p5()
                .text_size(px(11.))
                .text_color(content(0.35))
                .child("App default")
                .into_any_element()
        } else {
            div()
                .id("wf-model-default")
                .debug_selector(|| "wf-model-default".into())
                .role(Role::Button)
                .aria_label("Use app default")
                .cursor_pointer()
                .rounded_sm()
                .px_1p5()
                .py_0p5()
                .text_size(px(11.))
                .text_color(content(0.55))
                .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                .child("Use app default")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.board_node_edit(
                        &reset_id,
                        |n| {
                            if let NodeKind::Session {
                                harness,
                                model,
                                effort,
                                ..
                            } = &mut n.kind
                            {
                                *harness = None;
                                *model = None;
                                *effort = None;
                            }
                        },
                        cx,
                    )
                }))
                .into_any_element()
        };
        let mut out = vec![
            property(
                "Model",
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .child(picker)
                    .child(under),
            )
            .into_any_element(),
        ];
        if let Some(problem) = problem {
            out.push(
                div()
                    .debug_selector(|| "wf-model-issue".into())
                    .child(hint(problem))
                    .into_any_element(),
            );
        }
        if !efforts.is_empty() {
            let current = match &node.kind {
                NodeKind::Session { effort, .. } => effort.clone().unwrap_or_default(),
                _ => String::new(),
            };
            let options: Vec<(String, String)> = efforts
                .iter()
                .map(|e| (e.clone(), effort_name(e)))
                .collect();
            let selected = if current.is_empty() {
                "Default".to_string()
            } else {
                effort_name(&current)
            };
            out.push(
                self.pick_row(
                    "Effort",
                    BoardPick::Effort(id),
                    &selected,
                    options,
                    &current,
                    cx,
                )
                .into_any_element(),
            );
        }
        out
    }

    /// The model picker answered: the node names its harness and model, as the file stores them.
    /// A new model has its own effort levels, so the old choice does not carry over.
    pub(super) fn board_model_picked(
        &mut self,
        node: &str,
        kind: monocode_core::provider::Kind,
        native: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.board_node_edit(
            node,
            |n| {
                if let NodeKind::Session {
                    harness,
                    model,
                    effort,
                    ..
                } = &mut n.kind
                {
                    *harness = Some(kind.id().to_string());
                    *model = stored_model_id(kind, native.as_deref());
                    *effort = None;
                }
            },
            cx,
        );
        cx.notify();
    }

    pub(super) fn board_open_model_picker(
        &mut self,
        node: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(step) = self.wf.board.as_ref().and_then(|b| b.node(node).cloned()) else {
            return;
        };
        let state = self.board_model_state(&step, cx);
        let id = node.to_string();
        let anchor = self
            .wf
            .board
            .as_ref()
            .map(|b| b.model_anchor.clone())
            .unwrap_or_default();
        self.open_model_list(
            state.kind,
            state.native,
            anchor,
            move |this, kind, native, cx| this.board_model_picked(&id, kind, native, cx),
            window,
            cx,
        );
    }

    /// The model list as a flyout under `anchor`; `picked` hears the harness and model chosen.
    pub(super) fn open_model_list(
        &mut self,
        kind: monocode_core::provider::Kind,
        native: Option<String>,
        anchor: AnchorSlot,
        picked: impl Fn(&mut Self, monocode_core::provider::Kind, Option<String>, &mut Context<Self>)
        + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.wf.scope.project.clone();
        let list = cx.new(|cx| ModelList::new_for(kind, native, &project, Default::default(), cx));
        let focus = list.read(cx).search_focus(cx);
        let chosen = cx.subscribe(&list, move |this, _, event: &ModelPicked, cx| {
            this.picker = None;
            picked(this, event.0, event.1.clone(), cx);
        });
        let menu = cx.new(|cx| {
            Menu::new(
                "Model",
                vec![crate::popover::MenuItem::Custom(list.into(), focus)],
                window,
                cx,
            )
            .width(400.)
        });
        let closed = cx.subscribe(&menu, |this, _, _: &MenuEvent, cx| {
            this.picker = None;
            cx.notify();
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(anchor),
            width: 400.,
            align: Align::Start,
            _events: vec![chosen, closed],
        });
        cx.notify();
    }

    // ---- schedule, outputs ----------------------------------------------------------------------

    pub(super) fn board_schedule(
        &mut self,
        id: &str,
        schedule: &monocode_core::mono::habits::Schedule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = schedule.schedule_kind.clone();
        let pick = |this: &mut Self,
                    label: &'static str,
                    pick: BoardPick,
                    options: Vec<(String, String)>,
                    current: String,
                    cx: &mut Context<Self>| {
            let selected = Self::label_of(&options, &current);
            this.workflow_picker(
                format!("wf-pick:{pick:?}"),
                label.to_string(),
                selected,
                options,
                current,
                Pick::Board(pick),
                cx,
            )
        };
        let days = [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ];
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1p5()
            .py_0p5()
            .child(pick(
                self,
                "Repeats",
                BoardPick::ScheduleKind(id.to_string()),
                choices(&[
                    ("daily", "Daily"),
                    ("weekdays", "Weekdays"),
                    ("weekly", "Weekly"),
                    ("hourly", "Hourly"),
                    ("interval", "Every N hours"),
                ]),
                kind.clone(),
                cx,
            ));
        if kind == "interval" {
            let options = (1..=24)
                .map(|h| {
                    (
                        h.to_string(),
                        format!("{h} hour{}", if h == 1 { "" } else { "s" }),
                    )
                })
                .collect();
            row = row.child(pick(
                self,
                "Hours between runs",
                BoardPick::ScheduleHours(id.to_string()),
                options,
                schedule.interval_hours.unwrap_or(24).to_string(),
                cx,
            ));
        }
        if kind == "weekly" {
            row = row
                .child(div().text_color(content(0.45)).child("on"))
                .child(pick(
                    self,
                    "Day",
                    BoardPick::ScheduleDay(id.to_string()),
                    days.iter()
                        .enumerate()
                        .map(|(i, d)| (i.to_string(), d.to_string()))
                        .collect(),
                    schedule.day_of_week.to_string(),
                    cx,
                ));
        }
        row = row.child(
            div()
                .text_color(content(0.45))
                .child(if kind == "interval" { "from" } else { "at" }),
        );
        if kind == "hourly" {
            row = row.child(pick(
                self,
                "Minute",
                BoardPick::ScheduleMinute(id.to_string()),
                [0, 15, 30, 45]
                    .iter()
                    .map(|m| (m.to_string(), format!(":{m:02}")))
                    .collect(),
                schedule.minute.to_string(),
                cx,
            ));
        } else {
            let spec = BoardField::Time(id.to_string());
            let selector = spec.key();
            let field = self.board_field(
                spec,
                "Time",
                "09:00",
                &schedule.time,
                false,
                Some(5),
                window,
                cx,
            );
            row = row.child(
                habit_field(field)
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .w(px(84.))
                    .h(px(28.))
                    .bg(content(0.1)),
            );
        }
        row.into_any_element()
    }

    /// The cases of a Switch: a name each, a button to add one and one to remove each.
    pub(super) fn board_cases(
        &mut self,
        id: &str,
        cases: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let add_id = id.to_string();
        let add = Self::section_button("wf-case-add", "Add case", "plus")
            .on_click(cx.listener(move |this, _, _, cx| this.board_add_case(&add_id, cx)));
        let mut body = div().px_1().flex().flex_col().gap_1();
        for case in cases {
            let spec = BoardField::CaseName(id.to_string(), case.clone());
            let selector = spec.key();
            let input = self.board_field(spec, "Case", "", case, false, None, window, cx);
            let (remove_id, remove_case) = (id.to_string(), case.clone());
            let remove_selector = format!("wf-case-remove-{case}");
            let mut row = div().flex().items_center().gap_1p5().child(
                div().flex_1().min_w_0().child(
                    habit_field(input)
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector.clone())
                        .h(px(28.))
                        .bg(content(0.1)),
                ),
            );
            // A Switch needs a case, so the last one cannot be removed.
            if cases.len() > 1 {
                row = row.child(
                    habit_icon_button(
                        SharedString::from(remove_selector.clone()),
                        "Remove case",
                        "close",
                    )
                    .debug_selector(move || remove_selector.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.board_remove_case(&remove_id, &remove_case, cx)
                    })),
                );
            }
            body = body.child(row);
        }
        body = body.child(
            div()
                .px_1()
                .text_size(px(12.))
                .line_height(px(20.))
                .text_color(content(0.4))
                .child("Anything that matches no case leaves by default."),
        );
        section("Cases", Some(add.into_any_element())).child(body)
    }

    pub(super) fn board_outputs(
        &mut self,
        id: &str,
        fields: &monocode_core::mono::workflows::ordered::OrderedMap<OutputField>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let add_id = id.to_string();
        let add = Self::section_button("wf-output-add", "Add output field", "plus").on_click(
            cx.listener(move |this, _, _, cx| {
                this.board_node_edit(
                    &add_id,
                    |n| {
                        if let NodeKind::Session { output, .. } = &mut n.kind {
                            let name = next_field_name(output);
                            output.insert(name, OutputField::new(OutputFieldType::String));
                        }
                    },
                    cx,
                )
            }),
        );
        let mut body = div().px_1().flex().flex_col().gap_1();
        if fields.is_empty() {
            body = body.child(
                div()
                    .px_1()
                    .text_size(px(12.))
                    .line_height(px(20.))
                    .text_color(content(0.4))
                    .child("Later steps can use what this session reports, as {{id.field}}."),
            );
        }
        for (name, field) in fields.iter() {
            let spec = BoardField::OutputName(id.to_string(), name.clone());
            let selector = spec.key();
            let input = self.board_field(spec, "Name", "", name, false, None, window, cx);
            let ty = OUTPUT_TYPES
                .iter()
                .find(|(v, _)| {
                    OutputFieldType::ALL
                        .iter()
                        .any(|(n, t)| n == v && *t == field.ty)
                })
                .map_or("string", |(v, _)| v);
            let options = choices(&OUTPUT_TYPES);
            let selected = Self::label_of(&options, ty);
            let picker = self.workflow_picker(
                format!("wf-pick:OutputType({id},{name})"),
                format!("Type of {name}"),
                selected,
                options,
                ty.to_string(),
                Pick::Board(BoardPick::OutputType(id.to_string(), name.clone())),
                cx,
            );
            let on = field_in_report(field);
            let (toggle_id, toggle_name) = (id.to_string(), name.clone());
            let (remove_id, remove_name) = (id.to_string(), name.clone());
            let toggle_selector = format!("wf-output-report-{name}");
            let remove_selector = format!("wf-output-remove-{name}");
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div().flex_1().min_w_0().child(
                            habit_field(input)
                                .id(SharedString::from(selector.clone()))
                                .debug_selector(move || selector.clone())
                                .h(px(28.))
                                .bg(content(0.1)),
                        ),
                    )
                    .child(picker)
                    .child(
                        div()
                            .id(SharedString::from(toggle_selector.clone()))
                            .debug_selector(move || toggle_selector.clone())
                            .role(Role::Button)
                            .aria_label("In report")
                            .aria_selected(on)
                            .cursor_pointer()
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .rounded_md()
                            .text_size(px(11.))
                            .when(on, |d| d.bg(content(0.15)).text_color(content(1.)))
                            .when(!on, |d| {
                                d.text_color(content(0.45)).hover(|s| s.bg(content(0.08)))
                            })
                            .child("In report")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.board_node_edit(
                                    &toggle_id,
                                    |n| {
                                        if let NodeKind::Session { output, .. } = &mut n.kind {
                                            *output = toggle_in_report(output, &toggle_name);
                                        }
                                    },
                                    cx,
                                )
                            })),
                    )
                    .child(
                        habit_icon_button(
                            SharedString::from(remove_selector.clone()),
                            "Remove output field",
                            "close",
                        )
                        .debug_selector(move || remove_selector.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.board_node_edit(
                                &remove_id,
                                |n| {
                                    if let NodeKind::Session { output, .. } = &mut n.kind {
                                        output.remove(&remove_name);
                                    }
                                },
                                cx,
                            )
                        })),
                    ),
            );
        }
        section("Output", Some(add.into_any_element())).child(body)
    }

    // ---- the settings -----------------------------------------------------------------------------

    /// The Settings mode: name, description, inputs, requirements, and how AI decisions and the
    /// run summary work.
    pub(super) fn board_settings(
        &mut self,
        scope: &Scope,
        w: &Workflow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name_field = self.board_field(
            BoardField::Name,
            "Workflow name",
            "Name",
            &w.name,
            false,
            Some(80),
            window,
            cx,
        );
        let name = section("Name", None).child(
            habit_field(name_field)
                .id("wf-board-name")
                .debug_selector(|| "wf-board-name".into())
                .h(px(28.))
                .bg(content(0.1)),
        );
        let description = self.long_text(
            "What it does",
            BoardField::Description,
            w.description.as_deref().unwrap_or(""),
            "A line on what this workflow is for.",
            window,
            cx,
        );
        let problem = self.wf.board.as_ref().and_then(|b| b.input_problem.clone());
        let mut rows = div().px_2().flex().flex_col().gap_0p5();
        for (name, input) in w.inputs.iter() {
            let field = self.board_input_value(scope, w, name, input, window, cx);
            let remove = name.clone();
            let selector = format!("wf-input-remove-{name}");
            rows = rows.child(property(
                name.clone(),
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(field))
                    .child(
                        habit_icon_button(
                            SharedString::from(selector.clone()),
                            "Remove input",
                            "close",
                        )
                        .debug_selector(move || selector.clone())
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.board_remove_input(&remove, cx)),
                        ),
                    ),
            ));
        }
        let mut inputs = section("Inputs", None);
        if w.inputs.is_empty() {
            inputs = inputs.child(
                div()
                    .px_2()
                    .text_size(px(12.))
                    .line_height(px(20.))
                    .text_color(content(0.4))
                    .child(
                        "Values the steps use as {{inputs.name}}. A project you pick for a step is added here.",
                    ),
            );
        } else {
            inputs = inputs.child(rows);
        }
        if let Some(problem) = problem {
            inputs = inputs.child(
                div()
                    .debug_selector(|| "wf-input-problem".into())
                    .px_2()
                    .text_size(px(12.))
                    .text_color(gpui::Hsla::from(rgb(RED)).opacity(0.8))
                    .child(problem),
            );
        }
        inputs = inputs.child(self.board_add_input_row(w, window, cx));
        let workspace = self.board_workspace_section(scope, w, window, cx);
        let requires = self.board_requires_section(w, window, cx);
        let decisions = self.board_decisions_section(w, cx);
        let summary = self.board_summary_section(w, window, cx);
        div()
            .flex()
            .flex_col()
            .child(name)
            .child(description)
            .child(inputs)
            .child(workspace)
            .child(requires)
            .child(decisions)
            .child(summary)
            .into_any_element()
    }

    /// Where a run works and how many go at once: whether each run gets a worktree of its own,
    /// of which project (an app project, or a project input) and from which branch, and the
    /// number of runs at the same time (1 to 10). D51: edited here, not on the board.
    fn board_workspace_section(
        &mut self,
        scope: &Scope,
        w: &Workflow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        use monocode_core::mono::workflows::model::{CONCURRENCY_LIMIT, WorkspaceMode};
        let mode = match w.workspace.mode {
            WorkspaceMode::PerRun => "perRun",
            WorkspaceMode::None => "none",
        };
        let mode_row = self.choice_row(
            "Each run works",
            BoardPick::WorkspaceMode,
            vec![
                ("perRun".into(), "In its own worktree".into()),
                ("none".into(), "In the project folder".into()),
            ],
            mode,
            cx,
        );
        // An app project by path, or one of the workflow's project inputs.
        let current = w.workspace.project.clone();
        let mut options: Vec<(String, String)> = vec![(String::new(), "None".into())];
        options.extend(scope.projects.iter().map(|p| (p.clone(), basename(p))));
        options.extend(
            w.inputs
                .iter()
                .filter(|(_, input)| input.ty == InputType::Project)
                .map(|(name, _)| (format!("{{{{inputs.{name}}}}}"), format!("Input {name}"))),
        );
        if !current.is_empty() && !options.iter().any(|(value, _)| *value == current) {
            options.push((current.clone(), basename(&current)));
        }
        let project_row = self.choice_row(
            "Project",
            BoardPick::WorkspaceProject,
            options,
            &current,
            cx,
        );
        let per_run = w.workspace.mode == WorkspaceMode::PerRun;
        let branch = w.workspace.base_branch.clone().unwrap_or_default();
        let branch_row = per_run.then(|| {
            self.text_row(
                "Base branch",
                BoardField::BaseBranch,
                &branch,
                "main",
                window,
                cx,
            )
        });
        let limit = w.concurrency.limit();
        let limits: Vec<(String, String)> = (1..=CONCURRENCY_LIMIT)
            .map(|n| (n.to_string(), n.to_string()))
            .collect();
        let concurrency_row = self.choice_row(
            "Runs at the same time",
            BoardPick::Concurrency,
            limits,
            &limit.to_string(),
            cx,
        );
        section("Workspace", None)
            .id("wf-workspace")
            .debug_selector(|| "wf-workspace".into())
            .child(
                div()
                    .px_2()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .pb_1()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(content(0.35))
                            .child(
                                "A run in its own worktree starts from the base branch of the project, and its branch and worktree stay after the run until you remove them. Runs that wait for a free slot start in the order they were asked.",
                            ),
                    )
                    .child(mode_row)
                    .child(project_row)
                    .children(branch_row)
                    .child(concurrency_row),
            )
    }

    /// The model that answers the workflow's AI decisions (an If, a Switch or a loop the AI
    /// decides). Without one the decision uses the model of the step before it.
    fn board_decisions_section(&mut self, w: &Workflow, cx: &mut Context<Self>) -> Stateful<Div> {
        let mono_kind = self.board_mono_kind(cx);
        let Some(b) = self.wf.board.as_ref() else {
            return section("AI decisions", None).id("wf-decisions");
        };
        let anchor = b.decision_anchor.clone();
        let model = w.decision_model.clone();
        let kind = model
            .as_ref()
            .and_then(|m| Kind::parse(&m.harness))
            .or(mono_kind)
            .unwrap_or(Kind::Claude);
        let native = model
            .as_ref()
            .and_then(|m| native_model(m.model.as_deref()?, kind.id()));
        let efforts = model
            .as_ref()
            .and_then(|m| model_efforts(&b.catalog, kind.id(), m.model.as_deref()?))
            .unwrap_or_default();
        let label = match &model {
            Some(_) => cached_model_name(kind, native.as_deref()),
            None => "The model of the step before it".into(),
        };
        let picker = div()
            .id("wf-decision-model")
            .debug_selector(|| "wf-decision-model".into())
            .role(Role::Button)
            .aria_label(format!("Decision model: {label}"))
            .relative()
            .cursor_pointer()
            .h(px(28.))
            .max_w_full()
            .px_2()
            .rounded_md()
            .flex()
            .items_center()
            .gap_1p5()
            .text_size(px(12.))
            .hover(|s| s.bg(content(0.08)))
            .child(div().absolute().size_full().child(anchor.probe()))
            .when(model.is_some(), |d| d.child(provider_icon(kind.id(), 14.)))
            .child(div().min_w_0().truncate().child(label))
            .child(icon("chevron-down", 12.))
            .on_click(
                cx.listener(move |this, _, window, cx| this.board_open_decision_picker(window, cx)),
            );
        let under = if model.is_none() {
            div()
                .pb_0p5()
                .text_size(px(11.))
                .text_color(content(0.35))
                .child("Used when an AI decides")
                .into_any_element()
        } else {
            div()
                .id("wf-decision-default")
                .debug_selector(|| "wf-decision-default".into())
                .role(Role::Button)
                .aria_label("Use the model of the step before it")
                .cursor_pointer()
                .rounded_sm()
                .px_1p5()
                .py_0p5()
                .text_size(px(11.))
                .text_color(content(0.55))
                .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                .child("Use the model of the step before it")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.board_meta_edit(|m| m.decision_model = None, cx)
                }))
                .into_any_element()
        };
        let mut block = section("AI decisions", None)
            .id("wf-decisions")
            .debug_selector(|| "wf-decisions".into())
            .child(
                div().px_2().flex().flex_col().gap_0p5().child(property(
                    "Model",
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .child(picker)
                        .child(under),
                )),
            );
        if let Some(model) = &model
            && !efforts.is_empty()
        {
            let current = model.effort.clone().unwrap_or_default();
            let options: Vec<(String, String)> = efforts
                .iter()
                .map(|e| (e.clone(), effort_name(e)))
                .collect();
            let selected = if current.is_empty() {
                "Default".to_string()
            } else {
                effort_name(&current)
            };
            block = block.child(div().px_2().child(self.pick_row(
                "Effort",
                BoardPick::DecisionEffort,
                &selected,
                options,
                &current,
                cx,
            )));
        }
        block
    }

    /// The decision model picker answered: the workflow names its harness and model, as the file
    /// stores them. A new model has its own effort levels, so the old choice does not carry over.
    pub(super) fn board_decision_picked(
        &mut self,
        kind: Kind,
        native: Option<String>,
        cx: &mut Context<Self>,
    ) {
        // A harness's own default has no id to store: the file keeps only the harness, and the
        // run asks that harness's default model.
        let model = stored_model_id(kind, native.as_deref());
        self.board_meta_edit(
            |m| {
                m.decision_model = Some(DecisionModel {
                    harness: kind.id().to_string(),
                    model,
                    effort: None,
                })
            },
            cx,
        );
    }

    fn board_open_decision_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_ref() else {
            return;
        };
        let anchor = b.decision_anchor.clone();
        let model = b.meta.decision_model.clone();
        let kind = model
            .as_ref()
            .and_then(|m| Kind::parse(&m.harness))
            .or(self.board_mono_kind(cx))
            .unwrap_or(Kind::Claude);
        let native = model
            .as_ref()
            .and_then(|m| native_model(m.model.as_deref()?, kind.id()));
        self.open_model_list(
            kind,
            native,
            anchor,
            |this, kind, native, cx| this.board_decision_picked(kind, native, cx),
            window,
            cx,
        );
    }

    /// Who is told when a run ends, and what the summary says: the Mono it is sent to besides
    /// the run page, and a template for its text.
    fn board_summary_section(
        &mut self,
        w: &Workflow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let current = w
            .report_to
            .as_ref()
            .and_then(|r| r.mono.clone())
            .unwrap_or_default();
        let mut options: Vec<(String, String)> = vec![(String::new(), "No one".into())];
        options.extend(
            self.roster
                .monos
                .iter()
                .map(|mono| (mono.id.clone(), mono.label())),
        );
        // A Mono that is gone stays choosable, so the value is not hidden.
        if !current.is_empty() && !options.iter().any(|(id, _)| *id == current) {
            options.push((current.clone(), current.clone()));
        }
        let report = self.choice_row("Send to", BoardPick::ReportTo, options, &current, cx);
        let template = self.long_text(
            "Summary template",
            BoardField::SummaryTemplate,
            w.summary_template.as_deref().unwrap_or(""),
            "Leave empty for the usual summary. Use {{run.status}}, {{run.name}} and {{<step>.<field>}}.",
            window,
            cx,
        );
        div()
            .flex()
            .flex_col()
            .child(
                section("Run summary", None)
                    .id("wf-summary")
                    .debug_selector(|| "wf-summary".into())
                    .child(
                        div()
                            .px_2()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .pb_1()
                                    .text_size(px(11.))
                                    .line_height(px(16.))
                                    .text_color(content(0.35))
                                    .child(
                                        "Every run ends with a summary on its page and in a notification. It also goes to the Mono that started the run.",
                                    ),
                            )
                            .child(report),
                    ),
            )
            .child(template)
    }

    /// What the workflow needs on a machine before it can run: commands, environment variables and
    /// MCP servers, each by name. Declared, never run.
    pub(super) fn board_requires_section(
        &mut self,
        w: &Workflow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let problems: Vec<String> = self
            .wf
            .board
            .as_ref()
            .map(|b| {
                b.issues
                    .iter()
                    .filter(|i| {
                        i.node_id.is_none() && i.message.to_lowercase().contains("requirement")
                    })
                    .map(|i| i.message.clone())
                    .collect()
            })
            .unwrap_or_default();
        let add = Self::section_button("wf-require-add", "Add requirement", "plus")
            .on_click(cx.listener(|this, _, _, cx| this.board_add_requirement(cx)))
            .into_any_element();
        let mut block = section("Requires", Some(add))
            .id("wf-requires")
            .debug_selector(|| "wf-requires".into());
        if w.requires.is_empty() {
            block = block.child(
                div()
                    .px_2()
                    .text_size(px(12.))
                    .line_height(px(20.))
                    .text_color(content(0.4))
                    .child("Commands, environment variables and MCP servers this machine needs before it runs."),
            );
        }
        let kinds = choices(&[
            ("cli", "Command"),
            ("env", "Variable"),
            ("mcp", "MCP server"),
        ]);
        for (at, requirement) in w.requires.iter().enumerate() {
            let kind_key = requirement.kind.key();
            let picker = self.workflow_picker(
                format!("wf-require-kind-{at}"),
                "Requirement kind".into(),
                Self::label_of(&kinds, kind_key),
                kinds.clone(),
                kind_key.to_string(),
                Pick::Board(BoardPick::RequireKind(at)),
                cx,
            );
            let name = self.board_field(
                BoardField::RequireName(at),
                "Requirement name",
                "Name",
                &requirement.name,
                false,
                None,
                window,
                cx,
            );
            let hint_field = self.board_field(
                BoardField::RequireHint(at),
                "Requirement hint",
                "Hint, e.g. brew install gh",
                requirement.hint.as_deref().unwrap_or(""),
                false,
                None,
                window,
                cx,
            );
            let selector = format!("wf-require-remove-{at}");
            block = block.child(
                div()
                    .id(SharedString::from(format!("wf-require-{at}")))
                    .debug_selector(move || format!("wf-require-{at}"))
                    .px_2()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(picker)
                            .child(
                                div().flex_1().min_w_0().child(
                                    habit_field(name)
                                        .debug_selector(move || format!("wf-require-name-{at}"))
                                        .h(px(28.))
                                        .bg(content(0.1)),
                                ),
                            )
                            .child(
                                habit_icon_button(
                                    SharedString::from(selector.clone()),
                                    "Remove requirement",
                                    "close",
                                )
                                .debug_selector(move || selector.clone())
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.board_remove_requirement(at, cx),
                                )),
                            ),
                    )
                    .child(
                        habit_field(hint_field)
                            .debug_selector(move || format!("wf-require-hint-{at}"))
                            .h(px(28.))
                            .bg(content(0.1)),
                    ),
            );
        }
        for problem in problems.into_iter().take(1) {
            block = block.child(
                div()
                    .debug_selector(|| "wf-require-problem".into())
                    .px_2()
                    .text_size(px(12.))
                    .text_color(gpui::Hsla::from(rgb(RED)).opacity(0.8))
                    .child(problem),
            );
        }
        block
    }

    pub(super) fn board_input_value(
        &mut self,
        scope: &Scope,
        w: &Workflow,
        name: &str,
        input: &WorkflowInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let value = w.input_values.get(name);
        let pick = BoardPick::InputValue(name.to_string());
        match input.ty {
            InputType::Project => {
                let current = match value {
                    Some(InputValue::String(p)) => p.clone(),
                    _ => String::new(),
                };
                let mut options: Vec<(String, String)> = scope
                    .projects
                    .iter()
                    .map(|p| (p.clone(), basename(p)))
                    .collect();
                if !current.is_empty() && !options.iter().any(|(p, _)| *p == current) {
                    options.insert(0, (current.clone(), basename(&current)));
                }
                let selected = options
                    .iter()
                    .find(|(p, _)| *p == current)
                    .map_or_else(|| "Choose a project".to_string(), |(_, l)| l.clone());
                self.workflow_picker(
                    format!("wf-pick:{pick:?}"),
                    name.to_string(),
                    selected,
                    options,
                    current,
                    Pick::Board(pick),
                    cx,
                )
            }
            InputType::Boolean => {
                let current = match value {
                    Some(InputValue::Bool(true)) => "on",
                    Some(InputValue::Bool(false)) => "off",
                    _ => "",
                };
                let selected = match current {
                    "on" => "On",
                    "off" => "Off",
                    _ => "Choose",
                };
                self.workflow_picker(
                    format!("wf-pick:{pick:?}"),
                    name.to_string(),
                    selected.to_string(),
                    choices(&[("on", "On"), ("off", "Off")]),
                    current.to_string(),
                    Pick::Board(pick),
                    cx,
                )
            }
            InputType::String | InputType::Number => {
                let spec = BoardField::InputValue(name.to_string());
                let selector = spec.key();
                let placeholder = input
                    .description
                    .clone()
                    .unwrap_or_else(|| name.to_string());
                let field = self.board_field(
                    spec,
                    name,
                    &placeholder,
                    &value_text(value),
                    false,
                    None,
                    window,
                    cx,
                );
                habit_field(field)
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .h(px(28.))
                    .bg(content(0.1))
                    .into_any_element()
            }
        }
    }

    pub(super) fn board_add_input_row(
        &mut self,
        w: &Workflow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let ty = self
            .wf
            .board
            .as_ref()
            .map(|b| b.new_input_type.clone())
            .unwrap_or_else(|| "string".into());
        let field = self.board_field(
            BoardField::NewInput,
            "New input name",
            "New input",
            "",
            false,
            None,
            window,
            cx,
        );
        let typed = field.read(cx).content.to_string();
        let problem = input_name_problem(&typed, &|n| w.inputs.contains_key(n));
        let ready = !typed.is_empty() && problem.is_none();
        let options = choices(&INPUT_TYPES);
        let selected = Self::label_of(&options, &ty);
        let picker = self.workflow_picker(
            "wf-pick:NewInputType".into(),
            "New input type".into(),
            selected,
            options,
            ty.clone(),
            Pick::Board(BoardPick::NewInputType),
            cx,
        );
        let add = Self::section_button("wf-input-add", "Add input", "plus")
            .when(!ready, |d| d.opacity(0.4))
            .on_click(cx.listener(move |this, _, _, cx| {
                if ready {
                    this.board_add_input(&typed, &ty, cx);
                    if let Some(f) = this.fields.get(&BoardField::NewInput.key()).cloned() {
                        f.update(cx, |f, cx| f.set_text(String::new(), cx));
                    }
                }
            }));
        div()
            .px_2()
            .pt_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div().flex_1().min_w_0().child(
                            habit_field(field)
                                .debug_selector(|| "wf-input-new".into())
                                .h(px(28.))
                                .bg(content(0.1)),
                        ),
                    )
                    .child(picker)
                    .child(add),
            )
            .children(problem.map(|p| {
                div()
                    .px_2()
                    .pt_1()
                    .text_size(px(11.))
                    .text_color(gpui::Hsla::from(rgb(RED)).opacity(0.8))
                    .child(p)
            }))
    }
}
