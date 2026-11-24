//! Settings > Import from Monoptah: one button that prepares a copy of the user's Monoptah data
//! for zptah. The copy itself lives in `monocode_core::store::monoptah_import`. It cannot be
//! moved in while zptah runs, so the page builds and verifies it on a background thread and then
//! asks for a restart; the move happens at the next start (`main.rs`).
use crate::settings::{
    SettingsEvent, SettingsView,
    widgets::{RED, note, primary_button},
};
use gpui::{AnyElement, Context, SharedString, Task, div, prelude::*, px};
use monocode_core::store::monoptah_import::{
    self as import, Locations, Offer, Outcome, ProcessProbe, Summary, SystemProcesses,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// What the page remembers between frames.
pub(in crate::settings) struct ImportState {
    /// Monoptah's data folder. `None` turns the page off (tests never look at real data).
    source: Option<PathBuf>,
    /// The zptah database the import fills.
    database: Option<PathBuf>,
    locations: Locations,
    offer: Offer,
    running: bool,
    /// How the last import ended: prepared (waiting for the restart) or failed.
    result: Option<Result<Summary, String>>,
    /// Why the last attempt to move an import in at start-up failed.
    startup_error: Option<String>,
    probe: Arc<dyn ProcessProbe>,
    task: Option<Task<()>>,
}

impl Default for ImportState {
    fn default() -> Self {
        Self {
            source: (!cfg!(test)).then(import::monoptah_data_dir),
            database: None,
            locations: if cfg!(test) {
                Locations::none()
            } else {
                Locations::system()
            },
            offer: Offer::Hidden,
            running: false,
            result: None,
            startup_error: None,
            probe: Arc::new(SystemProcesses),
            task: None,
        }
    }
}

impl ImportState {
    /// The page and its rail item are shown while there is something to import or an import
    /// waits for the restart.
    pub(in crate::settings) fn visible(&self) -> bool {
        self.offer != Offer::Hidden
    }

    /// An import is prepared and only the restart is left.
    fn waiting(&self) -> bool {
        self.offer == Offer::Pending
    }

    pub(in crate::settings) fn detect(&mut self, data_dir: Option<&Path>) {
        self.database = data_dir.map(|dir| dir.join(monocode_core::brand::DATABASE_FILE));
        self.offer = match (&self.source, &self.database) {
            (Some(source), Some(database)) => import::offer(source, database),
            _ => Offer::Hidden,
        };
        self.startup_error = self.database.as_deref().and_then(import::last_error);
    }

    #[cfg(test)]
    pub(in crate::settings) fn for_test(
        source: PathBuf,
        data_dir: &Path,
        probe: Arc<dyn ProcessProbe>,
    ) -> Self {
        let mut state = Self {
            source: Some(source),
            probe,
            ..Self::default()
        };
        state.detect(Some(data_dir));
        state
    }
}

impl SettingsView {
    pub(in crate::settings) fn refresh_import(&mut self) {
        if !self.import.running {
            let dir = self.ctx.data_dir.clone();
            self.import.detect(dir.as_deref());
        }
    }

    fn start_import(&mut self, cx: &mut Context<Self>) {
        let (Some(source), Some(database)) =
            (self.import.source.clone(), self.import.database.clone())
        else {
            return;
        };
        if self.import.running || self.import.offer != Offer::Ready {
            return;
        }
        self.import.running = true;
        self.import.result = None;
        let probe = self.import.probe.clone();
        let locations = self.import.locations.clone();
        let work = cx.background_executor().spawn(async move {
            import::import_from_monoptah(
                &source,
                &database,
                &locations,
                probe.as_ref(),
                env!("CARGO_PKG_VERSION"),
            )
        });
        self.import.task = Some(cx.spawn(async move |this, cx| {
            let outcome = work.await;
            let _ = this.update(cx, |this, cx| {
                this.import.running = false;
                this.import.result = match outcome {
                    Ok(Outcome::Staged(summary)) => Some(Ok(summary)),
                    // The marker was there already: the page just goes away.
                    Ok(Outcome::AlreadyImported) => None,
                    Err(error) => Some(Err(error)),
                };
                this.refresh_import();
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn import_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut children = Vec::new();
        let waiting = self.import.waiting();
        if waiting {
            if let Some(Ok(summary)) = &self.import.result {
                children.push(note(format!(
                    "Prepared {} databases and {} files from Monoptah.",
                    summary.databases, summary.files
                )));
                if summary.unreadable_secrets > 0 {
                    children.push(note(format!(
                        "{} saved credentials could not be opened and will be copied as they are.",
                        summary.unreadable_secrets
                    )));
                }
            }
            children.push(note("Restart zptah to finish the import."));
            let backup = self
                .import
                .database
                .as_ref()
                .and_then(|db| db.parent())
                .map(|dir| dir.join("pre-import-backup"));
            if let Some(backup) = backup {
                children.push(note(format!(
                    "Anything zptah already saved here is kept in {}.",
                    backup.display()
                )));
            }
        } else {
            match &self.import.offer {
                Offer::Blocked(reason) => children.push(note(reason.clone())),
                _ => children.push(note(
                    "Quit Monoptah first. Your conversations, settings, saved sign-ins, Monos and files are copied. Monoptah keeps its own copy and is not changed.",
                )),
            }
            children.push(note(
                "Phone and web pairing are not copied: pair your devices again with zptah.",
            ));
        }
        let failure = match &self.import.result {
            Some(Err(error)) => Some(error.clone()),
            _ => self.import.startup_error.clone(),
        };
        if let Some(error) = failure {
            children.push(
                div()
                    .px_4()
                    .py(px(14.))
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(gpui::rgb(RED))
                    .debug_selector(|| "import-error".into())
                    .child(error)
                    .into_any_element(),
            );
        }
        let running = self.import.running;
        let ready = self.import.offer == Offer::Ready;
        let button: AnyElement = if waiting {
            primary_button("import-restart", "Restart zptah", false)
                .debug_selector(|| "import-restart".into())
                .child("Restart zptah")
                .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::RestartApp)))
                .into_any_element()
        } else {
            let label = if running {
                "Importing..."
            } else {
                "Import from Monoptah"
            };
            let disabled = running || !ready;
            primary_button("import-start", label, disabled)
                .debug_selector(|| "import-start".into())
                .child(label)
                .when(!disabled, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.start_import(cx)))
                })
                .into_any_element()
        };
        children.push(
            div()
                .flex()
                .px_4()
                .py(px(12.))
                .child(button)
                .into_any_element(),
        );
        vec![self.group(
            None,
            true,
            "Monoptah data",
            Some(SharedString::from(
                "Copy what Monoptah saved on this computer into zptah. This works once, into an empty zptah.",
            )),
            None,
            children,
        )]
    }
}
