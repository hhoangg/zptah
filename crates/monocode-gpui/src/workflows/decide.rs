//! AI decisions: a hidden one-shot, no-tools model call for an If, a Switch or a loop edge. The
//! model is what `decision_model_for` found (the workflow's `decisionModel`, else the nearest
//! upstream session's); a part it leaves unset is the app's default for a new session, never a
//! hardcoded "cheap" model. The call blocks, so it runs on a thread of its own with a cancel flag
//! and a timeout.
use super::{Workflows, runner::DecisionRequest, runner::model_harness};
use async_channel::bounded;
use gpui::{App, BackgroundExecutor};
use monocode_core::{
    accounts::Profile,
    agent::Settings,
    provider::{self, Kind, Request},
    store::native_model,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// What a decision call needs, decided on the UI thread.
pub(crate) struct DecisionSetup {
    kind: Kind,
    binary: PathBuf,
    cwd: PathBuf,
    model: Option<String>,
    account: Profile,
}

/// The harness a decision names: its own `harness`, else the one its model id names
/// (`grok:grok-4.6`), else `None` (the app's default harness).
pub(crate) fn named_kind(
    harness: Option<&str>,
    model: Option<&str>,
) -> Result<Option<Kind>, String> {
    match harness.filter(|h| !h.is_empty()) {
        Some(id) => Kind::parse(id)
            .map(Some)
            .ok_or_else(|| format!("Unknown harness \"{id}\"")),
        None => Ok(model.and_then(model_harness).and_then(Kind::parse)),
    }
}

/// The model a decision runs with: the one it names (as the harness's CLI writes it), else the
/// app's default model for the harness (`default`).
pub(crate) fn text_model(
    kind: Kind,
    model: Option<&str>,
    default: Option<String>,
) -> Option<String> {
    match model.filter(|m| !m.is_empty()) {
        Some(model) => native_model(model, kind.id()).or(default),
        None => default,
    }
}

/// Sets the cancel flag when the call is dropped, so abandoning a decision (Stop, quit) ends it.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Workflows {
    /// Resolves who answers a decision: the harness, model and account, from the request and the
    /// app's defaults. UI thread only.
    pub(in crate::workflows) fn decision_setup(
        &self,
        request: &DecisionRequest,
        cx: &App,
    ) -> Result<DecisionSetup, String> {
        let shell = self.shell.upgrade().ok_or("The app is closing")?;
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let project = request.project.clone();
        let cwd = project
            .as_deref()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or(home);
        let named = named_kind(request.harness.as_deref(), request.model.as_deref())?;
        let (kind, default_model) = shell
            .read(cx)
            .workflow_default_choice(project.as_deref().unwrap_or(""), named);
        if !monocode_core::session_title::supports(kind) {
            return Err(format!(
                "{} cannot answer a decision without tools; set the workflow's decisionModel to another harness",
                kind.label()
            ));
        }
        let binary = provider::resolve(kind).ok_or_else(|| kind.unavailable_hint())?;
        let account = crate::accounts::get(cx)
            .map(|center| {
                center
                    .read(cx)
                    .selected(kind, project.as_deref().unwrap_or(""))
            })
            .unwrap_or_default();
        Ok(DecisionSetup {
            kind,
            binary,
            cwd,
            model: text_model(kind, request.model.as_deref(), default_model),
            account,
        })
    }
}

/// Runs the call on its own thread and waits for it, up to the request's timeout. Dropping the
/// returned future (the run was stopped) cancels the call.
pub(crate) async fn decide(
    setup: Result<DecisionSetup, String>,
    request: DecisionRequest,
    executor: BackgroundExecutor,
) -> Result<String, String> {
    let setup = setup?;
    let cancel = Arc::new(AtomicBool::new(false));
    let _on_drop = CancelOnDrop(cancel.clone());
    let (done, answer) = bounded(2);
    let effort = request.effort.clone();
    let prompt = request.prompt.clone();
    let thread_cancel = cancel.clone();
    let thread_done = done.clone();
    std::thread::Builder::new()
        .name("workflow-decision".into())
        .spawn(move || {
            let reply = provider::run_text_with_effort(
                Request {
                    kind: setup.kind,
                    binary: setup.binary,
                    cwd: setup.cwd,
                    prompt,
                    provider_id: None,
                    model: setup.model,
                    settings: Settings {
                        account: setup.account,
                        ..Default::default()
                    },
                },
                effort,
                thread_cancel,
            );
            let _ = thread_done.send_blocking(reply);
        })
        .map_err(|error| format!("Could not start the decision: {error}"))?;
    let timer = executor.timer(Duration::from_millis(request.timeout_ms));
    let timeout_cancel = cancel.clone();
    let _timer = executor.spawn(async move {
        timer.await;
        timeout_cancel.store(true, Ordering::SeqCst);
        let _ = done.send(Err("The AI did not answer in time".into())).await;
    });
    answer
        .recv()
        .await
        .unwrap_or_else(|_| Err("The decision ended without an answer".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_the_node_names_wins_then_the_one_its_model_names() {
        assert_eq!(
            named_kind(Some("grok"), Some("claude:sonnet-5")),
            Ok(Some(Kind::Grok))
        );
        assert_eq!(
            named_kind(None, Some("claude:sonnet-5")),
            Ok(Some(Kind::Claude))
        );
        assert_eq!(named_kind(None, Some("sonnet")), Ok(None));
        assert_eq!(named_kind(Some(""), None), Ok(None));
        assert!(named_kind(Some("nope"), None).is_err());
    }

    #[test]
    fn a_model_the_node_names_is_written_for_its_cli_else_the_apps_default_is_used() {
        let default = Some("app-default".to_string());
        assert_eq!(
            text_model(Kind::Claude, Some("claude:sonnet-5"), default.clone()),
            Some("claude-sonnet-5".into())
        );
        assert_eq!(text_model(Kind::Claude, None, default.clone()), default);
        assert_eq!(text_model(Kind::Claude, Some(""), default.clone()), default);
        assert_eq!(text_model(Kind::Claude, None, None), None);
    }
}
