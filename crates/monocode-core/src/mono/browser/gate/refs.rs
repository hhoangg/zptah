//! Recipe locators on the zptah Browser extension (spec 15.5).
//!
//! A recipe (schema 1) finds elements by selector strings (`internal:role=button[name="Send"s]`,
//! optionally scoped with ` >> `). The extension has no selectors: its tools take the refs a
//! snapshot prints (`e12`). So a recipe step that names an element by selector is converted here,
//! at the gate, before the call goes on: the page is snapshotted, the selector is matched against
//! the snapshot the way a locator matches (exact, case sensitive role, name, label or text), and
//! a unique match becomes its ref. No match, several matches, or a selector the snapshot can not
//! express (a CSS selector, a test id) answers like a missing or ambiguous element, so the run
//! fails and the existing repair flow can propose another locator.
use super::super::recipe::snapshot::{self, Node};
use serde_json::Value;

/// A ref the snapshot prints: `e12`, or `f3e12` inside a frame.
pub(super) fn is_ref(text: &str) -> bool {
    let rest = text
        .strip_prefix('f')
        .map_or(text, |r| r.trim_start_matches(|c: char| c.is_ascii_digit()));
    rest.strip_prefix('e')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        && text.len() <= 64
}

/// The `target` of a call when it is a selector (anything that is not a ref).
pub(super) fn selector_of(args: &Value) -> Option<&str> {
    args["target"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty() && !is_ref(t))
}

/// What a selector found in a snapshot.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Located {
    Ref(String),
    Missing,
    Multiple(usize),
    /// The snapshot can not express this selector (CSS, test id): it is never found.
    Unsupported,
}

enum Part {
    Role { role: String, name: Option<String> },
    Label(String),
    Text(String),
    Unsupported,
}

/// The JSON string literal at the start of `text` followed by the exact flag `s`, and what is
/// left after both.
fn literal(text: &str) -> Option<(String, &str)> {
    let mut stream = serde_json::Deserializer::from_str(text).into_iter::<String>();
    let value = stream.next()?.ok()?;
    let rest = &text[stream.byte_offset()..];
    Some((value, rest.strip_prefix('s')?))
}

fn parse_part(part: &str) -> Part {
    let part = part.trim();
    if let Some(rest) = part.strip_prefix("internal:role=") {
        let Some((role, tail)) = rest.split_once('[') else {
            return Part::Role {
                role: rest.to_owned(),
                name: None,
            };
        };
        let Some((name, after)) = tail
            .strip_prefix("name=")
            .and_then(literal)
            .filter(|(_, after)| *after == "]")
        else {
            return Part::Unsupported;
        };
        let _ = after;
        return Part::Role {
            role: role.to_owned(),
            name: Some(name),
        };
    }
    if let Some(rest) = part.strip_prefix("internal:label=") {
        return match literal(rest) {
            Some((label, "")) => Part::Label(label),
            _ => Part::Unsupported,
        };
    }
    if let Some(rest) = part.strip_prefix("internal:text=") {
        return match literal(rest) {
            Some((text, "")) => Part::Text(text),
            _ => Part::Unsupported,
        };
    }
    Part::Unsupported
}

fn matches(node: &Node, part: &Part) -> bool {
    if node.reference().is_none() {
        return false;
    }
    match part {
        Part::Role { role, name } => {
            node.role == *role && name.as_ref().is_none_or(|name| node.name == *name)
        }
        Part::Label(label) => node.name == *label,
        Part::Text(text) => node.name == *text || node.text_content() == *text,
        Part::Unsupported => false,
    }
}

fn find<'a>(nodes: &[&'a Node], part: &Part) -> Vec<&'a Node> {
    nodes
        .iter()
        .copied()
        .filter(|node| matches(node, part))
        .collect()
}

fn unique(found: Vec<&Node>) -> Result<&Node, Located> {
    match found.as_slice() {
        [] => Err(Located::Missing),
        [one] => Ok(one),
        many => Err(Located::Multiple(many.len())),
    }
}

