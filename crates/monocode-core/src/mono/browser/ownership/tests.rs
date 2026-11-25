use super::*;
use serde_json::json;

const A: &str = "0123456789abcdef0123456789abcdef";
const B: &str = "fedcba9876543210fedcba9876543210";

fn tab(id: u64, url: &str, title: &str, owner: Option<&str>) -> TabInfo {
    TabInfo {
        tab: id,
        window: 1,
        url: url.into(),
        title: title.into(),
        owner: owner.map(str::to_owned),
        group_title: None,
        active_in_window: false,
        audible: false,
        loading: false,
    }
}

#[test]
fn a_tab_is_claimed_by_one_mono_and_a_second_claim_conflicts() {
    let mut t = Table::default();
    assert_eq!(t.claim(1, A, How::Created, 10), Ok(()));
    assert_eq!(t.claim(1, A, How::Adopted, 20), Ok(()));
    assert_eq!(
        t.get(1).unwrap().how,
        How::Created,
        "a repeat claim changes nothing"
    );
    assert_eq!(
        t.claim(1, B, How::Adopted, 30),
        Err(Conflict::OwnedBy(A.into()))
    );
    assert_eq!(t.owner_of(1), Some(A));
    assert_eq!(t.owner_of(2), None);
}

#[test]
fn the_tab_limit_counts_only_the_monos_owned_tabs() {
    let max = super::super::gate::tabs::MAX_TABS;
    let mut t = Table::default();
    for id in 0..max as u64 {
        assert!(t.may_open(A), "{id}");
        t.claim(id, A, How::Created, 1).unwrap();
    }
    assert!(!t.may_open(A));
    assert!(t.may_open(B), "another Mono has its own limit");
    t.release(0);
    assert!(t.may_open(A));
    assert_eq!(t.count_of(A), max - 1);
}

#[test]
fn transfer_moves_a_tab_between_monos_and_release_frees_it() {
    let mut t = Table::default();
    t.claim(5, A, How::Created, 1).unwrap();
    assert_eq!(t.transfer(5, B, 2).as_deref(), Some(A));
    assert_eq!(t.owner_of(5), Some(B));
    assert_eq!(t.get(5).unwrap().how, How::Adopted);
    // Transferring to the owner, or a tab nobody had, reports no previous owner.
    assert_eq!(t.transfer(5, B, 3), None);
    assert_eq!(t.transfer(6, A, 3), None);
    assert_eq!(t.release(5).map(|o| o.mono), Some(B.to_string()));
    assert_eq!(t.owner_of(5), None);
    assert_eq!(t.release_mono(A), vec![6]);
    assert!(t.tabs_of(A).is_empty());
}

#[test]
fn taking_a_tab_back_remembers_when_and_a_new_owner_forgets_it() {
    let mut t = Table::default();
    t.claim(7, A, How::Created, 1).unwrap();
    let owned = t.take_back(7, 500).expect("it was owned");
    assert_eq!(owned.mono, A);
    assert_eq!(t.owner_of(7), None);
    assert_eq!(t.taken_back_at(7), Some(500));
    assert!(t.take_back(7, 600).is_none(), "nothing to take back twice");
    t.claim(7, B, How::Adopted, 700).unwrap();
    assert_eq!(t.taken_back_at(7), None);
}

#[test]
fn a_list_replaces_what_the_mirror_knows() {
    let mut t = Table::default();
    t.claim(1, A, How::Created, 1).unwrap();
    t.claim(2, A, How::Created, 1).unwrap();
    t.claim(9, B, How::Created, 1).unwrap();
    // The full list: tab 1 is the user's now, tab 2 is B's, tab 9 is gone, tab 3 is A's.
    let all = [
        tab(1, "https://a.example/", "A", None),
        tab(2, "https://b.example/", "B", Some(B)),
        tab(3, "https://c.example/", "C", Some(A)),
    ];
    t.sync(&all, true, 100);
    assert_eq!(t.owner_of(1), None);
    assert_eq!(t.owner_of(2), Some(B));
    assert_eq!(t.owner_of(3), Some(A));
    assert_eq!(t.owner_of(9), None);
    // A list of one Mono's tabs only adds what it shows.
    t.sync(&[tab(4, "https://d.example/", "D", Some(A))], false, 200);
    assert_eq!(t.owner_of(4), Some(A));
    assert_eq!(t.owner_of(2), Some(B));
}

