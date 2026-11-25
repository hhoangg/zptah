use super::*;

fn set(cats: &[Category]) -> BTreeSet<Category> {
    cats.iter().copied().collect()
}

fn soul(block: &str) -> String {
    format!("# Mira\n\nSome prose. ask: click Pay on bank.example\n\n## Browser rules\n\nNotes.\n\n```browser-rules\n{block}\n```\n\n## Other\n")
}

fn facts<'a>(
    kind: FactKind,
    text: &'a str,
    url: &'a str,
    cats: &'a BTreeSet<Category>,
) -> ActionFacts<'a> {
    ActionFacts {
        kind,
        element_text: text,
        url,
        send: false,
        post: false,
        categories: cats,
    }
}

#[test]
fn the_spec_example_parses() {
    let parsed = parse(&soul(
        "ask: click \"Book\" on airbnb.com\nask: any action on *.vpbank.com.vn\nask: send on mail.google.com\nblock: navigate to facebook.com\nblock: post on *\n# a comment\n\nask: category payment on *",
    ));
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_eq!(parsed.rules.len(), 6);
    assert_eq!(parsed.lines.len(), 6);
    assert_eq!(parsed.rules[3].verb, Verb::Block);
}

#[test]
fn prose_and_other_fences_are_never_read_as_rules() {
    let text = "## Browser rules\n\nblock: navigate to evil.example\n\n```text\nblock: post on *\n```\n\n## Notes\n\n```browser-rules\nblock: post on *\n```\n";
    let parsed = parse(text);
    assert!(parsed.rules.is_empty() && parsed.errors.is_empty());
    // The heading is required and exact.
    assert!(parse("```browser-rules\nblock: post on *\n```")
        .rules
        .is_empty());
}

#[test]
fn a_line_that_does_not_parse_fails_closed() {
    for bad in [
        "allow: click on a.com",
        "ask click on a.com",
        "ask: dance on a.com",
        "ask: click \"unclosed on a.com",
        "ask: click \"\" on a.com",
        "ask: click on",
        "ask: click on notasite",
        "ask: navigate to",
        "ask: category nothing on *",
        "ask: click extra words",
    ] {
        let parsed = parse(&soul(bad));
        assert!(parsed.fail_closed(), "{bad} should not parse");
        assert!(parsed.rules.is_empty(), "{bad}");
    }
    // A fence that never closes is an error too.
    assert!(parse("## Browser rules\n```browser-rules\nask: click on a.com\n").fail_closed());
}

#[test]
fn there_is_no_allow_verb() {
    let parsed = parse(&soul("allow: any action on *"));
    assert!(parsed.fail_closed());
    assert!(parsed.rules.is_empty());
}

#[test]
fn quoted_text_matches_folded_substrings_and_sites_cover_subdomains() {
    let parsed = parse(&soul("ask: click \"đặt phòng\" on airbnb.com"));
    let none = BTreeSet::new();
    let hit = parsed.matching(&facts(
        FactKind::Click,
        "Dat phong ngay",
        "https://www.airbnb.com/rooms/1",
        &none,
    ));
    assert!(hit.ask && hit.blocked.is_none());
    let other_site = parsed.matching(&facts(
        FactKind::Click,
        "Dat phong",
        "https://other.com/",
        &none,
    ));
    assert!(!other_site.ask);
    let other_text = parsed.matching(&facts(
        FactKind::Click,
        "Search",
        "https://airbnb.com/",
        &none,
    ));
    assert!(!other_text.ask);
    let typed = parsed.matching(&facts(
        FactKind::Type,
        "Dat phong",
        "https://airbnb.com/",
        &none,
    ));
    assert!(!typed.ask);
}

#[test]
fn block_navigate_send_post_category_and_wildcards() {
    let parsed = parse(&soul(
        "block: navigate to facebook.com\nask: send on mail.google.com\nblock: post on *\nask: category payment on *.shop.example\nask: any action on *.vpbank.com.vn",
    ));
    let none = BTreeSet::new();
    let nav = parsed.matching(&facts(
        FactKind::Navigate,
        "",
        "https://m.facebook.com/x",
        &none,
    ));
    assert_eq!(
        nav.blocked.as_deref(),
        Some("block: navigate to facebook.com")
    );
    let mut send = facts(
        FactKind::Click,
        "Send",
        "https://mail.google.com/mail",
        &none,
    );
    assert!(!parsed.matching(&send).ask);
    send.send = true;
    assert!(parsed.matching(&send).ask);
    let mut post = facts(FactKind::Click, "Post", "https://x.example/", &none);
    post.post = true;
    assert!(parsed.matching(&post).blocked.is_some());
    let pay = set(&[Category::Payment]);
    assert!(
        parsed
            .matching(&facts(
                FactKind::Click,
                "Buy",
                "https://www.shop.example/c",
                &pay
            ))
            .ask
    );
    assert!(
        !parsed
            .matching(&facts(
                FactKind::Click,
                "Buy",
                "https://www.shop.example/c",
                &none
            ))
            .ask
    );
    assert!(
        parsed
            .matching(&facts(
                FactKind::Other,
                "",
                "https://online.vpbank.com.vn/",
                &none
            ))
            .ask
    );
    assert!(
        !parsed
            .matching(&facts(
                FactKind::Click,
                "x",
                "https://notvpbank.com.vn/",
                &none
            ))
            .ask
    );
}

fn lines(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn a_pure_addition_applies_at_once() {
    let confirmed = lines(&["ask: click on a.com", "block: post on *"]);
    let next =
        soul("ask: click on a.com\nask: type on b.com\nblock: post on *\nblock: navigate to c.com");
    let r = reconcile(Some(&confirmed), &next);
    assert!(!r.needs_confirmation);
    assert_eq!(r.active.rules.len(), 4);
    assert_eq!(r.confirmed_lines.len(), 4);
}

#[test]
fn weakening_a_rule_waits_for_confirmation_and_keeps_the_old_set() {
    let confirmed = lines(&["block: post on *", "ask: click on a.com"]);
    for changed in [
        // removed
        "ask: click on a.com",
        // block turned into ask
        "ask: post on *\nask: click on a.com",
        // edited
        "block: post on x.com\nask: click on a.com",
        // reordered
        "ask: click on a.com\nblock: post on *",
    ] {
        let r = reconcile(Some(&confirmed), &soul(changed));
        assert!(r.needs_confirmation, "{changed}");
        assert_eq!(r.active.lines, confirmed, "{changed}");
        assert_eq!(r.confirmed_lines, confirmed, "{changed}");
        // The block stays in force.
        assert!(r.active.rules.iter().any(|x| x.verb == Verb::Block));
    }
}

#[test]
fn the_first_set_and_an_empty_soul_are_handled() {
    let first = reconcile(None, &soul("ask: click on a.com"));
    assert!(!first.needs_confirmation);
    assert_eq!(first.confirmed_lines.len(), 1);
    // Deleting the whole block removes the confirmed lines: that needs confirmation.
    let confirmed = lines(&["ask: click on a.com"]);
    let gone = reconcile(Some(&confirmed), "# Mira\n");
    assert!(gone.needs_confirmation);
    assert_eq!(gone.active.rules.len(), 1);
}

#[test]
fn a_broken_line_in_the_current_soul_still_fails_closed_while_a_change_waits() {
    let confirmed = lines(&["block: post on *"]);
    let r = reconcile(Some(&confirmed), &soul("oops"));
    assert!(r.needs_confirmation);
    assert!(r.active.fail_closed());
}
