//! Workflows started by the Inbox. The Inbox says which items appeared (its notification tracker
//! never reports the backlog that existed before it looked, so items that were there before a
//! trigger was enabled never fire); each one is offered to every enabled workflow whose event
//! trigger matches it (`event::matching_trigger`), and `Workflows::start_event_run` claims it and
//! starts or queues the run.
//!
//! An item whose start failed (no run was made, the claim was given back) stays in a retry map,
//! kept in the setting `workflow-event-retries` with the (workflow, trigger) pairs that failed,
//! and only those are offered again at the next look of the Inbox, also after a restart. After
//! `RETRY_ATTEMPTS` failed offers the item is dropped with a log line. An item no workflow was
//! waiting for, and an item that a paused or unreviewed workflow held, is dropped at once.
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
use serde::{Deserialize, Serialize};

const RETRY_SETTING: &str = "workflow-event-retries";
/// How many items the retry map keeps (the newest).
const RETRY_CAP: usize = 500;
/// How many offers an item gets before it is given up on.
const RETRY_ATTEMPTS: u32 = 5;
/// How often the Shell looks whether the Inbox must run in the background.
const INBOX_LOOK: std::time::Duration = std::time::Duration::from_secs(60);

/// An Inbox item waiting to be offered (again) to the workflows.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Retry {
    item: Item,
    /// The (workflow id, trigger node) pairs whose start failed. Empty: not offered yet, so every
    /// matching workflow gets it.
    #[serde(default)]
    failed: Vec<(String, String)>,
    /// How many offers failed so far.
    #[serde(default)]
    attempts: u32,
}

impl Retry {
    fn new(item: Item) -> Self {
        Self {
            item,
            failed: Vec::new(),
            attempts: 0,
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    retries: HashMap<String, Retry>,
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

    /// Whether the Inbox runs (a test looks at what started it).
    #[cfg(test)]
    pub(crate) fn has_background_inbox(&self) -> bool {
        self.inbox.is_some()
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
            state
                .retries
                .entry(item.key())
                .or_insert_with(|| Retry::new(item));
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
        let offered: Vec<Retry> = state.retries.values().cloned().collect();
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
            for (at, retry) in items.iter().enumerate() {
                for workflow in loaded.workflows.iter().filter(|w| w.enabled) {
                    let Some(trigger) = matching_trigger(workflow, &retry.item, &projects) else {
                        continue;
                    };
                    let pair = (workflow.id.clone(), trigger.id.clone());
                    // After a failure only the pairs that failed are offered the item again.
                    if retry.failed.is_empty() || retry.failed.contains(&pair) {
                        plan.push((at, pair.0, pair.1));
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
                        let item = offered[*at].item.clone();
                        let task = workflows
                            .update(cx, |w, cx| w.start_event_run(workflow, trigger, item, cx));
                        (*at, workflow.clone(), trigger.clone(), task)
                    })
                    .collect::<Vec<_>>()
            });
            let mut failed = Vec::new();
            for (at, workflow, trigger, task) in started.unwrap_or_default() {
                if matches!(task.await, EventStarted::Failed(_)) {
                    failed.push((at, (workflow, trigger)));
                }
            }
            let _ = this.update(cx, |this, cx| {
                let unread = plan.as_ref().err();
                if let Some(error) = unread {
                    eprintln!("Could not read the workflows for Inbox events: {error}");
                }
                this.workflow_events_done(offered, failed, unread.is_some(), cx)
            });
        })
        .detach();
    }

    fn workflow_events_done(
        &mut self,
        offered: Vec<Retry>,
        failed: Vec<(usize, (String, String))>,
        unread: bool,
        cx: &mut Context<Self>,
    ) {
        let state = &mut self.workflow_events;
        state.busy = false;
        for (at, retry) in offered.iter().enumerate() {
            let key = retry.item.key();
            // Without the workflows nothing was offered: the same pairs stay.
            let still: Vec<(String, String)> = if unread {
                retry.failed.clone()
            } else {
                failed
                    .iter()
                    .filter(|(failed_at, _)| *failed_at == at)
                    .map(|(_, pair)| pair.clone())
                    .collect()
            };
            if still.is_empty() && !unread {
                state.retries.remove(&key);
                continue;
            }
            let attempts = retry.attempts + 1;
            if attempts >= RETRY_ATTEMPTS {
                eprintln!(
                    "Giving up on the Inbox item {key} for workflows after {attempts} failed starts"
                );
                state.retries.remove(&key);
            } else if let Some(kept) = state.retries.get_mut(&key) {
                kept.failed = still;
                kept.attempts = attempts;
            }
        }
        // Items that came while the offer went are still waiting.
        let again = state
            .retries
            .keys()
            .any(|key| !offered.iter().any(|retry| &retry.item.key() == key));
        let kept: Vec<Retry> = state.retries.values().cloned().collect();
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
                .map(|raw| raw.and_then(|raw| serde_json::from_str::<Vec<Retry>>(&raw).ok()))
        });
        cx.spawn(async move |this, cx| {
            let saved = work.await;
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.workflow_events;
                state.busy = false;
                state.loaded = true;
                match saved {
                    Ok(saved) => {
                        for retry in saved.unwrap_or_default() {
                            state.retries.entry(retry.item.key()).or_insert(retry);
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
fn cap_retries(retries: &mut HashMap<String, Retry>) {
    if retries.len() <= RETRY_CAP {
        return;
    }
    let mut all: Vec<Retry> = retries.drain().map(|(_, retry)| retry).collect();
    all.sort_by_key(|retry| std::cmp::Reverse(retry.item.updated_ms()));
    all.truncate(RETRY_CAP);
    retries.extend(all.into_iter().map(|retry| (retry.item.key(), retry)));
}

/// Writes the retry map unless a newer write was asked for meanwhile.
fn save_retries(store: &Database, revision: &AtomicU64, number: u64, retries: &[Retry]) {
    let Ok(saved) = serde_json::to_string(retries) else {
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
