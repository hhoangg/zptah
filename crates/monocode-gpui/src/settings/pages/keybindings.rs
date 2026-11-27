//! React's command names, defaults, scopes, filter and shortcut recorder.
use crate::{
    settings::{Preferences, SettingsView},
    theme::*,
};
use gpui::{AnyElement, Context, FontWeight, Role, div, prelude::*, px};
#[derive(Clone, Copy)]
pub(crate) struct Binding {
    pub command: &'static str,
    pub keys: &'static str,
    pub when: &'static str,
    pub fixed: bool,
}
pub(crate) const BINDINGS: &[Binding] = &[
    Binding {
        command: "App: Settings",
        keys: "⌘,",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Search",
        keys: "⌘K",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Go to File",
        keys: "⌘P",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Command Palette",
        keys: "⌘⇧P",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Find in Files",
        keys: "⌘⇧F",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Open Project",
        keys: "⌘O",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: New Window",
        keys: "⌘⇧N",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Quick Composer",
        keys: "⌘⇧Space",
        when: "Anywhere",
        fixed: false,
    },
    Binding {
        command: crate::monos::hotkeys::COMMAND,
        keys: "⌃⌥1-9",
        when: "Anywhere",
        fixed: false,
    },
    Binding {
        command: "App: Toggle Sidebar",
        keys: "⌘B",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Toggle Session Sidebar",
        keys: "⌘⇧B",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Switch Model",
        keys: "⌘.",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Toggle Mono",
        keys: "⌘I",
        when: "Project with a mono",
        fixed: false,
    },
    Binding {
        command: "Composer: Toggle Workspace",
        keys: "⌘⇧G",
        when: "Draft session composer",
        fixed: false,
    },
    Binding {
        command: "View: Reload",
        keys: "⌘⇧R",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "View: Zoom In",
        keys: "⌘+",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "View: Zoom Out",
        keys: "⌘-",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "View: Reset Zoom",
        keys: "⌘0",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: New",
        keys: "⌘T",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Close Others",
        keys: "⌘⌥T",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Close All",
        keys: "⌘⇧W",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Next",
        keys: "⌘⇧]",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Previous",
        keys: "⌘⇧[",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Cycle Next",
        keys: "⌃Tab",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Cycle Previous",
        keys: "⌃⇧Tab",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Back",
        keys: "⌘[",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Forward",
        keys: "⌘]",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Activate 1–8",
        keys: "⌘1 … ⌘8",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Activate Last",
        keys: "⌘9",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Session: Archive",
        keys: "⌘⇧A",
        when: "sessionFocus && !overlay",
        fixed: false,
    },
    Binding {
        command: "Session: Stop",
        keys: "Esc",
        when: "sessionBusy && !terminalFocus",
        fixed: true,
    },
    Binding {
        command: "Session: Previous",
        keys: "⌘⇧↑",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Session: Next",
        keys: "⌘⇧↓",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Session: Previous in Current Tab",
        keys: "⌘↑",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Session: Next in Current Tab",
        keys: "⌘↓",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Project: Previous",
        keys: "⌘⇧←",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Project: Next",
        keys: "⌘⇧→",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Project: Move Back",
        keys: "⌘⌥⇧←",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Project: Move Forward",
        keys: "⌘⌥⇧→",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Pane: Close",
        keys: "⌘W",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Pane: Split Right",
        keys: "⌘D",
        when: "!editorFocus",
        fixed: false,
    },
    Binding {
        command: "Pane: Split Down",
        keys: "⌘⇧D",
        when: "!editorFocus",
        fixed: false,
    },
    Binding {
        command: "Pane: Focus Left",
        keys: "⌘⌥←",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Pane: Focus Right",
        keys: "⌘⌥→",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Pane: Focus Up",
        keys: "⌘⌥↑",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Pane: Focus Down",
        keys: "⌘⌥↓",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Terminal: New",
        keys: "⌘`",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Terminal: New in Home",
        keys: "⌘⌥J",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Terminal: New Tab",
        keys: "⌘⇧`",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Terminal: Toggle Dock",
        keys: "⌘J",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Editor: Toggle Git Blame",
        keys: "⌘⌥B",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Editor: Find",
        keys: "⌘F",
        when: "editorFocus",
        fixed: false,
    },
    Binding {
        command: "Editor: Replace",
        keys: "⌘⌥F",
        when: "editorFocus",
        fixed: false,
    },
    Binding {
        command: "Text: Undo",
        keys: "⌘Z",
        when: "inputFocus",
        fixed: false,
    },
    Binding {
        command: "Text: Redo",
        keys: "⌘⇧Z",
        when: "inputFocus",
        fixed: false,
    },
    Binding {
        command: "Composer: Previous Prompt",
        keys: "↑",
        when: "composerFocus && caretOnFirstLine",
        fixed: true,
    },
    Binding {
        command: "Composer: Next Prompt",
        keys: "↓",
        when: "composerFocus && caretOnLastLine",
        fixed: true,
    },
];
pub(super) fn quick_composer_label(p: &Preferences) -> String {
    p.keybindings
        .get("App: Quick Composer")
        .and_then(|s| s.clone())
        .map(|s| shortcut_label(&s))
        .unwrap_or_else(|| "⌘⇧Space".into())
}
/// A command's shortcut as shown, honoring the user's rebinding; `None`
/// once they have disabled it.
pub(crate) fn command_shortcut(p: &Preferences, command: &str) -> Option<String> {
    if command == crate::monos::hotkeys::COMMAND {
        // Only the modifiers are stored; the digit is the Mono's position.
        return crate::monos::hotkeys::modifiers(&p.keybindings)
            .map(|m| format!("{}1-9", shortcut_label(&m)));
    }
    match p.keybindings.get(command) {
        Some(Some(s)) => Some(shortcut_label(s)),
        Some(None) => None,
        None => {
            let keys = BINDINGS.iter().find(|r| r.command == command)?.keys;
            Some(if cfg!(target_os = "macos") {
                keys.into()
            } else {
                keys.replace('⌘', "Ctrl+")
                    .replace('⇧', "Shift+")
                    .replace('⌥', "Alt+")
                    .replace('⌃', "Ctrl+")
            })
        }
    }
}
pub(crate) fn shortcut_label(shortcut: &str) -> String {
    let parts: Vec<_> = shortcut.split('-').collect();
    let mut label = String::new();
    for part in parts {
        label.push_str(&match part {
            "cmd" => "⌘".into(),
            "ctrl" => if cfg!(target_os = "macos") {
                "⌃"
            } else {
                "Ctrl+"
            }
            .into(),
            "alt" => if cfg!(target_os = "macos") {
                "⌥"
            } else {
                "Alt+"
            }
            .into(),
            "shift" => if cfg!(target_os = "macos") {
                "⇧"
            } else {
                "Shift+"
            }
            .into(),
            "space" => "Space".into(),
            "up" => "↑".into(),
            "down" => "↓".into(),
            "left" => "←".into(),
            "right" => "→".into(),
            "tab" => "Tab".into(),
            key => key.to_uppercase(),
        });
    }
    label
}
impl SettingsView {
    pub(super) fn keybindings_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let query = self.keybinding_query.trim().to_lowercase();
        let rows: Vec<_> = BINDINGS
            .iter()
            .filter(|r| shown_here(r))
            .filter(|r| {
                format!("{} {} {}", r.command, self.binding_label(r), r.when)
                    .to_lowercase()
                    .contains(&query)
            })
            .copied()
            .collect();
        let action = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.4))
                    .child(format!(
                        "{} {}",
                        rows.len(),
                        if rows.len() == 1 {
                            "binding"
                        } else {
                            "bindings"
                        }
                    )),
            )
            .child(
                div()
                    .h(px(28.))
                    .w(px(176.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.1))
                    .px_2()
                    .child(icon("search-01", 14.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(self.keybinding_filter.clone()),
                    ),
            )
            .into_any_element();
        let mut children = vec![
            div()
                .flex()
                .items_center()
                .border_b_1()
                .border_color(content(0.1))
                .bg(content(0.05))
                .px_4()
                .py_2()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(0.4))
                .child(div().min_w_0().flex_1().child("COMMAND"))
                .child(div().w(px(160.)).flex_shrink_0().child("KEYBINDING"))
                .child(div().w(px(112.)).flex_shrink_0().child("WHEN"))
                .into_any_element(),
        ];
        if rows.is_empty() {
            children.push(super::super::widgets::note("No matching bindings"));
        }
        for row in rows {
            let command = row.command;
            let recording = self.recording.as_deref() == Some(command);
            let modified = self.prefs.keybindings.contains_key(command);
            let disabled = self.prefs.keybindings.get(command) == Some(&None);
            let label = if recording {
                if self.recording_preview.is_empty() {
                    "Record…".into()
                } else {
                    self.recording_preview.clone()
                }
            } else {
                self.binding_label(&row)
            };
            children.push(
                div()
                    .h(px(44.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(content(0.05))
                    .px_4()
                    .text_size(px(12.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_color(content(if disabled { 0.45 } else { 1. }))
                            .child(command),
                    )
                    .child(
                        div()
                            .w(px(160.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .child(
                                div()
                                    .id(gpui::SharedString::from(format!("shortcut-{command}")))
                                    .debug_selector(move || format!("shortcut-{command}"))
                                    .role(if row.fixed { Role::Label } else { Role::Button })
                                    .aria_label(format!("Change {command} shortcut"))
                                    .h(px(24.))
                                    .w(px(112.))
                                    .flex()
                                    .items_center()
                                    .truncate()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(if recording {
                                        accent_color()
                                    } else {
                                        content(0.15)
                                    })
                                    .px(px(6.))
                                    .font_family(crate::fonts::mono())
                                    .text_size(px(11.))
                                    .text_color(content(if disabled {
                                        0.35
                                    } else if row.fixed {
                                        0.6
                                    } else {
                                        0.8
                                    }))
                                    .child(label)
                                    .when(!row.fixed, |d| {
                                        d.cursor_pointer().hover(|s| s.bg(content(0.1))).on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.recording = Some(command.into());
                                                this.recording_preview.clear();
                                                this.recording_error = None;
                                                window.focus(&this.recorder_focus, cx);
                                                cx.notify();
                                            }),
                                        )
                                    }),
                            )
                            .when(modified, |d| {
                                d.child(
                                    div()
                                        .id(gpui::SharedString::from(format!("reset-{command}")))
                                        .debug_selector(move || format!("reset-shortcut-{command}"))
                                        .role(Role::Button)
                                        .aria_label(format!("Reset {command} shortcut"))
                                        .p_1()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(content(0.1)))
                                        .child(icon("rotate-ccw", 14.))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.update(cx, |p| {
                                                p.keybindings.remove(command);
                                            })
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w(px(112.))
                            .flex_shrink_0()
                            .truncate()
                            .font_family(crate::fonts::mono())
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child(row.when),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = self.ctx.mono_hotkey_error.as_ref() {
            children.push(
                div()
                    .px_4()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(gpui::rgb(0xf87171))
                    .child(format!("{}: {error}", crate::monos::hotkeys::COMMAND))
                    .into_any_element(),
            );
        }
        if let Some(error) = &self.recording_error {
            children.push(
                div()
                    .px_4()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(gpui::rgb(0xf87171))
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        vec![self.group(None,true,"Shortcuts",Some("Click a shortcut to record new keys. Press Delete while recording to disable it.".into()),Some(action),children)]
    }
    /// The Mono shortcut row keeps only the modifiers of what was pressed; the
    /// digit is always 1 to 9.
    fn record_mono_modifiers(
        &mut self,
        command: &str,
        key: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) {
        match mono_modifiers_choice(&self.prefs, command, &key.modifiers) {
            Ok(modifiers) => {
                self.recording = None;
                self.recording_error = None;
                let command = command.to_string();
                self.update(cx, |p| {
                    p.keybindings.insert(command, Some(modifiers));
                });
            }
            Err(error) => {
                self.recording_error = Some(error);
                cx.notify();
            }
        }
    }
    fn binding_label(&self, row: &Binding) -> String {
        command_shortcut(&self.prefs, row.command).unwrap_or_else(|| "Disabled".into())
    }
    pub(in crate::settings) fn record_key(
        &mut self,
        key: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = self.recording.clone() else {
            return;
        };
        cx.stop_propagation();
        if key.key == "escape" || key.key == "tab" {
            self.recording = None;
            cx.notify();
            return;
        }
        if matches!(
            key.key.as_str(),
            "platform" | "control" | "alt" | "shift" | "function"
        ) {
            return;
        }
        let bare = !key.modifiers.platform
            && !key.modifiers.control
            && !key.modifiers.alt
            && !key.modifiers.shift;
        if bare && matches!(key.key.as_str(), "backspace" | "delete") {
            self.recording = None;
            self.update(cx, |p| {
                p.keybindings.insert(command, None);
            });
            return;
        }
        if command == crate::monos::hotkeys::COMMAND {
            if let Some(m) = crate::monos::hotkeys::from_modifiers(&key.modifiers) {
                self.recording_preview = format!("{}1-9", shortcut_label(&m));
            }
            self.record_mono_modifiers(&command, key, cx);
            return;
        }
        let shortcut = key.unparse();
        self.recording_preview = shortcut_label(&shortcut);
        if bare || (!key.modifiers.platform && !key.modifiers.control && !key.modifiers.alt) {
            self.recording_error = Some("Use Command, Control, or Option with a key.".into());
            cx.notify();
            return;
        }
        if command == "Tab: Activate 1–8"
            && !matches!(
                key.key.as_str(),
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8"
            )
        {
            self.recording_error = Some("Tab: Activate 1–8 needs a number key from 1 to 8".into());
            cx.notify();
            return;
        }
        if let Some(other) = shortcut_owner(&self.prefs, &command, &shortcut) {
            self.recording_error = Some(format!("Already used by {}", other));
            cx.notify();
            return;
        }
        self.recording = None;
        self.recording_error = None;
        self.update(cx, |p| {
            p.keybindings.insert(command, Some(shortcut));
        });
    }
}

/// Rows that only exist on macOS are hidden elsewhere.
fn shown_here(row: &Binding) -> bool {
    cfg!(target_os = "macos")
        || !matches!(
            row.command,
            "App: Quick Composer" | crate::monos::hotkeys::COMMAND
        )
}
/// Every shortcut a row holds: the Mono row expands its modifiers to the nine
/// digits (none while disabled); the others count their default and their
/// rebinding.
fn claimed_shortcuts(prefs: &Preferences, row: &Binding) -> Vec<String> {
    if row.command == crate::monos::hotkeys::COMMAND {
        return crate::monos::hotkeys::modifiers(&prefs.keybindings)
            .map(|m| {
                (1..=9)
                    .filter_map(|digit| {
                        gpui::Keystroke::parse(&crate::monos::hotkeys::shortcut(&m, digit))
                            .ok()
                            .map(|s| s.unparse())
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    let mut all = default_shortcuts(row);
    if let Some(Some(custom)) = prefs.keybindings.get(row.command) {
        all.push(custom.clone());
    }
    all
}
/// The other command that already holds this shortcut.
fn shortcut_owner(prefs: &Preferences, command: &str, shortcut: &str) -> Option<&'static str> {
    BINDINGS
        .iter()
        .find(|r| {
            r.command != command
                && shown_here(r)
                && claimed_shortcuts(prefs, r).iter().any(|s| s == shortcut)
        })
        .map(|r| r.command)
}
/// What recording the Mono row stores: the held modifiers, unless they are too
/// few or one of their digit combinations belongs to another command.
fn mono_modifiers_choice(
    prefs: &Preferences,
    command: &str,
    held: &gpui::Modifiers,
) -> Result<String, String> {
    let Some(modifiers) = crate::monos::hotkeys::from_modifiers(held) else {
        return Err("Use Command, Control, or Option with a key.".into());
    };
    for digit in 1..=9 {
        let Ok(parsed) =
            gpui::Keystroke::parse(&crate::monos::hotkeys::shortcut(&modifiers, digit))
        else {
            continue;
        };
        if let Some(other) = shortcut_owner(prefs, command, &parsed.unparse()) {
            return Err(format!("Already used by {other}"));
        }
    }
    Ok(modifiers)
}

fn default_shortcuts(row: &Binding) -> Vec<String> {
    if row.command == crate::monos::hotkeys::COMMAND {
        return (1..=9)
            .filter_map(|digit| {
                gpui::Keystroke::parse(&crate::monos::hotkeys::shortcut(
                    crate::monos::hotkeys::DEFAULT_MODIFIERS,
                    digit,
                ))
                .ok()
                .map(|s| s.unparse())
            })
            .collect();
    }
    let shortcut = row
        .keys
        .replace(
            '⌘',
            if cfg!(target_os = "macos") {
                "cmd-"
            } else {
                "ctrl-"
            },
        )
        .replace('⌃', "ctrl-")
        .replace('⌥', "alt-")
        .replace('⇧', "shift-")
        .replace('↑', "up")
        .replace('↓', "down")
        .replace('←', "left")
        .replace('→', "right")
        .replace('+', "=")
        .to_lowercase();
    let chords = if shortcut.contains('…') {
        let prefix = shortcut.split('1').next().unwrap_or("");
        (1..=8).map(|i| format!("{prefix}{i}")).collect()
    } else {
        vec![shortcut]
    };
    chords
        .into_iter()
        .filter_map(|s| gpui::Keystroke::parse(&s).ok().map(|s| s.unparse()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_new_in_home_is_listed_once_with_its_default() {
        let rows: Vec<_> = BINDINGS
            .iter()
            .filter(|b| b.command == "Terminal: New in Home")
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].keys, "⌘⌥J");
    }

    #[test]
    fn text_undo_and_redo_are_customizable_with_platform_defaults() {
        for (command, key) in [("Text: Undo", "z"), ("Text: Redo", "shift-z")] {
            let row = BINDINGS.iter().find(|b| b.command == command).unwrap();
            assert!(!row.fixed);
            let modifier = if cfg!(target_os = "macos") {
                "cmd"
            } else {
                "ctrl"
            };
            assert_eq!(default_shortcuts(row), [format!("{modifier}-{key}")]);
        }
    }

    #[test]
    fn mono_shortcut_row_shows_its_modifiers_and_default() {
        let mut p = Preferences::default();
        let label = |p: &Preferences| command_shortcut(p, crate::monos::hotkeys::COMMAND);
        let default = if cfg!(target_os = "macos") {
            "⌃⌥1-9"
        } else {
            "Ctrl+Alt+1-9"
        };
        assert_eq!(label(&p).as_deref(), Some(default));
        p.keybindings.insert(
            crate::monos::hotkeys::COMMAND.into(),
            Some("cmd-shift".into()),
        );
        let expected = if cfg!(target_os = "macos") {
            "⇧⌘1-9"
        } else {
            "Shift+⌘1-9"
        };
        assert_eq!(label(&p).as_deref(), Some(expected));
        p.keybindings
            .insert(crate::monos::hotkeys::COMMAND.into(), None);
        assert_eq!(label(&p), None);
    }

    #[test]
    fn mono_shortcut_row_is_anywhere_and_conflicts_with_its_digits() {
        let row = BINDINGS
            .iter()
            .find(|b| b.command == crate::monos::hotkeys::COMMAND)
            .unwrap();
        assert_eq!(
            (row.keys, row.when, row.fixed),
            ("⌃⌥1-9", "Anywhere", false)
        );
        assert_eq!(default_shortcuts(row).len(), 9);
        assert!(default_shortcuts(row).contains(&"ctrl-alt-9".to_string()));
    }

    fn held(control: bool, alt: bool, shift: bool, platform: bool) -> gpui::Modifiers {
        gpui::Modifiers {
            control,
            alt,
            shift,
            platform,
            ..Default::default()
        }
    }

    #[test]
    fn recording_the_mono_row_keeps_only_modifiers() {
        let p = Preferences::default();
        let command = crate::monos::hotkeys::COMMAND;
        assert_eq!(
            mono_modifiers_choice(&p, command, &held(true, true, true, false)).as_deref(),
            Ok("ctrl-alt-shift")
        );
        assert!(mono_modifiers_choice(&p, command, &held(false, false, false, false)).is_err());
        assert!(mono_modifiers_choice(&p, command, &held(false, false, true, false)).is_err());
    }

    #[test]
    fn recording_the_mono_row_detects_conflicts() {
        let command = crate::monos::hotkeys::COMMAND;
        let p = Preferences::default();
        // The tab shortcuts: Command+1..8 on macOS, Ctrl+1..8 elsewhere.
        let tab = if cfg!(target_os = "macos") {
            held(false, false, false, true)
        } else {
            held(true, false, false, false)
        };
        assert_eq!(
            mono_modifiers_choice(&p, command, &tab),
            Err("Already used by Tab: Activate 1–8".into())
        );
        let mut p = Preferences::default();
        p.keybindings
            .insert("App: Quick Composer".into(), Some("ctrl-alt-4".into()));
        assert_eq!(
            mono_modifiers_choice(&p, command, &held(true, true, false, false)),
            // Quick Composer is macOS-only, so elsewhere it holds nothing.
            if cfg!(target_os = "macos") {
                Err("Already used by App: Quick Composer".into())
            } else {
                Ok("ctrl-alt".into())
            }
        );
    }

    #[test]
    fn other_rows_cannot_take_a_digit_the_mono_row_holds() {
        let mut p = Preferences::default();
        // The Mono row is macOS-only: elsewhere it is hidden and holds nothing.
        let holder = cfg!(target_os = "macos").then_some(crate::monos::hotkeys::COMMAND);
        let owner =
            |p: &Preferences, shortcut: &str| shortcut_owner(p, "App: Toggle Sidebar", shortcut);
        let shifted = gpui::Keystroke::parse("shift-cmd-3").unwrap().unparse();
        assert_eq!(owner(&p, "ctrl-alt-3"), holder);
        p.keybindings.insert(
            crate::monos::hotkeys::COMMAND.into(),
            Some("cmd-shift".into()),
        );
        assert_eq!(owner(&p, "ctrl-alt-3"), None);
        assert_eq!(owner(&p, &shifted), holder);
        p.keybindings
            .insert(crate::monos::hotkeys::COMMAND.into(), None);
        assert_eq!(owner(&p, &shifted), None);
    }
}
