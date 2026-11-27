//! What the gate reads from the page right before an act tool, and how it turns that into the
//! inputs of the risk classifier (design spec section 6, "Act tools: fresh read, then act").
//!
//! Everything here is pure: the replies of `snapshot -i`, `get attr`, `form` and `get url` come
//! in as JSON and the classifier's `ElementSignals` and `FormSignals` come out. Fidelity is lower
//! than the extension's live DOM facts, so an incomplete read is reported (`complete: false`)
//! and the caller leans to asking.
use crate::mono::browser::risk::normalize::normalize;
use crate::mono::browser::risk::signals::{
    classify_field, extract_amounts, ElementSignals, FieldAttrs, FieldKind, FormField, FormSignals,
};
use serde_json::Value;
use std::collections::HashMap;

/// The exact sentence for a ref that no longer points at what the Mono saw.
pub const CHANGED: &str = "the element changed, take a new snapshot";

/// `e12`, `@e12` and `[ref=e12]` all name the ref `e12`.
pub fn parse_ref(raw: &str) -> Result<String, String> {
    let bad = || {
        format!(
            "`ref` must be an element ref from a browser_snapshot, such as e12 (got `{}`).",
            raw.chars().take(40).collect::<String>()
        )
    };
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix("[ref=")
        .and_then(|r| r.strip_suffix(']'))
        .unwrap_or(trimmed);
    let inner = inner.strip_prefix('@').unwrap_or(inner);
    let digits = inner.strip_prefix('e').ok_or_else(bad)?;
    if digits.is_empty() || digits.len() > 7 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    Ok(inner.to_owned())
}

/// The refs (role and name) of a `snapshot` reply's `data.refs`.
pub fn refs_of(data: &Value) -> HashMap<String, (String, String)> {
    data["refs"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, entry)| {
            (
                key.clone(),
                (
                    entry["role"].as_str().unwrap_or("").to_owned(),
                    entry["name"].as_str().unwrap_or("").to_owned(),
                ),
            )
        })
        .collect()
}

/// The refs of the snapshot the Mono took last on a tab, and the page they came from.
#[derive(Clone, Debug, Default)]
pub struct SeenRefs {
    /// The tab URL when the snapshot was taken (an act on another URL is refused).
    pub url: String,
    /// Ardia's `epoch` of the snapshot: it grows with every navigation, so refs of an older epoch
    /// are dropped when a newer snapshot arrives. Refs of one epoch accumulate (a scoped snapshot
    /// adds to a full one: refs survive a re-snapshot).
    pub epoch: Option<u64>,
    pub refs: HashMap<String, (String, String)>,
}
impl SeenRefs {
    /// Records a snapshot taken at `url`.
    pub fn note(&mut self, url: &str, epoch: Option<u64>, refs: HashMap<String, (String, String)>) {
        if self.epoch != epoch || self.url != url {
            self.refs.clear();
        }
        self.url = url.to_owned();
        self.epoch = epoch;
        self.refs.extend(refs);
    }
}

/// The line of a snapshot whose element has the focus, if the snapshot marks one (`[active]` or
/// `[focused]` next to `ref=eN`). Not measured against the real command: when nothing is marked
/// the focus is unknown, and callers treat unknown as unsafe.
pub fn focused_ref(snapshot: &str) -> Option<String> {
    snapshot.lines().find_map(|line| {
        let open = line.rfind('[')?;
        let attrs = &line[open..];
        let focused = attrs
            .trim_matches(|c| c == '[' || c == ']')
            .split(',')
            .map(str::trim)
            .any(|a| a == "active" || a == "focused");
        let ref_ = attrs
            .trim_matches(|c| c == '[' || c == ']')
            .split(',')
            .map(str::trim)
            .find_map(|a| a.strip_prefix("ref="))?;
        focused.then(|| ref_.to_owned())
    })
}

/// What `get attr` told about the target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    /// `type` of an input or button. `None`: not read (or the element has none).
    pub input_type: Option<String>,
    pub autocomplete: Option<String>,
    pub href: Option<String>,
    pub formaction: Option<String>,
}

