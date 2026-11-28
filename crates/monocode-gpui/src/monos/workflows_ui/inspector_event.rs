//! The inspector of an event trigger: which Inbox source and event starts the workflow, the
//! project and repository filters, and the latest matching items with "Test run with this item".
//! What the Inbox knows (its connections and items) is pushed in by the shell
//! (`WorkflowsPage::set_event_sources`); the page never reads the Inbox itself.
use super::super::*;
use super::board::{BoardField, BoardPick};
use super::page::basename;
use super::view::hint;
use gpui::{Div, FontWeight};
use monocode_core::{
    inbox::Item,
    mono::workflows::{
        event::{EventTrigger, event_label, events_for, matches, trigger_project},
        model::{InputType, Node, NodeKind, Workflow},
    },
};

/// The sources that can start a workflow, in the order they are offered.
pub(super) const SOURCES: [(&str, &str); 6] = [
    ("github", "GitHub"),
    ("gitlab", "GitLab"),
    ("linear", "Linear"),
    ("jira", "Jira"),
    ("clickup", "ClickUp"),
    ("azuredevops", "Azure DevOps"),
];

/// How many matching items the inspector lists.
pub(super) const LATEST: usize = 3;

pub(super) const LATENCY: &str =
    "Events arrive 2-5 minutes after they appear, and only while zptah is open.";

pub(super) fn source_name(source: &str) -> &str {
    SOURCES
        .iter()
        .find(|(id, _)| *id == source)
        .map_or(source, |(_, name)| name)
}

/// What the Inbox tells the inspector, and what the inspector keeps about its own work.
#[derive(Default)]
pub(super) struct EventSources {
    /// Per source: connected, and a line saying where.
    pub(super) connections: HashMap<String, (bool, String)>,
    /// The Inbox's items, newest first.
    pub(super) items: Vec<Item>,
    /// ClickUp spaces as (id, name), for the filter.
    spaces: Vec<(String, String)>,
    spaces_loading: bool,
    /// What the last test run said, with the trigger it was for.
    pub(super) notice: Option<(String, String)>,
    /// A test run is being started.
    testing: bool,
    /// The shell was asked for the Inbox (the sources are not known yet).
    asked: bool,
}

impl EventSources {
    pub(super) fn connected(&self, source: &str) -> bool {
        self.connections.get(source).is_some_and(|(on, _)| *on)
    }

    /// Connected sources first, then the others; `current` is always there.
    pub(super) fn source_options(&self, current: &str) -> Vec<(String, String)> {
        let mut options: Vec<(String, String)> = SOURCES
            .iter()
            .filter(|(id, _)| self.connected(id))
            .map(|(id, name)| (id.to_string(), name.to_string()))
            .collect();
        options.extend(
            SOURCES
                .iter()
                .filter(|(id, _)| !self.connected(id))
                .map(|(id, name)| (id.to_string(), format!("{name} (not connected)"))),
        );
        if !current.is_empty() && !options.iter().any(|(id, _)| id == current) {
            options.push((current.to_string(), current.to_string()));
        }
        options
    }
}

/// The items `trigger` would fire for, newest first (`LATEST` of them).
pub(super) fn latest_matching<'a>(
    w: &Workflow,
    trigger: &EventTrigger,
    projects: &[String],
    items: &'a [Item],
) -> Vec<&'a Item> {
    let text = trigger_project(w, trigger);
    let path = monocode_core::mono::resolve_project(projects, &text).unwrap_or(text);
    items
        .iter()
        .filter(|item| matches(trigger, item, &path))
        .take(LATEST)
        .collect()
}

fn item_line(item: &Item) -> String {
    let place = if item.repo.is_empty() {
        item.provider.clone()
    } else {
        item.repo.clone()
    };
    if item.identifier.trim().is_empty() {
        format!("#{} in {place}", item.number)
    } else {
        format!("{} in {place}", item.identifier.trim())
    }
}

