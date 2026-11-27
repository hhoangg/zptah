//! The user's own tab, found without touching anything (design spec section 4).
//!
//! `ardia call windows --json` lists every window with its tabs, each with its `profile` and
//! whether it is `active`. The tab the user is looking at is the active tab of the focused
//! window. `tab list` is never used for this: in a session with no bound tab it lists nothing,
//! and binding to look would already be taking the tab.
use serde_json::Value;

/// The tab the user has open now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserTab {
    pub id: String,
    pub url: String,
    pub title: String,
    /// The Ardia profile id of the tab.
    pub profile: String,
}

/// The active tab of the focused normal window (`None`: no focused window or no active tab).
pub fn active_tab(windows: &Value) -> Option<UserTab> {
    let window = windows
        .as_array()?
        .iter()
        .find(|w| w["focused"] == true && w["kind"].as_str().is_none_or(|k| k == "normal"))?;
    let tab = window["tabs"]
        .as_array()?
        .iter()
        .find(|t| t["active"] == true)?;
    let text = |key: &str| tab[key].as_str().unwrap_or("").to_owned();
    Some(UserTab {
        id: tab["id"].as_str().filter(|i| !i.is_empty())?.to_owned(),
        url: text("url"),
        title: text("title"),
        profile: text("profile"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn the_active_tab_of_the_focused_window_is_the_users() {
        let one = active_tab(&fixture(include_str!("../fixtures/windows0.json"))).unwrap();
        assert_eq!(one.id, "tab-mwz3kap1pqq0-4");
        assert_eq!(one.profile, "default");
        // The second tab of windows1.json is in another profile and not active.
        let two = active_tab(&fixture(include_str!("../fixtures/windows1.json"))).unwrap();
        assert_eq!(two.id, "tab-mwz3kap1pqq0-4");
    }

    #[test]
    fn no_focused_window_or_no_active_tab_is_none() {
        assert_eq!(active_tab(&serde_json::json!([])), None);
        assert_eq!(active_tab(&serde_json::json!({"x": 1})), None);
        assert_eq!(
            active_tab(
                &serde_json::json!([{"focused": false, "tabs": [{"id": "t", "active": true}]}])
            ),
            None
        );
        assert_eq!(
            active_tab(
                &serde_json::json!([{"focused": true, "tabs": [{"id": "t", "active": false}]}])
            ),
            None
        );
    }
}
