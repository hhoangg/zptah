//! What the workflow pages share: how a value and a run read, exporting a workflow and saving a
//! file the user picks. (The page of a workflow itself is `layout.rs`.)
use super::super::*;
use super::actions;
use monocode_core::mono::workflows::{
    model::{InputValue, RunStatus, Workflow},
    ordered::js_number_text,
    yaml::{export_path_problem, export_workflow},
};

/// The text an input field shows for a stored value.
pub(in crate::monos) fn value_text(value: Option<&InputValue>) -> String {
    match value {
        None => String::new(),
        Some(InputValue::String(text)) => text.clone(),
        Some(InputValue::Number(n)) => js_number_text(*n),
        Some(InputValue::Bool(on)) => on.to_string(),
    }
}

/// How a run reads in the history: its label and the colour of its dot.
pub(super) fn run_status(status: RunStatus, mono_color: u32) -> (&'static str, gpui::Hsla) {
    match status {
        RunStatus::Queued => ("Queued", content(0.55)),
        RunStatus::Skipped => ("Skipped", content(0.4)),
        RunStatus::Running => ("Running", rgb(mono_color).into()),
        RunStatus::Done => ("Finished", gpui::Hsla::from(rgb(0x34d399)).opacity(0.8)),
        RunStatus::DoneWithFailures => (
            "Finished with failures",
            gpui::Hsla::from(rgb(0xfbbf24)).opacity(0.8),
        ),
        RunStatus::Failed => ("Failed", gpui::Hsla::from(rgb(0xef4444)).opacity(0.7)),
        RunStatus::Stopped => ("Stopped", content(0.4)),
        RunStatus::Interrupted => ("Interrupted", gpui::Hsla::from(rgb(0xfbbf24)).opacity(0.8)),
    }
}

impl Monos {
    /// Saves the shareable definition where the user picks. Says why when the workflow cannot be
    /// shared or the save failed; a saved file or a cancelled dialog clears the notice.
    pub(in crate::monos) fn workflow_export(&mut self, w: &Workflow, cx: &mut Context<Self>) {
        if let Some(problem) = export_path_problem(w) {
            self.workflow_said(
                Some(format!("Can't export this workflow: {problem}.")),
                true,
                cx,
            );
            return;
        }
        self.save_text_file(
            actions::workflow_file_name(&w.name),
            export_workflow(w),
            "workflow",
            |this, saved, cx| {
                let notice = matches!(saved, Saved::Failed)
                    .then(|| "Could not save the workflow file.".to_string());
                this.workflow_said(notice, true, cx);
            },
            cx,
        );
    }

    /// Asks where to save `text` as `file_name` and writes it off the UI thread. `done` hears how
    /// it went; `what` names the file in the log when the write failed.
    pub(in crate::monos) fn save_text_file(
        &mut self,
        file_name: String,
        text: String,
        what: &'static str,
        done: impl FnOnce(&mut Self, Saved, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let directory = monocode_core::accounts::home().unwrap_or_default();
        let prompt = cx.prompt_for_new_path(&directory, Some(&file_name));
        cx.spawn(async move |this, cx| {
            let path = match prompt.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => {
                    let _ = this.update(cx, |this, cx| done(this, Saved::Cancelled, cx));
                    return;
                }
                _ => {
                    let _ = this.update(cx, |this, cx| done(this, Saved::Failed, cx));
                    return;
                }
            };
            let target = path.clone();
            let wrote = cx
                .background_executor()
                .spawn(async move { std::fs::write(target, text) })
                .await;
            let saved = match wrote {
                Ok(()) => Saved::To(path),
                Err(error) => {
                    eprintln!("Could not export the {what}: {error}");
                    Saved::Failed
                }
            };
            let _ = this.update(cx, |this, cx| done(this, saved, cx));
        })
        .detach();
    }
}

/// How saving a shared file went.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::monos) enum Saved {
    To(std::path::PathBuf),
    Cancelled,
    Failed,
}

pub(in crate::monos) fn basename(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}