impl Monos {
    /// The Inbox's connections and items changed.
    pub(in crate::monos) fn set_event_sources(
        &mut self,
        connections: HashMap<String, (bool, String)>,
        items: Vec<Item>,
        cx: &mut Context<Self>,
    ) {
        let events = &mut self.wf.events;
        events.asked &= connections.is_empty();
        if events.connections == connections && events.items == items {
            return;
        }
        events.connections = connections;
        events.items = items;
        cx.notify();
    }

    /// Looks up the ClickUp spaces once (off the UI thread); a failed lookup leaves the picker on
    /// the saved ids and is tried again the next time the inspector is drawn.
    fn load_event_spaces(&mut self, cx: &mut Context<Self>) {
        let events = &mut self.wf.events;
        if events.spaces_loading || !events.spaces.is_empty() || !events.connected("clickup") {
            return;
        }
        events.spaces_loading = true;
        let database = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { crate::inbox::spaces::list_spaces(&database) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.wf.events.spaces_loading = false;
                if let Ok(spaces) = result {
                    this.wf.events.spaces = spaces;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn board_event_fields(
        &mut self,
        scope: &Scope,
        w: &Workflow,
        node: &Node,
        trigger: &EventTrigger,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = node.id.clone();
        let connected = self.wf.events.connected(&trigger.source);
        // Nothing is known about the sources until the Inbox runs: ask the shell to start it.
        if self.wf.events.connections.is_empty() && !self.wf.events.asked {
            self.wf.events.asked = true;
            cx.emit(NeedEventSources);
        }
        if trigger.source == "clickup" {
            self.load_event_spaces(cx);
        }
        let sources = self.wf.events.source_options(&trigger.source);
        let source_row = self.choice_row(
            "Source",
            BoardPick::EventSource(id.clone()),
            sources,
            &trigger.source,
            cx,
        );
        let kinds: Vec<(String, String)> = events_for(&trigger.source)
            .into_iter()
            .map(|(value, label)| (value.to_string(), label.to_string()))
            .collect();
        let event_row = self.pick_row(
            "Event",
            BoardPick::EventKind(id.clone()),
            event_label(&trigger.source, &trigger.event),
            kinds,
            &trigger.event,
            cx,
        );
        let connect = (!connected).then(|| {
            div()
                .px_2()
                .pt_1()
                .flex()
                .items_center()
                .gap_1p5()
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(content(0.55))
                .child(format!(
                    "{} is not connected.",
                    source_name(&trigger.source)
                ))
                .child(
                    div()
                        .id("wf-event-connect")
                        .debug_selector(|| "wf-event-connect".into())
                        .role(Role::Link)
                        .aria_label("Connect in Settings")
                        .cursor_pointer()
                        .rounded_sm()
                        .px_1()
                        .text_color(accent_color())
                        .hover(|s| s.bg(content(0.08)))
                        .child("Connect")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenInboxSettings))),
                )
        });
        // The project: the workflow's own by default, an app project, or a project input.
        let current = trigger.project.clone().unwrap_or_default();
        let mut projects = vec![(String::new(), "Workspace project".to_string())];
        projects.extend(scope.projects.iter().map(|p| (p.clone(), basename(p))));
        projects.extend(
            w.inputs
                .iter()
                .filter(|(_, input)| input.ty == InputType::Project)
                .map(|(name, _)| (format!("{{{{inputs.{name}}}}}"), format!("Input {name}"))),
        );
        if !current.is_empty() && !projects.iter().any(|(value, _)| *value == current) {
            projects.push((current.clone(), basename(&current)));
        }
        let project_row = self.choice_row(
            "Project",
            BoardPick::EventProject(id.clone()),
            projects,
            &current,
            cx,
        );
        let code_host = !monocode_core::inbox::is_tracker(&trigger.source);
        let project_note = code_host.then(|| {
            hint("A pull request or issue is matched to a project, so one is required: pick it here or set the workspace project.")
        });
        let filter = if trigger.source == "clickup" {
            let saved = trigger.repos.first().cloned().unwrap_or_default();
            let spaces = self.wf.events.spaces.clone();
            let label = crate::inbox::spaces::space_label(&spaces, &saved);
            self.pick_row(
                "Space",
                BoardPick::EventSpace(id.clone()),
                &label,
                crate::inbox::spaces::space_options(&spaces, &saved),
                &saved,
                cx,
            )
        } else {
            self.text_row(
                "Repos",
                BoardField::EventRepos(id.clone()),
                &trigger.repos.join(", "),
                "Any (owner/name, comma separated)",
                window,
                cx,
            )
        };
        let rows = div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(source_row)
            .children(connect)
            .child(event_row)
            .child(project_row)
            .child(filter);
        let latency = div()
            .id("wf-event-latency")
            .debug_selector(|| "wf-event-latency".into())
            .px_4()
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(content(0.4))
            .child(LATENCY);
        let latest = self.board_event_items(scope, w, node, trigger, cx);
        div()
            .flex()
            .flex_col()
            .child(rows)
            .children(project_note)
            .child(latency)
            .child(latest)
            .into_any_element()
    }

