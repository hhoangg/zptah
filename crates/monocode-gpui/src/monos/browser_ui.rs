//! A Mono's Browser settings page: switch, Ardia readiness, the Ardia profile, the idle quit,
//! safety rules and the audit log. Design: `docs/superpowers/specs/2026-11-27-ardia-browser.md`.
//!
//! What a Mono keeps is `Mono::browser` in the roster (no secret). Anything that would run or
//! look up Ardia goes through [`BrowserOps`], which tests replace. The Ardia parts of the page
//! are in `browser_ardia.rs`.
use super::panels::{card, heading, switch_setting, switch_track};
use super::*;
use crate::{
    cloud_dialog_ui::alert,
    popover::{ChoiceEvent, Choices, Popover},
};
use gpui::{Div, Stateful};
#[cfg(not(test))]
use monocode_core::mono::browser::ardia::{self, Owner};
use monocode_core::mono::browser::ardia::{ArdiaProfile, Readiness, Report};
use monocode_core::mono::browser::copy;
use monocode_core::mono::browser::{self, AuditEntry, BrowserLevel, MonoBrowser, MonoBrowserStore};
use std::sync::Arc;

/// Drops the Mono's gate (its browser was turned off or changed). It is the app's own state, so
/// this never touches the user's Chrome.
fn stop_server(id: &str) {
    let id = id.to_owned();
    std::thread::spawn(move || browser::server::servers().stop(&id));
}

/// The selector id of a trust level's row (tests click it).
pub(super) fn trust_selector(trust: BrowserLevel) -> &'static str {
    match trust {
        BrowserLevel::ReadOnly => "mono-browser-trust-read-only",
        BrowserLevel::Act => "mono-browser-trust-act",
    }
}
const TRUST_SUBTITLE: &str =
    "How far this Mono may act in Ardia. Change it any time; it applies from the next message.";

