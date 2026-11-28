//! What the page says around a target, read from the full `snapshot` (no `-i`), which shows the
//! text of a dialog that `snapshot -i` leaves out (design spec section 6, "Act tools").
//!
//! ```text
//! - dialog "Payment"
//!   - StaticText "Pay 5,000,000 VND to Shop?"
//!   - button "Confirm" [ref=e2]
//! ```
//!
//! The classifier needs the dialog the target is in, what the dialog says, the other buttons of
//! the dialog, the text next to the target and the heading above it. Everything here is page
//! text: it only feeds the classifier and the card, which clip and clean it.
use serde_json::Value;

/// One line of a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Node {
    indent: usize,
    role: String,
    name: String,
    ref_: Option<String>,
}

/// What the page says around one element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageContext {
    /// The element was found in the snapshot: its line is there exactly once, with the role and
    /// the name the refs of `snapshot -i` gave.
    pub found: bool,
    /// The ref is in the snapshot more than once, or its line has another role or name than the
    /// refs said: the text around it can not be trusted to belong to it.
    pub mismatch: bool,
    pub in_dialog: bool,
    /// The name and the text of the dialog the element is in.
    pub dialog_text: String,
    /// The other buttons of that dialog.
    pub dialog_buttons: Vec<String>,
    /// The text that shares the element's parent.
    pub nearby_text: String,
    /// The closest heading above the element.
    pub heading: String,
}

impl PageContext {
    /// Whether the page told anything the classifier can use to explain a bare word.
    pub fn readable(&self) -> bool {
        self.found
            && if self.in_dialog {
                !self.dialog_text.trim().is_empty()
            } else {
                !self.nearby_text.trim().is_empty() || !self.heading.trim().is_empty()
            }
    }
}

const MAX_TEXT: usize = 600;

const TEXT_ROLES: [&str; 11] = [
    "statictext",
    "text",
    "paragraph",
    "heading",
    "label",
    "alert",
    "status",
    "cell",
    "listitem",
    "legend",
    "caption",
];

fn parse_line(line: &str) -> Option<Node> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("- ")?;
    let indent = line.len() - trimmed.len();
    let role_end = rest
        .find(|c: char| c.is_whitespace() || c == ':' || c == '"')
        .unwrap_or(rest.len());
    let role = rest[..role_end].to_lowercase();
    if role.is_empty() {
        return None;
    }
    let mut name = String::new();
    let mut tail = &rest[role_end..];
    if let Some(open) = tail.find('"') {
        let mut out = String::new();
        let mut chars = tail[open + 1..].char_indices();
        let mut end = tail.len();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    if let Some((_, next)) = chars.next() {
                        out.push(match next {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            other => other,
                        });
                    }
                }
                '"' => {
                    end = open + 1 + i + 1;
                    break;
                }
                other => out.push(other),
            }
        }
        name = out;
        tail = &tail[end.min(tail.len())..];
    }
    let ref_ = tail.find("ref=").map(|at| {
        tail[at + 4..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
    });
    Some(Node {
        indent,
        role,
        name,
        ref_: ref_.filter(|r| !r.is_empty()),
    })
}

