//! The Ardia part of a Mono's Browser page (design: `docs/superpowers/specs/2026-11-27-ardia-browser.md`,
//! sections 2, 2a and 10): the readiness row with its fix, "Ardia running (started by zptah)" with
//! Quit, the profile picker, the idle quit setting and the note about Ardia's Control.
//!
//! Everything that runs or looks up Ardia goes through `BrowserOps` and runs on a worker: the
//! controller blocks on `ardia` processes, so nothing here calls it from a render.
use super::browser_ui::{action, note};
use super::panels::{card, setting};
use super::*;
use crate::popover::Choice;
use monocode_core::mono::browser;
use monocode_core::mono::browser::ardia::controller::{DEFAULT_IDLE_MINUTES, IDLE_SETTING_KEY};
use monocode_core::mono::browser::ardia::{ArdiaProfile, Readiness, Report};

/// The field that holds the idle minutes.
pub(super) const IDLE_FIELD: &str = "browser-idle-minutes";
/// The largest idle timeout the field accepts: one week.
const MAX_IDLE_MINUTES: u32 = 10_080;
pub(super) const NO_UPLOADS_NOTE: &str = "Uploads are not available on this system.";
pub(super) const CONTROL_NOTE: &str = "Ardia's Control lets any local program drive this browser, not only zptah. That is how Ardia works. zptah quits an Ardia it started when it has been idle.";

/// What the Ardia cards keep between frames.
#[derive(Default)]
pub(super) struct ArdiaUi {
    /// The last readiness answer; none until the first one arrives.
    pub(super) report: Option<Report>,
    /// The last `profiles.list` answer.
    pub(super) profiles: Option<Result<Vec<ArdiaProfile>, Readiness>>,
    pub(super) starting: bool,
    pub(super) quitting: bool,
    /// Bumped by every refresh so a slow, older answer is dropped.
    generation: u64,
    pub(super) idle_minutes: u32,
    pub(super) error: Option<String>,
}

impl ArdiaUi {
    /// The profiles Ardia listed (none while it is closed or the list was not read).
    pub(super) fn listed(&self) -> &[ArdiaProfile] {
        match &self.profiles {
            Some(Ok(list)) => list,
            _ => &[],
        }
    }
}

/// The text of the Ardia version the last good check saw, for example `Ardia 0.2.49, protocol 4`.
fn version_text(report: &Report) -> Option<String> {
    report
        .cached
        .as_ref()
        .map(|live| format!("Ardia {}, protocol {}", live.version, live.protocol))
}

/// The idle minutes saved in the store, or the default.
fn saved_idle_minutes(store: &monocode_core::store::Store) -> u32 {
    store
        .setting(IDLE_SETTING_KEY)
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_IDLE_MINUTES)
}

impl Monos {
    /// The picker's rows: Ardia's profiles, plus the saved one when Ardia did not list it (Ardia is
    /// closed), so the current choice is always there.
    pub(super) fn browser_ardia_choices(&self) -> Vec<Choice<String>> {
        let saved = self.browser_setting();
        let label = |id: &str, name: &str| -> SharedString {
            if name.is_empty() { id } else { name }.to_owned().into()
        };
        let mut choices: Vec<Choice<String>> = self
            .browser_ui
            .ardia
            .listed()
            .iter()
            .map(|p| Choice {
                value: p.id.clone(),
                label: label(&p.id, &p.name),
                checked: p.id == saved.ardia_profile,
            })
            .collect();
        if browser::valid_ardia_profile(&saved.ardia_profile)
            && !choices.iter().any(|c| c.value == saved.ardia_profile)
        {
            choices.insert(
                0,
                Choice {
                    label: label(&saved.ardia_profile, &saved.ardia_profile_name),
                    value: saved.ardia_profile,
                    checked: true,
                },
            );
        }
        choices
    }

    /// Reads the saved idle minutes, shows them in the field and tells the controller.
    pub(super) fn browser_ardia_load_idle(&mut self, cx: &mut Context<Self>) {
        let minutes = locked(&self.store)
            .map(|s| saved_idle_minutes(&s))
            .unwrap_or(DEFAULT_IDLE_MINUTES);
        self.browser_ui.ardia.idle_minutes = minutes;
        self.browser_ui.ops.ardia_set_idle_minutes(minutes);
        if let Some(field) = self.fields.get(IDLE_FIELD).cloned() {
            field.update(cx, |f, cx| f.set_text(minutes.to_string(), cx));
        }
    }

