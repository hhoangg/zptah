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

/// What `get attr` told about the target, and what the page's `form` command says about the same
/// field (its `name` and `label`). A value that could not be read is `None`; `*_unreadable` says
/// the command itself was refused (not that the attribute is absent), which the caller must not
/// treat as "harmless".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    /// `type` of an input or button. `None`: not read (or the element has none).
    pub input_type: Option<String>,
    pub autocomplete: Option<String>,
    pub href: Option<String>,
    pub formaction: Option<String>,
    /// The `target` of a link (`_blank`, a window name ...).
    pub target: Option<String>,
    /// The `name` attribute of the field (from `form`).
    pub field_name: Option<String>,
    /// The `label` of the field (from `form`).
    pub field_label: Option<String>,
    pub field_id: Option<String>,
    pub placeholder: Option<String>,
    /// `get attr` of these was refused, so nothing is known about them.
    pub type_unreadable: bool,
    pub autocomplete_unreadable: bool,
    pub href_unreadable: bool,
    pub formaction_unreadable: bool,
    pub target_unreadable: bool,
}

/// How one `get attr` ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttrRead {
    /// The attribute has this value.
    Value(String),
    /// The element answered and has no such attribute (`value: null`) or an empty one.
    Absent,
    /// The command was refused: nothing is known.
    Unreadable,
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
    /// The reply says there are more fields than it lists.
    pub more: bool,
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
            more: data["more"] == true,
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
    /// One entry per form in document order (`form >> nth=i`): its resolved action, or `None`
    /// when it could not be read or resolved.
    pub per_form: Vec<Option<String>>,
    /// Every form was read. `false`: a form's action could not be read or resolved, there are
    /// more forms than were read, or the page's field list was cut: the gate can not say where a
    /// submit goes.
    pub complete: bool,
    /// How many forms the page has (`get count form`; `None`: not read).
    pub count: Option<usize>,
}
impl FormActions {
    /// The readable actions of every form.
    pub fn urls(&self) -> Vec<String> {
        self.per_form.iter().flatten().cloned().collect()
    }
    pub fn is_empty(&self) -> bool {
        self.per_form.is_empty()
    }
}

/// The most forms of a page whose actions are read.
pub const MAX_FORMS: usize = 20;

/// Whether an address is a `javascript:` one (a click that runs a script, not a navigation).
pub fn is_javascript(raw: &str) -> bool {
    raw.trim()
        .get(..11)
        .is_some_and(|head| head.eq_ignore_ascii_case("javascript:"))
}

/// `raw` (as the page wrote it) against the page address.
pub fn resolve(base: &str, raw: &str) -> Option<String> {
    url::Url::parse(base)
        .ok()?
        .join(raw.trim())
        .ok()
        .map(|u| u.to_string())
}

/// The attributes of a field as the classifier reads them. The accessible name and the label of
/// the `form` command are classified apart (the first kind that is not `Other` wins).
fn field_attr_sets(name: &str, attrs: &Attrs) -> Vec<FieldAttrs> {
    let base = FieldAttrs {
        input_type: attrs.input_type.clone().unwrap_or_default().to_lowercase(),
        autocomplete: attrs
            .autocomplete
            .clone()
            .unwrap_or_default()
            .to_lowercase(),
        name: attrs.field_name.clone().unwrap_or_default(),
        id: attrs.field_id.clone().unwrap_or_default(),
        placeholder: attrs.placeholder.clone().unwrap_or_default(),
        ..Default::default()
    };
    let mut sets = vec![FieldAttrs {
        label: name.to_owned(),
        ..base.clone()
    }];
    if let Some(label) = attrs.field_label.as_deref().filter(|l| *l != name) {
        sets.push(FieldAttrs {
            label: label.to_owned(),
            ..base
        });
    }
    sets
}

/// Why a field must never be typed into, if it must not: by what the page says it is
/// (`type=password`, `autocomplete` of a password or a one-time code), then by its `name`, `id`,
/// `placeholder`, label or accessible name (the risk lexicon: password, passcode, PIN, one-time
/// code, 2FA and their Vietnamese forms). No approval overrides it.
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
    field_attr_sets(name, attrs)
        .iter()
        .any(|set| matches!(classify_field(set), FieldKind::Password | FieldKind::Otp))
        .then_some("this field is named like a password or a one-time code, which is never typed")
}

