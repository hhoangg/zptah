//! Workflows started by the Inbox. The Inbox says which items appeared (its notification tracker
//! never reports the backlog that existed before it looked, so items that were there before a
//! trigger was enabled never fire); each one is offered to every enabled workflow whose event
//! trigger matches it (`event::matching_trigger`), and `Workflows::start_event_run` claims it and
//! starts or queues the run.
//!
//! An item whose start failed (no run was made, the claim was given back) stays in a retry map,
//! kept in the setting `workflow-event-retries`, and is offered again at the next look of the
//! Inbox, also after a restart. An item no workflow was waiting for is dropped at once.
use super::*;
use crate::workflows::events::EventStarted;
use monocode_core::{
    inbox::Item,
    mono::workflows::{
        event::matching_trigger,
        model::{NodeKind, Workflow},
        store::load_workflows,
    },
};

const RETRY_SETTING: &str = "workflow-event-retries";
/// How many items the retry map keeps (the newest).
const RETRY_CAP: usize = 500;
/// How often the Shell looks whether the Inbox must run in the background.
const INBOX_LOOK: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Default)]
pub(super) struct State {
    retries: HashMap<String, Item>,
    /// The saved retries were read.
    loaded: bool,
    /// Items are being offered to workflows now.
    busy: bool,
    /// Each write of the setting takes the next number; an older write is skipped.
    revision: Arc<AtomicU64>,
    _look: Option<gpui::Task<()>>,
}

fn has_event_trigger(workflows: &[Workflow]) -> bool {
    workflows.iter().any(|w| {
        w.enabled
            && w.nodes
                .iter()
                .any(|n| n.parent.is_none() && matches!(n.kind, NodeKind::Event(_)))
    })
}

impl Shell {
    /// Keeps the Inbox polling in the background while an enabled workflow has an event trigger.
    /// Called once, when the shell is made.
    pub(super) fn watch_workflow_event_inbox(&mut self, cx: &mut Context<Self>) {
        // A test looks by calling `sync_workflow_event_inbox` itself: a timer that waits in the
        // background would change when the other tests of the shell settle.
        if cfg!(test) {
            return;
        }
        let look = cx.spawn(async move |this, cx| {
            loop {
                if this
                    .update(cx, |this, cx| this.sync_workflow_event_inbox(cx))
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(INBOX_LOOK).await;
            }
        });
        self.workflow_events._look = Some(look);
    }

