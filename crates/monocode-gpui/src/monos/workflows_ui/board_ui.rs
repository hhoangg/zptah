//! The board's banners, loop card and leave dialog (`WorkflowBoard.tsx`). The page that holds them
//! is in `layout.rs`; the docked inspector is in `inspector.rs`.
use super::super::*;
use gpui::{Div, FontWeight, Stateful};
use monocode_core::mono::workflows::{board_edit::REVIEW_FIRST, board_model::NodeType};

/// The icon of a node type in the picker.
pub(super) fn type_glyph(ty: NodeType) -> &'static str {
    match ty {
        NodeType::Schedule => "clock",
        NodeType::Manual => "play",
        NodeType::Event => "zap",
        NodeType::Session => "message-multiple",
        NodeType::Command => "terminal",
        NodeType::If => "git-branch",
        NodeType::Switch => "share",
        NodeType::ForEach => "refresh-cw",
    }
}

/// A small text button of a banner.
pub(super) fn banner_button(id: &'static str, label: &'static str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Button)
        .aria_label(label)
        .flex_shrink_0()
        .rounded_sm()
        .px_1p5()
        .py_0p5()
        .text_color(content(0.7))
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
        })
        .when(!enabled, |d| d.opacity(0.4))
        .child(label)
}

fn banner(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .w_full()
        .flex_shrink_0()
        .border_b_1()
        .border_color(content(0.1))
        .px_4()
        .py_1p5()
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(content(0.6))
}

