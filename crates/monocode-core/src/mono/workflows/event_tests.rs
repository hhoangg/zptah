use super::event::*;
use crate::inbox::Item;

fn item(provider: &str, kind: &str) -> Item {
    Item {
        provider: provider.into(),
        kind: kind.into(),
        number: 7,
        repo: "acme/app".into(),
        project_path: "/work/app".into(),
        title: "Fix it".into(),
        url: "https://example.test/7".into(),
        ..Item::default()
    }
}

fn trigger(source: &str, event: &str) -> EventTrigger {
    EventTrigger {
        source: source.into(),
        event: event.into(),
        ..EventTrigger::default()
    }
}

#[test]
fn every_item_kind_maps_to_its_event() {
    let mut draft = item("github", "pr");
    draft.draft = true;
    let cases = [
        (draft, "draft_opened"),
        (item("github", "pr"), "pull_request_opened"),
        (item("github", "issue"), "issue_opened"),
        (item("gitlab", "pr"), "merge_request_opened"),
        (item("gitlab", "issue"), "issue_opened"),
        (item("linear", "issue"), "issue_created"),
        (item("jira", "issue"), "issue_created"),
        (item("clickup", "issue"), "issue_created"),
        (item("azuredevops", "pr"), "pull_request_appeared"),
        (item("azuredevops", "issue"), "work_item_appeared"),
    ];
    for (item, event) in cases {
        assert_eq!(event_of(&item), Some(event), "{}", item.provider);
        assert!(check_event(&item.provider, event).is_ok());
    }
    assert_eq!(event_of(&item("bitbucket", "pr")), None);
}

#[test]
fn keys_are_stable_and_distinct() {
    let a = item("github", "pr");
    assert_eq!(event_key(&a), event_key(&a.clone()));
    assert_eq!(event_key(&a), "github:pr:acme/app:7");
    let mut other_number = a.clone();
    other_number.number = 8;
    let mut other_repo = a.clone();
    other_repo.repo = "acme/other".into();
    let issue = item("github", "issue");
    let keys = [&a, &other_number, &other_repo, &issue].map(|i| event_key(i));
    for (i, x) in keys.iter().enumerate() {
        for y in &keys[i + 1..] {
            assert_ne!(x, y);
        }
    }
    let mut tracker = item("linear", "issue");
    tracker.id = "ABC 1".into();
    assert_eq!(event_key(&tracker), "linear:issue:abc_1");
    let mut clickup = item("clickup", "issue");
    clickup.id = "99".into();
    assert_eq!(event_key(&clickup), "clickup:task:99");
    // A draft turning ready keeps its key: the item is the same.
    let mut draft = a.clone();
    draft.draft = true;
    assert_eq!(event_key(&draft), event_key(&a));
}

#[test]
fn project_filter_applies_and_trackers_without_project_match_any() {
    let pr = item("github", "pr");
    let t = trigger("github", "pull_request_opened");
    assert!(matches(&t, &pr, "/work/app/"));
    assert!(!matches(&t, &pr, "/work/other"));
    assert!(!matches(&t, &pr, ""));
    let with_project = EventTrigger {
        project: Some("/work/app".into()),
        ..t.clone()
    };
    assert!(matches(&with_project, &pr, ""));
    assert!(!matches(
        &trigger("github", "issue_opened"),
        &pr,
        "/work/app"
    ));
    assert!(!matches(
        &trigger("gitlab", "pull_request_opened"),
        &pr,
        "/work/app"
    ));

    let mut linear = item("linear", "issue");
    linear.project_path.clear();
    let lt = trigger("linear", "issue_created");
    assert!(matches(&lt, &linear, "/anything"));
    assert!(matches(&lt, &linear, ""));
    linear.project_path = "/work/app".into();
    assert!(matches(&lt, &linear, "/work/app"));
    assert!(!matches(&lt, &linear, "/work/other"));
}

#[test]
fn repo_and_space_filters() {
    let pr = item("github", "pr");
    let mut t = trigger("github", "pull_request_opened");
    t.repos = vec!["ACME/app".into()];
    assert!(matches(&t, &pr, "/work/app"));
    t.repos = vec!["acme/other".into(), " ".into()];
    assert!(!matches(&t, &pr, "/work/app"));
    t.repos = vec!["acme/other".into(), "acme/app".into()];
    assert!(matches(&t, &pr, "/work/app"));

    let mut task = item("clickup", "issue");
    task.project_path.clear();
    task.team_id = "space-1".into();
    task.repo = "ignored".into();
    let mut ct = trigger("clickup", "issue_created");
    ct.repos = vec!["space-1".into()];
    assert!(matches(&ct, &task, ""));
    ct.repos = vec!["space-2".into()];
    assert!(!matches(&ct, &task, ""));
}

#[test]
fn unknown_pairs_are_rejected() {
    for (source, event) in [
        ("github", "pull_request_merged"),
        ("github", "review_requested"),
        ("github", "issue_assigned"),
        ("github", "check_failed"),
        ("gitlab", "draft_opened"),
        ("linear", "issue_opened"),
        ("bitbucket", "issue_created"),
        ("", ""),
    ] {
        assert!(check_event(source, event).is_err(), "{source}/{event}");
    }
    assert_eq!(EVENTS.len(), 10);
    assert_eq!(events_for("azuredevops").len(), 2);
    assert!(events_for("time").is_empty());
}

#[test]
fn trigger_item_describes_the_item() {
    let value = trigger_item(&item("gitlab", "pr"));
    assert_eq!(value["source"], "gitlab");
    assert_eq!(value["kind"], "merge request");
    assert_eq!(value["ref"], "#7");
    assert_eq!(value["title"], "Fix it");
    assert_eq!(value["url"], "https://example.test/7");
    assert_eq!(value["repo"], "acme/app");
    let mut tracker = item("linear", "issue");
    tracker.identifier = "ENG-12".into();
    tracker.title.clear();
    let value = trigger_item(&tracker);
    assert_eq!(value["kind"], "issue");
    assert_eq!(value["ref"], "ENG-12");
    assert_eq!(value["title"], "ENG-12");
}

#[test]
fn trigger_serializes_camel_case() {
    let t: EventTrigger =
        serde_json::from_str(r#"{"source":"github","event":"issue_opened","repos":["a/b"]}"#)
            .unwrap();
    assert_eq!(t.project, None);
    assert_eq!(t.repos, vec!["a/b"]);
}
