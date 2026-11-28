//! The fallback extraction: when a step ends and declared fields are still missing, a hidden
//! one-shot call reads them out of the session's conversation. It speaks with the voice of the
//! session itself (its harness, model and account, never a hardcoded cheap model), runs on a
//! thread of its own with the same cancel and timeout as an AI decision, and never writes into
//! the user's session.
use super::{
    Workflows,
    decide::{DecisionSetup, decide},
    runner::{DecisionRequest, ExtractRequest},
};
use crate::runtime::{Database, locked};
use gpui::{BackgroundExecutor, Context};
use monocode_core::{
    accounts::Profile,
    mono::workflows::output::{TRANSCRIPT_MAX_CHARS, extraction_prompt, transcript_text},
    provider::{self, Kind},
};
use std::path::PathBuf;

/// Who answers an extraction: the session's own harness, model, effort and account.
#[derive(Clone, Debug)]
pub(crate) struct Voice {
    pub kind: Kind,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub account: Profile,
    /// Where the call runs: the session's checkout when it is a folder here.
    pub cwd: PathBuf,
    /// The session whose conversation is read.
    pub local: i64,
}

impl Workflows {
    /// The voice of the session `persisted`. UI thread only.
    pub(in crate::workflows) fn extraction_voice(
        &mut self,
        persisted: &str,
        cx: &mut Context<Self>,
    ) -> Result<Voice, String> {
        let local = self
            .workflow_local_id(persisted)
            .ok_or("The session is no longer available.")?;
        if self.live.tui.contains_key(&local) {
            return Err("A terminal session has no conversation the app can read.".into());
        }
        let (_, view) = self
            .workflow_view(persisted, cx)
            .ok_or("The session is no longer available.")?;
        let view = view.read(cx);
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let cwd = PathBuf::from(
            view.settings
                .worktree_cwd
                .clone()
                .unwrap_or_else(|| view.summary.cwd.clone()),
        );
        Ok(Voice {
            kind: Kind::from_id(&view.summary.provider),
            model: view.settings.model.clone(),
            effort: view.settings.effort.clone(),
            account: view.settings.account.clone(),
            cwd: if cwd.is_dir() { cwd } else { home },
            local,
        })
    }
}

/// Reads the request's fields out of the voice's conversation. Dropping the future (the run was
/// stopped) cancels the call.
pub(crate) async fn extract(
    voice: Result<Voice, String>,
    store: Database,
    request: ExtractRequest,
    executor: BackgroundExecutor,
) -> Result<String, String> {
    let voice = voice?;
    if !monocode_core::session_title::supports(voice.kind) {
        return Err(format!(
            "{} cannot answer a one-shot question, so its conversation cannot be read for the fields",
            voice.kind.label()
        ));
    }
    let local = voice.local;
    let rows = executor
        .spawn(async move { locked(&store)?.load(local).map(|loaded| loaded.rows) })
        .await?;
    let transcript = transcript_text(&rows, TRANSCRIPT_MAX_CHARS);
    if transcript.trim().is_empty() {
        return Err("The conversation is empty".into());
    }
    let binary = provider::resolve(voice.kind).ok_or_else(|| voice.kind.unavailable_hint())?;
    let setup = DecisionSetup::new(voice.kind, binary, voice.cwd, voice.model, voice.account);
    let ask = DecisionRequest {
        harness: None,
        model: None,
        effort: voice.effort,
        project: None,
        prompt: extraction_prompt(&request.fields, &transcript),
        timeout_ms: request.timeout_ms,
    };
    decide(Ok(setup), ask, executor).await
}