impl Monos {
    /// The notices under the header: changed elsewhere, review first, and the problems.
    pub(super) fn board_banners(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(b) = self.wf.board.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let (saving, blocked) = (b.saving, b.blocked());
        if let Some((_, refused)) = &b.elsewhere {
            let refused = *refused;
            out.push(
                banner("wf-board-changed")
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x_2()
                    .gap_y_1()
                    .role(Role::Status)
                    .child(div().flex_1().min_w_0().child(if refused {
                        "This workflow changed elsewhere, so your changes were not saved"
                    } else {
                        "This workflow changed elsewhere"
                    }))
                    .child(
                        banner_button("wf-board-reload", "Reload", true).on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some((stored, _)) =
                                    this.wf.board.as_ref().and_then(|b| b.elsewhere.clone())
                                {
                                    this.board_load(stored, cx);
                                }
                            },
                        )),
                    )
                    .child(if refused {
                        banner_button("wf-board-overwrite", "Overwrite", !saving && !blocked)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !saving && !blocked {
                                    this.board_save(false, true, false, cx);
                                }
                            }))
                    } else {
                        banner_button("wf-board-keep", "Keep mine", true).on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(b) = this.wf.board.as_mut() {
                                    b.elsewhere = None;
                                }
                                cx.notify();
                            },
                        ))
                    })
                    .into_any_element(),
            );
        }
        if b.unreviewed() {
            let none = b.issues.is_empty();
            out.push(
                banner("wf-board-review")
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(REVIEW_FIRST))
                    .child(
                        banner_button("wf-board-review-open", "Review", true).on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some(id) = this.wf.board.as_ref().map(|b| b.base.id.clone())
                                {
                                    let back = this.wf.route.clone();
                                    this.open_workflow_review(
                                        &id,
                                        super::ReviewIntent::Enable,
                                        back,
                                        cx,
                                    );
                                }
                            }),
                        ),
                    )
                    .when(none, |d| {
                        d.child(
                            banner_button("wf-board-save-paused", "Save paused", !saving).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.board_save(true, false, false, cx)
                                }),
                            ),
                        )
                    })
                    .into_any_element(),
            );
        }
        out
    }

    /// "New loop": how many rounds and who decides, asked where the connection was let go.
    pub(super) fn board_loop_card(
        &mut self,
        ask: &super::board::LoopAsk,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let origin = self
            .wf
            .board
            .as_ref()
            .and_then(|b| b.area.get())
            .map(|a| a.origin)
            .unwrap_or_default();
        let width = 296.;
        let area_width = self
            .wf
            .board
            .as_ref()
            .and_then(|b| b.area.get())
            .map_or(width + 16., |a| a.size.width.as_f32());
        let left = (ask.at.x - origin.x)
            .as_f32()
            .clamp(8., (area_width - width - 8.).max(8.));
        let top = (ask.at.y - origin.y).as_f32().max(8.);
        let target = self
            .wf
            .board
            .as_ref()
            .and_then(|b| b.node(&ask.to))
            .map(super::review::node_title)
            .unwrap_or_default();
        let form = self.board_loop_form(None, &ask.spec, window, cx);
        div()
            .id("wf-loop-ask")
            .debug_selector(|| "wf-loop-ask".into())
            .role(Role::Dialog)
            .aria_label("New loop")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .p_2()
            .rounded_lg()
            .border_1()
            .border_color(content(0.12))
            .bg(floating_fill())
            .occlude()
            .shadow(crate::popover::shadow())
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(12.))
                    .line_height(px(20.))
                    .text_color(content(0.7))
                    .child(format!("Go back to {target} until it is right.")),
            )
            .child(form)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .px_1()
                    .pt_2()
                    .child(
                        div()
                            .id("wf-loop-cancel")
                            .debug_selector(|| "wf-loop-cancel".into())
                            .role(Role::Button)
                            .aria_label("Cancel")
                            .cursor_pointer()
                            .rounded_md()
                            .px_3()
                            .py_1()
                            .text_size(px(12.))
                            .text_color(content(0.7))
                            .hover(|s| s.bg(content(0.08)))
                            .child("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.board_cancel_loop(cx))),
                    )
                    .child(
                        div()
                            .id("wf-loop-add")
                            .debug_selector(|| "wf-loop-add".into())
                            .role(Role::Button)
                            .aria_label("Add loop")
                            .cursor_pointer()
                            .rounded_md()
                            .px_3()
                            .py_1()
                            .bg(content(0.1))
                            .text_size(px(12.))
                            .hover(|s| s.bg(content(0.14)))
                            .child("Add loop")
                            .on_click(cx.listener(|this, _, _, cx| this.board_confirm_loop(cx))),
                    ),
            )
            .into_any_element()
    }

    // ---- leaving ----------------------------------------------------------------------------

    /// "Save your changes?"
    pub(super) fn board_leave_dialog(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let b = self.wf.board.as_ref()?;
        // Only a board route asks: a review on top of it has its own Cancel.
        if !b.leaving || !self.wf_board_open() {
            return None;
        }
        let (blocked, saving) = (b.blocked(), b.saving);
        let text = if blocked {
            format!(
                "This workflow has changes you have not saved, and {}.",
                if b.issues.is_empty() {
                    "it needs a review before it can be enabled"
                } else {
                    "problems that keep it from being saved while enabled"
                }
            )
        } else {
            "This workflow has changes you have not saved.".to_string()
        };
        let size = window.viewport_size();
        let weak = cx.weak_entity();
        let width = 400f32.min(size.width.as_f32() - 24.);
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(gpui::point(px(0.), px(0.)))
                            .child(
                                div()
                                    .w(size.width)
                                    .h(size.height)
                                    .bg(crate::modal::scrim_color())
                                    .occlude(),
                            ),
                    )
                    .with_priority(79),
                )
                .child(
                    popover("wf-board-leave-modal", Anchor::Center)
                        .modal_surface(true)
                        .width(width)
                        .gap(0.)
                        .priority(80)
                        .on_dismiss(move |_, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                if let Some(b) = this.wf.board.as_mut() {
                                    b.leaving = false;
                                    b.leave_to = None;
                                }
                                cx.notify();
                            });
                        })
                        .child(
                            div()
                                .id("wf-board-leave")
                                .debug_selector(|| "wf-board-leave".into())
                                .role(Role::Dialog)
                                .aria_label("Save your changes?")
                                .p_4()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(content(1.))
                                        .child("Save your changes?"),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .line_height(px(19.))
                                        .text_color(content(0.7))
                                        .child(text),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            crate::modal::ghost_button(
                                                "wf-board-keep-editing",
                                                "Keep editing",
                                            )
                                            .debug_selector(|| "wf-board-keep-editing".into())
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    if let Some(b) = this.wf.board.as_mut() {
                                                        b.leaving = false;
                                                        b.leave_to = None;
                                                    }
                                                    cx.notify();
                                                }),
                                            ),
                                        )
                                        .child(
                                            crate::modal::ghost_button(
                                                "wf-board-discard",
                                                "Discard",
                                            )
                                            .debug_selector(|| "wf-board-discard".into())
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.board_back(cx)),
                                            ),
                                        )
                                        .child(
                                            crate::modal::disabled(
                                                crate::modal::primary_button(
                                                    "wf-board-leave-save",
                                                    if blocked { "Save paused" } else { "Save" },
                                                )
                                                .debug_selector(|| "wf-board-leave-save".into()),
                                                saving,
                                            )
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    if !saving {
                                                        this.board_save(blocked, false, true, cx);
                                                    }
                                                }),
                                            ),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
