//! Starting the session of a workflow step without a Mono: the same path the Operator uses for
//! `sessions.start` (store row, settings, first prompt, then the shell installs it), with the app's
//! own defaults where the node sets nothing, the app's projects only, and one sidebar folder per
//! workflow.
use super::{
    Workflows,
    runner::{CloudInstance, SessionRequest, StartedSession, model_harness},
};
use crate::{monos::CLOUD_ACCOUNT_NEEDS_TOKEN, runtime::locked};
use async_channel::Sender;
use gpui::{App, Context};
use monocode_core::{
    agent::AccessMode,
    app_access as app,
    mono::{
        self,
        cloud_api::{self, CloudApi},
        workflows::{
            model::{Checkout, Node, NodeKind, SessionTarget, TargetInstance, Workflow},
            models::{ModelCatalog, harnesses_used, session_node_problem},
            ordered::OrderedMap,
        },
    },
    provider::Kind,
    session_groups::Groups,
    store::native_model,
};
use serde_json::{Value, json};

const REMOTE_TERMINAL: &str = "A terminal step runs on this computer, but this step's project is on another machine. Use a chat step.";

/// A model the caller vouches for, with no settings of its own.
fn bare_model(id: &str) -> monocode_core::catalog::Model {
    monocode_core::catalog::Model {
        id: id.into(),
        name: id.into(),
        alias_target: None,
        efforts: Vec::new(),
        default_effort: None,
        fast: false,
        thinking: false,
        default: false,
        tiers: Vec::new(),
        controls: Vec::new(),
    }
}

/// What the background half of a start made.
struct Made {
    summary: monocode_core::store::Session,
    settings: monocode_core::agent::Settings,
    persisted: String,
    parent: Option<i64>,
}

impl Workflows {
    /// The app project a step names, by full path or folder name (D3: the app's projects only).
    pub(in crate::workflows) fn app_project(&self, named: &str, cx: &App) -> Option<String> {
        self.shell.upgrade()?.read(cx).workflow_project(named)
    }

