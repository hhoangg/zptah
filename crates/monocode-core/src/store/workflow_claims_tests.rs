use super::*;

fn open(dir: &tempfile::TempDir) -> Store {
    Store::open_monocode(&dir.path().join("monocode.db")).unwrap()
}

#[test]
fn claim_dedupes_within_one_open_and_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir);
    assert!(store.claim_workflow_event("w1", "t1", "k").unwrap());
    assert!(!store.claim_workflow_event("w1", "t1", "k").unwrap());
    drop(store);
    let store = open(&dir);
    assert!(!store.claim_workflow_event("w1", "t1", "k").unwrap());
    assert!(store.claim_workflow_event("w1", "t1", "other").unwrap());
}

#[test]
fn legacy_open_creates_the_table_too() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("legacy.db")).unwrap();
    assert!(store.claim_workflow_event("w1", "t1", "k").unwrap());
    assert!(!store.claim_workflow_event("w1", "t1", "k").unwrap());
}

#[test]
fn one_run_per_workflow_per_item_across_trigger_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir);
    assert!(store.claim_workflow_item("w1", "t1", "k").unwrap());
    assert!(!store.claim_workflow_item("w1", "t2", "k").unwrap());
    assert!(!store.claim_workflow_item("w1", "t1", "k").unwrap());
    assert!(store.claim_workflow_item("w2", "t1", "k").unwrap());
    assert!(store.claim_workflow_item("w1", "t2", "k2").unwrap());
    // The per-node claim stays independent of the other nodes.
    assert!(store.claim_workflow_event("w3", "t1", "k").unwrap());
    assert!(store.claim_workflow_event("w3", "t2", "k").unwrap());
}

#[test]
fn release_allows_reclaim_but_keeps_claims_with_a_run() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir);
    assert!(store.claim_workflow_item("w1", "t1", "k").unwrap());
    store.release_workflow_event("w1", "t1", "k").unwrap();
    assert!(store.claim_workflow_item("w1", "t2", "k").unwrap());
    store
        .set_workflow_claim_run("w1", "t2", "k", "run-1")
        .unwrap();
    store.release_workflow_event("w1", "t2", "k").unwrap();
    assert!(!store.claim_workflow_item("w1", "t1", "k").unwrap());
}

#[test]
fn run_id_is_recorded_and_unclaimed_lists_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir);
    store.claim_workflow_event("w1", "t1", "a").unwrap();
    store.claim_workflow_event("w1", "t1", "b").unwrap();
    store
        .set_workflow_claim_run("w1", "t1", "a", "run-1")
        .unwrap();
    assert_eq!(
        store.workflow_claim_run("w1", "a").as_deref(),
        Some("run-1")
    );
    let open_claims = store.unclaimed_workflow_claims().unwrap();
    assert_eq!(open_claims.len(), 1);
    assert_eq!(open_claims[0].event_key, "b");
    assert_eq!(open_claims[0].workflow_id, "w1");
}

#[test]
fn forget_removes_only_that_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir);
    store.claim_workflow_event("w1", "t1", "k").unwrap();
    store.claim_workflow_event("w2", "t1", "k").unwrap();
    store.forget_workflow_claims("w1").unwrap();
    assert!(store.claim_workflow_event("w1", "t1", "k").unwrap());
    assert!(!store.claim_workflow_event("w2", "t1", "k").unwrap());
}

#[test]
fn read_only_open_does_not_create_the_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monocode.db");
    // An existing database from before this table.
    drop(Store::open_monocode(&path).unwrap());
    Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE workflow_event_claims;")
        .unwrap();
    let reader = Store::read_monocode(&path).unwrap();
    assert!(reader.claim_workflow_event("w1", "t1", "k").is_err());
    drop(reader);
    let exists: i64 = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'workflow_event_claims'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(exists, 0);
    // The next writable open brings it back.
    assert!(open(&dir).claim_workflow_event("w1", "t1", "k").unwrap());
}

#[test]
fn concurrent_claims_have_exactly_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    drop(open(&dir));
    let path = dir.path().join("monocode.db");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|n| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let store = Store::open_monocode(&path).unwrap();
                barrier.wait();
                let mut wins = 0;
                for key in 0..50 {
                    if store
                        .claim_workflow_item("w1", &format!("t{n}"), &format!("k{key}"))
                        .unwrap()
                    {
                        wins += 1;
                    }
                }
                wins
            })
        })
        .collect();
    let total: i32 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(total, 50);
}
