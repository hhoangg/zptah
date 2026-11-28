//! When a workflow is due. Pure: the clock and the "is a run going" answer are passed in, so the
//! scheduler tick decides what to start and what only has to move on.
//!
//! A workflow may have several schedule triggers. With one, its next time is `next_run_at`; with
//! several, each has its own entry in `next_run_at_by_node` and `next_run_at` is the earliest.
//! When more than one comes due in the same look, the workflow runs once.
use super::model::{Node, NodeKind, Workflow};
use crate::mono::habits::{Schedule, GRACE_MS};

/// The schedule triggers of a workflow, in definition order.
pub fn schedule_triggers(workflow: &Workflow) -> Vec<(&Node, &Schedule)> {
    workflow
        .nodes
        .iter()
        .filter(|n| n.parent.is_none())
        .filter_map(|n| match &n.kind {
            NodeKind::Schedule { schedule } => Some((n, schedule)),
            _ => None,
        })
        .collect()
}

/// When each schedule trigger runs next (`None`: not picked yet).
fn next_times(workflow: &Workflow) -> Vec<(&Node, &Schedule, Option<u64>)> {
    let triggers = schedule_triggers(workflow);
    let single = triggers.len() == 1;
    triggers
        .into_iter()
        .map(|(node, schedule)| {
            let at = if single {
                workflow.next_run_at
            } else {
                workflow.next_run_at_by_node.get(&node.id).copied()
            };
            (node, schedule, at)
        })
        .collect()
}

/// The schedule trigger that is due at `now`: the one with the earliest time that has passed.
pub fn due_schedule_trigger(workflow: &Workflow, now: u64) -> Option<&Node> {
    next_times(workflow)
        .into_iter()
        .filter_map(|(node, _, at)| Some((node, at.filter(|at| *at <= now)?)))
        .min_by_key(|(_, at)| *at)
        .map(|(node, _)| node)
}

/// What the scheduler tick found: workflows to start, and ones whose schedule only moves on.
#[derive(Debug, PartialEq)]
pub struct Due<'a> {
    pub run: Vec<&'a Workflow>,
    pub advance: Vec<&'a Workflow>,
}

/// Workflows to run now, and ones whose schedule only has to move on.
/// Like habits, one that came due long ago (the app was closed) is not run, and one that
/// comes due while a run of it already waits in the queue is skipped: it runs again at its next
/// time. A requested run is never dropped: when no slot is free the claim queues it.
/// `queued(id)` says whether a run of the workflow waits in the queue.
pub fn due_workflows<'a>(
    workflows: &'a [Workflow],
    now: u64,
    queued: impl Fn(&str) -> bool,
) -> Due<'a> {
    let mut due = Due {
        run: Vec::new(),
        advance: Vec::new(),
    };
    for workflow in workflows {
        // Made from a file: nothing starts it, on a schedule or on request, until the user has
        // approved this exact version.
        if workflow.needs_review {
            continue;
        }
        if workflow.run_requested {
            due.run.push(workflow);
            continue;
        }
        let busy = queued(&workflow.id);
        let times = next_times(workflow);
        if !workflow.enabled || times.is_empty() {
            continue;
        }
        // Enabled with a trigger that has no next time yet: pick one, do not run.
        if times.iter().any(|(_, _, at)| at.is_none()) {
            due.advance.push(workflow);
            continue;
        }
        let Some(first) = times.iter().filter_map(|(_, _, at)| *at).min() else {
            continue;
        };
        if first > now {
            continue;
        }
        if !busy && now - first <= GRACE_MS {
            due.run.push(workflow);
        } else {
            due.advance.push(workflow);
        }
    }
    due
}

