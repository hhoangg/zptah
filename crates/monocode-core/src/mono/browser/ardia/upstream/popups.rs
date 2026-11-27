//! What an act can do to the browser outside the session's own tab (design spec section 12b,
//! "Act inputs": popups).
//!
//! Measured on Ardia 0.7.0: a click on a `target=_blank` link, or a `window.open`, opens a page
//! target that `ardia tabs` does not list (`appTabId: null` in `tab list`, `changes` says
//! nothing) and switches the user's window to the Mono's profile. So around every act that can
//! reach a page script (click, key, type, fill, select, check, hover) the gate
//!
//! * lists the session's page targets and the profile of the window of the bound tab before,
//! * closes after it only a new page target that has no `appTabId` AND whose `openerId` (read
//!   with `cdp --target browser -- Target.getTargets`) is the session's bound tab (the gate's
//!   own call: the Mono never gets `cdp`). Any other new target without an `appTabId` is not
//!   known to be this page's: it is reported and left alone. A target that has an `appTabId` is a
//!   tab (the user's, or ours) and is never closed, and
//! * puts the window back on the profile it showed (`windows.switchProfile`), only when it now
//!   shows the Mono's profile, showed another one before and a new target appeared.
//!
//! This cleanup also runs after a Stop (with calls that ignore the Stop of the turn and a short
//! timeout each): a window the page opened is the user's to see if it cannot be closed. Whatever
//! could not be checked or undone is reported to the Mono and audited, never dropped.
//!
//! A link with a `target` is not clicked at all: the gate loads its address in the session's own
//! tab instead (see `run_act`).
use super::*;

/// How long one call of the cleanup waits.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

/// What `tab list` showed.
struct Listing {
    /// The `targetId` of every page target listed.
    ids: HashSet<String>,
    /// Page targets with no `targetId` (the gate cannot tell them apart).
    nameless: usize,
    targets: Vec<Target>,
}

/// What was listed before the act.
pub(super) struct Watch {
    /// `None`: `tab list` could not be read.
    listing: Option<Listing>,
    /// The window of the bound tab: its id as Ardia gave it, and the profile it showed.
    window: Option<(Value, String)>,
    /// `windows` could not be read.
    windows_unreadable: bool,
    /// The `targetId` of the bound tab (from the bind reply): the opener a popup must name.
    opener: Option<String>,
}

/// What the gate did after the act, for the Mono's reply.
#[derive(Default)]
pub(super) struct Effects {
    /// The addresses (redacted, one line) of the new windows that were closed.
    pub closed: Vec<String>,
    /// A new window of this page that could not be closed.
    pub left_open: Vec<String>,
    /// New windows without an `appTabId` that are not known to be this page's: left alone.
    pub others: Vec<String>,
    /// Something that could not be checked or undone: a window may have been left open.
    pub unchecked: bool,
    /// The window showed another profile after the act and was put back.
    pub profile_restored: bool,
    /// The window showed another profile after the act and could not be put back.
    pub profile_stuck: bool,
}

impl Effects {
    /// Whether anything is left for the user to look at.
    pub fn needs_attention(&self) -> bool {
        !self.left_open.is_empty() || self.unchecked || self.profile_stuck
    }

    /// The lines of the Mono's reply about what the page did outside its tab.
    pub fn lines(&self) -> String {
        let mut out = String::new();
        for url in &self.closed {
            out.push_str(&format!(
                "- The page opened a new window ({url}); zptah closed it. Use browser_navigate with that address if you need it (it is checked like any address).\n"
            ));
        }
        for url in &self.left_open {
            out.push_str(&format!(
                "- The page opened a new window ({url}) that zptah could not close. Tell the user.\n"
            ));
        }
        for url in &self.others {
            out.push_str(&format!(
                "- Another new window ({url}) appeared that this page did not open, so zptah left it alone.\n"
            ));
        }
        if self.unchecked {
            out.push_str("- zptah could not check whether the page opened a new window, so a window may have been left open in Ardia. Tell the user.\n");
        }
        if self.profile_restored {
            out.push_str("- The click had switched the user's window to another profile; zptah put it back.\n");
        }
        if self.profile_stuck {
            out.push_str("- The user's window shows another profile than before and zptah could not put it back. Tell the user.\n");
        }
        out
    }
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
    /// `ardia browse` in the conversation's session that a Stop of the turn does not kill, with a
    /// short timeout: the cleanup after an act runs even when the turn was stopped.
    fn browse_quiet(&self, id: i64, cmd: &[&str], values: &[&str]) -> Result<Value, Failure> {
        self.controller
            .cli()
            .browse_timeout(&self.session(id), cmd, values, CLEANUP_TIMEOUT)
    }

    fn listing(&self, id: i64) -> Option<Listing> {
        let data = self.browse_quiet(id, &["tab", "list"], &[]).ok()?;
        let tabs = data["tabs"].as_array()?;
        let mut out = Listing {
            ids: HashSet::new(),
            nameless: 0,
            targets: Vec::new(),
        };
        for tab in tabs
            .iter()
            .filter(|t| t["type"].as_str().is_none_or(|ty| ty == "page"))
        {
            let app_tab = tab["appTabId"].as_str().is_some_and(|a| !a.is_empty());
            match tab["targetId"].as_str().filter(|t| !t.is_empty()) {
                Some(target) => {
                    out.ids.insert(target.to_owned());
                    out.targets.push(Target {
                        id: target.to_owned(),
                        app_tab,
                        url: tab["url"].as_str().unwrap_or("").to_owned(),
                    });
                }
                None => out.nameless += 1,
            }
        }
        Some(out)
    }

