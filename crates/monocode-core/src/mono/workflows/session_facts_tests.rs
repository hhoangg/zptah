use super::session_facts::{head_commit, session_facts, CHANGED_FILES_MAX};
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "user.name=Fixture",
        ])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-m", "Initial"]);
    dir
}

#[test]
fn a_directory_that_is_not_a_repository_has_only_its_path() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(head_commit(dir.path()), None);
    let facts = session_facts(dir.path(), Some("abc"));
    assert_eq!(facts.worktree, dir.path().to_string_lossy());
    assert!(facts.commit.is_empty() && facts.branch.is_empty());
    assert!(facts.changed_files.is_empty() && facts.diff_stat.is_empty());
}

#[test]
fn a_clean_checkout_at_its_base_has_no_changes() {
    let dir = repo();
    let base = head_commit(dir.path()).unwrap();
    let facts = session_facts(dir.path(), Some(&base));
    assert_eq!(facts.commit, base);
    assert_eq!(facts.branch, "main");
    assert!(facts.changed_files.is_empty());
    assert!(facts.diff_stat.is_empty());
}

#[test]
fn changes_since_the_base_count_committed_uncommitted_and_new_files() {
    let dir = repo();
    let base = head_commit(dir.path()).unwrap();
    // Committed after the base.
    std::fs::write(dir.path().join("b b.txt"), "two\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "Add b"]);
    // Edited and not committed, and a file Git does not track yet.
    std::fs::write(dir.path().join("a.txt"), "one\nmore\n").unwrap();
    std::fs::write(dir.path().join("new.txt"), "new\n").unwrap();
    let facts = session_facts(dir.path(), Some(&base));
    assert_eq!(facts.changed_files, ["a.txt", "b b.txt", "new.txt"]);
    assert!(facts.diff_stat.contains("a.txt") && facts.diff_stat.contains("b b.txt"));
    assert_ne!(facts.commit, base);
    // Without a base only what is not committed counts.
    let open = session_facts(dir.path(), None);
    assert_eq!(open.changed_files, ["a.txt", "new.txt"]);
}

#[test]
fn a_base_git_does_not_know_gives_no_changes_instead_of_an_error() {
    let dir = repo();
    let facts = session_facts(dir.path(), Some("0123456789abcdef0123456789abcdef01234567"));
    assert!(!facts.commit.is_empty());
    assert!(facts.diff_stat.is_empty());
    assert!(facts.changed_files.is_empty());
}

#[test]
fn the_file_list_is_capped() {
    let dir = repo();
    for i in 0..(CHANGED_FILES_MAX + 20) {
        std::fs::write(dir.path().join(format!("f{i:04}.txt")), "x").unwrap();
    }
    let facts = session_facts(dir.path(), None);
    assert_eq!(facts.changed_files.len(), CHANGED_FILES_MAX);
}
