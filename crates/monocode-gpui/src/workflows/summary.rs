//! Delivering a run's summary. The run page always has it (the runner stores it on the run). It
//! also always goes out as a notification, attached to the run's last session as `agentFinished`
//! (desktop and phone), or desktop only when the run has no session. Besides, it reaches the chat
//! of the Mono named in `reportTo` and the chat of the Mono that started the run; the same Mono
//! twice is told once.
use super::{Workflows, runner::Summary};
use gpui::Context;
use monocode_core::mono::workflows::model::{CallerKind, WorkflowRun};

/// The Monos whose chats get the summary of `run`, in order and without repeats: `reportTo.mono`,
/// then the Mono that started the run.
pub(crate) fn summary_monos(run: &WorkflowRun) -> Vec<String> {
    let named = run
        .definition
        .report_to
        .as_ref()
        .and_then(|r| r.mono.clone());
    let caller = (run.caller.kind == CallerKind::Mono)
        .then(|| run.caller.mono_id.clone())
        .flatten();
    let mut monos: Vec<String> = Vec::new();
    for mono in named.into_iter().chain(caller) {
        if !mono.is_empty() && !monos.contains(&mono) {
            monos.push(mono);
        }
    }
    monos
}

/// The notification text: the title and the first line of the summary that is not a heading.
pub(crate) fn notice_body(title: &str, text: &str) -> String {
    match text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
    {
        Some(line) => format!("{title}: {line}"),
        None => title.to_owned(),
    }
}

impl Workflows {
    /// Sends the summary where it goes (see the module docs).
    pub(in crate::workflows) fn deliver(&mut self, summary: Summary, cx: &mut Context<Self>) {
        let Summary {
            run,
            title,
            text,
            launches,
        } = summary;
        let last_session = launches.iter().rev().find_map(|launch| launch.local_id);
        if let Some(shell) = self.shell.upgrade() {
            let body = notice_body(&title, &text);
            shell.update(cx, |shell, cx| {
                shell.workflow_notice(last_session, &title, &body, cx)
            });
        }
        let cards: Vec<(i64, String)> = launches
            .iter()
            .filter_map(|launch| Some((launch.local_id?, launch.title.clone())))
            .collect();
        for mono in summary_monos(&run) {
            let (text, cards) = (text.clone(), cards.clone());
            if let Some(monos) = self.monos.upgrade() {
                monos.update(cx, |monos, cx| monos.workflow_post(&mono, text, cards, cx));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monocode_core::mono::workflows::model::Node;
    use monocode_core::mono::workflows::{
        engine::start_run,
        model::{NodeKind, ReportTo, RunCaller, Workflow},
    };

    fn run_of(report_to: Option<&str>, caller: RunCaller) -> WorkflowRun {
        let mut w = Workflow::new("wf-1", "Ship");
        w.nodes = vec![Node::new("start", NodeKind::Manual)];
        w.report_to = report_to.map(|mono| ReportTo {
            mono: Some(mono.into()),
        });
        start_run(&w, caller, 1, "run-1")
    }

    #[test]
    fn the_summary_goes_to_no_chat_unless_a_mono_asked_for_it() {
        let run = run_of(None, RunCaller::new(CallerKind::Manual, "start"));
        assert!(summary_monos(&run).is_empty());
        let scheduled = run_of(None, RunCaller::new(CallerKind::Schedule, "start"));
        assert!(summary_monos(&scheduled).is_empty());
    }

    #[test]
    fn report_to_and_the_mono_that_started_the_run_each_get_the_summary() {
        let run = run_of(Some("mono-a"), RunCaller::new(CallerKind::Manual, "start"));
        assert_eq!(summary_monos(&run), ["mono-a"]);
        let run = run_of(None, RunCaller::mono("mono-b", "start"));
        assert_eq!(summary_monos(&run), ["mono-b"]);
        let run = run_of(Some("mono-a"), RunCaller::mono("mono-b", "start"));
        assert_eq!(summary_monos(&run), ["mono-a", "mono-b"]);
    }

    #[test]
    fn a_mono_that_is_both_named_and_the_caller_is_told_once() {
        let run = run_of(Some("mono-a"), RunCaller::mono("mono-a", "start"));
        assert_eq!(summary_monos(&run), ["mono-a"]);
    }

    #[test]
    fn the_notification_reads_the_title_and_the_first_line_that_is_not_a_heading() {
        assert_eq!(
            notice_body(
                "Ship · done",
                "## Ship · done\n\nAll 3 steps passed.\nMore."
            ),
            "Ship · done: All 3 steps passed."
        );
        assert_eq!(
            notice_body("Ship · done", "## Ship · done\n"),
            "Ship · done"
        );
    }
}
