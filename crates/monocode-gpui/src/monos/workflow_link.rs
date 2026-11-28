//! The ties between the Monos and the global workflows. A Mono owns no workflow and no run: its
//! `workflows.*` actions go through the global store (`host.rs`), and the pages that still live in
//! this module read the runner that the `Workflows` entity drives.
use super::*;

impl Monos {
    /// Stops the run `run_id` of the workflow, or every run it has going. False when none is going.
    pub(super) fn stop_workflow_run(&mut self, workflow_id: &str, run_id: Option<&str>) -> bool {
        self.workflows.stop(workflow_id, run_id)
    }
}
