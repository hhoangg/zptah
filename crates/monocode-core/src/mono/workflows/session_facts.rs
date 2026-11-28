//! What a finished session step shows about its checkout: the built-in `branch`, `commit`,
//! `changedFiles` and `diffStat` fields. Every call runs Git and blocks, so callers use a
//! thread of their own. A directory that is not a repository, or a base Git does not know, gives
//! empty values and never an error: a step must not fail because it could not look.
use crate::git::git;
use std::path::Path;

/// The most files `changedFiles` lists, so a run record stays small.
pub const CHANGED_FILES_MAX: usize = 500;

/// The git built-ins of a session step. Empty text and an empty list mean "not known".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFacts {
    pub worktree: String,
    pub branch: String,
    pub commit: String,
    pub changed_files: Vec<String>,
    pub diff_stat: String,
}

/// The commit `dir` is at, or `None` outside a repository or before the first commit.
pub fn head_commit(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "--verify", "-q", "HEAD"])
        .ok()
        .map(|out| out.trim().to_owned())
        .filter(|out| !out.is_empty())
}

/// The facts of the checkout at `dir`. `base` is the commit the step started from: the changes
/// are everything that differs from it, committed or not, plus the files Git does not track yet.
/// Without a base only the uncommitted changes count.
pub fn session_facts(dir: &Path, base: Option<&str>) -> SessionFacts {
    let mut facts = SessionFacts {
        worktree: dir.to_string_lossy().into_owned(),
        ..SessionFacts::default()
    };
    let Some(commit) = head_commit(dir) else {
        return facts;
    };
    facts.commit = commit;
    facts.branch = git(dir, &["branch", "--show-current"])
        .map(|out| out.trim().to_owned())
        .unwrap_or_default();
    let base = base.filter(|b| !b.is_empty()).unwrap_or("HEAD");
    let mut files: Vec<String> = git(dir, &["diff", "--name-only", "-z", base])
        .map(|out| split_nul(&out))
        .unwrap_or_default();
    let untracked = git(dir, &["ls-files", "--others", "--exclude-standard", "-z"])
        .map(|out| split_nul(&out))
        .unwrap_or_default();
    files.extend(untracked);
    files.sort();
    files.dedup();
    files.truncate(CHANGED_FILES_MAX);
    facts.changed_files = files;
    facts.diff_stat = git(dir, &["diff", "--stat", base])
        .map(|out| out.trim_end().to_owned())
        .unwrap_or_default();
    facts
}

fn split_nul(out: &str) -> Vec<String> {
    out.split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}
