//! The card and marks the template galleries share (workflow templates).
use crate::theme::*;
use gpui::{AnyElement, Role, SharedString, Stateful, div, prelude::*, px, rgb};

/// Providers whose mark keeps its own colours instead of taking the text colour.
pub(crate) fn color_artwork(kind: &str) -> Option<&'static str> {
    match kind {
        "gitlab" => Some("inbox-gitlab-color.svg"),
        "clickup" => Some("inbox-clickup.svg"),
        _ => None,
    }
}

/// Event providers and schedules are separate from agent harness branding.
pub(crate) fn trigger_mark(kind: &str, side: f32) -> AnyElement {
    if let Some(path) = color_artwork(kind) {
        return gpui::img(path)
            .size(px(side))
            .flex_shrink_0()
            .object_fit(gpui::ObjectFit::Contain)
            .into_any_element();
    }
    let (path, color) = match kind {
        "time" => ("clock", content(0.45)),
        "github" => ("inbox-github", content(0.65)),
        "linear" => ("inbox-linear", content(0.65)),
        "jira" => ("inbox-jira", rgb(0x2684ff).into()),
        "azuredevops" => ("inbox-azuredevops", content(0.65)),
        _ => ("zap", content(0.45)),
    };
    icon(path, side).text_color(color).into_any_element()
}

/// The glyph a template's icon name stands for.
pub(crate) fn template_icon(name: &str) -> &'static str {
    match name {
        "alert" => "alert-circle",
        "lock" => "lock",
        "file" => "file",
        "check" => "check",
        "pr" => "git-pull-request",
        "inbox" => "inbox",
        "terminal" => "terminal",
        "note" => "note",
        "gauge" => "gauge",
        _ => "search",
    }
}

/// One template card: a round glyph, the name and description, and the trigger line at the
/// foot. The caller adds the click handler.
pub(crate) fn template_card(
    id: impl Into<SharedString>,
    glyph: &'static str,
    name: impl Into<SharedString>,
    description: impl Into<SharedString>,
    trigger: Option<(AnyElement, SharedString)>,
) -> Stateful<gpui::Div> {
    let name: SharedString = name.into();
    div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(name.clone())
        .min_h(px(148.))
        .p(px(16.))
        .rounded(px(12.))
        .border_1()
        .border_color(content(0.1))
        .flex()
        .flex_col()
        .gap(px(12.))
        .hover(|s| s.bg(content(0.05)).border_color(content(0.16)))
        .child(
            div()
                .flex()
                .gap(px(12.))
                .child(
                    div()
                        .size(px(36.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(content(0.08))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(glyph, 16.)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(name),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(content(0.5))
                                .child(description.into()),
                        ),
                ),
        )
        .children(trigger.map(|(mark, label)| {
            div()
                .mt_auto()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(11.))
                .text_color(content(0.45))
                .child(mark)
                .child(label)
        }))
}

/// The cards in CSS's two-column grid.
pub(crate) fn card_grid(cards: Vec<AnyElement>) -> AnyElement {
    let mut grid = div().flex().flex_col().gap(px(12.));
    let mut iter = cards.into_iter();
    while let Some(left) = iter.next() {
        let right = iter.next();
        grid = grid.child(
            div()
                .flex()
                .gap(px(12.))
                .child(div().flex_1().min_w_0().child(left))
                .child(div().flex_1().min_w_0().children(right)),
        );
    }
    grid.into_any_element()
}
