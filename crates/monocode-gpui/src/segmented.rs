//! The segmented control: equal-width options in a hairline frame, one of them chosen. Shared so
//! any entity can draw one (the Workflows page header uses it for Edit, Runs and Settings).
use crate::theme::*;
use gpui::{Context, Div, Role, SharedString, Stateful, div, prelude::*, px};

/// Draws the control. `value` is the chosen option's id, `options` are `(id, text)` pairs and
/// `change` hears the id of the option pressed. Each option has the selector
/// `segmented-<id>-<option>`, and the whole control `segmented-<id>`.
pub(crate) fn segmented<T: 'static>(
    id: &'static str,
    label: &'static str,
    value: &str,
    options: &[(&'static str, &'static str)],
    cx: &mut Context<T>,
    change: impl Fn(&mut T, &'static str, &mut Context<T>) + Clone + 'static,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("segmented-{id}")))
        .role(Role::RadioGroup)
        .aria_label(label)
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
            let change = change.clone();
            div()
                .id(SharedString::from(format!("segmented-{id}-{option}")))
                .debug_selector(move || format!("segmented-{id}-{option}"))
                .role(Role::RadioButton)
                .aria_label(text)
                .aria_selected(selected)
                .min_w_0()
                .flex()
                .justify_center()
                .whitespace_nowrap()
                .rounded(px(5.))
                .px(px(10.))
                .py(px(2.))
                .cursor_pointer()
                .map(|d| {
                    if selected {
                        d.bg(content(0.1)).text_color(content(1.))
                    } else {
                        d.text_color(content(0.5))
                            .hover(|s| s.text_color(content(1.)))
                    }
                })
                .child(text)
                .on_click(cx.listener(move |this, _, _, cx| change(this, option, cx)))
        }))
}
