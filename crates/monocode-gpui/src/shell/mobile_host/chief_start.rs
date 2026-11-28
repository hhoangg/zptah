//! A new session the Chief starts directly, for a project no Mono covers: the owner's default
//! harness and model for the project, the app's default access mode (never full access, as for
//! a session a remote device starts), and the confirmed text as its first prompt. The session
//! is an ordinary one in the sidebar; nothing follows it afterwards.
use super::*;

#[cfg(test)]
thread_local! {
    /// Tests: the started session records its prompt but never runs a provider.
    static HOLD_CHIEF_STARTS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl Shell {
    /// Tests: sessions the Chief starts on this thread record their prompt and run no provider.
    #[cfg(test)]
    pub(crate) fn test_hold_chief_starts(hold: bool) {
        HOLD_CHIEF_STARTS.with(|flag| flag.set(hold));
    }

    /// Tests: whether sessions started on this thread are to run no provider.
    #[cfg(test)]
    pub(crate) fn test_chief_starts_held() -> bool {
        HOLD_CHIEF_STARTS.with(|hold| hold.get())
    }

    /// The projects the Chief may start work in: the sidebar's and every Mono's, as
    /// `(path, name)`. `mono_projects` comes from the caller, which reads the Monos itself (this
    /// can run inside their update).
    pub(crate) fn chief_known_projects(&self, mono_projects: Vec<String>) -> Vec<(String, String)> {
        let mut paths: Vec<String> = Vec::new();
        for path in self.workspace.projects.iter().cloned().chain(mono_projects) {
            if !path.is_empty()
                && path != "~"
                && !paths.iter().any(|p| {
                    monocode_core::files::path_key(p) == monocode_core::files::path_key(&path)
                })
            {
                paths.push(path);
            }
        }
        paths
            .into_iter()
            .map(|path| {
                let name = self.appearance(&path).label(&path).to_string();
                (path, name)
            })
            .collect()
    }

    /// Starts a new session in `cwd` (named `title` when given) and sends `text` to it. Returns the session id and title.
    /// Local projects only: a cloud workspace is started from the app, never by the Chief.
    pub(crate) fn chief_start_session(
        &mut self,
        cwd: &str,
        title: Option<&str>,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<(i64, String), String> {
        if monocode_core::remote::parse_remote_path(cwd).is_some() {
            return Err("The Chief does not start sessions in a cloud project".into());
        }
        let (kind, model, access) = self.mobile_defaults(cwd, cx);
        let account = self.mobile_account(kind, cwd, &json!({}), cx)?;
        let effort = crate::conversation::preferred_effort(kind.id(), cx);
        let (summary, settings) = {
            let store = locked(&self.store)?;
            let mut summary = store.create(kind.id(), cwd)?;
            if let Some(title) = title {
                store.title(summary.id, title)?;
                summary.title = title.to_owned();
            }
            let mut settings = session_loading::new_session_settings(
                store
                    .desktop_preferences()
                    .and_then(|preferences| preferences.session_defaults(kind.id(), cwd)),
                model,
            );
            settings.access = access;
            settings.account = account;
            if let Some(effort) = effort {
                settings.effort = Some(effort);
            }
            store.save_agent_settings(summary.id, &settings)?;
            (summary, settings)
        };
        let (id, title) = (summary.id, summary.title.clone());
        self.add(summary, cx);
        if let Some(view) = self.sessions.get(&id) {
            #[cfg(test)]
            if HOLD_CHIEF_STARTS.with(|hold| hold.get()) {
                view.update(cx, |v, _| v.test_hold_launches());
            }
            view.update(cx, |v, cx| {
                v.install_transfer(settings, None, cx);
                v.mobile_submit(text, Vec::new(), cx);
            });
        }
        self.order.retain(|s| *s != id);
        self.order.insert(0, id);
        self.invalidate_chrome(cx);
        cx.notify();
        // Saving the workspace and publishing the catalog read the Monos, which are being
        // updated by the click that got us here: they run once that update is over.
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = this.update(cx, |shell, cx| {
                shell.persist(cx);
                shell.publish_mobile_catalog(cx);
            });
        });
        Ok((id, title))
    }
}