    /// Starts a workflow node's session. Everything that can be refused is refused before the
    /// session row exists; the answer arrives on `reply`. `draft` only puts the prompt in the
    /// composer (tests).
    pub(in crate::workflows) fn workflow_session(
        &mut self,
        req: SessionRequest,
        draft: bool,
        reply: Sender<Result<StartedSession, String>>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let shell = self.shell.upgrade().ok_or("The app is closing")?;
        // A project on another machine has no local terminal: refused before anything is made,
        // so no empty chat is left behind.
        if req.terminal && monocode_core::surface::is_remote(&req.project) {
            return Err(REMOTE_TERMINAL.into());
        }
        let project = self
            .app_project(&req.project, cx)
            .ok_or("Not one of the app's projects")?;
        // A model alone is started in its own harness, as the picker pins both. A cloud step
        // runs Claude.
        let harness = req
            .harness
            .clone()
            .or_else(|| {
                req.model
                    .as_deref()
                    .and_then(model_harness)
                    .map(str::to_owned)
            })
            .or_else(|| req.target.is_some().then(|| "claude".to_owned()));
        let asked_kind = match &harness {
            Some(id) => Some(Kind::parse(id).ok_or("Unknown harness")?),
            None => None,
        };
        let mut input = json!({
            "project": project,
            "prompt": req.prompt,
            "runtimeMode": req.access,
            "notifyOnComplete": false,
            "draft": draft,
        });
        if let Some(harness) = &harness {
            input["harness"] = json!(harness);
        }
        if let Some(effort) = &req.effort {
            input["effort"] = json!(effort);
        }
        // A cloud session works in its instance's own checkout: never a worktree.
        if req.target.is_none() {
            if req.new_worktree {
                input["workspaceMode"] = json!("worktree");
            }
            if let Some(cwd) = &req.worktree_cwd {
                input["worktreeCwd"] = json!(cwd);
            }
        }
        // Nesting is decoration: a parent the app does not know is just a top-level session.
        let parent = req
            .parent_session_id
            .as_deref()
            .and_then(|parent| self.workflow_local_id(parent));
        // The `sessions.start` target, with the instance of another session resolved. The Mono
        // API checks it before a start; this path skips that, so it is checked here, before
        // anything is created.
        let mut cloud = None;
        if let Some(target) = &req.target {
            let instance = match &target.instance {
                CloudInstance::New => json!("new"),
                CloudInstance::OfSession(persisted) => {
                    let local = self
                        .workflow_local_id(persisted)
                        .ok_or("The cloud instance to use is no longer available")?;
                    json!({"session": local})
                }
            };
            let mut options = target.options.clone();
            options.insert("kind".into(), json!(target.kind));
            options.insert("instance".into(), instance);
            input["target"] = Value::Object(options);
            if let Some(account) = &target.account {
                input["account"] = json!(account);
            }
            mono::cloud_api::validate::validate_target(&input["target"], &input)?;
            let parsed = cloud_api::target_from_input(&input)?;
            if let Some(parsed) = &parsed {
                shell.read(cx).refuse_new_session_on(parsed)?;
            }
            cloud = parsed;
        }
        // The account of a cloud instance: the one the node names, which must carry a token or
        // key (the CLI login of this machine does not travel to a codespace).
        let mut cloud_account = None;
        if let (Some(selector), Some(target)) =
            (input.get("account").and_then(Value::as_str), cloud.as_mut())
        {
            let center = crate::accounts::get(cx).ok_or("Provider accounts are unavailable")?;
            let center = center.read(cx);
            let id = monocode_core::accounts::resolve(&center.book, Kind::Claude, selector)
                .map_err(|error| error.to_string())?;
            if !center.uses_claude_token(&id) {
                return Err(CLOUD_ACCOUNT_NEEDS_TOKEN.into());
            }
            target.account = Some(id.clone());
            cloud_account = Some(center.profile(&id));
        }
        let surface = if req.terminal {
            monocode_core::surface::Surface::Tui
        } else {
            monocode_core::surface::Surface::Chat
        };
        let defaults = shell
            .read(cx)
            .workflow_defaults(&project, asked_kind, surface, cx);
        let kind = defaults.kind();
        // Only Claude's CLI takes a first prompt from the app, so a terminal step that would run
        // anything else is refused before a session exists.
        if req.terminal && kind != Kind::Claude {
            return Err(format!(
                "A terminal step runs Claude only, but this step would run {}. Set the step's harness to claude.",
                kind.label()
            ));
        }
        // The harness the node did not name is the app's default one, and so is a model it did
        // not name. Only what the node says is checked against the catalog.
        let check = (harness.is_some() || req.model.is_some() || req.effort.is_some()).then(|| {
            Node::new(
                "start",
                NodeKind::Session {
                    project: String::new(),
                    prompt: String::new(),
                    checkout: Checkout::Root,
                    output: OrderedMap::new(),
                    mode: None,
                    surface: None,
                    harness: harness.clone(),
                    model: req.model.clone(),
                    effort: req.effort.clone(),
                    reuse_session: None,
                    nest_under: None,
                    timeout_minutes: None,
                    account: None,
                    access: None,
                    folder: None,
                    link_work_item: None,
                    // Which cloud kind or instance does not matter to the model check; that it is
                    // a cloud step does (Claude runs in the instance, not here).
                    target: req.target.as_ref().map(|cloud| SessionTarget {
                        kind: cloud.kind.clone(),
                        instance: TargetInstance::New,
                        options: OrderedMap::new(),
                    }),
                },
            )
        });
        let probe = self.probes().models;
        let store = self.store.clone();
        let model = req.model.clone();
        let effort = req.effort.clone();
        let request_id = mono::new_id();
        let terminal = req.terminal;
        let access = req.access.clone();
        let work_item = req.work_item.clone();
        let make = {
            let project = project.clone();
            let input = input.clone();
            let prompt = req.prompt.clone();
            let provider = kind.id().to_owned();
            move || -> Result<Made, String> {
                if let Some(node) = check {
                    let mut probed = Workflow::new("wf-start", "start");
                    probed.nodes.push(node.clone());
                    let catalog = ModelCatalog::for_harnesses(&harnesses_used(&probed), probe);
                    if let Some(problem) = session_node_problem(&node, &catalog) {
                        return Err(problem);
                    }
                }
                let worktree =
                    input.get("workspaceMode").and_then(Value::as_str) == Some("worktree");
                if worktree && monocode_core::git::unborn(std::path::Path::new(&project)) {
                    return Err("This project has no commits yet; make an initial commit before using a new worktree".into());
                }
                // Every check that needs the store runs before a codespace is created.
                let mut parent = parent;
                {
                    let db = locked(&store)?;
                    if let Some(candidate) = parent {
                        let key = monocode_core::files::path_key(&project);
                        let known = db.sessions()?.iter().any(|s| {
                            s.id == candidate && monocode_core::files::path_key(&s.cwd) == key
                        });
                        // A session started under another one cannot be a parent: no child of a
                        // child. A parent that does not qualify only drops the nesting.
                        let child = db.started_links()?.contains_key(&candidate);
                        if !known || child {
                            parent = None;
                        }
                    }
                }
                let startup = match cloud {
                    Some(target) => {
                        Some(CloudApi::shared(&store)?.begin_start(&project, target, &provider)?)
                    }
                    None => None,
                };
                let note = |e: String| crate::cloud_actions::failure_note(startup.as_ref(), e);
                let mut db = locked(&store).map_err(note)?;
                let (mut summary, mut settings) = match &startup {
                    Some(start) => {
                        let mut settings = monocode_core::agent::Settings::default();
                        if let Some(account) = cloud_account {
                            settings.account = account;
                        }
                        (start.session().clone(), settings)
                    }
                    None => defaults.create(&db, &project).map_err(note)?,
                };
                let mut made = None;
                let built = (|| {
                    // The model and effort were judged against the workflow's own catalog above,
                    // which may be unable to list them, so they are applied as given.
                    let native = model
                        .as_deref()
                        .map(|m| native_model(m, &provider).unwrap_or_else(|| m.to_owned()));
                    let mut bare = input.clone();
                    if let Some(fields) = bare.as_object_mut() {
                        fields.remove("effort");
                        if let Some(native) = &native {
                            fields.insert("model".into(), json!(native));
                        }
                    }
                    let known: Vec<_> = native.iter().map(|id| bare_model(id)).collect();
                    settings = app::launch_settings(
                        &bare,
                        &provider,
                        std::mem::take(&mut settings),
                        kind,
                        &known,
                    )?;
                    if let Some(effort) = &effort {
                        settings.effort = Some(effort.clone());
                    }
                    settings.access = AccessMode::from_id(&access);
                    settings.revision = 0;
                    // A cloud session has no local checkout to prepare.
                    if startup.is_none() {
                        app::launch_checkout(&input, &project, summary.id, &mut settings)?;
                    }
                    if worktree {
                        made = settings.worktree_cwd.as_ref().map(std::path::PathBuf::from);
                    }
                    // The item of the event that started the run, linked on this computer only.
                    if let Some(item) = &work_item {
                        db.set_work_item(summary.id, Some(item))?;
                        summary.work_item = Some(item.clone());
                    }
                    summary.title = prompt.chars().take(160).collect();
                    db.title(summary.id, &summary.title)?;
                    if terminal {
                        // The CLI starts in this folder and a terminal has no first send to make a
                        // worktree later: the one the step asked for exists already.
                        settings.new_worktree = false;
                        settings.worktree_base = None;
                    }
                    db.configure_agent(summary.id, &provider, &settings)?;
                    // A terminal takes its prompt on the command line, not from the store.
                    if !terminal {
                        db.prepare_app_prompt(summary.id, &request_id, &prompt)?;
                    }
                    // A terminal session is listed once the CLI has saved a conversation id.
                    summary.has_messages = !terminal;
                    if let Some(parent) = parent {
                        db.save_started_link(summary.id, parent)?;
                    }
                    Ok::<_, String>(())
                })();
                if let Err(e) = built {
                    // Nothing else refers to the row or its worktree yet (a sandbox row is kept:
                    // it is the only handle on the sandbox, see `discard_failed_start`).
                    crate::cloud_actions::discard_failed_start(
                        &mut db,
                        startup.as_ref(),
                        summary.id,
                        &e,
                    );
                    let e = match made.map(|tree| {
                        monocode_core::worktree_management::discard_new(&tree)
                            .map_err(|c| (tree, c))
                    }) {
                        Some(Err((tree, cause))) => format!(
                            "{e} (the new worktree {} was left on disk: {cause})",
                            tree.display()
                        ),
                        _ => e,
                    };
                    return Err(note(e));
                }
                let persisted = db.persisted_session_id(summary.id)?;
                Ok(Made {
                    summary,
                    settings,
                    persisted,
                    parent,
                })
            }
        };
        let work = cx.background_executor().spawn(async move { make() });
        let folder = req.folder_name.clone();
        let folder_id = req.folder_id.clone();
        let terminal_prompt = terminal.then(|| req.prompt.clone());
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let started = result.and_then(|made| {
                    this.install_started(
                        made,
                        &project,
                        (folder, folder_id),
                        draft,
                        terminal_prompt,
                        cx,
                    )
                });
                let _ = reply.try_send(started);
            });
        })
        .detach();
        Ok(())
    }

    /// Puts the session a start made into the app, files it, and starts watching it.
    fn install_started(
        &mut self,
        made: Made,
        project: &str,
        folder: (Option<String>, Option<String>),
        draft: bool,
        terminal_prompt: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<StartedSession, String> {
        let Made {
            summary,
            settings,
            persisted,
            parent,
        } = made;
        let local = summary.id;
        if let Some(shell) = self.shell.upgrade() {
            // The state of a terminal step's turn exists before its pane can report anything.
            if terminal_prompt.is_some() {
                self.live.tui.insert(local, Default::default());
            }
            let opened = shell.update(cx, |shell, cx| {
                let opened = match &terminal_prompt {
                    Some(prompt) => shell.workflow_install_tui(summary, &settings, prompt, cx),
                    None => {
                        shell.mono_install_app(summary, settings, draft, cx);
                        true
                    }
                };
                shell.workflow_session_started(local, cx);
                if let Some(parent) = parent {
                    shell.mono_link_started(local, parent, cx);
                }
                opened
            });
            if !opened {
                self.live.tui.remove(&local);
                return Err("The terminal session could not be opened.".into());
            }
            // The node's own folder wins; the workflow's folder is the fallback (D31).
            let (name, id) = folder;
            self.file_in_folder(project, local, name.as_deref(), id.as_deref(), cx);
        }
        self.live.ids.insert(persisted.clone(), local);
        // A terminal turn is told by the pane's events, which wake the wait as they come; the CLI
        // takes a while to show a turn, which is not a reason to give up.
        if terminal_prompt.is_some() {
            let settled = self.workflow_settled(local, true, cx);
            return Ok(StartedSession {
                session_id: persisted,
                settled,
            });
        }
        let view = self
            .shell
            .upgrade()
            .and_then(|shell| shell.read(cx).mono_session_entity(local));
        let armed = match &view {
            Some(view) => {
                self.workflow_watch(local, view, cx);
                // A draft only leaves the prompt in the composer: no turn is coming for the run
                // to wait on, so it is not armed (it gives up after a while, as for any prompt
                // that is never taken).
                !draft && view.read(cx).workflow_working()
            }
            None => false,
        };
        let settled = self.workflow_settled(local, armed, cx);
        Ok(StartedSession {
            session_id: persisted,
            settled,
        })
    }

    /// Files `session` in a folder of `project`: the folder `id` when the node chose one and it
    /// still exists, else the workflow's folder with its name when there is one (every run uses
    /// it), else a new one. Nothing is filed without either.
    pub(in crate::workflows) fn file_in_folder(
        &mut self,
        project: &str,
        session: i64,
        name: Option<&str>,
        id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        let mut groups: Groups = shell.read(cx).mono_groups(project);
        let chosen = id
            .and_then(|id| groups.folders.iter().find(|f| f.id == id))
            .map(|f| f.id.clone());
        if let Some(chosen) = chosen {
            groups.add_to_folder(&chosen, session);
            let _ = shell.update(cx, |shell, cx| {
                shell.mono_apply_groups(project.to_owned(), groups, cx)
            });
            return;
        }
        let Some(name) = name else {
            return;
        };
        match groups
            .folders
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.id.clone())
        {
            Some(id) => groups.add_to_folder(&id, session),
            None => {
                if let Some(folder) = groups.create_folder(&[session], mono::new_id()) {
                    groups.rename_folder(&folder, name);
                }
            }
        }
        // A failed save shows in the shell's error line; nobody waits for it here.
        let _ = shell.update(cx, |shell, cx| {
            shell.mono_apply_groups(project.to_owned(), groups, cx)
        });
    }
}
