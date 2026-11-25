//! The Settings > Browser cards of the one-time migration (spec 15.3, 15.4, decision 15 of
//! section 18): the trust and tab-group notice, the offer to switch to the new blocked list and
//! the "Reconnect needed" state. Their text follows the app language like the approval cards.
use super::browser_ui::{action, note};
use super::panels::{card, setting};
use super::*;
use monocode_core::mono::browser::copy::{self, Lang};
use monocode_core::mono::browser::migration::{RECONNECT_NEEDED, TAB_GROUP_NOTICE, TRUST_NOTICE};
use monocode_core::mono::browser::{BrowserTrust, MonoBrowser};

/// The trust levels a user can choose. `Ask every` and `Autonomous` are gone from the page.
pub(super) const CHOICES: [BrowserTrust; 2] = [BrowserTrust::ReadOnly, BrowserTrust::AskRisky];

/// Whether `trust` is the shown choice `choice`: a stored legacy level displays as `Act for me`.
pub(super) fn shows_as(trust: BrowserTrust, choice: BrowserTrust) -> bool {
    match choice {
        BrowserTrust::ReadOnly => trust == BrowserTrust::ReadOnly,
        _ => trust != BrowserTrust::ReadOnly,
    }
}

pub(super) fn trust_label(trust: BrowserTrust, lang: Lang) -> &'static str {
    match (trust, lang) {
        (BrowserTrust::ReadOnly, Lang::En) => "Read only",
        (BrowserTrust::ReadOnly, Lang::Vi) => "Chỉ đọc",
        (_, Lang::En) => "Act for me",
        (_, Lang::Vi) => "Làm giúp tôi",
    }
}

struct Text {
    notice_title: &'static str,
    trust: &'static str,
    tab_group: &'static str,
    dismiss: &'static str,
    suggest_title: &'static str,
    suggest_line: fn(&str) -> String,
    switch: &'static str,
    keep: &'static str,
    reconnect_title: &'static str,
    reconnect_line: &'static str,
    reconnect_button: &'static str,
}

fn text(lang: Lang) -> Text {
    match lang {
        Lang::En => Text {
            notice_title: "What changed in zptah Browser",
            trust: TRUST_NOTICE,
            tab_group: TAB_GROUP_NOTICE,
            dismiss: "Dismiss",
            suggest_title: "New defaults for banking and payment sites",
            suggest_line: |mono| {
                format!(
                    "zptah now asks before risky steps on banking and payment sites instead of blocking them. Switch {mono} to the new defaults?"
                )
            },
            switch: "Switch to the new defaults",
            keep: "Keep my list",
            reconnect_title: RECONNECT_NEEDED,
            reconnect_line: "This Mono kept its Chrome profile but is not bound to a zptah Browser connection yet. Connect the extension in Browser connection.",
            reconnect_button: "Connect extension",
        },
        Lang::Vi => Text {
            notice_title: "Có gì thay đổi trong zptah Browser",
            trust: "Từ nay các Mono chỉ hỏi bạn trước những bước rủi ro như thanh toán và xóa dữ liệu.",
            tab_group: "zptah Browser tạo cho mỗi Mono một nhóm tab mới. Để chuyển nhóm Playwright cũ sang, hãy kéo các tab của nó vào nhóm mới. Bạn có thể gỡ \"Playwright MCP Bridge\" tại chrome://extensions.",
            dismiss: "Đóng",
            suggest_title: "Mặc định mới cho trang ngân hàng và thanh toán",
            suggest_line: |mono| {
                format!(
                    "zptah giờ hỏi trước các bước rủi ro trên trang ngân hàng và thanh toán thay vì chặn. Chuyển {mono} sang mặc định mới? Danh sách trang web bị chặn của bạn sẽ không bị xóa nếu bạn chọn giữ."
                )
            },
            switch: "Chuyển sang mặc định mới",
            keep: "Giữ danh sách của tôi",
            reconnect_title: "Cần kết nối lại",
            reconnect_line: "Mono này vẫn giữ hồ sơ Chrome nhưng chưa gắn với kết nối zptah Browser nào. Hãy kết nối tiện ích trong mục Kết nối trình duyệt.",
            reconnect_button: "Kết nối tiện ích",
        },
    }
}

impl Monos {
    /// The one-time notice of a migrated Mono, until it is dismissed.
    pub(super) fn browser_notice_card(
        &mut self,
        browser: &MonoBrowser,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !browser.migration_notice {
            return None;
        }
        let t = text(copy::app_language());
        Some(
            card()
                .child(setting(
                    t.notice_title,
                    None,
                    action("mono-browser-notice-dismiss", t.dismiss)
                        .on_click(cx.listener(|this, _, _, cx| this.browser_dismiss_notice(cx)))
                        .into_any_element(),
                ))
                .child(
                    div()
                        .px_3()
                        .pb_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(note("mono-browser-notice-trust", t.trust, 0.7))
                        .child(note("mono-browser-notice-tab-group", t.tab_group, 0.7)),
                )
                .into_any_element(),
        )
    }

    /// The offer to move from the old default blocked list to the new one, shown once.
    pub(super) fn browser_suggest_card(
        &mut self,
        mono: &str,
        browser: &MonoBrowser,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !browser.suggest_new_defaults {
            return None;
        }
        let t = text(copy::app_language());
        Some(
            card()
                .child(setting(
                    t.suggest_title,
                    Some(&(t.suggest_line)(mono)),
                    div().into_any_element(),
                ))
                .child(
                    div()
                        .px_3()
                        .pb_2()
                        .flex()
                        .gap_2()
                        .child(action("mono-browser-defaults-switch", t.switch).on_click(
                            cx.listener(|this, _, _, cx| this.browser_switch_defaults(cx)),
                        ))
                        .child(
                            action("mono-browser-defaults-keep", t.keep)
                                .on_click(cx.listener(|this, _, _, cx| this.browser_keep_list(cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// "Reconnect needed": a migrated Mono with a profile and no bound connection. It sits right
    /// above the Browser connection card it points at.
    pub(super) fn browser_reconnect_card(
        &mut self,
        browser: &MonoBrowser,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !browser.enabled
            || browser.migrated == 0
            || !browser.use_extension
            || !browser.needs_reconnect()
        {
            return None;
        }
        let t = text(copy::app_language());
        Some(
            card()
                .child(setting(
                    t.reconnect_title,
                    Some(t.reconnect_line),
                    action("mono-browser-reconnect", t.reconnect_button)
                        .on_click(cx.listener(|this, _, _, cx| this.browser_connect_start(cx)))
                        .into_any_element(),
                ))
                .into_any_element(),
        )
    }

    pub(super) fn browser_dismiss_notice(&mut self, cx: &mut Context<Self>) {
        self.browser_edit(cx, |b| b.migration_notice = false);
    }

    pub(super) fn browser_switch_defaults(&mut self, cx: &mut Context<Self>) {
        self.browser_edit(cx, |b| b.switch_to_new_defaults());
    }

    pub(super) fn browser_keep_list(&mut self, cx: &mut Context<Self>) {
        self.browser_edit(cx, |b| b.keep_blocked_list());
    }
}