    /// Starts the background Inbox when an enabled workflow has an event trigger and it is not
    /// running yet.
    pub(super) fn sync_workflow_event_inbox(&mut self, cx: &mut Context<Self>) {
        if self.inbox.is_some() {
            return;
        }
        let Some(workflows) = self.workflows.clone() else {
            return;
        };
        let dir = workflows.read(cx).dir().clone();
        let work = cx
            .background_executor()
            .spawn(async move { load_workflows(&dir).map(|l| has_event_trigger(&l.workflows)) });
        cx.spawn(async move |this, cx| {
            if matches!(work.await, Ok(true)) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.ensure_background_inbox(window, cx)
                });
            }
        })
        .detach();
    }

    /// Tells the Workflows page which sources are connected and which items the Inbox holds, for
    /// the event inspector.
    pub(super) fn push_workflow_event_sources(&self, cx: &mut Context<Self>) {
        let (Some(page), Some(inbox)) = (&self.workflows_page, &self.inbox) else {
            return;
        };
        let inbox = inbox.read(cx);
        let (connections, items) = (inbox.connection_statuses(), inbox.items().to_vec());
        page.update(cx, |page, cx| {
            page.set_event_sources(connections, items, cx)
        });
    }

    /// The Inbox looked: `appeared` are the items it had not seen, and the retry map is offered
    /// again with them.
    pub(super) fn inbox_workflow_events(&mut self, appeared: Vec<Item>, cx: &mut Context<Self>) {
        let state = &mut self.workflow_events;
        for item in appeared {
            state.retries.insert(item.key(), item);
        }
        cap_retries(&mut state.retries);
        if state.busy {
            return;
        }
        if !state.loaded {
            self.load_workflow_event_retries(cx);
            return;
        }
        if state.retries.is_empty() {
            return;
        }
        let Some(workflows) = self.workflows.clone() else {
            return;
        };
        state.busy = true;
        let offered: Vec<Item> = state.retries.values().cloned().collect();
        let dir = workflows.read(cx).dir().clone();
        let mut projects = self.workspace.projects.clone();
        if !self.workspace.project.is_empty() && !projects.contains(&self.workspace.project) {
            projects.push(self.workspace.project.clone());
        }
        let store = self.store.clone();
        let revision = state.revision.clone();
        let (items, number) = (offered.clone(), revision.fetch_add(1, Ordering::SeqCst) + 1);
        let work = cx.background_executor().spawn(async move {
            // Saved before the starts, so a crash in between offers the items again.
            save_retries(&store, &revision, number, &items);
            let loaded = load_workflows(&dir)?;
            let mut plan = Vec::new();
            for (at, item) in items.iter().enumerate() {
                for workflow in loaded.workflows.iter().filter(|w| w.enabled) {
                    if let Some(trigger) = matching_trigger(workflow, item, &projects) {
                        plan.push((at, workflow.id.clone(), trigger.id.clone()));
                    }
                }
            }
            Ok::<_, String>(plan)
        });
        cx.spawn(async move |this, cx| {
            let plan = work.await;
            let started = this.update(cx, |_, cx| {
                let Ok(plan) = &plan else {
                    return Vec::new();
                };
                plan.iter()
                    .map(|(at, workflow, trigger)| {
                        let item = offered[*at].clone();
                        let task = workflows
                            .update(cx, |w, cx| w.start_event_run(workflow, trigger, item, cx));
                        (*at, task)
                    })
                    .collect::<Vec<_>>()
            });
            let mut failed = HashSet::new();
            for (at, task) in started.unwrap_or_default() {
                if matches!(task.await, EventStarted::Failed(_)) {
                    failed.insert(at);
                }
            }
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = &plan {
                    eprintln!("Could not read the workflows for Inbox events: {error}");
                    // Nothing was offered: every item stays for the next look.
                    failed.extend(0..offered.len());
                }
                this.workflow_events_done(offered, failed, cx)
            });
        })
        .detach();
    }

    fn workflow_events_done(
        &mut self,
        offered: Vec<Item>,
        failed: HashSet<usize>,
        cx: &mut Context<Self>,
    ) {
        let state = &mut self.workflow_events;
        state.busy = false;
        let mut again = false;
        for (at, item) in offered.iter().enumerate() {
            if !failed.contains(&at) {
                state.retries.remove(&item.key());
            }
        }
        // Items that came while the offer went are still waiting.
        again |= state
            .retries
            .keys()
            .any(|key| !offered.iter().any(|item| &item.key() == key));
        let kept: Vec<Item> = state.retries.values().cloned().collect();
        let number = state.revision.fetch_add(1, Ordering::SeqCst) + 1;
        let (store, revision) = (self.store.clone(), state.revision.clone());
        cx.background_executor()
            .spawn(async move { save_retries(&store, &revision, number, &kept) })
            .detach();
        if again {
            self.inbox_workflow_events(Vec::new(), cx);
        }
    }

    /// Reads the saved retries once, then offers what is there.
    fn load_workflow_event_retries(&mut self, cx: &mut Context<Self>) {
        self.workflow_events.busy = true;
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store)?
                .setting(RETRY_SETTING)
                .map(|raw| raw.and_then(|raw| serde_json::from_str::<Vec<Item>>(&raw).ok()))
        });
        cx.spawn(async move |this, cx| {
            let saved = work.await;
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.workflow_events;
                state.busy = false;
                state.loaded = true;
                match saved {
                    Ok(saved) => {
                        for item in saved.unwrap_or_default() {
                            state.retries.entry(item.key()).or_insert(item);
                        }
                        cap_retries(&mut state.retries);
                    }
                    Err(error) => eprintln!("Could not read the saved Inbox retries: {error}"),
                }
                this.inbox_workflow_events(Vec::new(), cx);
            });
        })
        .detach();
    }
}

/// Keeps the newest items.
fn cap_retries(retries: &mut HashMap<String, Item>) {
    if retries.len() <= RETRY_CAP {
        return;
    }
    let mut items: Vec<Item> = retries.drain().map(|(_, item)| item).collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.updated_ms()));
    items.truncate(RETRY_CAP);
    retries.extend(items.into_iter().map(|item| (item.key(), item)));
}

/// Writes the retry map unless a newer write was asked for meanwhile.
fn save_retries(store: &Database, revision: &AtomicU64, number: u64, items: &[Item]) {
    let Ok(saved) = serde_json::to_string(items) else {
        return;
    };
    let result = locked(store).and_then(|store| {
        if revision.load(Ordering::SeqCst) == number {
            store.save_setting(RETRY_SETTING, &saved)
        } else {
            Ok(())
        }
    });
    if let Err(error) = result {
        eprintln!("Could not save the Inbox retries of workflows: {error}");
    }
}

#[cfg(test)]
mod tests;
