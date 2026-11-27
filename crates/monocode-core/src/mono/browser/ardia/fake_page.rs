//! The page the fake `ardia` shows to the act tools: elements with refs and attributes, forms,
//! and the replies of `snapshot -i`, `get attr`, `form`, `get title` and the act commands, built
//! from the recorded replies in `fixtures/` (shapes are kept exact; the values are the test's).
//!
//! Measured on a real Ardia (2026-11-28) and kept here: `snapshot` never marks the focus (the gate
//! asks with one fixed `eval`); `get count form` and `get attr "form >> nth=i" action` read the
//! forms; an absent attribute is `{"attribute":..,"value":null}` with exit 0; `check` on a button
//! clicks it and only then fails; `press @eN KEY` presses in that element; with no form `form`
//! answers `count: 0` and no `submits`; with an open dialog `snapshot -i` is scoped to it and says
//! so in `modal`, and the full `snapshot` shows its text.
//!
//! Also measured and kept: `eval` is always sent with `--isolated`; `press @eN Enter` and
//! `Control+Enter` answer `{pressed, in, changes}`; with a dialog open the full snapshot nests
//! its text under the `- dialog` line; a `target=_blank` click opens a target no tab lists.
//!
//! Not measured against a real Ardia (marked where used): the reply of `hover` on success (the
//! key that names the act is a guess: the gate does not read it).
use super::runner::{Output, RunFailure};
use super::tests::{json_of, State};
use serde_json::{json, Value};
use std::collections::HashMap;

const CLICK_ADD: &str = include_str!("fixtures/act_click_add.json");
const CLICK_LINK: &str = include_str!("fixtures/act_click_link.json");
const TYPE: &str = include_str!("fixtures/act_type.json");
const FILL_PASSWORD: &str = include_str!("fixtures/act_fill_password.json");
const PRESS_TAB: &str = include_str!("fixtures/act_press_tab.json");
const PRESS_ENTER: &str = include_str!("fixtures/act_press_enter.json");
const SELECT: &str = include_str!("fixtures/act_select.json");
const UPLOAD: &str = include_str!("fixtures/act_upload.json");
const STALE: &str = include_str!("fixtures/browse_stale_ref.json");
const UNKNOWN_REF: &str = include_str!("fixtures/act_unknown_ref.json");
const SUPERSEDED: &str = include_str!("fixtures/act_vocab_superseded.json");
const SUPERSEDED_NAVIGATED: &str = include_str!("fixtures/act_vocab_superseded_navigated.json");
const NOT_VISIBLE: &str = include_str!("fixtures/act_not_visible.json");
const SNAPSHOT_I: &str = include_str!("fixtures/browse_snapshot_interactive.json");
const GET_TITLE: &str = include_str!("fixtures/browse_get_title.json");
const GET_ATTR: &str = include_str!("fixtures/get_attr_type.json");
const FORM: &str = include_str!("fixtures/browse_form_login.json");
const CDP_CLOSE: &str = include_str!("fixtures/cdp_close_target.json");
const PRESS_IN_ENTER: &str = include_str!("fixtures/act_press_in_enter.json");
const PRESS_IN_CTRL_ENTER: &str = include_str!("fixtures/act_press_in_ctrl_enter.json");
const DAEMON_GONE: &str = include_str!("fixtures/browse_daemon_unreachable.json");