#[test]
fn events_follow_the_users_drags_and_report_a_take_back() {
    let mut t = Table::default();
    t.claim(1, A, How::Created, 1).unwrap();
    // The user drags the tab out of A's group.
    let change = t
        .apply_event(&json!({"tab": 1, "owner": null, "by": "user_drag"}), 50)
        .expect("a change");
    assert_eq!(
        change,
        Change {
            tab: 1,
            from: Some(A.into()),
            to: None,
            taken_back: true
        }
    );
    assert_eq!(t.taken_back_at(1), Some(50));
    // The user drags it into B's group: adopted by the user, no prompt.
    let change = t
        .apply_event(&json!({"tab": 1, "owner": B, "by": "user_drag"}), 60)
        .unwrap();
    assert_eq!(
        (change.from, change.to.as_deref(), change.taken_back),
        (None, Some(B), false)
    );
    assert_eq!(t.get(1).unwrap().how, How::UserDrag);
    assert_eq!(t.taken_back_at(1), None);
    // A popup of B's tab is B's.
    t.apply_event(&json!({"tab": 2, "owner": B, "by": "popup"}), 70)
        .unwrap();
    assert_eq!(t.get(2).unwrap().how, How::Popup);
    // A closed group is a take back too: the old reference does not adopt the tab again.
    let change = t
        .apply_event(&json!({"tab": 2, "owner": null, "by": "group_closed"}), 80)
        .unwrap();
    assert!(change.taken_back);
    assert_eq!(t.taken_back_at(2), Some(80));
    // Ungrouping is a take back too; an unknown tab or a malformed event changes nothing.
    t.claim(3, A, How::Created, 1).unwrap();
    assert!(
        t.apply_event(&json!({"tab": 3, "owner": null, "by": "user_ungroup"}), 90)
            .unwrap()
            .taken_back
    );
    assert!(t
        .apply_event(&json!({"tab": 4, "owner": null, "by": "user_drag"}), 90)
        .is_none());
    assert!(t
        .apply_event(&json!({"owner": null, "by": "user_drag"}), 90)
        .is_none());
    assert!(t
        .apply_event(&json!({"tab": 5, "owner": 7, "by": "user_drag"}), 90)
        .is_none());
}

#[test]
fn a_users_drag_moves_a_tab_away_from_another_mono() {
    let mut t = Table::default();
    t.claim(1, A, How::Created, 1).unwrap();
    let change = t
        .apply_event(&json!({"tab": 1, "owner": B, "by": "user_drag"}), 5)
        .unwrap();
    assert_eq!(change.from.as_deref(), Some(A));
    assert_eq!(t.owner_of(1), Some(B));
}

fn said(texts: &[(&str, u64)]) -> Vec<(String, u64)> {
    texts.iter().map(|(t, ms)| ((*t).to_owned(), *ms)).collect()
}

#[test]
fn a_reference_must_be_long_enough_and_words_the_user_wrote() {
    let messages = said(&[(
        "Please use the  Quarterly   Report tab to fill in the form",
        100,
    )]);
    assert_eq!(check_reference("quarterly report", &messages), Ok(100));
    assert_eq!(check_reference("Quarterly Report tab", &messages), Ok(100));
    assert_eq!(
        check_reference("form", &messages),
        Err(ReferenceError::TooShort)
    );
    // One long word is enough (8 characters).
    assert_eq!(check_reference("Quarterly", &messages), Ok(100));
    assert_eq!(
        check_reference("budget sheet", &messages),
        Err(ReferenceError::NotSaid)
    );
    assert_eq!(
        check_reference("quarterly report", &[]),
        Err(ReferenceError::NotSaid)
    );
    // The newest message that holds it wins.
    let twice = said(&[
        ("open the quarterly report", 10),
        ("the quarterly report again", 90),
    ]);
    assert_eq!(check_reference("quarterly report", &twice), Ok(90));
    let text = ReferenceError::NotSaid.text();
    assert!(text.contains(ASK_THE_USER), "{text}");
}

#[test]
fn a_reference_must_point_at_exactly_one_tab() {
    let tabs = [
        tab(
            1,
            "https://sheets.example/d/1",
            "Quarterly Report - Sheets",
            None,
        ),
        tab(2, "https://mail.example/", "Inbox (3)", None),
        tab(3, "https://sheets.example/d/2", "Budget - Sheets", Some(B)),
    ];
    assert_eq!(find_tab("the quarterly report tab", &tabs).unwrap().tab, 1);
    assert_eq!(
        find_tab("budget sheets", &tabs).unwrap().tab,
        3,
        "another Mono's tab can be named"
    );
    assert_eq!(find_tab("inbox", &tabs).unwrap().tab, 2);
    // The host counts too.
    assert_eq!(find_tab("mail example", &tabs).unwrap().tab, 2);
    // Two tabs fit: refused with their ids.
    assert_eq!(
        find_tab("sheets", &tabs),
        Err(MatchError::Several(vec![1, 3]))
    );
    assert_eq!(find_tab("nothing like it", &tabs), Err(MatchError::None));
    // Filler words alone name nothing.
    assert_eq!(find_tab("the tab", &tabs), Err(MatchError::None));
    assert!(MatchError::None.text().contains(ASK_THE_USER));
}

#[test]
fn the_mirror_of_each_link_is_separate_and_can_be_forgotten() {
    let owner = |link: &str| with_table(link, |t| t.owner_of(1).map(str::to_owned));
    with_table("ownership-test-link-1", |t| {
        t.claim(1, A, How::Created, 1).unwrap()
    });
    assert_eq!(owner("ownership-test-link-2"), None);
    assert_eq!(owner("ownership-test-link-1"), Some(A.into()));
    forget("ownership-test-link-1");
    assert_eq!(owner("ownership-test-link-1"), None);
}
