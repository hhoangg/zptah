//! The ties between the Monos and the global workflows. A Mono owns no workflow and no run: its
//! `workflows.*` actions go through the global store (`host.rs`), and the pages that still live in
//! this module read the runner that the `Workflows` entity drives.
use super::*;

impl Monos {
    /// Stops the workflow's run. False when none is going.
    pub(super) fn stop_workflow_run(&mut self, workflow_id: &str) -> bool {
        self.workflows.stop(workflow_id)
    }
}
