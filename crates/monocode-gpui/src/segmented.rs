//! The segmented control: equal-width options in a hairline frame, one of them chosen. Shared so
//! any entity can draw one (Contributors and the Workflows page header use it).
use crate::theme::*;
use gpui::{Context, Div, Role, SharedString, Stateful, div, prelude::*, px};

/// Draws the control. `value` is the chosen option's id, `options` are `(id, text)` pairs and
/// `change` hears the id of the option pressed. Each option has the test selector
/// `<selector>-<id>-<option>`.
pub(crate) fn segmented<T: 'static>(
    selector: &'static str,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    value: &str,
    options: &[(&'static str, &str)],
    cx: &mut Context<T>,
    change: impl Fn(&mut T, &'static str, &mut Context<T>) + Clone + 'static,
) -> Stateful<Div> {
    segmented_with(selector, id, label, value, options, &[], cx, change)
}

/// `segmented` where the options named in `disabled` are drawn dimmed and cannot be pressed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn segmented_with<T: 'static>(
    selector: &'static str,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    value: &str,
    options: &[(&'static str, &str)],
    disabled: &[&str],
    cx: &mut Context<T>,
    change: impl Fn(&mut T, &'static str, &mut Context<T>) + Clone + 'static,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    div()
        .id(SharedString::from(format!("segmented-{id}")))
        .role(Role::RadioGroup)
        .aria_label(label.into())
        .grid()
        .grid_cols(options.len() as u16)
        .flex_shrink_0()
        .gap(px(2.))
        .rounded_md()
        .border_1()
        .border_color(content(0.1))
        .p(px(2.))
        .text_size(px(12.))
        .line_height(px(16.))
        .children(options.iter().map(|&(option, text)| {
            let selected = value == option;
            let off = disabled.contains(&option);
            let change = change.clone();
            let id = id.clone();
            div()
                .id(SharedString::from(format!("segmented-{id}-{option}")))
                .debug_selector({
                    let id = id.clone();
                    move || format!("{selector}-{id}-{option}")
                })
                .role(Role::RadioButton)
                .aria_label(text.to_owned())
                .aria_selected(selected)
                .min_w_0()
                .flex()
                .justify_center()
                .whitespace_nowrap()
                .rounded(px(5.))
                .px(px(10.))
                .py_1()
                .map(|d| {
                    if off {
                        d.cursor_default().text_color(content(0.25))
                    } else {
                        d.cursor_pointer()
                    }
                })
                .map(|d| {
                    if off {
                        d
                    } else if selected {
                        d.bg(content(0.1)).text_color(content(1.))
                    } else {
                        d.text_color(content(0.5))
                            .hover(|s| s.text_color(content(1.)))
                    }
                })
                .child(text.to_owned())
                .when(!off, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| change(this, option, cx)))
                })
        }))
}