/// Finds the element `selector` names in a snapshot `forest`.
pub(super) fn locate(forest: &[Node], selector: &str) -> Located {
    let everything = snapshot::all(forest);
    let (scope, own) = match selector.split_once(" >> ") {
        Some((scope, own)) => (Some(parse_part(scope)), parse_part(own)),
        None => (None, parse_part(selector)),
    };
    if matches!(own, Part::Unsupported) || matches!(scope, Some(Part::Unsupported)) {
        return Located::Unsupported;
    }
    let result = match &scope {
        Some(scope) => unique(find(&everything, scope)).and_then(|scope| {
            // The scope itself does not count: only what is inside it.
            let mut inside = Vec::new();
            for child in &scope.children {
                child.walk(&mut inside);
            }
            unique(find(&inside, &own))
        }),
        None => unique(find(&everything, &own)),
    };
    match result.map(Node::reference) {
        Ok(Some(reference)) => Located::Ref(reference.to_owned()),
        Ok(None) => Located::Missing,
        Err(other) => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "- main [ref=e1]:\n  - dialog \"Compose\" [ref=e2]:\n    - textbox \"Message\" [ref=e3]\n    - button \"Send\" [ref=e4]\n  - button \"Send\" [ref=e5]\n  - button \"Save\" [ref=e6]\n  - heading \"Inbox\" [level=1]\n";

    fn forest() -> Vec<Node> {
        snapshot::parse(PAGE)
    }

    #[test]
    fn refs_are_told_from_selectors() {
        for good in ["e1", "e12", "f1e3", "f12e40"] {
            assert!(is_ref(good), "{good}");
        }
        for bad in [
            "",
            "e",
            "f1",
            "ex",
            "internal:role=button",
            "button",
            "#send",
        ] {
            assert!(!is_ref(bad), "{bad}");
        }
        assert_eq!(selector_of(&serde_json::json!({"target": "e3"})), None);
        assert_eq!(
            selector_of(&serde_json::json!({"target": " internal:role=button "})),
            Some("internal:role=button")
        );
        assert_eq!(selector_of(&serde_json::json!({})), None);
    }

    #[test]
    fn a_role_and_name_selector_becomes_the_ref_of_its_one_match() {
        let forest = forest();
        assert_eq!(
            locate(&forest, r#"internal:role=textbox[name="Message"s]"#),
            Located::Ref("e3".into())
        );
        assert_eq!(
            locate(&forest, r#"internal:role=button[name="Save"s]"#),
            Located::Ref("e6".into())
        );
        // Exact and case sensitive.
        assert_eq!(
            locate(&forest, r#"internal:role=button[name="save"s]"#),
            Located::Missing
        );
        assert_eq!(
            locate(&forest, r#"internal:role=button[name="Sav"s]"#),
            Located::Missing
        );
        // A node without a ref is not something a call can act on.
        assert_eq!(
            locate(&forest, r#"internal:role=heading[name="Inbox"s]"#),
            Located::Missing
        );
    }

    #[test]
    fn several_matches_are_ambiguous_and_a_scope_narrows_them() {
        let forest = forest();
        assert_eq!(
            locate(&forest, r#"internal:role=button[name="Send"s]"#),
            Located::Multiple(2)
        );
        assert_eq!(
            locate(
                &forest,
                r#"internal:role=dialog[name="Compose"s] >> internal:role=button[name="Send"s]"#
            ),
            Located::Ref("e4".into())
        );
        assert_eq!(
            locate(
                &forest,
                r#"internal:role=dialog[name="Nope"s] >> internal:role=button[name="Send"s]"#
            ),
            Located::Missing
        );
        // Without a name, the role alone.
        assert_eq!(
            locate(&forest, "internal:role=dialog"),
            Located::Ref("e2".into())
        );
    }

    #[test]
    fn labels_and_texts_match_names_and_what_a_css_or_test_id_selector_can_not_be_found() {
        let forest = forest();
        assert_eq!(
            locate(&forest, r#"internal:label="Message"s"#),
            Located::Ref("e3".into())
        );
        assert_eq!(
            locate(&forest, r#"internal:text="Save"s"#),
            Located::Ref("e6".into())
        );
        for selector in [
            "#send",
            "div > button",
            r#"internal:testid=[data-testid="send"s]"#,
            r#"internal:role=button[name="Send"i]"#,
            "",
        ] {
            assert_eq!(
                locate(&forest, selector),
                Located::Unsupported,
                "{selector}"
            );
        }
    }

    #[test]
    fn a_name_with_quotes_and_brackets_is_read_as_one_literal() {
        let page = "- button \"Say \\\"hi\\\" [now]\" [ref=e9]\n";
        let forest = snapshot::parse(page);
        let name = r#"Say "hi" [now]"#;
        let selector = format!(
            "internal:role=button[name={}s]",
            serde_json::Value::String(name.into())
        );
        assert_eq!(locate(&forest, &selector), Located::Ref("e9".into()));
    }
}