/// The workflow with the next run time of every schedule trigger right for its enabled state:
/// picked from `now` while it is enabled, cleared while it is not.
pub fn scheduled(workflow: &Workflow, now: u64) -> Result<Workflow, String> {
    let mut next = workflow.clone();
    next.next_run_at = None;
    next.next_run_at_by_node.clear();
    if !workflow.enabled {
        return Ok(next);
    }
    let triggers = schedule_triggers(workflow);
    for (node, schedule) in &triggers {
        let at = schedule.next(now)?;
        if triggers.len() > 1 {
            next.next_run_at_by_node.insert(node.id.clone(), at);
        }
        next.next_run_at = Some(next.next_run_at.map_or(at, |old| old.min(at)));
    }
    Ok(next)
}

/// The workflow with its schedule moved past `now` and its run request answered. A schedule that
/// cannot produce a next time (it fails its own validation) leaves no time, so the tick only
/// keeps trying to pick one and never fires on a stale one.
pub fn after_claim(workflow: &Workflow, now: u64, run_id: Option<&str>) -> Workflow {
    let run_id = run_id.filter(|id| !id.is_empty());
    let mut next = workflow.clone();
    next.run_requested = false;
    next.run_caller = None;
    // The values were for the run that is being claimed now; they never carry to a later one.
    next.run_values = None;
    // A stop asked for before a run started was meant for an earlier one. Moving the schedule
    // past a workflow whose run is still going must keep a stop written for that run.
    next.stop_requested = workflow.stop_requested && run_id.is_none();
    let triggers = schedule_triggers(workflow);
    if triggers.len() == 1 {
        let keep = workflow.run_requested && workflow.next_run_at.is_some_and(|at| at > now);
        if !keep {
            next.next_run_at = triggers[0].1.next(now).ok();
        }
    } else if triggers.len() > 1 {
        // Each trigger moves on only when its own time has passed (or it has none yet).
        let mut by_node = std::collections::BTreeMap::new();
        for (node, schedule) in &triggers {
            let at = match workflow.next_run_at_by_node.get(&node.id) {
                Some(at) if *at > now => Some(*at),
                _ => schedule.next(now).ok(),
            };
            if let Some(at) = at {
                by_node.insert(node.id.clone(), at);
            }
        }
        next.next_run_at = by_node.values().min().copied();
        next.next_run_at_by_node = by_node;
    } else {
        next.next_run_at_by_node.clear();
    }
    if let Some(id) = run_id {
        next.last_run_id = Some(id.to_string());
        // The claimed run takes the next number of the workflow.
        next.run_counter = workflow.run_counter + 1;
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mono::habits::GRACE_MS;
    use crate::mono::workflows::fixtures::workflow_from;
    use chrono::{Local, TimeZone};

    fn now() -> u64 {
        Local
            .with_ymd_and_hms(2026, 10, 22, 12, 0, 0)
            .unwrap()
            .timestamp_millis() as u64
    }

    fn workflow_with(trigger: &str, extra: &str) -> Workflow {
        let mut w = workflow_from(&format!(
            r#"{{"id":"wf-abcd1234","name":"Ship it","inputs":{{}},"inputValues":{{}},
                "nodes":[{trigger}],"edges":[],"enabled":true,"createdAt":0{extra}}}"#
        ));
        w.next_run_at = Some(now() - 1_000);
        w
    }

    const DAILY: &str = r#"{"id":"start","type":"schedule","schedule":{"scheduleKind":"daily","time":"09:00","minute":0,"dayOfWeek":1}}"#;

    fn workflow() -> Workflow {
        workflow_with(DAILY, "")
    }

    fn never(_: &str) -> bool {
        false
    }

    mod after_claim_tests {
        use super::*;

        #[test]
        fn a_claim_clears_the_run_request_and_any_stop_asked_for_before_it() {
            let mut w = workflow();
            w.run_requested = true;
            w.stop_requested = true;
            let claimed = after_claim(&w, now(), Some("run-1"));
            assert!(!claimed.run_requested);
            assert!(!claimed.stop_requested);
            assert_eq!(claimed.last_run_id.as_deref(), Some("run-1"));
            assert!(claimed.next_run_at.unwrap() > now());
        }

        #[test]
        fn a_claim_clears_the_run_values_asked_for_that_run() {
            let mut w = workflow();
            w.run_requested = true;
            let mut values = crate::mono::workflows::ordered::OrderedMap::new();
            values.insert(
                "n".to_string(),
                crate::mono::workflows::model::InputValue::Number(1.0),
            );
            w.run_values = Some(values);
            let claimed = after_claim(&w, now(), Some("run-1"));
            assert!(claimed.run_values.is_none());
            assert!(after_claim(&w, now(), None).run_values.is_none());
        }

        #[test]
        fn moving_the_schedule_on_without_a_claim_keeps_a_stop_written_for_the_run_that_is_going() {
            let mut w = workflow();
            w.stop_requested = true;
            let advanced = after_claim(&w, now(), None);
            assert!(advanced.stop_requested);
            assert!(advanced.next_run_at.unwrap() > now());
        }

        #[test]
        fn a_request_made_for_a_later_time_keeps_that_time() {
            let mut w = workflow();
            w.run_requested = true;
            w.next_run_at = Some(now() + 5_000);
            let claimed = after_claim(&w, now(), Some("run-2"));
            assert_eq!(claimed.next_run_at, Some(now() + 5_000));
            assert!(!claimed.run_requested);
        }

        #[test]
        fn a_manual_workflow_has_no_schedule_to_move() {
            let mut w = workflow_with(r#"{"id":"start","type":"manual"}"#, "");
            w.run_requested = true;
            w.next_run_at = None;
            let claimed = after_claim(&w, now(), Some("run-3"));
            assert_eq!(claimed.next_run_at, None);
            assert_eq!(claimed.last_run_id.as_deref(), Some("run-3"));
        }

        #[test]
        fn an_empty_run_id_is_no_claim() {
            let mut w = workflow();
            w.stop_requested = true;
            w.last_run_id = Some("earlier".into());
            let moved = after_claim(&w, now(), Some(""));
            assert!(moved.stop_requested);
            assert_eq!(moved.last_run_id.as_deref(), Some("earlier"));
        }

        #[test]
        fn an_interval_schedule_moves_to_its_next_slot() {
            let w = workflow_with(
                r#"{"id":"start","type":"schedule","schedule":{"scheduleKind":"interval","time":"00:00","minute":0,"dayOfWeek":1,"intervalHours":6,"intervalFrom":"2026-10-01"}}"#,
                "",
            );
            let moved = after_claim(&w, now(), Some("r")).next_run_at.unwrap();
            let schedule = match &w.first_trigger().unwrap().kind {
                NodeKind::Schedule { schedule } => schedule.clone(),
                _ => unreachable!(),
            };
            assert_eq!(moved, schedule.next(now()).unwrap());
            assert!(moved > now() && moved - now() <= 6 * 60 * 60 * 1000);
        }

        #[test]
        fn a_schedule_that_cannot_be_computed_leaves_no_time_to_fire_on() {
            let w = workflow_with(
                r#"{"id":"start","type":"schedule","schedule":{"scheduleKind":"daily","time":"99:99","minute":0,"dayOfWeek":1}}"#,
                "",
            );
            assert_eq!(after_claim(&w, now(), Some("r")).next_run_at, None);
        }
    }

    mod due_workflows_tests {
        use super::*;

        #[test]
        fn skips_and_advances_a_due_workflow_whose_run_is_still_going() {
            let list = [workflow()];
            let due = due_workflows(&list, now(), |_| true);
            assert!(due.run.is_empty());
            assert_eq!(due.advance.len(), 1);
        }

        #[test]
        fn runs_one_that_came_due_within_the_grace_and_advances_a_stale_one() {
            let mut fresh = workflow();
            fresh.next_run_at = Some(now() - GRACE_MS);
            let mut stale = workflow();
            stale.id = "wf-stale000".into();
            stale.next_run_at = Some(now() - GRACE_MS - 1);
            let list = [fresh, stale];
            let due = due_workflows(&list, now(), never);
            assert_eq!(due.run.len(), 1);
            assert_eq!(due.run[0].id, "wf-abcd1234");
            assert_eq!(due.advance.len(), 1);
            assert_eq!(due.advance[0].id, "wf-stale000");
        }

        #[test]
        fn leaves_one_that_is_not_due_yet_or_not_enabled() {
            let mut later = workflow();
            later.next_run_at = Some(now() + 1);
            let mut off = workflow();
            off.enabled = false;
            let manual = workflow_with(r#"{"id":"start","type":"manual"}"#, "");
            let list = [later, off, manual];
            let due = due_workflows(&list, now(), never);
            assert!(due.run.is_empty() && due.advance.is_empty());
        }

        #[test]
        fn picks_a_first_time_for_one_enabled_without_a_next_run() {
            let mut w = workflow();
            w.next_run_at = None;
            let list = [w];
            let due = due_workflows(&list, now(), never);
            assert!(due.run.is_empty());
            assert_eq!(due.advance.len(), 1);
        }

        #[test]
        fn a_requested_workflow_is_always_claimed_even_with_a_run_queued() {
            // The claim decides between a slot and the queue; a request is never held back here.
            let mut w = workflow();
            w.run_requested = true;
            w.enabled = false;
            w.next_run_at = None;
            let list = [w];
            assert_eq!(due_workflows(&list, now(), never).run.len(), 1);
            let queued = due_workflows(&list, now(), |_| true);
            assert_eq!(queued.run.len(), 1);
        }

        #[test]
        fn asks_about_the_workflow_by_id() {
            let list = [workflow()];
            let due = due_workflows(&list, now(), |id| id == "wf-other000");
            assert_eq!(due.run.len(), 1);
        }
    }

    mod a_workflow_that_needs_review {
        use super::*;

        #[test]
        fn is_not_claimed_when_due() {
            let mut due = workflow();
            due.needs_review = true;
            let list = [due];
            let result = due_workflows(&list, now(), never);
            assert!(result.run.is_empty() && result.advance.is_empty());
        }

        #[test]
        fn is_not_claimed_when_a_run_was_requested() {
            let mut asked = workflow();
            asked.needs_review = true;
            asked.enabled = false;
            asked.run_requested = true;
            let list = [asked];
            let result = due_workflows(&list, now(), never);
            assert!(result.run.is_empty() && result.advance.is_empty());
        }

        #[test]
        fn runs_once_the_review_is_done() {
            let mut reviewed = workflow();
            reviewed.needs_review = false;
            let list = [reviewed];
            assert_eq!(due_workflows(&list, now(), never).run.len(), 1);
        }
    }

    /// A workflow with a manual trigger and two schedule triggers, all leading to one step.
    mod several_schedules {
        use super::*;

        fn two_schedules() -> Workflow {
            let mut w = workflow_from(
                r#"{"id":"wf-abcd1234","name":"Ship it","inputs":{},"inputValues":{},
                    "nodes":[{"id":"hand","type":"manual"},
                      {"id":"morning","type":"schedule","schedule":{"scheduleKind":"daily","time":"08:00","minute":0,"dayOfWeek":1}},
                      {"id":"evening","type":"schedule","schedule":{"scheduleKind":"daily","time":"18:00","minute":0,"dayOfWeek":1}}],
                    "edges":[],"enabled":true,"createdAt":0}"#,
            );
            w.next_run_at = None;
            w
        }

        #[test]
        fn picking_times_gives_every_schedule_its_own_and_keeps_the_earliest() {
            let w = scheduled(&two_schedules(), now()).unwrap();
            let morning = w.next_run_at_by_node["morning"];
            let evening = w.next_run_at_by_node["evening"];
            assert!(morning > now() && evening > now());
            assert!(!w.next_run_at_by_node.contains_key("hand"));
            assert_eq!(w.next_run_at, Some(morning.min(evening)));
        }

        #[test]
        fn a_paused_workflow_has_no_times_and_a_single_schedule_keeps_the_plain_field() {
            let mut paused = two_schedules();
            paused.enabled = false;
            let w = scheduled(&paused, now()).unwrap();
            assert_eq!((w.next_run_at, w.next_run_at_by_node.len()), (None, 0));
            let single = scheduled(&workflow(), now()).unwrap();
            assert!(single.next_run_at.is_some());
            assert!(single.next_run_at_by_node.is_empty());
        }

        #[test]
        fn a_workflow_with_a_schedule_without_a_time_yet_is_advanced_not_run() {
            let mut w = two_schedules();
            w.next_run_at_by_node
                .insert("morning".into(), now() - 1_000);
            let list = [w];
            let due = due_workflows(&list, now(), never);
            assert!(due.run.is_empty());
            assert_eq!(due.advance.len(), 1);
        }

        #[test]
        fn the_workflow_runs_once_when_two_schedules_come_due_in_the_same_look() {
            let mut w = two_schedules();
            w.next_run_at_by_node
                .insert("morning".into(), now() - 2_000);
            w.next_run_at_by_node
                .insert("evening".into(), now() - 1_000);
            w.next_run_at = Some(now() - 2_000);
            let list = [w.clone()];
            let due = due_workflows(&list, now(), never);
            assert_eq!(due.run.len(), 1);
            // The earlier one is the trigger that fired.
            assert_eq!(due_schedule_trigger(&w, now()).unwrap().id, "morning");
            // Claiming moves both on, so the minute does not run again.
            let claimed = after_claim(&w, now(), Some("run-1"));
            assert!(claimed.next_run_at_by_node["morning"] > now());
            assert!(claimed.next_run_at_by_node["evening"] > now());
            let after = [claimed];
            let due = due_workflows(&after, now(), never);
            assert!(due.run.is_empty() && due.advance.is_empty());
        }

        #[test]
        fn a_schedule_that_is_not_due_keeps_its_time_when_the_other_one_runs() {
            let mut w = two_schedules();
            let later = now() + 3_600_000;
            w.next_run_at_by_node
                .insert("morning".into(), now() - 1_000);
            w.next_run_at_by_node.insert("evening".into(), later);
            w.next_run_at = Some(now() - 1_000);
            let claimed = after_claim(&w, now(), Some("run-1"));
            assert_eq!(claimed.next_run_at_by_node["evening"], later);
            assert!(claimed.next_run_at_by_node["morning"] > now());
            assert_eq!(
                claimed.next_run_at,
                claimed.next_run_at_by_node.values().min().copied()
            );
            assert_eq!(due_schedule_trigger(&w, now()).unwrap().id, "morning");
        }

        #[test]
        fn a_busy_workflow_skips_the_occurrence_and_moves_on() {
            let mut w = two_schedules();
            w.next_run_at_by_node
                .insert("morning".into(), now() - 1_000);
            w.next_run_at_by_node
                .insert("evening".into(), now() + 3_600_000);
            w.next_run_at = Some(now() - 1_000);
            let list = [w];
            let due = due_workflows(&list, now(), |_| true);
            assert!(due.run.is_empty());
            assert_eq!(due.advance.len(), 1);
        }

        #[test]
        fn a_schedule_that_came_due_long_ago_is_not_run() {
            let mut w = two_schedules();
            let old = now() - GRACE_MS - 1_000;
            w.next_run_at_by_node.insert("morning".into(), old);
            w.next_run_at_by_node
                .insert("evening".into(), now() + 3_600_000);
            w.next_run_at = Some(old);
            let list = [w];
            let due = due_workflows(&list, now(), never);
            assert!(due.run.is_empty());
            assert_eq!(due.advance.len(), 1);
        }
    }
}
