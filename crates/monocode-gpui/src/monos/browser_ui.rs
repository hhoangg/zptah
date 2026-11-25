//! A Mono's Browser settings page: switch, Chrome check, Chrome profile, connection to the zptah
//! Browser extension, safety rules, recipes and the audit log. Design:
//! `docs/superpowers/specs/2026-11-25-zptah-browser-extension.md`.
//!
//! What a Mono keeps is `Mono::browser` in the roster (no secret). Anything that would touch
//! Chrome, System Settings or a running gate goes through [`BrowserOps`], which tests replace.
use super::panels::{card, heading, setting, switch_setting, switch_track};
use super::*;
use crate::{
    cloud_dialog_ui::alert,
    popover::{Choice, ChoiceEvent, Choices, Popover},
};
use gpui::{Div, Stateful};
use monocode_core::mono::browser::copy;
use monocode_core::mono::browser::recipe::manage::DryRunReport;
use monocode_core::mono::browser::{
    self, AuditEntry, BrowserLevel, Check, ChromeProfile, MonoBrowser, MonoBrowserStore, PrereqEnv,
    Prerequisites,
};
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
const TRUST_SUBTITLE: &str = "How far this Mono may act in your Chrome. Change it any time; it applies from the next message.";

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

/// The Full Disk Access pane of System Settings. The scheme has a dot: `x-apple.systempreferences`.
#[cfg(any(target_os = "macos", test))]
pub(super) const FULL_DISK_ACCESS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
const SELECT_PRIORITY: usize = 90;
const AUDIT_ROWS: usize = 20;
pub(super) const SUBTITLE: &str = "Let this Mono use your own Chrome, with the sessions you are signed in to, through the zptah Browser extension. Off by default.";

/// What touches the outside world. The real one opens Chrome and System Settings and reaches the
/// Mono's running gate.
pub(super) trait BrowserOps: Send + Sync {
    fn open_full_disk_access(&self);
    /// A recipe's dry run through the Mono's running gate (see `Gate::dry_run_recipe`). It
    /// blocks until a run in progress ends, so it is called off the UI thread.
    fn dry_run_recipe(&self, mono_id: &str, recipe_id: &str) -> Result<DryRunReport, String>;
}
struct RealOps;
impl BrowserOps for RealOps {
    fn open_full_disk_access(&self) {
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open")
            .arg(FULL_DISK_ACCESS_URL)
            .spawn();
    }
    fn dry_run_recipe(&self, mono_id: &str, recipe_id: &str) -> Result<DryRunReport, String> {
        browser::server::servers()
            .gate(mono_id)
            .ok_or_else(|| {
                "This Mono's browser is not running yet. Send the Mono a message that uses the browser, then try the dry run again.".to_owned()
            })?
            .dry_run_recipe(recipe_id)
    }
}