    /// "Latest matching items": up to three, each with a button that runs the saved workflow once
    /// for it.
    fn board_event_items(
        &mut self,
        scope: &Scope,
        w: &Workflow,
        node: &Node,
        trigger: &EventTrigger,
        cx: &mut Context<Self>,
    ) -> Div {
        let found: Vec<Item> = latest_matching(w, trigger, &scope.projects, &self.wf.events.items)
            .into_iter()
            .cloned()
            .collect();
        let testing = self.wf.events.testing;
        let mut list = div().flex().flex_col().gap_1p5();
        if found.is_empty() {
            list = list.child(
                div()
                    .id("wf-event-no-items")
                    .debug_selector(|| "wf-event-no-items".into())
                    .px_2()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.4))
                    .child(if self.wf.events.connected(&trigger.source) {
                        "No matching item in your Inbox yet."
                    } else {
                        "Connect the source to see its items."
                    }),
            );
        }
        for (at, item) in found.into_iter().enumerate() {
            let (trigger_id, picked) = (node.id.clone(), item.clone());
            let selector = format!("wf-event-test:{at}");
            list = list.child(
                div()
                    .id(SharedString::from(format!("wf-event-item:{at}")))
                    .debug_selector(move || format!("wf-event-item:{at}"))
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .bg(content(0.05))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.85))
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child(item_line(&item)),
                    )
                    .child(
                        super::super::panels::button(
                            SharedString::from(selector.clone()),
                            "Test run with this item",
                        )
                        .debug_selector(move || selector.clone())
                        .self_start()
                        .h(px(24.))
                        .py_0()
                        .text_size(px(11.))
                        .when(testing, |d| d.opacity(0.4))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.board_event_test_run(&trigger_id, picked.clone(), cx)
                        })),
                    ),
            );
        }
        let notice = self
            .wf
            .events
            .notice
            .as_ref()
            .filter(|(trigger_id, _)| *trigger_id == node.id)
            .map(|(_, text)| text.clone());
        div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .px_2()
            .pt_3()
            .pb_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .px_2()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(0.4))
                    .child("Latest matching items"),
            )
            .child(list)
            .children(notice.map(|text| {
                div()
                    .id("wf-event-notice")
                    .debug_selector(|| "wf-event-notice".into())
                    .px_2()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(content(0.55))
                    .child(text)
            }))
    }

    /// The advice about a step (see `workflow_warnings`): a step with full access after an event
    /// trigger says so in its own form, not only in the Problems bar.
    pub(super) fn board_node_warnings(&self, id: &str) -> Option<gpui::Stateful<Div>> {
        let lines: Vec<String> = self
            .wf
            .board
            .as_ref()?
            .warnings
            .iter()
            .filter(|warning| warning.node_id.as_deref() == Some(id))
            .map(|warning| warning.message.clone())
            .collect();
        (!lines.is_empty()).then(|| {
            div()
                .id("wf-node-warning")
                .debug_selector(|| "wf-node-warning".into())
                .px_4()
                .pt_2()
                .flex()
                .flex_col()
                .gap_1()
                .children(lines.into_iter().map(|line| {
                    div()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(gpui::Hsla::from(rgb(0xfbbf24)).opacity(0.9))
                        .child(line)
                }))
        })
    }

    fn event_said(&mut self, trigger: &str, text: impl Into<String>, cx: &mut Context<Self>) {
        self.wf.events.notice = Some((trigger.to_string(), text.into()));
        cx.notify();
    }

    /// Runs the saved workflow once for `item`, started by hand (no claim is taken, so the item
    /// can still fire the trigger). Refused while the draft differs from the saved workflow, and
    /// by the core while the workflow needs a review.
    pub(super) fn board_event_test_run(
        &mut self,
        trigger: &str,
        item: Item,
        cx: &mut Context<Self>,
    ) {
        if self.wf.events.testing {
            return;
        }
        let Some(b) = self.wf.board.as_ref() else {
            return;
        };
        if b.is_new || b.changed() {
            self.event_said(
                trigger,
                "Save the workflow first: a test run uses the saved version.",
                cx,
            );
            return;
        }
        let workflow = b.base.id.clone();
        let Some(workflows) = self.workflows_entity.as_ref().and_then(|w| w.upgrade()) else {
            self.event_said(trigger, "The workflows are not available.", cx);
            return;
        };
        self.wf.events.testing = true;
        self.event_said(trigger, "Starting the test run...", cx);
        let (node, label) = (trigger.to_string(), item_line(&item));
        let task = workflows.update(cx, |workflows, cx| {
            workflows.start_test_run(&workflow, &node, item, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                use crate::workflows::events::EventStarted;
                this.wf.events.testing = false;
                let text = match result {
                    Ok(EventStarted::Queued { position, .. }) => {
                        format!("Test run for {label} is queued (number {position}).")
                    }
                    Ok(_) => format!("Started a test run for {label}."),
                    Err(error) => error,
                };
                this.event_said(&node, text, cx);
                this.refresh_workflows(cx);
            });
        })
        .detach();
    }

    /// What a drop-down of the event inspector chose.
    pub(super) fn board_event_picked(
        &mut self,
        pick: &BoardPick,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        let v = value.to_string();
        match pick {
            BoardPick::EventSource(id) => self.board_event_edit(
                id,
                |t| {
                    if t.source != v {
                        t.source = v.clone();
                        // The events and the filters of another source do not carry over.
                        t.event = events_for(&v)
                            .first()
                            .map(|(event, _)| event.to_string())
                            .unwrap_or_default();
                        t.repos.clear();
                    }
                },
                cx,
            ),
            BoardPick::EventKind(id) => self.board_event_edit(id, |t| t.event = v.clone(), cx),
            BoardPick::EventProject(id) => {
                self.board_event_edit(id, |t| t.project = (!v.is_empty()).then(|| v.clone()), cx)
            }
            BoardPick::EventSpace(id) => self.board_event_edit(
                id,
                |t| {
                    t.repos = if v.is_empty() {
                        Vec::new()
                    } else {
                        vec![v.clone()]
                    }
                },
                cx,
            ),
            _ => {}
        }
    }

    /// The repositories typed in the filter: names split at commas.
    pub(super) fn board_event_repos(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        let repos: Vec<String> = text
            .split(',')
            .map(str::trim)
            .filter(|repo| !repo.is_empty())
            .map(str::to_string)
            .collect();
        self.board_event_edit(id, |t| t.repos = repos, cx);
    }

    fn board_event_edit(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut EventTrigger),
        cx: &mut Context<Self>,
    ) {
        self.board_node_edit(
            id,
            |n| {
                if let NodeKind::Event(trigger) = &mut n.kind {
                    f(trigger);
                }
            },
            cx,
        );
    }
}
