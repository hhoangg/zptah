//! The focused element of the page, for `browser_press` without a ref (design spec section 6).
//!
//! `snapshot` never marks the focus (measured on Ardia 0.7.0), so the gate asks the page with one
//! fixed `eval` it wrote itself. The result is page-controlled text: it is parsed here, used for
//! the decision only, and never shown to the Mono. A reply that does not parse, an `IFRAME`, a
//! content-editable element or a shadow host is [`Focus::Unknown`].
use serde_json::Value;

/// The script the gate sends. Fixed: no part of it comes from the Mono or the page. It returns a
/// JSON string with the tag, `type`, `autocomplete`, `name`, `aria-label` and (for a button or a
/// link) the visible text of the focused element, the index of its form among `document.forms`,
/// the text of that form's default submit button and the resolved `formAction` and `href`. It
/// never reads the value of a field.
pub const FOCUS_SCRIPT: &str = r#"JSON.stringify((function(e){return e?{tag:e.tagName,type:e.type||null,ac:e.autocomplete||null,name:e.name||null,label:e.getAttribute("aria-label"),text:(/^(BUTTON|A)$/.test(e.tagName)||/^(submit|button|reset|image)$/.test(e.type))?(e.innerText||e.value||"").slice(0,80):null,ce:e.isContentEditable===true,sh:!!e.shadowRoot,fi:e.form?Array.prototype.indexOf.call(document.forms,e.form):null,sb:(function(f){var b=f&&f.querySelector('button:not([type]),button[type=submit],input[type=submit],input[type=image]');return b?(b.innerText||b.value||b.getAttribute("aria-label")||"").slice(0,80):null})(e.form),fa:e.formAction||null,hr:e.href||null}:null})(document.activeElement))"#;

/// What has the focus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Focus {
    /// The page could not say (the reply did not parse, a frame, content-editable or shadow DOM).
    Unknown,
    /// Nothing is focused (`BODY`): a key goes to the page itself.
    Nothing,
    Element(Box<FocusEl>),
}

/// The focused element, as far as the script reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FocusEl {
    pub tag: String,
    pub input_type: Option<String>,
    pub autocomplete: Option<String>,
    pub name: Option<String>,
    pub label: Option<String>,
    /// Visible text of a button or link.
    pub text: Option<String>,
    /// Index of its form among the page's forms (`document.forms`).
    pub form: Option<usize>,
    /// Text of the default submit button of its form.
    pub default_submit: Option<String>,
    /// The resolved `formAction` of a button or input.
    pub form_action: Option<String>,
    pub href: Option<String>,
}

const TEXT_LESS_TYPES: [&str; 11] = [
    "button",
    "submit",
    "reset",
    "image",
    "checkbox",
    "radio",
    "file",
    "color",
    "range",
    "hidden",
    "select-one",
];

impl FocusEl {
    pub fn is_textarea(&self) -> bool {
        self.tag.eq_ignore_ascii_case("textarea")
    }
    /// A control a character key can type into (a text-like input or a textarea).
    pub fn is_text_entry(&self) -> bool {
        if self.is_textarea() {
            return true;
        }
        if !self.tag.eq_ignore_ascii_case("input") {
            return false;
        }
        let ty = self.input_type.as_deref().unwrap_or("text").to_lowercase();
        !TEXT_LESS_TYPES.contains(&ty.as_str())
    }
    /// A control Enter or Space presses: a button, a link, a submit or button input.
    pub fn is_pressable(&self) -> bool {
        let tag = self.tag.to_ascii_uppercase();
        let ty = self.input_type.as_deref().unwrap_or("").to_lowercase();
        tag == "BUTTON"
            || tag == "A"
            || (tag == "INPUT" && matches!(ty.as_str(), "submit" | "button" | "reset" | "image"))
    }
    /// A checkbox or radio (Space toggles it).
    pub fn is_checkable(&self) -> bool {
        self.tag.eq_ignore_ascii_case("input")
            && matches!(
                self.input_type
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .as_str(),
                "checkbox" | "radio"
            )
    }
    /// What the control shows: its text, or its `aria-label`.
    pub fn shown_name(&self) -> String {
        self.text
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .or(self.label.as_deref())
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Reads the reply of `eval` (`data`: `{origin, result}`, where `result` is the script's string).
pub fn parse(data: &Value) -> Focus {
    let Some(text) = data["result"].as_str() else {
        return Focus::Unknown;
    };
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Focus::Unknown;
    };
    if value.is_null() {
        return Focus::Nothing;
    }
    let Some(tag) = value["tag"].as_str().filter(|t| !t.is_empty()) else {
        return Focus::Unknown;
    };
    let tag = tag.to_ascii_uppercase();
    if matches!(tag.as_str(), "BODY" | "HTML") {
        return Focus::Nothing;
    }
    if matches!(tag.as_str(), "IFRAME" | "FRAME" | "OBJECT" | "EMBED")
        || value["ce"] == true
        || value["sh"] == true
    {
        return Focus::Unknown;
    }
    let text = |key: &str| {
        value[key]
            .as_str()
            .map(str::to_owned)
            .filter(|v| !v.is_empty())
    };
    Focus::Element(Box::new(FocusEl {
        tag,
        input_type: text("type"),
        autocomplete: text("ac"),
        name: text("name"),
        label: text("label"),
        text: text("text"),
        form: value["fi"].as_u64().map(|i| i as usize),
        default_submit: text("sb"),
        form_action: text("fa"),
        href: text("hr"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(result: &str) -> Value {
        json!({"origin": "file://", "result": result})
    }

    #[test]
    fn the_measured_replies_are_read() {
        let input = parse(&reply(
            r#"{"tag":"INPUT","type":"text","ac":null,"name":null,"label":"B1"}"#,
        ));
        let Focus::Element(el) = input else {
            panic!("{input:?}")
        };
        assert!(el.is_text_entry() && !el.is_pressable());
        assert_eq!(el.label.as_deref(), Some("B1"));
        assert_eq!(parse(&reply(r#"{"tag":"BODY"}"#)), Focus::Nothing);
    }

    #[test]
    fn anything_the_gate_cannot_vouch_for_is_unknown() {
        for bad in [
            "not json",
            "{}",
            r#"{"tag":"IFRAME"}"#,
            r#"{"tag":"DIV","ce":true}"#,
            r#"{"tag":"DIV","sh":true}"#,
            "[1]",
        ] {
            assert_eq!(parse(&reply(bad)), Focus::Unknown, "{bad}");
        }
        assert_eq!(parse(&json!({"result": 5})), Focus::Unknown);
        assert_eq!(parse(&json!({})), Focus::Unknown);
    }

    #[test]
    fn a_button_shows_its_text_and_a_field_types() {
        let Focus::Element(button) = parse(&reply(
            r#"{"tag":"BUTTON","type":"submit","text":"Delete  account","fi":1,"sb":"Delete account"}"#,
        )) else {
            panic!()
        };
        assert!(button.is_pressable() && !button.is_text_entry());
        assert_eq!(button.shown_name(), "Delete account");
        assert_eq!(button.form, Some(1));
        let Focus::Element(area) = parse(&reply(r#"{"tag":"TEXTAREA","type":"textarea"}"#)) else {
            panic!()
        };
        assert!(area.is_text_entry() && area.is_textarea());
        let Focus::Element(check) = parse(&reply(r#"{"tag":"INPUT","type":"checkbox"}"#)) else {
            panic!()
        };
        assert!(check.is_checkable() && !check.is_text_entry());
    }
}