/// The page's own state; the setting itself is on the Mono.
pub(super) struct BrowserUi {
    pub(super) ops: Arc<dyn BrowserOps>,
    pub(super) env: Option<PrereqEnv>,
    pub(super) prereq: Option<Prerequisites>,
    pub(super) profiles: Vec<ChromeProfile>,
    /// Chrome's profile list could not be read: `profiles` holds only the saved profile.
    pub(super) profiles_unreadable: bool,
    pub(super) audit: Vec<AuditEntry>,
    pub(super) recipes: super::recipes_ui::RecipesUi,
    pub(super) select: Option<Choices<String>>,
    pub(super) profile_anchor: AnchorSlot,
    pub(super) error: Option<String>,
    pub(super) connect: super::browser_connect::ConnectUi,
    pub(super) approval: super::browser_approval::ApprovalUi,
}
impl BrowserUi {
    pub(super) fn new() -> Self {
        Self {
            ops: Arc::new(RealOps),
            // Tests start with Chrome absent, never the real one.
            env: cfg!(test).then(|| PrereqEnv {
                chrome_binaries: Vec::new(),
                user_data_dir: PathBuf::new(),
            }),
            prereq: None,
            profiles: Vec::new(),
            profiles_unreadable: false,
            audit: Vec::new(),
            recipes: Default::default(),
            select: None,
            profile_anchor: AnchorSlot::default(),
            error: None,
            connect: Default::default(),
            approval: super::browser_approval::ApprovalUi::new(),
        }
    }
    /// Chrome's environment as the page sees it now.
    fn env(&self) -> PrereqEnv {
        self.env.clone().unwrap_or_else(PrereqEnv::detect)
    }
    #[cfg(test)]
    pub(super) fn use_fakes(&mut self, ops: Arc<dyn BrowserOps>, env: PrereqEnv) {
        self.ops = ops;
        self.env = Some(env);
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

    /// Reads what the page shows from disk: prerequisites, profiles and the audit log. Then keeps
    /// the Mono's `ready` flag true to it.
    pub(super) fn browser_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            return;
        };
        self.browser_connect_refresh(cx);
        self.browser_approval_page_refresh(cx);
        let env = self.browser_ui.env();
        let (profile_dir, profile_name) = self
            .roster
            .get(&id)
            .and_then(|m| m.browser.as_ref())
            .map(|b| (b.profile_dir.clone(), b.profile_name.clone()))
            .unwrap_or_default();
        let ui = &mut self.browser_ui;
        ui.prereq = Some(browser::check_prerequisites(&env));
        let listed = browser::read_local_state(&env)
            .ok()
            .and_then(|text| browser::parse_local_state(&text).ok());
        ui.profiles_unreadable = listed.is_none();
        ui.profiles = listed.unwrap_or_default();
        // The saved profile is always offered, even when Chrome's profile list cannot be read
        // (Full Disk Access missing), so the picker still opens and shows the current choice.
        if browser::valid_profile_dir(&profile_dir)
            && !ui.profiles.iter().any(|p| p.dir == profile_dir)
        {
            ui.profiles.push(ChromeProfile {
                dir: profile_dir.clone(),
                name: if profile_name.is_empty() {
                    profile_dir.clone()
                } else {
                    profile_name
                },
                email: String::new(),
                avatar_icon: String::new(),
                picture: None,
            });
        }
        let store = self.browser_store().ok();
        let ui = &mut self.browser_ui;
        ui.audit = store
            .as_ref()
            .map(|s| s.recent(&id, AUDIT_ROWS))
            .unwrap_or_default();
        self.recipes_refresh();
        self.browser_sync(cx);
    }

    /// `ready` is the page's own summary: Chrome and a profile. A launch does not read it. Full
    /// Disk Access is not part of it: it only lets the page list other profiles.
    fn browser_sync(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            return;
        };
        let ready = self
            .browser_ui
            .prereq
            .as_ref()
            .is_some_and(Prerequisites::can_attach);
        let Some(setting) = self.roster.get_mut(&id).and_then(|m| m.browser.as_mut()) else {
            cx.notify();
            return;
        };
        let ready = ready && !setting.profile_dir.is_empty();
        if setting.ready != ready {
            setting.ready = ready;
            self.save(cx);
            self.configure_chat_with(&id, false, cx);
        }
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

    /// The "Show agent actions on the page" switch was flipped.
    pub(super) fn browser_toggle_show_actions(&mut self, cx: &mut Context<Self>) {
        self.browser_edit(cx, |b| b.show_actions = !b.show_actions);
    }

    fn browser_show_actions_card(
        &mut self,
        setting_: &MonoBrowser,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The switch is the Mono's virtual cursor (spec 10).
        let title = "Show cursor";
        let hint = "While the Mono acts, a cursor in the Mono's color moves to each target in the page, with its name and what it is doing. Off: no cursor. The glow and the Stop button in the tab stay.";
        let toggle = switch_track("mono-browser-show-actions", title, setting_.show_actions)
            .debug_selector(|| "mono-browser-show-actions".into())
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.browser_toggle_show_actions(cx)));
        card()
            .child(switch_setting(
                title,
                hint,
                false,
                toggle.into_any_element(),
            ))
            .into_any_element()
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

    fn browser_open_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.browser_ui.select.take() {
            open.close(window, cx);
            cx.notify();
            return;
        }
        let current = self.browser_setting().profile_dir;
        let choices = self
            .browser_ui
            .profiles
            .iter()
            .map(|p| Choice {
                value: p.dir.clone(),
                label: p.label().into(),
                checked: p.dir == current,
            })
            .collect::<Vec<_>>();
        if choices.is_empty() {
            return;
        }
        let width = self
            .browser_ui
            .profile_anchor
            .get()
            .map_or(260., |b| b.size.width.as_f32().max(220.));
        self.browser_ui.select = Some(Choices::open(
            "Chrome profile",
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

    /// A choice always takes effect, also when Chrome's profile list cannot be read.
    pub(super) fn browser_choose_profile(&mut self, dir: &str, cx: &mut Context<Self>) {
        // A listed profile, else the saved one, else any valid directory name.
        let Some(profile) = self
            .browser_ui
            .profiles
            .iter()
            .find(|p| p.dir == dir)
            .map(|p| (p.dir.clone(), p.name.clone()))
            .or_else(|| browser::valid_profile_dir(dir).then(|| (dir.to_owned(), dir.to_owned())))
        else {
            return;
        };
        let changed = self.browser_setting().profile_dir != dir;
        self.browser_edit(cx, |b| {
            b.profile_dir = profile.0.clone();
            b.profile_name = profile.1.clone();
            if changed {
                b.ready = false;
            }
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
        sections.extend(self.browser_notice_card(&setting, cx));
        let toggle = switch_track("mono-browser-enabled", "Use the browser", setting.enabled)
            .debug_selector(|| "mono-browser-enabled".into())
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.browser_toggle(cx)));
        sections.push(
            card()
                .child(switch_setting(
                    "Use the browser",
                    "Off by default. The Mono drives the Chrome profile you choose.",
                    false,
                    toggle.into_any_element(),
                ))
                .into_any_element(),
        );
        if setting.enabled {
            sections.extend(self.browser_reconnect_card(&setting, cx));
            sections.extend(self.browser_connect_card(cx));
            sections.extend(self.browser_extension_cards(mono, &setting, cx));
            sections.push(self.browser_prerequisites(cx));
            sections.push(self.browser_profile_card(mono, &setting, cx));
            sections.push(self.browser_safety_card(&mono.label(), &setting, window, cx));
            sections.push(self.browser_show_actions_card(&setting, cx));
            sections.push(self.recipes_card(cx));
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

    fn browser_prerequisites(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(prereq) = self.browser_ui.prereq.clone() else {
            return div().into_any_element();
        };
        let row = |id: &'static str, label: &str, check: &Check, fix: Option<AnyElement>| {
            let (text, alpha) = match check {
                Check::Ok => ("Ready".to_owned(), 0.7),
                Check::Missing(why) => (why.clone(), 0.5),
                Check::Denied => ("Needs Full Disk Access".to_owned(), 0.5),
            };
            setting(
                label,
                None,
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(note(id, text, alpha))
                    .children(fix)
                    .into_any_element(),
            )
        };
        let fda_fix = (prereq.full_disk_access == Check::Denied).then(|| {
            action("mono-browser-fda-fix", "Open settings")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.browser_ui.ops.open_full_disk_access();
                    cx.notify();
                }))
                .into_any_element()
        });
        card()
            .child(row(
                "mono-browser-prereq-chrome",
                "Google Chrome",
                &prereq.chrome,
                None,
            ))
            .child(div().h(px(1.)).bg(content(0.05)))
            .child(row(
                "mono-browser-prereq-fda",
                "Access to Chrome's profiles",
                &prereq.full_disk_access,
                fda_fix,
            ))
            .into_any_element()
    }

    fn browser_profile_card(
        &mut self,
        mono: &Mono,
        setting_: &MonoBrowser,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = self
            .browser_ui
            .profiles
            .iter()
            .find(|p| p.dir == setting_.profile_dir)
            .map(ChromeProfile::label)
            .or_else(|| (!setting_.profile_name.is_empty()).then(|| setting_.profile_name.clone()))
            .unwrap_or_else(|| "Choose a profile".into());
        let trigger = div()
            .id("mono-browser-profile")
            .debug_selector(|| "mono-browser-profile".into())
            .role(Role::Button)
            .aria_label(label.clone())
            .relative()
            .h(px(28.))
            .min_w(px(160.))
            .max_w(px(260.))
            .px_2p5()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(content(0.1))
            .text_size(px(12.))
            .text_color(content(0.85))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.06)))
            .on_click(cx.listener(|this, _, window, cx| this.browser_open_select(window, cx)))
            .child(self.browser_ui.profile_anchor.probe())
            .child(div().min_w_0().truncate().child(label))
            .child(icon("chevron-down", 12.).text_color(content(0.5)));
        let shared = browser::profile_shared_with(&self.roster.monos, &mono.id);
        let mut card_ = card()
            .child(setting(
                "Chrome profile",
                Some("The signed-in browser this Mono drives."),
                trigger.into_any_element(),
            ))
            .child(div().h(px(1.)).bg(content(0.05)));
        if !shared.is_empty() {
            card_ = card_.child(div().px_3().py_2().child(note(
                "mono-browser-shared-warning",
                format!(
                    "{} also uses this profile. Two Monos on one browser can fight over its tabs.",
                    shared.join(", ")
                ),
                0.75,
            )));
        }
        if self.browser_ui.profiles_unreadable {
            card_ = card_.child(div().px_3().py_2().child(note(
                "mono-browser-profiles-note",
                "Other Chrome profiles are not listed: the app cannot read Chrome's profile data. Grant Full Disk Access to list them. The saved profile keeps working.",
                0.6,
            )));
        }
        card_.into_any_element()
    }

    fn browser_safety_card(
        &mut self,
        mono_name: &str,
        setting_: &MonoBrowser,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows: Vec<AnyElement> = super::browser_migration::CHOICES
            .into_iter()
            .map(|trust| trust_row(trust, setting_.trust == trust, cx))
            .collect();
        let suggest = self.browser_suggest_card(mono_name, setting_, cx);
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
            .children(suggest)
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
