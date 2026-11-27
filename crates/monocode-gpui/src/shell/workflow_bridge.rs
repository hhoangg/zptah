//! What the global workflow runner asks of the shell: which projects the app has, the defaults of
//! a new session, installing a session a run started, and the run summary notification.
use super::*;
use monocode_core::agent::Settings;

/// The settings a new workflow session starts from: the app's defaults for the harness (the
/// node's, else the app's default one), read on the UI thread.
pub(crate) struct WorkflowDefaults {
    defaults: session_loading::NewSessionDefaults,
    kind: Kind,
}

impl WorkflowDefaults {
    pub(crate) fn kind(&self) -> Kind {
        self.kind
    }

    /// Creates the (chat) session row and its starting settings. Call off the UI thread.
    pub(crate) fn create(
        self,
        store: &monocode_core::store::Store,
        cwd: &str,
    ) -> Result<(Session, Settings), String> {
        self.defaults.create(store, cwd, None)
    }
}

impl Shell {
    /// Whether a session wears the zap badge: an automation or a workflow started it.
    pub(crate) fn session_zap(&self, id: i64) -> bool {
        self.automation_state.session_automations.contains_key(&id)
            || self.workflow_sessions.contains(&id)
    }

    /// A workflow started session `id`: it gets the zap badge.
    pub(crate) fn workflow_session_started(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.workflow_sessions.insert(id) {
            self.notify_sidebar(cx);
        }
    }

    /// Reads which sessions the workflows' run files name, off the UI thread, once at startup.
    pub(crate) fn load_workflow_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(workflows) = self.workflows() else {
            return;
        };
        let dir = workflows.read(cx).dir().clone();
        let store = self.store.clone();
        let task = cx.background_executor().spawn(async move {
            let persisted = crate::workflows::run_session_ids(&dir);
            let db = locked(&store).ok()?;
            Some(
                persisted
                    .iter()
                    .filter_map(|id| db.resolve_existing_session_id(id))
                    .collect::<HashSet<i64>>(),
            )
        });
        cx.spawn(async move |this, cx| {
            if let Some(ids) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.workflow_sessions.extend(ids);
                    this.notify_sidebar(cx);
                });
            }
        })
        .detach();
    }

    /// The workflows entity (there is one per app).
    pub(crate) fn workflows(&self) -> Option<Entity<crate::workflows::Workflows>> {
        self.workflows.clone()
    }

    /// The app project a workflow step names, by full path or folder name. A project the app does
    /// not have is refused.
    pub(crate) fn workflow_project(&self, requested: &str) -> Option<String> {
        let requested = requested.trim();
        if requested.is_empty() {
            return None;
        }
        // The project that is open counts too, whether or not the rail lists it yet.
        let mut projects = self.workspace.projects.clone();
        if !self.workspace.project.is_empty() && !projects.contains(&self.workspace.project) {
            projects.push(self.workspace.project.clone());
        }
        monocode_core::mono::resolve_project(&projects, requested).ok()
    }

    /// The defaults of a new session in `project`: `harness` when the node names one, else the
    /// app's default harness for new sessions. The model, effort, account and access follow the
    /// app's own choices for that harness.
    pub(crate) fn workflow_defaults(
        &self,
        project: &str,
        harness: Option<Kind>,
        cx: &App,
    ) -> WorkflowDefaults {
        let choice = match harness {
            Some(kind) => {
                new_chat_choice::NewChatChoice::explicit(&self.workspace.preferences, project, kind)
            }
            None => self.new_chat_choice(project),
        };
        let kind = choice.kind;
        WorkflowDefaults {
            defaults: session_loading::NewSessionDefaults::read(
                choice,
                project,
                monocode_core::surface::Surface::Chat,
                cx,
            ),
            kind,
        }
    }

    /// The harness and model a new session in `project` starts with: `harness` when the node
    /// names one, else the app's default harness, each with its default model.
    pub(crate) fn workflow_default_choice(
        &self,
        project: &str,
        harness: Option<Kind>,
    ) -> (Kind, Option<String>) {
        let choice = match harness {
            Some(kind) => {
                new_chat_choice::NewChatChoice::explicit(&self.workspace.preferences, project, kind)
            }
            None => self.new_chat_choice(project),
        };
        (choice.kind, choice.model)
    }

    /// Shows a run summary: attached to `session` as `agentFinished` (desktop and phone) when the
    /// run has one, else as a desktop notification of its own.
    pub(crate) fn workflow_notice(
        &mut self,
        session: Option<i64>,
        title: &str,
        body: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(summary) = session.and_then(|id| self.session_summaries.get(&id).cloned()) {
            self.deliver_session_notice(&summary, "agentFinished", body, None, cx);
            return;
        }
        self.deliver_plain_notice(title, body);
    }
}