/// The kind of a field for the classifier (card, ID, bank and the rest; a secret is refused before
/// this is used).
pub fn field_kind(name: &str, attrs: &Attrs) -> FieldKind {
    field_attr_sets(name, attrs)
        .iter()
        .map(classify_field)
        .find(|k| *k != FieldKind::Other)
        .unwrap_or(FieldKind::Other)
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

/// The key of a chord: what follows the last `+` (`Control+Enter` is `Enter`, `Control++` and `+`
/// are `+`).
pub fn chord_key(key: &str) -> &str {
    if key == "+" || key.ends_with("++") {
        return "+";
    }
    key.rsplit('+').next().unwrap_or(key)
}

/// The modifiers of a chord (`Control+Shift+a` is `Control`, `Shift`).
pub fn chord_modifiers(key: &str) -> Vec<&str> {
    let last = chord_key(key);
    let head = key.strip_suffix(last).unwrap_or("");
    head.split('+').filter(|m| !m.is_empty()).collect()
}

/// A key that types a character: one character, or a chord ending in one (`Control+v`, `a`), or
/// Space.
pub fn is_printable_key(key: &str) -> bool {
    let last = chord_key(key);
    last.chars().count() == 1 || last.eq_ignore_ascii_case("space")
}

/// Enter in any chord (`Enter`, `Control+Enter`, `Shift+Return`, `NumpadEnter`): it submits a form
/// or presses a control. Space is a press, not a submit.
pub fn is_submit_key(key: &str) -> bool {
    matches!(
        chord_key(key).to_ascii_lowercase().as_str(),
        "enter" | "return" | "numpadenter"
    )
}

/// Delete or Backspace in any chord: it removes text, or acts as a shortcut where no field has the
/// focus.
pub fn is_delete_key(key: &str) -> bool {
    matches!(
        chord_key(key).to_ascii_lowercase().as_str(),
        "delete" | "backspace"
    )
}

/// Whether a link's `target` makes it open a window of its own: anything set that is not
/// `_self`, `_top` or `_parent` (a window name, `_blank`).
pub fn opens_new_window(target: &str) -> bool {
    let target = target.trim().to_ascii_lowercase();
    !target.is_empty() && !matches!(target.as_str(), "_self" | "_top" | "_parent")
}

/// Space in any chord: it presses a focused button or checkbox, or types a space.
pub fn is_space_key(key: &str) -> bool {
    let last = chord_key(key);
    last == " " || last.eq_ignore_ascii_case("space")
}

/// A key that acts on the focused control: Enter or Space.
pub fn is_activation_key(key: &str) -> bool {
    is_submit_key(key) || is_space_key(key)
}

/// Paste (`Control+v`, `Meta+v`, `Shift+Insert`): it puts text the gate never sees into the field.
pub fn is_paste_chord(key: &str) -> bool {
    let mods = chord_modifiers(key);
    let last = chord_key(key).to_ascii_lowercase();
    let command = mods.iter().any(|m| {
        matches!(
            m.to_ascii_lowercase().as_str(),
            "control" | "ctrl" | "meta" | "command" | "cmd" | "controlormeta"
        )
    });
    (command && last == "v")
        || (last == "insert" && mods.iter().any(|m| m.eq_ignore_ascii_case("shift")))
}

/// The text a plain key types (one character, or a space), `None` for a chord with Control, Meta
/// or Alt (a shortcut) and for a key that types nothing.
pub fn key_text(key: &str) -> Option<String> {
    let blocks = chord_modifiers(key).iter().any(|m| {
        matches!(
            m.to_ascii_lowercase().as_str(),
            "control" | "ctrl" | "meta" | "command" | "cmd" | "alt" | "option" | "controlormeta"
        )
    });
    if blocks {
        return None;
    }
    if is_space_key(key) {
        return Some(" ".to_owned());
    }
    let last = chord_key(key);
    (last.chars().count() == 1).then(|| last.to_owned())
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
    // The address of any element that has one (a link, a button made of an `a`). A `javascript:`
    // address does not navigate: the element is judged as a button and the page it leaves the tab
    // on is checked after the click.
    let href = input
        .attrs
        .href
        .as_deref()
        .filter(|h| !is_javascript(h))
        .and_then(|h| resolve(input.page_url, h));
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
        input_name: input
            .attrs
            .field_name
            .clone()
            .or_else(|| input.form.field(input.ref_).map(|f| f.name.clone())),
        input_id: input.attrs.field_id.clone(),
        placeholder: input.attrs.placeholder.clone(),
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
    fn a_chord_that_ends_in_enter_submits_and_space_is_a_press() {
        for key in [
            "Enter",
            "Return",
            "NumpadEnter",
            "Control+Enter",
            "Shift+Return",
            "Meta+NumpadEnter",
        ] {
            assert!(is_submit_key(key), "{key}");
            assert!(is_activation_key(key), "{key}");
            assert!(!is_printable_key(key), "{key}");
        }
        for key in ["Space", " ", "Control+Space", "Shift+ "] {
            assert!(is_space_key(key) && is_activation_key(key), "{key}");
            assert!(!is_submit_key(key), "{key}");
        }
        for key in ["Tab", "Delete", "Backspace", "Escape", "Control+a"] {
            assert!(!is_submit_key(key) && !is_space_key(key), "{key}");
        }
        assert_eq!(chord_key("Control++"), "+");
        assert_eq!(chord_key("+"), "+");
        assert_eq!(chord_modifiers("Control+Shift+a"), vec!["Control", "Shift"]);
    }

    #[test]
    fn paste_chords_and_the_text_a_key_types() {
        for key in [
            "Control+v",
            "Meta+V",
            "Command+v",
            "Shift+Insert",
            "ControlOrMeta+v",
        ] {
            assert!(is_paste_chord(key), "{key}");
        }
        for key in ["v", "Control+c", "Shift+Tab"] {
            assert!(!is_paste_chord(key), "{key}");
        }
        assert_eq!(key_text("a").as_deref(), Some("a"));
        assert_eq!(key_text("Shift+A").as_deref(), Some("A"));
        assert_eq!(key_text("Space").as_deref(), Some(" "));
        assert_eq!(key_text("Control+a"), None);
        assert_eq!(key_text("Enter"), None);
    }

    #[test]
    fn a_link_target_opens_a_window_unless_it_is_the_same_page() {
        for target in ["_blank", "popup", " _BLANK "] {
            assert!(opens_new_window(target), "{target}");
        }
        for target in ["", "  ", "_self", "_TOP", "_parent"] {
            assert!(!opens_new_window(target), "{target}");
        }
        assert!(is_javascript(" JavaScript:void(0)") && !is_javascript("https://a.example/"));
    }

    #[test]
    fn delete_keys_are_found_in_any_chord() {
        for key in ["Delete", "Backspace", "Control+Backspace", "shift+delete"] {
            assert!(is_delete_key(key), "{key}");
        }
        for key in ["a", "Enter", "Tab", "Escape"] {
            assert!(!is_delete_key(key), "{key}");
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
