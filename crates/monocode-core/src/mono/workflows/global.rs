//! The global workflow store: `<app data>/workflows/workflows.json` and
//! `<app data>/workflows/runs/<workflowId>.json`. It has the format, the hash-checked atomic
//! writes and the conflict semantics of the per-Mono files (`MonoFiles`), so `store.rs` and
//! `cli.rs` work over it unchanged. The old `monos/<id>/workflows.json` and `workflow-runs/`
//! files are never read, written or removed here.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::mono::files::{self, AgentFile, CONFLICT, WORKFLOWS_FILE, WORKFLOW_RUNS_DIR};
use crate::store::Store;

use super::store::MonoFiles;

/// The folder under the app data dir that holds the global workflows.
pub const WORKFLOWS_DIR: &str = "workflows";
/// The folder inside it that holds one runs file per workflow.
pub const RUNS_DIR: &str = "runs";
/// The cap on `workflows.json` (decision D13: raised from 2 MiB for 100 workflows), above the
/// 8 MiB budget `store.rs` keeps its own writes under.
const WORKFLOWS_FILE_MAX: usize = 9 * 1024 * 1024;
/// A workflow's runs file holds up to 20 runs with definition snapshots and node outputs.
const RUNS_FILE_MAX: usize = 4 * 1024 * 1024;

/// Serializes hash-checked writes, so only one caller can save over a version of a file.
static LOCK: Mutex<()> = Mutex::new(());

/// The global workflows folder next to `monos/`, under the app data dir.
pub fn workflows_root(store: &Store) -> PathBuf {
    store
        .data_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("monocode-test-{}", crate::mono::new_id()))
        })
        .join(WORKFLOWS_DIR)
}

/// The global workflow files under one root folder.
#[derive(Clone, Debug)]
pub struct WorkflowsDir {
    root: PathBuf,
}

impl WorkflowsDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn for_store(store: &Store) -> Self {
        Self::new(workflows_root(store))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Maps the file names the store uses to files under the root. Nothing nested and nothing
    /// hidden, so a path from the CLI can never leave the folder.
    fn resolve(&self, relative: &str) -> Result<PathBuf, String> {
        if relative == WORKFLOWS_FILE {
            return Ok(self.root.join(WORKFLOWS_FILE));
        }
        let id = relative
            .strip_prefix(WORKFLOW_RUNS_DIR)
            .and_then(|rest| rest.strip_prefix('/'))
            .and_then(|rest| rest.strip_suffix(".json"))
            .filter(|id| files::is_workflow_id(id))
            .ok_or_else(|| format!("Not a workflow file: {relative}"))?;
        Ok(self.root.join(RUNS_DIR).join(format!("{id}.json")))
    }
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

impl MonoFiles for WorkflowsDir {
    fn read(&self, relative: &str) -> Result<AgentFile, String> {
        let _guard = LOCK.lock().map_err(|e| e.to_string())?;
        let text = read_optional(&self.resolve(relative)?)?;
        Ok(AgentFile {
            hash: files::content_hash(text.as_deref().unwrap_or("")),
            text,
        })
    }

    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String> {
        let max = if relative == WORKFLOWS_FILE {
            WORKFLOWS_FILE_MAX
        } else {
            RUNS_FILE_MAX
        };
        if text.len() > max {
            return Err(format!("{relative} is too large"));
        }
        let _guard = LOCK.lock().map_err(|e| e.to_string())?;
        let path = self.resolve(relative)?;
        if let Some(expected) = expected {
            let current = read_optional(&path)?.unwrap_or_default();
            if files::content_hash(&current) != expected {
                return Err(CONFLICT.into());
            }
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        files::write_atomic(&path, text)?;
        Ok(files::content_hash(text))
    }
}
