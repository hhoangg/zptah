use super::*;
use crate::template_gallery::{card_grid, template_card, template_icon};

impl Automations {
    pub(super) fn render_gallery(&self, cx: &mut Context<Self>) -> AnyElement {
        let categories = [
            ("popular", "Popular"),
            ("review", "Code Review"),
            ("security", "Security"),
            ("incidents", "Incidents & Triage"),
            ("research", "Data & Research"),
            ("environment", "Environment"),
        ];
        let mut cards = vec![
            div()
                .id("automation-scratch")
                .role(Role::Button)
                .aria_label("Start from scratch")
                .min_h(px(148.))
                .p(px(16.))
                .rounded(px(12.))
                .border_1()
                .border_color(content(0.15))
                .flex()
                .flex_col()
                .gap(px(12.))
                .hover(|s| s.bg(content(0.05)).border_color(content(0.25)))
                .child(
                    div()
                        .flex()
                        .gap(px(12.))
                        .child(
                            div()
                                .size(px(36.))
                                .rounded_full()
                                .bg(content(0.08))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon("plus", 16.)),
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
                                        .child("Start from scratch"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(content(0.5))
                                        .child("Write your own instructions and choose a trigger."),
                                ),
                        ),
                )
                .on_click(cx.listener(|this, _, _, cx| this.begin(None, cx)))
                .into_any_element(),
        ];
        for template in self.templates.iter().filter(|t| {
            if self.category == "popular" {
                t.popular
            } else {
                t.category == self.category
            }
        }) {
            let selected = template.clone();
            let path = template_icon(&template.icon);
            cards.push(
                template_card(
                    SharedString::from(format!("template-{}", template.id)),
                    path,
                    template.name.clone(),
                    template.description.clone(),
                    Some((
                        trigger_mark(template.trigger["kind"].as_str().unwrap_or("time"), 12.),
                        template.trigger_label.clone().into(),
                    )),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.begin(Some(selected.clone()), cx)))
                .into_any_element(),
            );
        }
        let grid = card_grid(cards);
        div()
            .id("automation-gallery")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .max_w(px(1024.))
                    .mx_auto()
                    .px(px(32.))
                    .pt(px(20.))
                    .pb(px(40.))
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("New automation"),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .text_size(px(13.))
                            .text_color(content(0.5))
                            .child("Pick an example or start from scratch."),
                    )
                    .child(
                        div()
                            .mt(px(16.))
                            .mb(px(16.))
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .children(categories.map(|(id, label)| {
                                let selected = self.category == id;
                                div()
                                    .id(SharedString::from(format!("automation-category-{id}")))
                                    .role(Role::Tab)
                                    .aria_label(label)
                                    .h(px(28.))
                                    .px(px(12.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.))
                                    .when(selected, |d| d.bg(content(1.)).text_color(shell_fill()))
                                    .when(!selected, |d| {
                                        d.text_color(content(0.55)).hover(|s| s.bg(content(0.08)))
                                    })
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.category = id.into();
                                        cx.notify();
                                    }))
                            })),
                    )
                    .child(grid),
            )
            .into_any_element()
    }
}