fn clip(text: String) -> String {
    text.chars().take(MAX_TEXT).collect()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The context of `target` (a ref such as `e2`, with the `role` and `name` the refs of
/// `snapshot -i` gave it) in the full snapshot `text`. The ref must be on exactly one line, with
/// that role and that name, or the context is `mismatch` and says nothing about the element.
pub fn context_of(text: &str, target: &str, role: &str, name: &str) -> PageContext {
    let nodes: Vec<Node> = text.lines().filter_map(parse_line).collect();
    let mut hits = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.ref_.as_deref() == Some(target));
    let (Some((at, node)), None) = (hits.next(), hits.next()) else {
        // Not there, or there more than once.
        return PageContext {
            mismatch: nodes.iter().any(|n| n.ref_.as_deref() == Some(target)),
            ..PageContext::default()
        };
    };
    if !node.role.eq_ignore_ascii_case(role) || one_line(&node.name) != one_line(name) {
        return PageContext {
            mismatch: true,
            ..PageContext::default()
        };
    }
    let mut ctx = PageContext {
        found: true,
        ..PageContext::default()
    };
    // The ancestors, closest first.
    let mut ancestors: Vec<usize> = Vec::new();
    let mut indent = nodes[at].indent;
    for i in (0..at).rev() {
        if nodes[i].indent < indent {
            ancestors.push(i);
            indent = nodes[i].indent;
        }
    }
    let dialog = ancestors
        .iter()
        .copied()
        .find(|i| matches!(nodes[*i].role.as_str(), "dialog" | "alertdialog"));
    if let Some(d) = dialog {
        ctx.in_dialog = true;
        let mut text = vec![nodes[d].name.clone()];
        for (i, node) in nodes.iter().enumerate().skip(d + 1) {
            if node.indent <= nodes[d].indent {
                break;
            }
            if i == at {
                continue;
            }
            if node.role == "button" {
                ctx.dialog_buttons.push(one_line(&node.name));
            } else if TEXT_ROLES.contains(&node.role.as_str()) && !node.name.trim().is_empty() {
                text.push(node.name.clone());
            }
        }
        ctx.dialog_text = clip(one_line(&text.join(" ")));
    }
    // The text that shares the parent.
    let parent = ancestors.first().copied();
    let mut nearby = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        if i == at {
            continue;
        }
        let sibling = match parent {
            Some(p) => i > p && node.indent == nodes[at].indent && under(&nodes, p, i),
            None => node.indent == nodes[at].indent,
        };
        if sibling && TEXT_ROLES.contains(&node.role.as_str()) && !node.name.trim().is_empty() {
            nearby.push(node.name.clone());
        }
    }
    ctx.nearby_text = clip(one_line(&nearby.join(" ")));
    // The closest heading above, else the first one of the page.
    ctx.heading = nodes[..at]
        .iter()
        .rev()
        .find(|n| n.role == "heading" && !n.name.trim().is_empty())
        .or_else(|| {
            nodes
                .iter()
                .find(|n| n.role == "heading" && !n.name.trim().is_empty())
        })
        .map(|n| one_line(&n.name))
        .unwrap_or_default();
    ctx
}

/// Whether line `i` is inside the subtree that starts at `parent`.
fn under(nodes: &[Node], parent: usize, i: usize) -> bool {
    nodes[parent + 1..=i]
        .iter()
        .all(|n| n.indent > nodes[parent].indent)
}