/// One level of the trust selector: a radio dot, the level's name and what it means.
fn trust_row(trust: BrowserLevel, selected: bool, cx: &mut Context<Monos>) -> AnyElement {
    let selector = trust_selector(trust);
    div()
        .id(selector)
        .debug_selector(move || selector.into())
        .role(Role::RadioButton)
        .aria_label(super::browser_migration::trust_label(
            trust,
            copy::app_language(),
        ))
        .aria_selected(selected)
        .flex()
        .items_start()
        .gap_3()
        .px_3()
        .py_2()
        .cursor_pointer()
        .hover(|s| s.bg(content(0.04)))
        .on_click(cx.listener(move |this, _, _, cx| this.browser_set_trust(trust, cx)))
        .child(
            div()
                .mt(px(3.))
                .size(px(14.))
                .flex_shrink_0()
                .rounded_full()
                .border_1()
                .border_color(if selected {
                    accent_color()
                } else {
                    content(0.3)
                })
                .flex()
                .items_center()
                .justify_center()
                .when(selected, |d| {
                    d.child(div().size(px(8.)).rounded_full().bg(accent_color()))
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(content(0.9))
                        .child(super::browser_migration::trust_label(
                            trust,
                            copy::app_language(),
                        )),
                )
                .child(note(format!("{selector}-text"), trust.explanation(), 0.5)),
        )
        .into_any_element()
}

const SELECT_PRIORITY: usize = 90;
const AUDIT_ROWS: usize = 20;
pub(super) const SUBTITLE: &str = "Let this Mono use Ardia, with the sessions you are signed in to in the chosen Ardia profile. Off by default.";

/// What runs or looks up Ardia. The real one asks the process-wide Ardia controller; every method
/// except the two `ardia_owned` and `ardia_can_launch` lookups blocks on an `ardia` process, so the
/// page calls them off the UI thread.
pub(super) trait BrowserOps: Send + Sync {
    /// The readiness of the Mono's profile (see `Controller::report`). Never launches Ardia.
    fn ardia_report(&self, profile: &str) -> Report;
    /// The profiles of the running Ardia.
    fn ardia_profiles(&self) -> Result<Vec<ArdiaProfile>, Readiness>;
    /// Starts a closed Ardia in the background. `Ok(true)`: zptah started it now.
    fn ardia_start(&self) -> Result<bool, Readiness>;
    /// Closes the session tabs and quits an Ardia that zptah started.
    fn ardia_quit(&self) -> Result<(), String>;
    /// Whether zptah may start and quit Ardia on this platform.
    fn ardia_can_launch(&self) -> bool;
    /// Whether zptah started the running Ardia. A lock lookup, safe on the UI thread.
    fn ardia_owned(&self) -> bool;
    /// The minutes of idleness after which zptah quits an Ardia it started (0 = never).
    fn ardia_set_idle_minutes(&self, minutes: u32);
}
#[cfg(not(test))]
struct RealOps;
#[cfg(not(test))]
impl BrowserOps for RealOps {
    fn ardia_report(&self, profile: &str) -> Report {
        ardia::controller().report(profile)
    }
    fn ardia_profiles(&self) -> Result<Vec<ArdiaProfile>, Readiness> {
        ardia::controller().profiles()
    }
    fn ardia_start(&self) -> Result<bool, Readiness> {
        ardia::controller().start_now()
    }
    fn ardia_quit(&self) -> Result<(), String> {
        ardia::controller().quit_now()
    }
    fn ardia_can_launch(&self) -> bool {
        ardia::controller().platform().can_launch()
    }
    fn ardia_owned(&self) -> bool {
        ardia::controller().owner() == Owner::Zptah
    }
    fn ardia_set_idle_minutes(&self, minutes: u32) {
        ardia::controller().set_idle_minutes(minutes);
    }
}

/// What tests get until they install their own: Ardia closed, nothing to start or quit, and never
/// a look at the real app.
#[cfg(test)]
struct InertOps;
#[cfg(test)]
impl BrowserOps for InertOps {
    fn ardia_report(&self, _profile: &str) -> Report {
        Report {
            failure: None,
            running: false,
            cached: None,
            owner: monocode_core::mono::browser::ardia::Owner::Nobody,
        }
    }
    fn ardia_profiles(&self) -> Result<Vec<ArdiaProfile>, Readiness> {
        Err(Readiness::NotRunning)
    }
    fn ardia_start(&self) -> Result<bool, Readiness> {
        Err(Readiness::NotRunning)
    }
    fn ardia_quit(&self) -> Result<(), String> {
        Ok(())
    }
    fn ardia_can_launch(&self) -> bool {
        false
    }
    fn ardia_owned(&self) -> bool {
        false
    }
    fn ardia_set_idle_minutes(&self, _minutes: u32) {}
}

#[cfg(test)]
fn default_ops() -> Arc<dyn BrowserOps> {
    Arc::new(InertOps)
}
#[cfg(not(test))]
fn default_ops() -> Arc<dyn BrowserOps> {
    Arc::new(RealOps)
}

/// The page's own state; the setting itself is on the Mono.
pub(super) struct BrowserUi {
    pub(super) ops: Arc<dyn BrowserOps>,
    pub(super) ardia: super::browser_ardia::ArdiaUi,
    pub(super) audit: Vec<AuditEntry>,
    pub(super) select: Option<Choices<String>>,
    pub(super) profile_anchor: AnchorSlot,
    pub(super) error: Option<String>,
    pub(super) approval: super::browser_approval::ApprovalUi,
}
impl BrowserUi {
    pub(super) fn new() -> Self {
        Self {
            ops: default_ops(),
            ardia: Default::default(),
            audit: Vec::new(),
            select: None,
            profile_anchor: AnchorSlot::default(),
            error: None,
            approval: super::browser_approval::ApprovalUi::new(),
        }
    }
    #[cfg(test)]
    pub(super) fn use_fakes(&mut self, ops: Arc<dyn BrowserOps>) {
        self.ops = ops;
    }
    #[cfg(test)]
    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    #[cfg(test)]
    pub(super) fn select_is_open(&self) -> bool {
        self.select.is_some()
    }
}

pub(super) fn action(id: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
    crate::modal::ghost_button(id, label)
        .debug_selector(move || id.into())
        .border_1()
        .border_color(content(0.1))
        .cursor_pointer()
}
pub(super) fn note(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    alpha: f32,
) -> Stateful<Div> {
    let id = id.into();
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(content(alpha))
        .child(text.into())
}
pub(super) fn ago(ts_ms: u64) -> String {
    let seconds = monocode_core::time::now_ms().saturating_sub(ts_ms) / 1000;
    match seconds {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}

impl Monos {
    pub(super) fn browser_store(&self) -> Result<MonoBrowserStore, String> {
        let store = locked(&self.store)?;
        MonoBrowserStore::for_store(&store)
    }

    /// Reads what the page shows: the audit log and the idle setting, then asks Ardia for its
    /// readiness and profiles on a worker.
    pub(super) fn browser_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            return;
        };
        self.browser_approval_page_refresh(cx);
        let store = self.browser_store().ok();
        let ui = &mut self.browser_ui;
        ui.audit = store
            .as_ref()
            .map(|s| s.recent(&id, AUDIT_ROWS))
            .unwrap_or_default();
        self.browser_ardia_load_idle(cx);
        self.browser_ardia_refresh(cx);
        cx.notify();
    }

    pub(super) fn browser_edit(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut MonoBrowser),
    ) {
        let Some(id) = self.active.clone() else {
            return;
        };
        let Some(mono) = self.roster.get_mut(&id) else {
            return;
        };
        let setting = mono.browser.get_or_insert_with(MonoBrowser::default);
        let before = setting.clone();
        change(setting);
        setting.normalize();
        // The gate starts again with the new setting on the next run. A change that does not
        // shape it (the trust level) keeps it; the running gate takes the new level at once.
        if before.server_key() != setting.server_key() {
            stop_server(&id);
        }
        browser::server::servers().update_policy(&id, setting.policy());
        self.save(cx);
        self.configure_chat_with(&id, false, cx);
        self.browser_ui.error = None;
        cx.notify();
    }

    /// A level row was chosen.
    pub(super) fn browser_set_trust(&mut self, trust: BrowserLevel, cx: &mut Context<Self>) {
        self.browser_edit(cx, |b| b.trust = trust);
    }

    fn browser_toggle(&mut self, cx: &mut Context<Self>) {
        self.browser_edit(cx, |b| b.enabled = !b.enabled);
        self.browser_refresh(cx);
    }

    pub(super) fn browser_setting(&self) -> MonoBrowser {
        self.current()
            .and_then(|m| m.browser.clone())
            .unwrap_or_default()
    }

    pub(super) fn browser_open_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.browser_ui.select.take() {
            open.close(window, cx);
            cx.notify();
            return;
        }
        let choices = self.browser_ardia_choices();
        if choices.is_empty() {
            return;
        }
        let width = self
            .browser_ui
            .profile_anchor
            .get()
            .map_or(260., |b| b.size.width.as_f32().max(220.));
        self.browser_ui.select = Some(Choices::open(
            "Ardia profile",
            choices,
            width,
            window,
            cx,
            |this, event, window, cx| {
                if let Some(open) = this.browser_ui.select.take() {
                    open.close(window, cx);
                }
                if let ChoiceEvent::Picked(dir) = event {
                    this.browser_choose_profile(&dir, cx);
                }
                cx.notify();
            },
        ));
        cx.notify();
    }

    pub(super) fn browser_select_popover(&self, cx: &App) -> Option<Popover> {
        let open = self.browser_ui.select.as_ref()?;
        Some(open.popover(&self.browser_ui.profile_anchor, SELECT_PRIORITY, cx))
    }

    /// A choice always takes effect, also when Ardia's profile list cannot be read.
    pub(super) fn browser_choose_profile(&mut self, id: &str, cx: &mut Context<Self>) {
        // A listed profile, else the saved one, else any valid id.
        let saved = self.browser_setting();
        let Some(profile) = self
            .browser_ui
            .ardia
            .listed()
            .iter()
            .find(|p| p.id == id)
            .map(|p| (p.id.clone(), p.name.clone()))
            .or_else(|| {
                (saved.ardia_profile == id && !saved.ardia_profile_name.is_empty())
                    .then(|| (id.to_owned(), saved.ardia_profile_name.clone()))
            })
            .or_else(|| browser::valid_ardia_profile(id).then(|| (id.to_owned(), id.to_owned())))
        else {
            return;
        };
        self.browser_edit(cx, |b| {
            b.ardia_profile = profile.0.clone();
            b.ardia_profile_name = profile.1.clone();
        });
        self.browser_refresh(cx);
    }

    pub(super) fn browser_add_origin(&mut self, allowed: bool, cx: &mut Context<Self>) {
        let key = if allowed {
            "browser-allowed-add"
        } else {
            "browser-blocked-add"
        };
        let text = self.value(key, cx);
        if text.trim().is_empty() {
            return;
        }
        match browser::normalize_origin(&text) {
            Ok(origin) => {
                self.browser_edit(cx, |b| {
                    let list = if allowed {
                        &mut b.allowed_origins
                    } else {
                        &mut b.blocked_origins
                    };
                    list.push(origin);
                });
                if let Some(input) = self.fields.get(key).cloned() {
                    input.update(cx, |f, cx| f.set_text(String::new(), cx));
                }
            }
            Err(error) => {
                self.browser_ui.error = Some(error);
                cx.notify();
            }
        }
    }

    pub(super) fn browser_remove_origin(
        &mut self,
        allowed: bool,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.browser_edit(cx, |b| {
            let list = if allowed {
                &mut b.allowed_origins
            } else {
                &mut b.blocked_origins
            };
            if index < list.len() {
                list.remove(index);
            }
        });
    }

    fn browser_origin_card(
        &mut self,
        allowed: bool,
        origins: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind: &'static str = if allowed { "allowed" } else { "blocked" };
        let key = format!("browser-{kind}-add");
        let input = self.field(
            &key,
            "Add an origin, for example https://example.com",
            String::new(),
            false,
            cx,
        );
        if self.field_blurs.insert(input.entity_id()) {
            self.observations.push(
                cx.subscribe(&input, move |this, _, _: &input::Submitted, cx| {
                    this.browser_add_origin(allowed, cx)
                }),
            );
        }
        let _ = window;
        let rows = origins.iter().enumerate().map(|(ix, origin)| {
            let row = format!("mono-browser-{kind}-{ix}");
            let remove = format!("mono-browser-{kind}-remove-{ix}");
            div()
                .id(SharedString::from(row.clone()))
                .debug_selector(move || row.clone())
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(28.))
                .text_size(px(12.))
                .text_color(content(0.8))
                .child(div().flex_1().min_w_0().truncate().child(origin.clone()))
                .child(
                    crate::modal::ghost_button(SharedString::from(remove.clone()), "Remove")
                        .debug_selector(move || remove.clone())
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.browser_remove_origin(allowed, ix, cx)
                        })),
                )
        });
        let mut footer = div().flex().items_center().gap_2().px_3().py_2().child(
            div()
                .id(SharedString::from(format!("mono-browser-{kind}-input")))
                .flex_1()
                .min_w_0()
                .h(px(28.))
                .px_2()
                .flex()
                .items_center()
                .rounded_md()
                .border_1()
                .border_color(content(0.1))
                .child(input),
        );
        footer = footer.child(
            if allowed {
                action("mono-browser-allowed-add", "Add")
            } else {
                action("mono-browser-blocked-add", "Add")
            }
            .on_click(cx.listener(move |this, _, _, cx| this.browser_add_origin(allowed, cx))),
        );
        card()
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .child(if allowed {
                        "Allowed origins. When any are listed, only these may be used."
                    } else {
                        "Blocked origins: the Mono never opens these. Banking and payment sites are not blocked by default: zptah asks before a risky step there."
                    }),
            )
            .children(rows)
            .child(footer)
            .into_any_element()
    }

    pub(super) fn browser_page(
        &mut self,
        mono: &Mono,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let setting = mono.browser.clone().unwrap_or_default();
        let mut sections: Vec<AnyElement> =
            vec![note("mono-browser-subtitle", SUBTITLE, 0.55).into_any_element()];
        if let Some(error) = self.browser_ui.error.clone() {
            sections.push(alert("mono-browser-error", error).into_any_element());
        }
        if let Some(reason) = browser::server::servers()
            .attach_error(&mono.id)
            .filter(|_| setting.enabled)
        {
            sections.push(
                alert(
                    "mono-browser-attach-error",
                    format!("The browser could not be attached to the last run: {reason}."),
                )
                .into_any_element(),
            );
        }
        let toggle = switch_track("mono-browser-enabled", "Use the browser", setting.enabled)
            .debug_selector(|| "mono-browser-enabled".into())
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.browser_toggle(cx)));
        sections.push(
            card()
                .child(switch_setting(
                    "Use the browser",
                    "Off by default. The Mono drives the Ardia profile you choose.",
                    false,
                    toggle.into_any_element(),
                ))
                .into_any_element(),
        );
        if setting.enabled {
            sections.extend(self.browser_ardia_cards(mono, cx));
            sections.push(self.browser_safety_card(&setting, window, cx));
            sections.extend(self.browser_extension_cards(mono, &setting, cx));
            sections.push(self.browser_audit_card());
        }
        div()
            .id("mono-browser-page")
            .debug_selector(|| "mono-browser-page".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(Self::settings_page_header("Browser"))
            .child(
                div()
                    .id("mono-browser-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(
                        sections
                            .into_iter()
                            .map(|section| div().flex_shrink_0().child(section)),
                    ),
            )
            .into_any_element()
    }

    fn browser_safety_card(
        &mut self,
        setting_: &MonoBrowser,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows: Vec<AnyElement> = super::browser_migration::CHOICES
            .into_iter()
            .map(|trust| trust_row(trust, setting_.trust == trust, cx))
            .collect();
        let blocked = self.browser_origin_card(false, &setting_.blocked_origins, window, cx);
        let allowed = self.browser_origin_card(true, &setting_.allowed_origins, window, cx);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading("Trust level", Some(TRUST_SUBTITLE)))
            .child(card().children(rows))
            .child(
                note(
                    "mono-browser-trust-note",
                    BrowserLevel::ENFORCEMENT_NOTE,
                    0.5,
                )
                .px_1()
                .pb_2(),
            )
            .child(blocked)
            .child(allowed)
            .into_any_element()
    }

    fn browser_audit_card(&self) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .browser_ui
            .audit
            .iter()
            .enumerate()
            .map(|(ix, e)| {
                let line = [e.summary.as_str(), e.url.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join("  ");
                div()
                    .flex()
                    .gap_3()
                    .px_3()
                    .py_1()
                    .child(
                        note(format!("mono-browser-audit-time-{ix}"), ago(e.ts_ms), 0.4).w(px(64.)),
                    )
                    .child(
                        note(
                            format!("mono-browser-audit-{ix}"),
                            format!("{}  {line}", e.tool),
                            0.8,
                        )
                        .flex_1()
                        .min_w_0()
                        .truncate(),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading("Activity", Some("Pages and actions of this Mono's browser, newest first. Typed values are never kept.")))
            .child(card().children(rows).when(empty, |c| {
                c.child(div().px_3().py_3().child(note("mono-browser-audit-empty", "Nothing yet", 0.45)))
            }))
            .into_any_element()
    }

    /// A deleted Mono leaves no token and no audit rows.
    pub(super) fn browser_forget(&mut self, id: &str) {
        stop_server(id);
        if let Ok(store) = self.browser_store() {
            let _ = store.delete_mono(id);
        }
    }
}