/// One element of the fake page.
#[derive(Clone, Debug, Default)]
pub(super) struct El {
    pub r: String,
    pub role: String,
    pub name: String,
    /// `type`, `autocomplete`, `href`, `formaction`, `name` ... as `get attr` reads them.
    pub attrs: HashMap<String, String>,
}
impl El {
    pub fn new(r: &str, role: &str, name: &str) -> Self {
        Self {
            r: r.into(),
            role: role.into(),
            name: name.into(),
            attrs: HashMap::new(),
        }
    }
    pub fn attr(mut self, key: &str, value: &str) -> Self {
        self.attrs.insert(key.into(), value.into());
        self
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Page {
    pub title: String,
    pub elements: Vec<El>,
    /// The `action` attribute of each form, in order (`None`: a form with no action).
    pub forms: Vec<Option<String>>,
    /// The reply of the `form` command (`None`: the page has no form: `count: 0`).
    pub form: Option<Value>,
    /// The `form` command is refused.
    pub form_refuses: bool,
    /// What the focus `eval` answers as an object (`None`: `{"tag":"BODY"}`).
    pub focus: Option<Value>,
    pub epoch: u64,
    /// The text the full `snapshot` (no `-i`) shows.
    pub full: Option<String>,
    /// The `modal` of `snapshot -i` (a dialog is open).
    pub modal: Option<Value>,
}

/// The focus `eval` result of a plain text input.
pub(super) fn focus_input(input_type: &str, name: Option<&str>, label: Option<&str>) -> Value {
    json!({"tag": "INPUT", "type": input_type, "ac": null, "name": name, "label": label, "text": null, "ce": false, "sh": false, "fi": null, "sb": null, "fa": null, "hr": null})
}

/// The focus `eval` result of a button (`form`: its index among the forms).
pub(super) fn focus_button(text: &str, form: Option<usize>) -> Value {
    json!({"tag": "BUTTON", "type": "submit", "ac": null, "name": null, "label": null, "text": text, "ce": false, "sh": false, "fi": form, "sb": text, "fa": null, "hr": null})
}

/// How a command that is made to fail fails.
#[derive(Clone, Copy, Debug)]
pub(super) enum FailKind {
    /// Ardia went away while the command ran (exit 2).
    Gone,
    Timeout,
    /// Exit 0 with a reply nobody can read.
    Garbage,
}

fn ok(value: Value) -> Result<Output, RunFailure> {
    Ok(Output {
        code: 0,
        stdout: value.to_string(),
        stderr: String::new(),
    })
}
fn refused(text: &str) -> Result<Output, RunFailure> {
    Ok(Output {
        code: 1,
        stdout: text.into(),
        stderr: String::new(),
    })
}
fn with_data(fixture: &str, edit: impl FnOnce(&mut Value)) -> Result<Output, RunFailure> {
    let mut reply = json_of(fixture);
    edit(&mut reply["data"]);
    ok(reply)
}

impl State {
    /// The reply to a `browse` command that deals with the fake page, or `None` for a command
    /// this module does not know.
    pub(super) fn page_reply(
        &mut self,
        cmd: &[&str],
        values: &[&str],
    ) -> Option<Result<Output, RunFailure>> {
        if self
            .on_cmd
            .as_ref()
            .is_some_and(|(name, _)| cmd.first() == Some(&name.as_str()))
        {
            if let Some((_, hook)) = self.on_cmd.take() {
                hook(self);
            }
        }
        if let Some((name, kind)) = self.fail_on.clone() {
            if cmd.first() == Some(&name.as_str()) {
                self.fail_on = None;
                self.act_sent
                    .push(cmd.iter().chain(values).map(|s| s.to_string()).collect());
                return Some(match kind {
                    FailKind::Gone => {
                        self.up = false;
                        Ok(Output {
                            code: 2,
                            stdout: DAEMON_GONE.into(),
                            stderr: String::new(),
                        })
                    }
                    FailKind::Timeout => Err(RunFailure::TimedOut),
                    FailKind::Garbage => Ok(Output {
                        code: 0,
                        stdout: "not json at all".into(),
                        stderr: String::new(),
                    }),
                });
            }
        }
        match (cmd, values) {
            (["get", "title"], []) => Some(with_data(GET_TITLE, |d| {
                d["title"] = json!(self.page.title.clone());
            })),
            (["snapshot", ..], []) if !self.page.elements.is_empty() => {
                let interactive = cmd.contains(&"-i");
                let refs: serde_json::Map<String, Value> = self
                    .page
                    .elements
                    .iter()
                    .map(|e| (e.r.clone(), json!({"name": e.name, "role": e.role})))
                    .collect();
                let lines: Vec<String> = self
                    .page
                    .elements
                    .iter()
                    .map(|e| format!("- {} \"{}\" [ref={}]", e.role, e.name, e.r))
                    .collect();
                let origin = self
                    .snapshot_origin
                    .clone()
                    .unwrap_or_else(|| self.url.clone());
                let epoch = self.page.epoch;
                let (full, modal) = (self.page.full.clone(), self.page.modal.clone());
                Some(with_data(SNAPSHOT_I, move |d| {
                    d["origin"] = json!(origin);
                    d["refs"] = Value::Object(refs);
                    d["snapshot"] = json!(lines.join("\n"));
                    d["epoch"] = json!(epoch);
                    match (interactive, full, modal) {
                        // The full snapshot shows the text of a dialog.
                        (false, Some(full), _) => d["snapshot"] = json!(full),
                        (true, _, Some(modal)) => d["modal"] = modal,
                        _ => {}
                    }
                }))
            }
            (["eval"], _) => panic!("an eval without --isolated: the page's own world can lie"),
            (["eval", "--isolated"], [script]) => {
                self.evals.push(script.to_string());
                assert_eq!(
                    *script,
                    super::upstream::FOCUS_SCRIPT,
                    "the only eval the gate sends is its own fixed focus script"
                );
                if self.focus_fails {
                    return Some(refused(
                        r#"{"success":false,"data":null,"error":"eval is unavailable","code":"command_failed"}"#,
                    ));
                }
                let forms = self.page.forms.len();
                let result = self.focus_raw.clone().unwrap_or_else(|| {
                    let mut focus = self
                        .page
                        .focus
                        .clone()
                        .unwrap_or_else(|| json!({"tag": "BODY"}));
                    // The script also reports `document.forms.length`.
                    if focus.get("fl").is_none() {
                        focus["fl"] = json!(forms);
                    }
                    focus.to_string()
                });
                Some(ok(
                    json!({"success": true, "data": {"origin": "file://", "result": result}, "error": null}),
                ))
            }
            (["cdp", "--target", "browser"], ["Target.getTargets"]) => {
                if self.targets_fail {
                    return Some(refused(
                        r#"{"success":false,"data":null,"error":"cdp is unavailable","code":"command_failed"}"#,
                    ));
                }
                // The shape measured on Ardia 0.7.0: `data.result.targetInfos[]`.
                let mut infos = vec![
                    json!({"targetId": super::tests::BOUND_TARGET, "type": "page", "title": "Page", "url": self.url, "attached": true}),
                    json!({"targetId": "BACKGROUNDPAGE0001", "type": "background_page", "title": "Extension", "url": "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/bg.html", "attached": false}),
                ];
                for (target, url) in &self.popups {
                    let opener = self
                        .popup_openers
                        .get(target)
                        .map_or(super::tests::BOUND_TARGET, String::as_str);
                    infos.push(json!({"targetId": target, "type": "page", "title": "Popup", "url": url, "attached": false, "openerId": opener, "canAccessOpener": true}));
                }
                for (target, _, url) in &self.new_tabs {
                    infos.push(json!({"targetId": target, "type": "page", "title": "Tab", "url": url, "attached": false, "openerId": super::tests::BOUND_TARGET}));
                }
                Some(ok(
                    json!({"success": true, "data": {"method": "Target.getTargets", "target": "browser", "session": "attached", "result": {"targetInfos": infos}}, "error": null}),
                ))
            }
            (["cdp", "--param", param, "--target", "browser"], ["Target.closeTarget"]) => {
                let target = param.strip_prefix("targetId=").unwrap_or("").to_owned();
                self.target_closes.push(target.clone());
                if self.close_target_fails {
                    return Some(refused(
                        r#"{"success":false,"data":null,"error":"No target with that id","code":"command_failed"}"#,
                    ));
                }
                self.popups.retain(|(id, _)| *id != target);
                Some(ok(json_of(CDP_CLOSE)))
            }
            (["get", "count"], ["form"]) => Some(ok(
                json!({"success": true, "data": {"count": self.page.forms.len(), "selector": "form"}, "error": null}),
            )),
            (["get", "attr"], [target, name]) => Some(self.get_attr(target, name)),
            (["form"], []) => Some(if self.page.form_refuses {
                refused(
                    r#"{"success":false,"data":null,"error":"No form on the page","code":"command_failed"}"#,
                )
            } else {
                match &self.page.form {
                    Some(form) => with_data(FORM, |d| *d = form.clone()),
                    // No form: a count of 0 and no `submits` key (measured).
                    None => ok(
                        json!({"success": true, "data": {"url": self.url, "count": 0, "more": false, "stopped": null, "fields": []}, "error": null}),
                    ),
                }
            }),
            (
                ["click" | "check" | "uncheck" | "hover" | "fill" | "type" | "select" | "upload"],
                [target, ..],
            ) => {
                self.act_sent
                    .push(cmd.iter().chain(values).map(|s| s.to_string()).collect());
                Some(self.act(cmd[0], target, &values[1..]))
            }
            (["press"], [key]) => {
                self.act_sent.push(vec!["press".into(), key.to_string()]);
                Some(self.press(key, None))
            }
            (["press"], [target, key]) => {
                self.act_sent
                    .push(vec!["press".into(), target.to_string(), key.to_string()]);
                if let Err(reply) = self.element(target) {
                    return Some(reply);
                }
                Some(self.press(key, Some(target)))
            }
            _ => None,
        }
    }

    fn element(&self, target: &str) -> Result<El, Result<Output, RunFailure>> {
        let r = target.trim_start_matches('@');
        if let Some(code) = self.broken.get(r) {
            return Err(match *code {
                "vocab_superseded" => ok(json_of(SUPERSEDED)),
                "vocab_superseded_navigated" => ok(json_of(SUPERSEDED_NAVIGATED)),
                "not_visible" => ok(json_of(NOT_VISIBLE)),
                "unknown_ref" => ok(json_of(UNKNOWN_REF)),
                _ => ok(json_of(STALE)),
            }
            .map(|mut o| {
                o.code = 1;
                o
            }));
        }
        self.page
            .elements
            .iter()
            .find(|e| e.r == r)
            .cloned()
            .ok_or_else(|| {
                ok(json_of(STALE)).map(|mut o| {
                    o.code = 1;
                    o
                })
            })
    }

    fn get_attr(&mut self, target: &str, name: &str) -> Result<Output, RunFailure> {
        if self.attr_fail.iter().any(|a| a == name) {
            return refused(
                r#"{"success":false,"data":null,"error":"The attribute could not be read","code":"command_failed"}"#,
            );
        }
        let value = if target == "form" {
            self.page.forms.first().cloned()
        } else if let Some(n) = target
            .strip_prefix("form >> nth=")
            .and_then(|n| n.parse::<usize>().ok())
        {
            // Measured: `form:nth-of-type(N)` does not find the Nth form; `form >> nth=N` does.
            self.page.forms.get(n).cloned()
        } else {
            match self.element(target) {
                // An absent attribute is a reply with `value: null`, not a refusal (measured).
                Ok(el) => Some(el.attrs.get(name).cloned()),
                Err(reply) => return reply,
            }
        };
        match value {
            Some(v) => with_data(GET_ATTR, |d| {
                d["attribute"] = json!(name);
                d["value"] = json!(v);
            }),
            None => refused(
                r#"{"success":false,"data":null,"error":"No element matches the selector","code":"command_failed"}"#,
            ),
        }
    }

    fn act(&mut self, cmd: &str, target: &str, rest: &[&str]) -> Result<Output, RunFailure> {
        let el = match self.element(target) {
            Ok(el) => el,
            Err(reply) => return reply,
        };
        if let Some(text) = self.act_error.take() {
            return refused(
                &json!({"success": false, "data": null, "error": text, "code": "command_failed"})
                    .to_string(),
            );
        }
        let url = self.url.clone();
        match cmd {
            "click"
                if !el.attrs.contains_key("target")
                    && (el.attrs.contains_key("navigates_to")
                        || (el.role == "link"
                            && el.attrs.get("href").is_some_and(|h| h.starts_with("http")))) =>
            {
                let to = el
                    .attrs
                    .get("navigates_to")
                    .or_else(|| el.attrs.get("href"))
                    .cloned()
                    .unwrap();
                self.url = to.clone();
                if let Some(next) = self.pages.get(&to) {
                    self.page = next.clone();
                } else {
                    self.page.epoch += 1;
                    self.page.elements.clear();
                }
                with_data(CLICK_LINK, |d| {
                    d["clicked"] = json!(target);
                    d["changes"]["url"] = json!({"from": url, "to": to});
                })
            }
            "click" => {
                // Measured: a `target=_blank` link (or `window.open`) opens a page target that
                // `tab list` shows with no `appTabId`, `changes` says nothing, and the user's
                // window switches to the Mono's profile.
                if el.attrs.contains_key("target") {
                    self.popup_clicks += 1;
                }
                if el.attrs.contains_key("target") || el.attrs.contains_key("opens_popup") {
                    let url = el
                        .attrs
                        .get("href")
                        .or_else(|| el.attrs.get("opens_popup"))
                        .cloned()
                        .unwrap_or_default();
                    let n = self.popups.len() + self.target_closes.len() + 1;
                    self.popups.push((format!("POPUP{n:04}"), url));
                    self.window_profile = super::tests::PROFILE.to_owned();
                }
                with_data(CLICK_ADD, |d| {
                    d["clicked"] = json!(target);
                    // The recorded click inserted a button `e12`; only an element that does so here.
                    if !el.attrs.contains_key("adds") {
                        d["changes"]["added"] = json!([]);
                    }
                })
            }
            "fill"
                if el.attrs.get("type").map(String::as_str) == Some("password")
                    || el.attrs.contains_key("hidden_password") =>
            {
                with_data(FILL_PASSWORD, |d| d["filled"] = json!(target))
            }
            "fill" => with_data(TYPE, |d| {
                let changes = d["changes"].clone();
                *d = json!({"filled": target, "changes": changes});
            }),
            "type" => with_data(TYPE, |d| d["typed"] = json!(target)),
            // Measured: `check` on a button CLICKS it (the page script ran) and only then fails.
            "check" | "uncheck"
                if !matches!(
                    el.role.as_str(),
                    "checkbox" | "radio" | "switch" | "menuitemcheckbox" | "menuitemradio"
                ) =>
            {
                self.misclicks += 1;
                refused(&format!(
                    r#"{{"success":false,"data":null,"error":"Clicking {target} didn't check it","code":"command_failed"}}"#
                ))
            }
            "select" => with_data(SELECT, |d| d["selected"] = json!(rest)),
            "upload" => with_data(UPLOAD, |d| d["uploaded"] = json!(rest)),
            // Not measured: the key that names a successful check, uncheck or hover.
            other => with_data(PRESS_TAB, |d| {
                let changes = d["changes"].clone();
                *d = json!({ (match other { "check" => "checked", "uncheck" => "unchecked", _ => "hovered" }): target, "changes": changes });
            }),
        }
    }

    fn press(&mut self, key: &str, target: Option<&str>) -> Result<Output, RunFailure> {
        let enter = key
            .rsplit('+')
            .next()
            .is_some_and(|k| matches!(k, "Enter" | "NumpadEnter" | "Return"));
        if enter {
            if let Some(to) = self.enter_to.clone() {
                let from = self.url.clone();
                self.url = to.clone();
                return with_data(PRESS_ENTER, |d| {
                    d["pressed"] = json!(key);
                    if let Some(target) = target {
                        d["in"] = json!(target);
                    }
                    d["changes"]["url"] = json!({"from": from, "to": to});
                    d["changes"]["network"][0]["url"] = json!("https://shop.example/session");
                    if !self.posts.is_empty() {
                        d["changes"]["network"] = Value::Array(
                            self.posts
                                .iter()
                                .map(|u| json!({"method": "POST", "url": u, "type": "Fetch", "statuses": [], "status": 200, "failed": null}))
                                .collect(),
                        );
                    }
                });
            }
        }
        match (target, key) {
            // Measured: a key pressed in an element reports where it landed.
            (Some(target), "Enter") => with_data(PRESS_IN_ENTER, |d| d["in"] = json!(target)),
            (Some(target), "Control+Enter") => {
                with_data(PRESS_IN_CTRL_ENTER, |d| d["in"] = json!(target))
            }
            _ => with_data(PRESS_TAB, |d| {
                d["pressed"] = json!(key);
                if let Some(target) = target {
                    d["in"] = json!(target);
                }
            }),
        }
    }
}
