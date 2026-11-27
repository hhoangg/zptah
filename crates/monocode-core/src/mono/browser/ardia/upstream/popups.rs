//! What an act can do to the browser outside the session's own tab (design spec section 12b,
//! "Act inputs": popups).
//!
//! Measured on Ardia 0.7.0: a click on a `target=_blank` link, or a `window.open`, opens a page
//! target that `ardia tabs` does not list (`appTabId: null` in `tab list`, `changes` says
//! nothing) and switches the user's window to the Mono's profile. So around every click, key,
//! select and check the gate
//!
//! * lists the session's page targets and the profile of the window of the bound tab before,
//! * closes after it every new page target that has no `appTabId` (`cdp Target.closeTarget`, the
//!   gate's own call: the Mono never gets `cdp`), never one that has an `appTabId`, and
//! * puts the window back on the profile it showed (`windows.switchProfile`).
//!
//! A link with a `target` is not clicked at all: the gate loads its address in the session's own
//! tab instead (see `run_act`).
use super::*;

/// What was listed before the act.
pub(super) struct Watch {
    /// The `targetId` of every page target the session listed (`None`: the list could not be
    /// read, so nothing is closed after).
    targets: Option<HashSet<String>>,
    /// The window of the bound tab: its id as Ardia gave it, and the profile it showed.
    window: Option<(Value, String)>,
}

/// What the gate did after the act, for the Mono's reply.
#[derive(Default)]
pub(super) struct Effects {
    /// The addresses (redacted, one line) of the new windows that were closed.
    pub closed: Vec<String>,
    /// A new window could not be told from the old ones, or could not be closed.
    pub left_open: Vec<String>,
    /// The window showed another profile after the act and was put back.
    pub profile_restored: bool,
}

/// One page target of `tab list`.
struct Target {
    id: String,
    app_tab: bool,
    url: String,
}

/// A `targetId` is passed to `cdp --param targetId=...`: only the plain ids Ardia gives.
fn plain_target_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

impl ArdiaUpstream {
    fn page_targets(&self, id: i64) -> Option<Vec<Target>> {
        let data = self.browse(id, &["tab", "list"], &[]).ok()?;
        let tabs = data["tabs"].as_array()?;
        Some(
            tabs.iter()
                .filter(|t| t["type"].as_str().is_none_or(|ty| ty == "page"))
                .filter_map(|t| {
                    Some(Target {
                        id: t["targetId"].as_str()?.to_owned(),
                        app_tab: t["appTabId"].as_str().is_some_and(|a| !a.is_empty()),
                        url: t["url"].as_str().unwrap_or("").to_owned(),
                    })
                })
                .collect(),
        )
    }

    /// The window that holds `tab`: its id and the profile it shows now.
    fn window_of(&self, tab: &str) -> Option<(Value, String)> {
        let windows = self.controller.cli().call_without_params("windows").ok()?;
        windows.as_array()?.iter().find_map(|w| {
            let holds = w["tabs"]
                .as_array()?
                .iter()
                .any(|t| t["id"].as_str() == Some(tab));
            holds.then(|| {
                (
                    w["id"].clone(),
                    w["profile"].as_str().unwrap_or("").to_owned(),
                )
            })
        })
    }

    pub(super) fn watch_before(&self, id: i64, tab: &str) -> Watch {
        Watch {
            targets: self
                .page_targets(id)
                .map(|all| all.into_iter().map(|t| t.id).collect()),
            window: self
                .window_of(tab)
                .filter(|(w, p)| !w.is_null() && !p.is_empty()),
        }
    }

    pub(super) fn watch_after(&self, id: i64, tab: &str, before: Watch) -> Effects {
        let mut out = Effects::default();
        if self.not_stopped(id).is_err() {
            return out;
        }
        if let Some(seen) = &before.targets {
            if let Some(now) = self.page_targets(id) {
                for target in now.into_iter().filter(|t| !seen.contains(&t.id)) {
                    // A target that has an `appTabId` is a tab (the user's, or ours): never closed.
                    if target.app_tab {
                        continue;
                    }
                    let shown = defense::redact_url_secrets(&one_line(&target.url));
                    if !plain_target_id(&target.id) || !self.close_target(id, &target.id) {
                        out.left_open.push(shown);
                        continue;
                    }
                    self.audit(
                        id,
                        "popup_closed",
                        &target.url,
                        "the page opened a new window; zptah closed it",
                    );
                    out.closed.push(shown);
                }
            }
        }
        if let Some((window, profile)) = &before.window {
            let now = self.window_of(tab);
            if let Some((now_window, now_profile)) = now {
                if now_window == *window && now_profile != *profile {
                    let params = json!({"window": window, "profile": profile});
                    if self
                        .controller
                        .cli()
                        .call("windows.switchProfile", &params)
                        .is_ok()
                    {
                        out.profile_restored = true;
                        self.audit(
                            id,
                            "window_profile_restored",
                            "",
                            &format!("the act switched the window to profile {now_profile}; put back on {profile}"),
                        );
                    }
                }
            }
        }
        out
    }

    /// `ardia browse ... cdp Target.closeTarget`, with the gate's own words only.
    fn close_target(&self, id: i64, target: &str) -> bool {
        let param = format!("targetId={target}");
        self.browse(
            id,
            &["cdp", "--param", &param, "--target", "browser"],
            &["Target.closeTarget"],
        )
        .is_ok_and(|data| data["result"]["success"] == true)
    }
}