/// The value of a `get attr` reply (`{attribute, value}`): `None` for an absent or empty one.
pub fn attr_value(data: &Value) -> Option<String> {
    data["value"]
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// One field of the `form` command's reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormFieldRead {
    pub ref_: String,
    pub role: String,
    pub label: String,
    pub input_type: String,
    pub name: String,
    pub filled: bool,
    pub value: String,
}

/// The `form` command's reply: the page's fields and submit buttons.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormRead {
    /// The reply was read. `false`: the command failed, so nothing is known about the form.
    pub known: bool,
    pub fields: Vec<FormFieldRead>,
    /// Submit buttons: ref and label.
    pub submits: Vec<(String, String)>,
}
impl FormRead {
    pub fn parse(data: &Value) -> Self {
        let fields = data["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| {
                let text = |key: &str| f[key].as_str().unwrap_or("").to_owned();
                let value = text("value");
                FormFieldRead {
                    ref_: text("ref"),
                    role: text("role"),
                    label: text("label"),
                    input_type: text("type"),
                    name: text("name"),
                    filled: !value.is_empty() || f["checked"] == true,
                    value,
                }
            })
            .collect();
        let submits = data["submits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| {
                (
                    s["ref"].as_str().unwrap_or("").to_owned(),
                    s["label"].as_str().unwrap_or("").to_owned(),
                )
            })
            .collect();
        Self {
            known: true,
            fields,
            submits,
        }
    }
    pub fn is_submit(&self, ref_: &str) -> bool {
        self.submits.iter().any(|(r, _)| r == ref_)
    }
    pub fn field(&self, ref_: &str) -> Option<&FormFieldRead> {
        self.fields.iter().find(|f| f.ref_ == ref_)
    }
    fn kind_of(field: &FormFieldRead) -> FieldKind {
        classify_field(&FieldAttrs {
            input_type: field.input_type.clone(),
            name: field.name.clone(),
            label: field.label.clone(),
            ..Default::default()
        })
    }
    /// Whether any field is a password or a one-time code (by its type or its words).
    pub fn has_secret_field(&self) -> bool {
        self.fields
            .iter()
            .any(|f| matches!(Self::kind_of(f), FieldKind::Password | FieldKind::Otp))
    }
    pub fn signals(&self) -> FormSignals {
        let recipients = self
            .fields
            .iter()
            .filter(|f| Self::kind_of(f) == FieldKind::Email && !f.value.trim().is_empty())
            .map(|f| f.value.trim().to_owned())
            .collect();
        FormSignals {
            fields: self
                .fields
                .iter()
                .map(|f| FormField {
                    kind: Self::kind_of(f),
                    label: f.label.clone(),
                    filled: f.filled,
                })
                .collect(),
            recipients,
            conversation_title: None,
            visibility: None,
        }
    }
}

/// The addresses forms of the page send to, resolved against the page address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormActions {
    pub urls: Vec<String>,
    /// Every form was read. `false`: a form's action could not be read or resolved, so the gate
    /// can not say where a submit goes.
    pub complete: bool,
}

/// `raw` (as the page wrote it) against the page address.
pub fn resolve(base: &str, raw: &str) -> Option<String> {
    url::Url::parse(base)
        .ok()?
        .join(raw.trim())
        .ok()
        .map(|u| u.to_string())
}

/// Why a field must never be typed into, if it must not: by what the page says it is
/// (`type=password`, `autocomplete` of a password or a one-time code), then by its name or label
/// (the risk lexicon: password, passcode, PIN, one-time code, 2FA and their Vietnamese forms). No
/// approval overrides it.
pub fn secret_field(name: &str, attrs: &Attrs) -> Option<&'static str> {
    let ty = attrs.input_type.as_deref().unwrap_or("").to_lowercase();
    let ac = attrs.autocomplete.as_deref().unwrap_or("").to_lowercase();
    let ac_has = |token: &str| ac.split_whitespace().any(|t| t == token);
    if ty == "password"
        || ac_has("current-password")
        || ac_has("new-password")
        || ac_has("one-time-code")
    {
        return Some("typing into a password or one-time code field is never allowed");
    }
    let by_name = classify_field(&FieldAttrs {
        input_type: ty,
        autocomplete: ac,
        name: name.to_owned(),
        label: name.to_owned(),
        ..Default::default()
    });
    matches!(by_name, FieldKind::Password | FieldKind::Otp)
        .then_some("this field is named like a password or a one-time code, which is never typed")
}

