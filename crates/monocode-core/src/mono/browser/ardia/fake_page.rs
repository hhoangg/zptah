//! The page the fake `ardia` shows to the act tools: elements with refs and attributes, forms,
//! and the replies of `snapshot -i`, `get attr`, `form`, `get title` and the act commands, built
//! from the recorded replies in `fixtures/` (shapes are kept exact; the values are the test's).
//!
//! Not measured against a real Ardia (marked where used): the reply of `check`, `uncheck` and
//! `hover` on success (the key that names the act is a guess: the gate does not read it), and
//! `get attr` on a form after the first (`form:nth-of-type(N)`).
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
    /// The reply of the `form` command (`None`: the command refuses).
    pub form: Option<Value>,
    /// The ref marked `[active]` in the snapshot.
    pub focus: Option<String>,
    pub epoch: u64,
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
                    .map(|e| {
                        let active = if self.page.focus.as_deref() == Some(&e.r) {
                            "active, "
                        } else {
                            ""
                        };
                        format!("- {} \"{}\" [{active}ref={}]", e.role, e.name, e.r)
                    })
                    .collect();
                let origin = self
                    .snapshot_origin
                    .clone()
                    .unwrap_or_else(|| self.url.clone());
                let epoch = self.page.epoch;
                Some(with_data(SNAPSHOT_I, move |d| {
                    d["origin"] = json!(origin);
                    d["refs"] = Value::Object(refs);
                    d["snapshot"] = json!(lines.join("\n"));
                    d["epoch"] = json!(epoch);
                }))
            }
            (["get", "attr"], [target, name]) => Some(self.get_attr(target, name)),
            (["tab", "list"], []) if self.user_tab.is_some() => {
                // Own tabs first (not active), then the user's active tab.
                let (id, url, title) = self.user_tab.clone().unwrap();
                let mut tabs: Vec<Value> = self
                    .tabs
                    .iter()
                    .map(|t| json!({"tabId": "t1", "appTabId": t, "title": "Mono tab", "url": self.url, "type": "page", "active": false, "openedBySession": false}))
                    .collect();
                tabs.push(json!({"tabId": "t9", "appTabId": id, "title": title, "url": url, "type": "page", "active": true, "openedBySession": false}));
                Some(ok(
                    json!({"success": true, "data": {"tabs": tabs}, "error": null}),
                ))
            }
            (["form"], []) => Some(match &self.page.form {
                Some(form) => with_data(FORM, |d| *d = form.clone()),
                None => refused(
                    r#"{"success":false,"data":null,"error":"No form on the page","code":"command_failed"}"#,
                ),
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
                Some(self.press(key))
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
        let value = if target == "form" {
            self.page.forms.first().cloned()
        } else if let Some(n) = target
            .strip_prefix("form:nth-of-type(")
            .and_then(|r| r.strip_suffix(')'))
            .and_then(|n| n.parse::<usize>().ok())
        {
            // Not measured: the selector of a later form.
            self.page.forms.get(n - 1).cloned()
        } else {
            match self.element(target) {
                Ok(el) => match el.attrs.get(name) {
                    Some(v) => Some(Some(v.clone())),
                    None => {
                        return refused(
                            r#"{"success":false,"data":null,"error":"No such attribute","code":"command_failed"}"#,
                        )
                    }
                },
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
        let url = self.url.clone();
        match cmd {
            "click"
                if el.attrs.contains_key("navigates_to")
                    || (el.role == "link" && el.attrs.contains_key("href")) =>
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
            "click" => with_data(CLICK_ADD, |d| {
                d["clicked"] = json!(target);
                // The recorded click inserted a button `e12`; only an element that does so here.
                if !el.attrs.contains_key("adds") {
                    d["changes"]["added"] = json!([]);
                }
            }),
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
            "select" => with_data(SELECT, |d| d["selected"] = json!(rest)),
            "upload" => with_data(UPLOAD, |d| d["uploaded"] = json!(rest)),
            // Not measured: the key that names a successful check, uncheck or hover.
            other => with_data(PRESS_TAB, |d| {
                let changes = d["changes"].clone();
                *d = json!({ (match other { "check" => "checked", "uncheck" => "unchecked", _ => "hovered" }): target, "changes": changes });
            }),
        }
    }

    fn press(&mut self, key: &str) -> Result<Output, RunFailure> {
        if matches!(key, "Enter" | "NumpadEnter") {
            if let Some(to) = self.enter_to.clone() {
                let from = self.url.clone();
                self.url = to.clone();
                return with_data(PRESS_ENTER, |d| {
                    d["pressed"] = json!(key);
                    d["changes"]["url"] = json!({"from": from, "to": to});
                    d["changes"]["network"][0]["url"] = json!("https://shop.example/session");
                });
            }
        }
        with_data(PRESS_TAB, |d| d["pressed"] = json!(key))
    }
}
