//! The confirmation an update needs when work is running: what the dialog says and when it is
//! needed at all. Pure data, so every entry point (sidebar, Settings, a remote request) and the
//! tests share one decision.
use super::remote::RunningSession;

/// Names listed in the dialog before the rest is summarised as "and N more".
pub const LISTED: usize = 5;

/// What the dialog says. The buttons are fixed: [`CONFIRM`] and [`CANCEL`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub title: &'static str,
    pub detail: String,
}

pub const TITLE: &str = "Update will interrupt running work";
pub const CONFIRM: &str = "Update anyway";
pub const CANCEL: &str = "Cancel";

/// The dialog for `running`, or `None` when it would not tell the user anything new: nothing is
/// running, or everything that runs was already accepted (`acknowledged`, the work the user
/// confirmed earlier in the same update). `requester` names who asked for a remote update.
pub fn prompt(
    running: &[RunningSession],
    acknowledged: &[RunningSession],
    requester: Option<&str>,
) -> Option<Prompt> {
    if running
        .iter()
        .all(|r| acknowledged.iter().any(|a| a.id == r.id))
    {
        return None;
    }
    let mut detail = String::new();
    if let Some(requester) = requester {
        detail.push_str(&format!("{requester} asked to update zptah. "));
    }
    detail.push_str(
        "Updating restarts zptah. Running replies will stop and need to be resumed afterwards. Cloud sessions keep running.\n\nRunning now:\n",
    );
    detail.push_str(&list(running));
    Some(Prompt {
        title: TITLE,
        detail,
    })
}

/// One line per name, at most [`LISTED`], then "and N more".
pub fn list(running: &[RunningSession]) -> String {
    // One Mono can run several things (its chat and a habit): name it once.
    let mut names: Vec<&str> = Vec::new();
    for work in running {
        if !names.contains(&name(work)) {
            names.push(name(work));
        }
    }
    let mut lines: Vec<String> = names
        .iter()
        .take(LISTED)
        .map(|name| format!("- {name}"))
        .collect();
    if names.len() > LISTED {
        lines.push(format!("and {} more", names.len() - LISTED));
    }
    lines.join("\n")
}

fn name(session: &RunningSession) -> &str {
    let title = session.title.trim();
    if title.is_empty() {
        "Untitled"
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(id: &str, title: &str) -> RunningSession {
        RunningSession {
            id: id.into(),
            title: title.into(),
        }
    }

    #[test]
    fn nothing_running_needs_no_dialog() {
        assert_eq!(prompt(&[], &[], None), None);
    }

    #[test]
    fn running_work_is_listed() {
        let p = prompt(&[work("1", "Fix login")], &[], None).unwrap();
        assert_eq!(p.title, TITLE);
        assert!(p.detail.contains("- Fix login"));
        assert!(p.detail.contains("Cloud sessions keep running"));
    }

    #[test]
    fn long_lists_are_cut_after_five() {
        let many: Vec<_> = (0..8)
            .map(|i| work(&i.to_string(), &format!("S{i}")))
            .collect();
        let text = list(&many);
        assert!(text.contains("- S4") && !text.contains("- S5"));
        assert!(text.ends_with("and 3 more"));
        assert!(!list(&many[..5]).contains("more"));
    }

    #[test]
    fn only_new_work_asks_again() {
        let a = work("1", "A");
        let b = work("2", "B");
        assert_eq!(
            prompt(std::slice::from_ref(&a), std::slice::from_ref(&a), None),
            None
        );
        let again = prompt(&[a.clone(), b], &[a], None).unwrap();
        assert!(again.detail.contains("- A") && again.detail.contains("- B"));
    }

    #[test]
    fn a_remote_requester_is_named() {
        let p = prompt(&[work("1", "A")], &[], Some("Phone")).unwrap();
        assert!(p.detail.starts_with("Phone asked to update zptah."));
    }

    #[test]
    fn the_same_name_is_listed_once() {
        let twice = [work("1", "Ada"), work("2", "Ada")];
        assert_eq!(list(&twice), "- Ada");
    }

    #[test]
    fn untitled_work_still_has_a_name() {
        assert_eq!(list(&[work("1", "  ")]), "- Untitled");
    }
}
