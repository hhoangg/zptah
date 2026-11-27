//! The global workflow store: where its files go, the hash-checked writes over them, the limits
//! of the global store (100 workflows, 20 runs each, a file budget for 100), and that the old
//! per-Mono files are never read, written or removed.
use super::board_edit::{save_into, SaveRefusal};
use super::engine::start_run;
use super::fixtures::workflow_from;
use super::global::{workflows_root, WorkflowsDir, RUNS_DIR, WORKFLOWS_DIR};
use super::mention::workflow_slugs;
use super::model::*;
use super::store::*;
use crate::mono::files::{self, CONFLICT};
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

const ID: &str = "wf-abcd1234";

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("wf-global-{}", crate::mono::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn workflow_with(id: &str, extra: serde_json::Value) -> Workflow {
    let mut v = json!({
        "id": id, "name": "Daily", "inputs": {}, "inputValues": {},
        "nodes": [{"id": "start", "type": "manual"}],
        "edges": [],
        "enabled": true, "createdAt": 1
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    workflow_from(&v.to_string())
}

fn workflow() -> Workflow {
    workflow_with(ID, json!({}))
}

fn run(id: &str, at: u64) -> WorkflowRun {
    let mut run = start_run(
        &workflow(),
        RunCaller::new(CallerKind::Manual, "start"),
        at,
        id,
    );
    run.status = RunStatus::Done;
    run.nodes.clear();
    run
}

/// An id that is a valid workflow id: `wf-` and eight base36 characters.
fn wf_id(n: usize) -> String {
    format!("wf-{n:08}")
}

fn text(dir: &impl MonoFiles, path: &str) -> String {
    dir.read(path).unwrap().text.unwrap_or_default()
}

/// Every file under `dir` with its bytes, sorted by path.
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push((
                    path.to_string_lossy().into_owned(),
                    std::fs::read(&path).unwrap(),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

#[test]
fn the_store_writes_workflows_json_and_one_runs_file_per_workflow_under_its_root() {
    let root = temp_root();
    let dir = WorkflowsDir::new(root.join(WORKFLOWS_DIR));
    update_workflows(&dir, |_| Ok((vec![workflow()], ()))).unwrap();
    update_runs(&dir, ID, |_| Ok((vec![run("r1", 5)], ()))).unwrap();
    let base = root.join(WORKFLOWS_DIR);
    assert!(base.join("workflows.json").is_file());
    assert!(base.join(RUNS_DIR).join(format!("{ID}.json")).is_file());
    assert_eq!(load_workflows(&dir).unwrap().workflows, vec![workflow()]);
    assert_eq!(load_runs(&dir, ID).unwrap().len(), 1);
    // Nothing else is written, and nothing under a `monos` folder.
    assert!(!root.join("monos").exists());
    assert_eq!(tree(&base).len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_root_sits_next_to_monos_under_the_app_data_dir() {
    let base = temp_root();
    let store = crate::store::Store::open(&base.join("native.sqlite")).unwrap();
    let root = workflows_root(&store);
    assert_eq!(root, base.join(WORKFLOWS_DIR));
    assert_eq!(root.parent(), crate::mono::root(&store).parent());
    assert_ne!(root, crate::mono::root(&store));
    assert_eq!(WorkflowsDir::for_store(&store).root(), root);
    drop(store);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_file_name_outside_the_two_known_shapes_is_refused() {
    let root = temp_root();
    let dir = WorkflowsDir::new(&root);
    for bad in [
        "SOUL.md",
        "habits.json",
        "../workflows.json",
        "workflow-runs/../x.json",
        "workflow-runs/not-an-id.json",
        "workflow-runs/wf-abcd1234",
        "runs/wf-abcd1234.json",
    ] {
        assert!(dir.read(bad).is_err(), "{bad}");
        assert!(dir.save(bad, "[]", None).is_err(), "{bad}");
    }
    assert!(tree(&root).is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_save_over_a_changed_file_conflicts_and_a_missing_file_has_the_hash_of_nothing() {
    let root = temp_root();
    let dir = WorkflowsDir::new(&root);
    let missing = dir.read("workflows.json").unwrap();
    assert_eq!(missing.text, None);
    assert_eq!(missing.hash, files::content_hash(""));
    let first = dir
        .save("workflows.json", "[]\n", Some(&missing.hash))
        .unwrap();
    assert_eq!(first, files::content_hash("[]\n"));
    // The old hash no longer matches.
    assert_eq!(
        dir.save("workflows.json", "[ ]", Some(&missing.hash)),
        Err(CONFLICT.to_string())
    );
    assert_eq!(text(&dir, "workflows.json"), "[]\n");
    // No hash means overwrite.
    dir.save("workflows.json", "[ ]", None).unwrap();
    assert_eq!(text(&dir, "workflows.json"), "[ ]");
    std::fs::remove_dir_all(root).unwrap();
}

/// A store that lets a rival writer in before the first saves, to make the hash check fail.
struct Racy {
    disk: WorkflowsDir,
    rivals: RefCell<Vec<String>>,
    saves: Cell<usize>,
}

impl MonoFiles for Racy {
    fn read(&self, relative: &str) -> Result<files::AgentFile, String> {
        self.disk.read(relative)
    }
    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String> {
        self.saves.set(self.saves.get() + 1);
        let rival = {
            let mut rivals = self.rivals.borrow_mut();
            (!rivals.is_empty()).then(|| rivals.remove(0))
        };
        if let Some(rival) = rival {
            self.disk.save(relative, &rival, None)?;
        }
        self.disk.save(relative, text, expected)
    }
}

fn list_text(workflows: &[Workflow]) -> String {
    let entries: Vec<_> = workflows
        .iter()
        .map(|w| super::ordered::Json::parse(&serde_json::to_string(w).unwrap()).unwrap())
        .collect();
    format!("{}\n", super::ordered::Json::Array(entries).stringify(true))
}

#[test]
fn a_save_that_loses_a_race_reads_again_and_applies_its_edit_over_the_rival() {
    let root = temp_root();
    let disk = WorkflowsDir::new(&root);
    disk.save("workflows.json", &list_text(&[workflow()]), None)
        .unwrap();
    let rival = workflow_with("wf-rival000", json!({}));
    let racy = Racy {
        disk: disk.clone(),
        rivals: RefCell::new(vec![list_text(&[workflow(), rival])]),
        saves: Cell::new(0),
    };
    update_workflows(&racy, |mut list| {
        list[0].name = "Renamed".into();
        Ok((list, ()))
    })
    .unwrap();
    // One save lost, one went through, and both edits are in the file.
    assert_eq!(racy.saves.get(), 2);
    let stored = load_workflows(&disk).unwrap().workflows;
    assert_eq!(
        stored
            .iter()
            .map(|w| (w.id.as_str(), w.name.as_str()))
            .collect::<Vec<_>>(),
        [(ID, "Renamed"), ("wf-rival000", "Daily")]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_save_that_loses_every_try_says_the_file_changed() {
    let root = temp_root();
    let disk = WorkflowsDir::new(&root);
    disk.save("workflows.json", &list_text(&[workflow()]), None)
        .unwrap();
    let racy = Racy {
        disk: disk.clone(),
        // Each rival leaves an enabled workflow behind, so every retry still has an edit to make.
        rivals: RefCell::new(
            ["One", "Two", "Three"]
                .map(|name| list_text(&[workflow_with(ID, json!({"name": name}))]))
                .to_vec(),
        ),
        saves: Cell::new(0),
    };
    let error = update_workflows(&racy, |mut list| {
        for w in &mut list {
            w.enabled = false;
        }
        Ok((list, ()))
    })
    .unwrap_err();
    assert_eq!(error, "The file changed while you were editing");
    assert_eq!(racy.saves.get(), 3);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_workflows_file_that_is_not_a_json_list_is_never_overwritten() {
    let root = temp_root();
    let dir = WorkflowsDir::new(&root);
    dir.save("workflows.json", "this is not json", None)
        .unwrap();
    let error = update_workflows(&dir, |list| Ok((list, ()))).unwrap_err();
    assert!(error.contains("could not be read"), "{error}");
    let error = update_workflows(&dir, |_| Ok((vec![workflow()], ()))).unwrap_err();
    assert!(error.contains("could not be read"), "{error}");
    assert_eq!(text(&dir, "workflows.json"), "this is not json");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_global_store_holds_one_hundred_workflows() {
    assert_eq!(WORKFLOWS_MAX, 100);
    let root = temp_root();
    let dir = WorkflowsDir::new(&root);
    let many: Vec<Workflow> = (0..WORKFLOWS_MAX)
        .map(|n| workflow_with(&wf_id(n), json!({"name": format!("Workflow {n}")})))
        .collect();
    update_workflows(&dir, |_| Ok((many.clone(), ()))).unwrap();
    assert_eq!(load_workflows(&dir).unwrap().workflows.len(), WORKFLOWS_MAX);
    // The board refuses the 101st, and says so, but still saves a change to an existing one.
    let extra = workflow_with(&wf_id(WORKFLOWS_MAX), json!({}));
    match save_into(many.clone(), &extra, false, "", 1) {
        Err(SaveRefusal::Message(message)) => assert_eq!(message, "At most 100 workflows."),
        other => panic!("expected a refusal, got {other:?}"),
    }
    let seen = super::board_model::definition_key(&many[3]);
    let mut renamed = many[3].clone();
    renamed.name = "Renamed".into();
    assert!(save_into(many, &renamed, false, &seen, 1).is_ok());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_file_budget_fits_one_hundred_large_workflows_and_still_refuses_more_than_it() {
    let root = temp_root();
    let dir = WorkflowsDir::new(&root);
    // 100 workflows of 60 KiB: over the old 2 MiB budget, well inside the new one.
    let many: Vec<Workflow> = (0..WORKFLOWS_MAX)
        .map(|n| workflow_with(&wf_id(n), json!({"description": "d".repeat(60 * 1024)})))
        .collect();
    assert!(many.len() * 60 * 1024 > 2 * 1024 * 1024);
    update_workflows(&dir, |_| Ok((many.clone(), ()))).unwrap();
    assert_eq!(load_workflows(&dir).unwrap().workflows.len(), WORKFLOWS_MAX);
    let before = text(&dir, "workflows.json");
    let huge = workflow_with(
        ID,
        json!({"description": "x".repeat(WORKFLOWS_FILE_BUDGET)}),
    );
    let error = update_workflows(&dir, |_| Ok((vec![huge.clone()], ()))).unwrap_err();
    assert_eq!(
        error,
        "The workflows are too large to save (over 8 MiB in total). Remove a workflow or shorten its steps."
    );
    assert_eq!(text(&dir, "workflows.json"), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_twenty_newest_runs_of_a_workflow_are_kept() {
    let root = temp_root();
    let dir = WorkflowsDir::new(&root);
    for n in 0..25u64 {
        update_runs(&dir, ID, |mut runs| {
            runs.insert(0, run(&format!("run-{n}"), n));
            Ok((runs, ()))
        })
        .unwrap();
    }
    let runs = load_runs(&dir, ID).unwrap();
    assert_eq!(runs.len(), WORKFLOW_RUNS_KEPT);
    assert_eq!(runs[0].id, "run-24");
    assert_eq!(runs[WORKFLOW_RUNS_KEPT - 1].id, "run-5");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_runs_of_two_workflows_do_not_mix() {
    let root = temp_root();
    let dir = WorkflowsDir::new(&root);
    let other = wf_id(7);
    update_runs(&dir, ID, |_| Ok((vec![run("mine", 1)], ()))).unwrap();
    assert!(load_runs(&dir, &other).unwrap().is_empty());
    clear_runs(&dir, ID);
    assert!(load_runs(&dir, ID).unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn old_per_mono_files_are_ignored_and_left_byte_identical() {
    let root = temp_root();
    // What an older version left: a Mono folder with its own workflows and runs.
    let mono = root.join("monos").join("mono-1");
    std::fs::create_dir_all(mono.join("workflow-runs")).unwrap();
    std::fs::create_dir_all(mono.join("memory")).unwrap();
    let old_list = list_text(&[workflow_with("wf-old00001", json!({"name": "Per Mono"}))]);
    // A workflow with the node types the new version dropped is kept as written too.
    let odd =
        r#"[{"id":"wf-old00002","name":"Old","nodes":[{"id":"r","type":"report"}],"edges":[]}]"#;
    std::fs::write(mono.join("workflows.json"), &old_list).unwrap();
    std::fs::write(mono.join("workflow-runs").join("wf-old00001.json"), odd).unwrap();
    std::fs::write(mono.join("memory").join("MEMORY.md"), "notes").unwrap();
    let before = tree(&root.join("monos"));
    assert_eq!(before.len(), 3);

    let dir = WorkflowsDir::new(root.join(WORKFLOWS_DIR));
    // The global store starts empty: the old workflows and runs are not read.
    assert!(load_workflows(&dir).unwrap().workflows.is_empty());
    assert!(load_workflows(&dir).unwrap().unreadable.is_empty());
    assert!(load_runs(&dir, "wf-old00001").unwrap().is_empty());
    assert!(last_run_of(&dir, "wf-old00001").unwrap().is_none());
    // Writing, updating and clearing never touch them either.
    update_workflows(&dir, |_| Ok((vec![workflow()], ()))).unwrap();
    update_runs(&dir, "wf-old00001", |_| Ok((vec![run("r", 1)], ()))).unwrap();
    clear_runs(&dir, "wf-old00001");
    update_workflows(&dir, |mut list| {
        list.clear();
        Ok((list, ()))
    })
    .unwrap();
    assert_eq!(tree(&root.join("monos")), before);
    // And the old workflow ids are unknown to the new store.
    assert!(load_workflows(&dir)
        .unwrap()
        .workflows
        .iter()
        .all(|w| w.id != "wf-old00001"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn slugs_are_unique_over_the_whole_global_list_whatever_the_names() {
    // Workflows that used to live in different Monos can share a name once they are together.
    let same: Vec<Workflow> = (0..30)
        .map(|n| workflow_with(&wf_id(n), json!({"name": "Nightly digest"})))
        .collect();
    let slugs = workflow_slugs(&same);
    let mut seen = std::collections::HashSet::new();
    for slug in &slugs {
        assert!(seen.insert(slug.slug.clone()), "{} repeats", slug.slug);
        assert_ne!(slug.slug, "nightly-digest");
    }
    // Reversing the list gives every workflow the same slug.
    let mut reversed = same.clone();
    reversed.reverse();
    for slug in workflow_slugs(&reversed) {
        let first = slugs.iter().find(|s| s.id == slug.id).unwrap();
        assert_eq!(first.slug, slug.slug);
    }
}