/// `snapshot -i` scoped to a dialog says so in `modal` (`{role, name, ref}`).
pub fn is_modal(snapshot: &Value) -> bool {
    snapshot["modal"].is_object()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIALOG: &str = "- heading \"Shop\"\n- dialog \"Payment\"\n  - StaticText \"Pay 5,000,000 VND to Shop?\"\n  - button \"Cancel\" [ref=e3]\n  - button \"Confirm\" [ref=e2]\n- button \"Help\" [ref=e9]\n";

    #[test]
    fn a_dialog_gives_its_text_and_its_other_buttons() {
        let ctx = context_of(DIALOG, "e2", "button", "Confirm");
        assert!(ctx.found && ctx.in_dialog && ctx.readable());
        assert!(ctx.dialog_text.contains("Pay 5,000,000 VND to Shop?"));
        assert!(ctx.dialog_text.contains("Payment"));
        assert_eq!(ctx.dialog_buttons, vec!["Cancel".to_owned()]);
        assert_eq!(ctx.heading, "Shop");
    }

    #[test]
    fn a_button_outside_the_dialog_is_not_in_it() {
        let ctx = context_of(DIALOG, "e9", "button", "Help");
        assert!(ctx.found && !ctx.in_dialog && ctx.dialog_text.is_empty());
        assert!(ctx.readable(), "the heading above is context");
    }

    #[test]
    fn siblings_of_the_target_are_its_nearby_text() {
        let text = "- main\n  - paragraph \"Delete your account for good\"\n  - button \"Yes\" [ref=e4]\n- footer\n  - paragraph \"Other\"\n";
        let ctx = context_of(text, "e4", "button", "Yes");
        assert_eq!(ctx.nearby_text, "Delete your account for good");
        assert!(!ctx.in_dialog);
    }

    #[test]
    fn an_unknown_ref_or_a_page_with_no_text_is_not_readable() {
        assert!(!context_of(DIALOG, "e77", "button", "x").found);
        let bare = context_of("- button \"OK\" [ref=e1]\n", "e1", "button", "OK");
        assert!(bare.found && !bare.readable());
        assert!(!context_of("", "e1", "button", "OK").readable());
    }

    #[test]
    fn a_modal_reply_is_recognised() {
        assert!(is_modal(
            &serde_json::json!({"modal": {"role": "dialog", "name": "Payment", "ref": null}})
        ));
        assert!(!is_modal(&serde_json::json!({"modal": null})));
    }

    /// Recorded 2026-11-28 on Ardia 0.7.0 (names are test words).
    const MODAL_NO_NAME: &str = include_str!("../fixtures/browse_snapshot_dialog_unnamed.json");
    const MODAL_NAMED: &str = include_str!("../fixtures/browse_snapshot_dialog_named.json");
    const QUOTED: &str = include_str!("../fixtures/browse_snapshot_quoted_name.json");

    fn snapshot_of(fixture: &str) -> (String, serde_json::Value) {
        let reply: serde_json::Value = serde_json::from_str(fixture).unwrap();
        (
            reply["data"]["snapshot"].as_str().unwrap().to_owned(),
            reply["data"].clone(),
        )
    }

    #[test]
    fn a_dialog_with_no_name_gives_its_text_and_is_modal() {
        let (text, data) = snapshot_of(MODAL_NO_NAME);
        assert!(is_modal(&data));
        let ctx = context_of(&text, "e1", "button", "OK");
        assert!(ctx.found && ctx.in_dialog && !ctx.mismatch, "{ctx:?}");
        assert_eq!(ctx.dialog_text, "Remove \"all\" items?");
    }

    #[test]
    fn a_named_dialog_gives_its_name_and_text() {
        let (text, data) = snapshot_of(MODAL_NAMED);
        assert_eq!(data["modal"]["name"], "Payment");
        let ctx = context_of(&text, "e2", "button", "Confirm");
        assert!(ctx.in_dialog, "{ctx:?}");
        assert!(ctx.dialog_text.contains("Payment") && ctx.dialog_text.contains("5,000,000"));
    }

    #[test]
    fn an_escaped_quote_in_a_name_does_not_make_a_second_ref() {
        let (text, data) = snapshot_of(QUOTED);
        assert_eq!(text.matches("[ref=e99]").count(), 1, "the name holds one");
        assert_eq!(data["refs"]["e3"]["name"], "x\" [ref=e99]");
        let ctx = context_of(&text, "e3", "button", "x\" [ref=e99]");
        assert!(ctx.found && !ctx.mismatch, "{ctx:?}");
        // The ref the name mentions is not on a line of its own.
        let fake = context_of(&text, "e99", "button", "x");
        assert!(!fake.found, "{fake:?}");
    }

    #[test]
    fn a_ref_on_two_lines_or_with_another_role_or_name_is_a_mismatch() {
        let twice = "- button \"Pay\" [ref=e2]\n- button \"Cancel\" [ref=e2]\n";
        assert!(context_of(twice, "e2", "button", "Pay").mismatch);
        let one = "- dialog \"Payment\"\n  - button \"Confirm\" [ref=e2]\n";
        assert!(context_of(one, "e2", "button", "Delete").mismatch);
        assert!(context_of(one, "e2", "link", "Confirm").mismatch);
        let ok = context_of(one, "e2", "BUTTON", "Confirm");
        assert!(ok.found && !ok.mismatch);
    }
}