/// The kind of a field for the classifier (card, ID, bank and the rest; a secret is refused before
/// this is used).
pub fn field_kind(name: &str, attrs: &Attrs) -> FieldKind {
    classify_field(&FieldAttrs {
        input_type: attrs.input_type.clone().unwrap_or_default(),
        autocomplete: attrs.autocomplete.clone().unwrap_or_default(),
        name: name.to_owned(),
        label: name.to_owned(),
        ..Default::default()
    })
}

const LOGIN_PHRASES: [&str; 12] = [
    "login",
    "log in",
    "logon",
    "sign in",
    "signin",
    "sso",
    "dang nhap",
    "dangnhap",
    "authenticate",
    "verify your identity",
    "xac minh",
    "xac thuc",
];

fn has_phrase(text: &str, phrase: &str) -> bool {
    let plain = normalize(text).joined(true);
    format!(" {plain} ").contains(&format!(" {phrase} "))
}

/// Whether the page looks like a sign-in page: the address, the title or the form says so, or the
/// form has a password or one-time code field.
pub fn login_like(url: &str, title: &str, form: &FormRead) -> bool {
    let path_hit = url::Url::parse(url).is_ok_and(|u| {
        u.path()
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .any(|seg| {
                matches!(
                    seg,
                    "login" | "logon" | "signin" | "sso" | "auth" | "session" | "sessions"
                )
            })
    });
    path_hit
        || LOGIN_PHRASES.iter().any(|p| has_phrase(title, p))
        || form
            .submits
            .iter()
            .any(|(_, label)| LOGIN_PHRASES.iter().any(|p| has_phrase(label, p)))
        || form.has_secret_field()
}

/// A key that types a character: one character, or a chord ending in one (`Control+v`, `a`).
pub fn is_printable_key(key: &str) -> bool {
    let last = key.rsplit('+').next().unwrap_or(key);
    if key == "+" {
        return true;
    }
    last.chars().count() == 1 || last == "Space"
}

/// Whether a key presses the focused control or submits its form.
pub fn is_submit_key(key: &str) -> bool {
    matches!(key, "Enter" | "NumpadEnter" | "Return" | " " | "Space")
}

/// What the classifier sees of the target element.
pub struct ElementInput<'a> {
    pub role: &'a str,
    pub name: &'a str,
    pub attrs: &'a Attrs,
    pub form: &'a FormRead,
    pub ref_: &'a str,
    /// The page address (to resolve the link).
    pub page_url: &'a str,
}

pub fn element_signals(input: &ElementInput<'_>) -> ElementSignals {
    let role = input.role.to_lowercase();
    let is_submit = input.form.is_submit(input.ref_)
        || matches!(
            input
                .attrs
                .input_type
                .as_deref()
                .map(str::to_lowercase)
                .as_deref(),
            Some("submit" | "image")
        );
    let href = (role == "link")
        .then(|| {
            input
                .attrs
                .href
                .as_deref()
                .and_then(|h| resolve(input.page_url, h))
        })
        .flatten();
    let input_type = input
        .attrs
        .input_type
        .clone()
        .or_else(|| (role == "checkbox").then(|| "checkbox".to_owned()));
    ElementSignals {
        amounts: extract_amounts(input.name),
        role,
        name: input.name.to_owned(),
        text: String::new(),
        tag: String::new(),
        input_type,
        autocomplete: input.attrs.autocomplete.clone(),
        input_name: input.form.field(input.ref_).map(|f| f.name.clone()),
        label: Some(input.name.to_owned()).filter(|n| !n.is_empty()),
        href,
        form_action: input
            .attrs
            .formaction
            .as_deref()
            .and_then(|a| resolve(input.page_url, a)),
        is_submit,
        ..Default::default()
    }
}