    /// Asks Ardia for the readiness of the Mono's profile and for its profiles, on a worker. It
    /// never launches Ardia.
    pub(super) fn browser_ardia_refresh(&mut self, cx: &mut Context<Self>) {
        let setting = self.browser_setting();
        let ui = &mut self.browser_ui.ardia;
        ui.generation += 1;
        if !setting.enabled {
            ui.report = None;
            ui.profiles = None;
            return;
        }
        let generation = ui.generation;
        let ops = self.browser_ui.ops.clone();
        let profile = setting.ardia_profile;
        cx.spawn(async move |this, cx| {
            let (report, profiles) = cx
                .background_executor()
                .spawn(async move { (ops.ardia_report(&profile), ops.ardia_profiles()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let ui = &mut this.browser_ui.ardia;
                if ui.generation != generation {
                    return;
                }
                ui.report = Some(report);
                ui.profiles = Some(profiles);
                cx.notify();
            });
        })
        .detach();
    }

    /// "Start Ardia": launches the closed Ardia in the background, then reads it again.
    pub(super) fn browser_ardia_start(&mut self, cx: &mut Context<Self>) {
        if self.browser_ui.ardia.starting {
            return;
        }
        self.browser_ui.ardia.starting = true;
        self.browser_ui.ardia.error = None;
        let ops = self.browser_ui.ops.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops.ardia_start() })
                .await;
            let _ = this.update(cx, |this, cx| {
                let ui = &mut this.browser_ui.ardia;
                ui.starting = false;
                ui.error = result.err().map(|why| why.message());
                this.browser_ardia_refresh(cx);
                this.browser_surfaces_refresh(false, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// "Quit": closes the session tabs and quits the Ardia zptah started.
    pub(super) fn browser_ardia_quit(&mut self, cx: &mut Context<Self>) {
        if self.browser_ui.ardia.quitting {
            return;
        }
        self.browser_ui.ardia.quitting = true;
        self.browser_ui.ardia.error = None;
        let ops = self.browser_ui.ops.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops.ardia_quit() })
                .await;
            let _ = this.update(cx, |this, cx| {
                let ui = &mut this.browser_ui.ardia;
                ui.quitting = false;
                ui.error = result.err();
                this.browser_ardia_refresh(cx);
                this.browser_surfaces_refresh(false, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves the idle minutes in the field and applies them at once.
    pub(super) fn browser_ardia_save_idle(&mut self, cx: &mut Context<Self>) {
        let text = self.value(IDLE_FIELD, cx);
        let minutes = match text.trim().parse::<u32>() {
            Ok(n) if n <= MAX_IDLE_MINUTES => n,
            _ => {
                self.browser_ui.error = Some(format!(
                    "Enter the idle minutes as a whole number from 0 to {MAX_IDLE_MINUTES}. 0 means never."
                ));
                cx.notify();
                return;
            }
        };
        let saved = locked(&self.store)
            .and_then(|s| s.save_setting(IDLE_SETTING_KEY, &minutes.to_string()));
        match saved {
            Ok(()) => {
                self.browser_ui.ardia.idle_minutes = minutes;
                self.browser_ui.ops.ardia_set_idle_minutes(minutes);
                self.browser_ui.error = None;
            }
            Err(error) => self.browser_ui.error = Some(error),
        }
        cx.notify();
    }

    fn browser_readiness_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let can_launch = self.browser_ui.ops.ardia_can_launch();
        let mut lines = div().flex_1().min_w_0().flex().flex_col().gap_1();
        match self.browser_ui.ardia.report.clone() {
            None => {
                lines = lines.child(note(
                    "mono-browser-readiness-checking",
                    "Checking Ardia...",
                    0.6,
                ));
            }
            Some(report) => match &report.failure {
                Some(failure) => {
                    lines = lines
                        .child(note("mono-browser-readiness-check", failure.check(), 0.5))
                        .child(note(
                            "mono-browser-readiness-problem",
                            failure.problem(),
                            0.9,
                        ))
                        .child(note("mono-browser-readiness-fix", failure.fix(), 0.7));
                }
                None if report.running => {
                    let text = version_text(&report)
                        .map_or_else(|| "Ardia is ready.".to_owned(), |v| format!("Ready. {v}."));
                    lines = lines.child(note("mono-browser-readiness-ok", text, 0.8));
                }
                None => {
                    let text = if can_launch {
                        "Ardia is closed. zptah opens it in the background when this Mono uses the browser."
                    } else {
                        "Ardia is closed. Open it before this Mono uses the browser."
                    };
                    lines = lines.child(note("mono-browser-readiness-closed", text, 0.8));
                    if let Some(last) = version_text(&report) {
                        lines = lines.child(note(
                            "mono-browser-readiness-last",
                            format!("Last seen: {last}."),
                            0.5,
                        ));
                    }
                }
            },
        }
        card()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(lines)
                    .child(
                        action("mono-browser-ardia-retry", "Try again")
                            .on_click(cx.listener(|this, _, _, cx| this.browser_ardia_refresh(cx))),
                    ),
            )
            .into_any_element()
    }

    /// "Ardia running (started by zptah)" with Quit, only for an Ardia zptah started.
    fn browser_running_card(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let report = self.browser_ui.ardia.report.as_ref()?;
        if !report.quittable() {
            return None;
        }
        let quitting = self.browser_ui.ardia.quitting;
        Some(
            card()
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().flex_1().min_w_0().child(note(
                            "mono-browser-ardia-running",
                            "Ardia running (started by zptah)",
                            0.9,
                        )))
                        .child(
                            action(
                                "mono-browser-ardia-quit",
                                if quitting { "Quitting..." } else { "Quit" },
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.browser_ardia_quit(cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    fn browser_profile_card(&mut self, mono: &Mono, cx: &mut Context<Self>) -> AnyElement {
        let label = self
            .browser_ardia_choices()
            .into_iter()
            .find(|c| c.checked)
            .map(|c| c.label.to_string())
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
        let can_launch = self.browser_ui.ops.ardia_can_launch();
        let starting = self.browser_ui.ardia.starting;
        let mut card_ = card()
            .child(setting(
                "Ardia profile",
                Some("The signed-in browser profile this Mono drives."),
                trigger.into_any_element(),
            ))
            .child(div().h(px(1.)).bg(content(0.05)));
        match self.browser_ui.ardia.profiles.clone() {
            Some(Err(Readiness::NotRunning)) => {
                let row = div().px_3().py_2().flex().items_center().gap_3();
                card_ = card_.child(if can_launch {
                    row.child(div().flex_1().min_w_0().child(note(
                        "mono-browser-profiles-closed",
                        "Ardia is closed, so its profiles are not listed. The saved profile keeps working.",
                        0.6,
                    )))
                    .child(
                        action(
                            "mono-browser-ardia-start",
                            if starting { "Starting..." } else { "Start Ardia" },
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.browser_ardia_start(cx))),
                    )
                } else {
                    row.child(note("mono-browser-ardia-open", "Open Ardia", 0.9))
                        .child(note(
                            "mono-browser-profiles-closed",
                            "Ardia's profiles are listed while it runs. The saved profile keeps working.",
                            0.6,
                        ))
                });
            }
            Some(Err(why)) => {
                card_ = card_.child(div().px_3().py_2().child(note(
                    "mono-browser-profiles-note",
                    format!("Ardia's profiles could not be listed. {}", why.problem()),
                    0.6,
                )));
            }
            _ => {}
        }
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
        card_.into_any_element()
    }

    fn browser_idle_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let minutes = self.browser_ui.ardia.idle_minutes;
        let input = self.field(IDLE_FIELD, "Minutes", minutes.to_string(), false, cx);
        if self.field_blurs.insert(input.entity_id()) {
            self.observations
                .push(cx.subscribe(&input, |this, _, _: &input::Submitted, cx| {
                    this.browser_ardia_save_idle(cx)
                }));
        }
        let control = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("mono-browser-idle-input")
                    .w(px(72.))
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.1))
                    .child(input),
            )
            .child(
                action("mono-browser-idle-save", "Save")
                    .on_click(cx.listener(|this, _, _, cx| this.browser_ardia_save_idle(cx))),
            );
        card()
            .child(setting(
                "Quit Ardia when idle",
                Some("Minutes without a browser call before zptah quits an Ardia it started. 0 means never. Default 10."),
                control.into_any_element(),
            ))
            .into_any_element()
    }

    /// The Ardia cards in page order: readiness, running, profile, idle quit, Control note.
    pub(super) fn browser_ardia_cards(
        &mut self,
        mono: &Mono,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut cards = vec![self.browser_readiness_card(cx)];
        if let Some(error) = self.browser_ui.ardia.error.clone() {
            cards.push(
                crate::cloud_dialog_ui::alert("mono-browser-ardia-error", error).into_any_element(),
            );
        }
        cards.extend(self.browser_running_card(cx));
        cards.push(self.browser_profile_card(mono, cx));
        cards.push(self.browser_idle_card(cx));
        if !self.browser_ui.ops.uploads_available() {
            cards.push(
                div()
                    .px_1()
                    .child(note("mono-browser-uploads-note", NO_UPLOADS_NOTE, 0.55))
                    .into_any_element(),
            );
        }
        cards.push(
            div()
                .px_1()
                .child(note("mono-browser-control-note", CONTROL_NOTE, 0.55))
                .into_any_element(),
        );
        cards
    }
}