    /// `Target.getTargets`: the `openerId` of every target, by `targetId`.
    fn openers(&self, id: i64) -> Option<HashMap<String, String>> {
        let data = self
            .browse_quiet(id, &["cdp", "--target", "browser"], &["Target.getTargets"])
            .ok()?;
        let infos = data["result"]["targetInfos"].as_array()?;
        Some(
            infos
                .iter()
                .filter_map(|info| {
                    let target = info["targetId"].as_str()?;
                    Some((
                        target.to_owned(),
                        info["openerId"].as_str().unwrap_or("").to_owned(),
                    ))
                })
                .collect(),
        )
    }

    /// Every window with its tabs, or `None` when `windows` could not be read.
    fn windows_now(&self) -> Option<Value> {
        self.controller
            .cli()
            .call_without_params_timeout("windows", CLEANUP_TIMEOUT)
            .ok()
    }

    /// The window that holds `tab`: its id and the profile it shows now.
    fn window_in(windows: &Value, tab: &str) -> Option<(Value, String)> {
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
        let windows = self.windows_now();
        Watch {
            listing: self.listing(id),
            windows_unreadable: windows.is_none(),
            window: windows
                .as_ref()
                .and_then(|w| Self::window_in(w, tab))
                .filter(|(w, p)| !w.is_null() && !p.is_empty()),
            opener: self.conv(id, |c| c.targets.get(tab).cloned()),
        }
    }

    pub(super) fn watch_after(&self, id: i64, tab: &str, before: Watch) -> Effects {
        let mut out = Effects::default();
        // What was listed before is what a new target is compared with: when it could not be
        // read, nothing can be told apart and a window may have been left open.
        let mut new_target = false;
        match &before.listing {
            None => out.unchecked = true,
            Some(seen) => match self.listing(id) {
                None => out.unchecked = true,
                Some(now) => {
                    if now.nameless > seen.nameless {
                        // A new target with no id: it cannot be closed or told apart.
                        out.unchecked = true;
                    }
                    let fresh: Vec<&Target> = now
                        .targets
                        .iter()
                        .filter(|t| !seen.ids.contains(&t.id))
                        .collect();
                    new_target = !fresh.is_empty() || now.nameless > seen.nameless;
                    // A target that has an `appTabId` is a tab (the user's, or ours): never closed.
                    let loose: Vec<&Target> = fresh.into_iter().filter(|t| !t.app_tab).collect();
                    if !loose.is_empty() {
                        self.close_popups(id, &before, &loose, &mut out);
                    }
                }
            },
        }
        self.restore_profile(id, tab, &before, new_target, &mut out);
        if before.windows_unreadable {
            out.unchecked = true;
        }
        if out.needs_attention() {
            self.audit(
                id,
                "popup_unchecked",
                "",
                "a window the page opened may have been left open, or the window profile could not be put back",
            );
        }
        out
    }

    /// Closes the new targets that this page opened (their `openerId` is the bound tab); the
    /// others are reported and left alone.
    fn close_popups(&self, id: i64, before: &Watch, loose: &[&Target], out: &mut Effects) {
        let openers = self.openers(id);
        for target in loose {
            let shown = defense::redact_url_secrets(&one_line(&target.url));
            let ours = match (&openers, &before.opener) {
                (Some(openers), Some(opener)) => {
                    openers.get(&target.id).is_some_and(|o| o == opener)
                }
                _ => false,
            };
            if !ours {
                if openers.is_none() {
                    // Who opened it could not be read: it may be this page's.
                    out.unchecked = true;
                }
                out.others.push(shown);
                continue;
            }
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

    /// Puts the window back on the profile it showed, only when it now shows the Mono's profile,
    /// showed another one before and a new target appeared (the popup is what switches it).
    fn restore_profile(
        &self,
        id: i64,
        tab: &str,
        before: &Watch,
        new_target: bool,
        out: &mut Effects,
    ) {
        let Some((window, profile)) = &before.window else {
            return;
        };
        let mono_profile = self.config().profile;
        if *profile == mono_profile || !new_target {
            return;
        }
        let Some(windows) = self.windows_now() else {
            out.profile_stuck = true;
            return;
        };
        let Some((now_window, now_profile)) = Self::window_in(&windows, tab) else {
            return;
        };
        if now_window != *window || now_profile != mono_profile {
            return;
        }
        let params = json!({"window": window, "profile": profile});
        if self
            .controller
            .cli()
            .call_timeout("windows.switchProfile", &params, CLEANUP_TIMEOUT)
            .is_ok()
        {
            out.profile_restored = true;
            self.audit(
                id,
                "window_profile_restored",
                "",
                &format!(
                    "the act switched the window to profile {now_profile}; put back on {profile}"
                ),
            );
        } else {
            out.profile_stuck = true;
        }
    }

    /// `ardia browse ... cdp Target.closeTarget`, with the gate's own words only.
    fn close_target(&self, id: i64, target: &str) -> bool {
        let param = format!("targetId={target}");
        self.browse_quiet(
            id,
            &["cdp", "--param", &param, "--target", "browser"],
            &["Target.closeTarget"],
        )
        .is_ok_and(|data| data["result"]["success"] == true)
    }
}