/// The POSTs (and other writes) the act caused, from `changes.network`: `METHOD host/path`.
pub fn writes_of(changes: &Value) -> Vec<String> {
    changes["network"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let method = n["method"].as_str()?.to_uppercase();
            if matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS") {
                return None;
            }
            let url = n["url"].as_str().unwrap_or("");
            let shown = crate::mono::browser::gate::audit_url(url);
            Some(if shown.is_empty() {
                method
            } else {
                format!("{method} {shown}")
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refs_are_read_in_every_written_form_and_nothing_else() {
        for ok in ["e12", "@e12", " @e12 ", "[ref=e12]"] {
            assert_eq!(parse_ref(ok).unwrap(), "e12", "{ok}");
        }
        for bad in [
            "",
            "12",
            "e",
            "e1x",
            "--json",
            "@e",
            "e-1",
            "button",
            "e12345678",
        ] {
            assert!(parse_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn printable_keys_are_single_characters_and_chords_ending_in_one() {
        for key in ["a", "A", "1", "-", "+", "Control+v", "Meta+a", "Space", "é"] {
            assert!(is_printable_key(key), "{key}");
        }
        for key in [
            "Enter",
            "Tab",
            "Escape",
            "ArrowDown",
            "Backspace",
            "F5",
            "Shift+Tab",
        ] {
            assert!(!is_printable_key(key), "{key}");
        }
    }

    #[test]
    fn a_secret_field_is_found_by_type_autocomplete_and_name() {
        let attrs = |ty: Option<&str>, ac: Option<&str>| Attrs {
            input_type: ty.map(str::to_owned),
            autocomplete: ac.map(str::to_owned),
            ..Default::default()
        };
        assert!(secret_field("Secret", &attrs(Some("password"), None)).is_some());
        for ac in [
            "current-password",
            "new-password",
            "one-time-code",
            "section-x one-time-code",
        ] {
            assert!(
                secret_field("Code", &attrs(Some("text"), Some(ac))).is_some(),
                "{ac}"
            );
        }
        for name in [
            "Password",
            "Mật khẩu",
            "Mã OTP",
            "Verification code",
            "Mã PIN",
            "2FA code",
        ] {
            assert!(secret_field(name, &attrs(None, None)).is_some(), "{name}");
        }
        assert!(secret_field("Email", &attrs(Some("email"), Some("username"))).is_none());
        assert!(secret_field("Field X", &attrs(Some("text"), None)).is_none());
    }

    #[test]
    fn the_focused_line_of_a_snapshot_is_found() {
        let snapshot = "- textbox \"Email\" [ref=e3]\n- textbox \"Secret\" [active, ref=e4]\n- button \"Go\" [ref=e7]";
        assert_eq!(focused_ref(snapshot).as_deref(), Some("e4"));
        assert_eq!(focused_ref("- textbox \"Email\" [ref=e3]"), None);
    }

    #[test]
    fn a_login_page_is_found_by_address_title_form_or_secret_field() {
        let none = FormRead::default();
        assert!(login_like(
            "https://a.example/users/login",
            "Welcome",
            &none
        ));
        assert!(login_like("https://a.example/", "Sign in - Test", &none));
        assert!(login_like(
            "https://a.example/",
            "Đăng nhập tài khoản",
            &none
        ));
        assert!(!login_like(
            "https://a.example/blog",
            "Shipping notes",
            &none
        ));
        let form = FormRead::parse(
            &json!({"fields": [{"ref": "e4", "role": "textbox", "label": "Secret", "type": "password", "name": "pw", "value": ""}], "submits": []}),
        );
        assert!(login_like("https://a.example/", "Home", &form));
    }

    #[test]
    fn form_actions_resolve_against_the_page_address() {
        assert_eq!(
            resolve("https://shop.example/cart/view", "/session").as_deref(),
            Some("https://shop.example/session")
        );
        assert_eq!(
            resolve("https://shop.example/a/b", "pay").as_deref(),
            Some("https://shop.example/a/pay")
        );
        assert_eq!(resolve("not a url", "/x"), None);
    }
}
