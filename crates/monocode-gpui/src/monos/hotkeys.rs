//! Global shortcuts that open the floating chat of the first nine Monos. The
//! preference stores only the modifiers (for example `ctrl-alt`); the digit is
//! the Mono's position in the roster, the same order the menu bar lists.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
use std::collections::HashMap;

/// The Keybindings row and the key of its preference.
pub(crate) const COMMAND: &str = "Mono: Open 1-9";
pub(crate) const DEFAULT_MODIFIERS: &str = "ctrl-alt";
/// How many Monos get a shortcut.
pub(crate) const MAX: usize = 9;

/// The modifiers in one fixed order, or `None` when the text is not a set of
/// distinct modifiers that includes ctrl, alt or cmd (shift alone would hijack
/// typing `!`, `@`, `#` in every app).
pub(crate) fn canonical(modifiers: &str) -> Option<String> {
    let mut seen = [false; 4];
    for part in modifiers.split('-') {
        let slot = ["ctrl", "alt", "shift", "cmd"]
            .iter()
            .position(|name| *name == part)?;
        if std::mem::replace(&mut seen[slot], true) {
            return None;
        }
    }
    let parts: Vec<_> = ["ctrl", "alt", "shift", "cmd"]
        .iter()
        .zip(seen)
        .filter_map(|(name, on)| on.then_some(*name))
        .collect();
    (seen[0] || seen[1] || seen[3]).then(|| parts.join("-"))
}

/// The modifiers the user's preferences select: the default when unset or
/// unreadable, `None` once they disabled the shortcut.
pub(crate) fn modifiers(keybindings: &HashMap<String, Option<String>>) -> Option<String> {
    match keybindings.get(COMMAND) {
        None => Some(DEFAULT_MODIFIERS.into()),
        Some(None) => None,
        Some(Some(value)) => Some(canonical(value).unwrap_or_else(|| DEFAULT_MODIFIERS.into())),
    }
}

/// The modifiers held in a recorded keystroke; `None` when none of ctrl, alt
/// or cmd is held.
pub(crate) fn from_modifiers(held: &gpui::Modifiers) -> Option<String> {
    let mut parts = Vec::new();
    if held.control {
        parts.push("ctrl");
    }
    if held.alt {
        parts.push("alt");
    }
    if held.shift {
        parts.push("shift");
    }
    if held.platform {
        parts.push("cmd");
    }
    canonical(&parts.join("-"))
}

/// The Monos that get a shortcut, with their digit (1 to 9), in roster order.
pub(crate) fn assignments<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<(String, u8)> {
    ids.into_iter()
        .take(MAX)
        .enumerate()
        .map(|(i, id)| (id.to_string(), i as u8 + 1))
        .collect()
}

/// The shortcut string for one digit, as the hotkey parser reads it.
pub(crate) fn shortcut(modifiers: &str, digit: u8) -> String {
    format!("{modifiers}-{digit}")
}

/// NSEventModifierFlags for a menu item's key equivalent.
pub(crate) fn menu_mask(modifiers: &str) -> usize {
    modifiers
        .split('-')
        .map(|part| match part {
            "shift" => 1 << 17,
            "ctrl" => 1 << 18,
            "alt" => 1 << 19,
            "cmd" => 1 << 20,
            _ => 0,
        })
        .fold(0, |all, bit| all | bit)
}

/// What pressing a Mono's shortcut does.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Press {
    Hide,
    Open,
}

/// A panel already showing this Mono is hidden; anything else opens it.
pub(crate) fn press(shown_on_this_mono: bool) -> Press {
    if shown_on_this_mono {
        Press::Hide
    } else {
        Press::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(value: Option<Option<&str>>) -> HashMap<String, Option<String>> {
        value
            .map(|v| (COMMAND.to_string(), v.map(str::to_string)))
            .into_iter()
            .collect()
    }

    #[test]
    fn modifiers_default_disable_and_override() {
        assert_eq!(modifiers(&prefs(None)).as_deref(), Some("ctrl-alt"));
        assert_eq!(modifiers(&prefs(Some(None))), None);
        assert_eq!(
            modifiers(&prefs(Some(Some("cmd-shift")))).as_deref(),
            Some("shift-cmd")
        );
        assert_eq!(
            modifiers(&prefs(Some(Some("ctrl-alt-1")))).as_deref(),
            Some("ctrl-alt")
        );
        assert_eq!(
            modifiers(&prefs(Some(Some("shift")))).as_deref(),
            Some("ctrl-alt")
        );
    }

    #[test]
    fn canonical_needs_distinct_known_modifiers() {
        assert_eq!(canonical("alt-ctrl").as_deref(), Some("ctrl-alt"));
        assert_eq!(canonical(""), None);
        assert_eq!(canonical("ctrl-ctrl"), None);
        assert_eq!(canonical("ctrl-a"), None);
        assert_eq!(canonical("shift"), None);
        assert_eq!(canonical("shift-cmd").as_deref(), Some("shift-cmd"));
    }

    #[test]
    fn recorded_keystroke_keeps_only_its_modifiers() {
        let held = gpui::Modifiers {
            control: true,
            alt: true,
            ..Default::default()
        };
        assert_eq!(from_modifiers(&held).as_deref(), Some("ctrl-alt"));
        assert_eq!(from_modifiers(&gpui::Modifiers::default()), None);
        let shift = gpui::Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(from_modifiers(&shift), None);
    }

    #[test]
    fn only_the_first_nine_monos_get_a_digit() {
        let ids: Vec<String> = (0..12).map(|i| format!("m{i}")).collect();
        let got = assignments(ids.iter().map(String::as_str));
        assert_eq!(got.len(), 9);
        assert_eq!(got[0], ("m0".to_string(), 1));
        assert_eq!(got[8], ("m8".to_string(), 9));
        assert!(assignments([]).is_empty());
    }

    #[test]
    fn shortcut_and_menu_mask_follow_the_modifiers() {
        assert_eq!(shortcut("ctrl-alt", 3), "ctrl-alt-3");
        assert_eq!(menu_mask("ctrl-alt"), (1 << 18) | (1 << 19));
        assert_eq!(menu_mask("cmd-shift"), (1 << 20) | (1 << 17));
    }

    #[test]
    fn a_press_on_the_shown_mono_hides_otherwise_opens() {
        assert_eq!(press(true), Press::Hide);
        assert_eq!(press(false), Press::Open);
    }
}
