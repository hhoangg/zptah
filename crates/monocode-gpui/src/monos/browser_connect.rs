//! The "Browser connection" card of a Mono's Browser page (spec 12.3): detect Chrome and its
//! profiles, write the native messaging host manifest, open the pair page in the chosen profile and
//! show where the connection stands. The link layer lives in `monocode_core::mono::browser::connect`
//! and runs for the whole app; this file only reads its status and calls `connect`, `cancel_pairing`
//! and `disconnect`. The Playwright cards stay beside it until their removal ticket.
use super::browser_ui::{action, note};
use super::panels::{card, setting};
use super::*;
use crate::cloud_dialog_ui::alert;
use monocode_core::mono::browser::connect::{self as link, BrowserConnect, ConnectStatus, Phase};
use std::sync::Arc;
use std::time::Duration;

const POLL: Duration = Duration::from_secs(1);
/// A pairing window lasts 5 minutes; the watcher outlives it a little to show the last state.
const WATCH_SECONDS: u32 = 330;

/// What the card keeps between frames.
#[derive(Default)]
pub(super) struct ConnectUi {
    /// Tests inject their own service; the app uses the global one.
    #[cfg(test)]
    pub(super) service: Option<Arc<BrowserConnect>>,
    pub(super) status: Option<ConnectStatus>,
    /// The profile directory the user picked (the first listed one until then).
    pub(super) profile: Option<String>,
    pub(super) error: Option<String>,
    pub(super) watching: bool,
}

impl ConnectUi {
    fn service(&self) -> Option<Arc<BrowserConnect>> {
        #[cfg(test)]
        {
            self.service.clone()
        }
        #[cfg(not(test))]
        {
            link::global()
        }
    }

    /// The profile a Connect press uses: the picked one, else the first listed, else Default.
    fn chosen(&self) -> (String, String) {
        let profiles: Vec<_> = self
            .status
            .iter()
            .flat_map(|s| s.browsers.first())
            .flat_map(|b| b.profiles.iter())
            .collect();
        let pick = self
            .profile
            .as_ref()
            .and_then(|dir| profiles.iter().find(|p| &p.dir == dir))
            .or(profiles.first());
        match pick {
            Some(p) => (p.dir.clone(), p.name.clone()),
            None => ("Default".to_owned(), "Default".to_owned()),
        }
    }
}

fn phase_text(phase: Phase) -> &'static str {
    match phase {
        Phase::NoBrowser => "Google Chrome was not found. Install it, then come back here.",
        Phase::Ready => "Connect the zptah Browser extension to let this Mono use your Chrome.",
        Phase::ManifestWritten => {
            "The browser host is set up. Press Connect to pair the extension."
        }
        Phase::WaitingForInstall => {
            "Waiting for the extension. Install zptah Browser in that Chrome profile, then press Try again."
        }
        Phase::Pairing => {
            "Pairing: finish in the browser tab that opened. The window closes in 5 minutes."
        }
        Phase::Connected => "Connected.",
        Phase::ProtocolMismatch => {
            "The extension speaks a different version than this app. Update zptah Browser or the app, then try again."
        }
    }
}

fn link_label(l: &link::LinkView) -> String {
    format!(
        "{}, profile {}{}",
        l.brand,
        l.name,
        if l.connected { "" } else { " (not connected)" }
    )
}

impl Monos {
    /// Reads the link status into the card and starts the watcher when something is in progress.
    pub(super) fn browser_connect_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.browser_ui.connect.service() else {
            return;
        };
        let status = service.status();
        let busy = matches!(status.phase, Phase::Pairing | Phase::WaitingForInstall);
        self.browser_ui.connect.status = Some(status);
        if busy {
            self.browser_connect_watch(cx);
        }
        cx.notify();
    }

    fn browser_connect_watch(&mut self, cx: &mut Context<Self>) {
        if self.browser_ui.connect.watching {
            return;
        }
        self.browser_ui.connect.watching = true;
        cx.spawn(async move |this, cx| {
            for _ in 0..WATCH_SECONDS {
                cx.background_executor().timer(POLL).await;
                let done = this
                    .update(cx, |this, cx| {
                        this.browser_connect_refresh_quiet(cx);
                        let phase = this.browser_ui.connect.status.as_ref().map(|s| s.phase);
                        !matches!(phase, Some(Phase::Pairing | Phase::WaitingForInstall))
                            || this.settings != Some(SettingsSection::Browser)
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.browser_ui.connect.watching = false);
        })
        .detach();
    }

    /// Like the refresh, without starting another watcher.
    fn browser_connect_refresh_quiet(&mut self, cx: &mut Context<Self>) {
        if let Some(service) = self.browser_ui.connect.service() {
            self.browser_ui.connect.status = Some(service.status());
            cx.notify();
        }
    }

    pub(super) fn browser_connect_pick(&mut self, dir: &str, cx: &mut Context<Self>) {
        self.browser_ui.connect.profile = Some(dir.to_owned());
        cx.notify();
    }

    pub(super) fn browser_connect_start(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.browser_ui.connect.service() else {
            return;
        };
        if self.browser_ui.connect.status.is_none() {
            self.browser_connect_refresh_quiet(cx);
        }
        let (dir, name) = self.browser_ui.connect.chosen();
        self.browser_ui.connect.error = service.connect(&dir, &name).err();
        self.browser_connect_refresh(cx);
    }

    pub(super) fn browser_connect_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(service) = self.browser_ui.connect.service() {
            service.cancel_pairing();
        }
        self.browser_ui.connect.error = None;
        self.browser_connect_refresh_quiet(cx);
    }

    pub(super) fn browser_connect_disconnect(&mut self, instance_id: &str, cx: &mut Context<Self>) {
        if let Some(service) = self.browser_ui.connect.service() {
            self.browser_ui.connect.error = service.disconnect(instance_id).err();
        }
        self.browser_connect_refresh_quiet(cx);
    }

    /// The card, or nothing when the link layer is not running (it always is in the app).
    pub(super) fn browser_connect_card(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.browser_ui.connect.service()?;
        let status = self.browser_ui.connect.status.clone()?;
        let phase = status.phase;
        let (chosen_dir, _) = self.browser_ui.connect.chosen();
        let mut card_ = card()
            .child(setting(
                "Browser connection",
                Some("Lets the Mono use your own Chrome through the zptah Browser extension."),
                div().into_any_element(),
            ))
            .child(div().h(px(1.)).bg(content(0.05)))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .child(note("mono-connect-status", phase_text(phase), 0.8)),
            );
        if let Some(browser) = status.browsers.first() {
            let rows: Vec<AnyElement> = browser
                .profiles
                .iter()
                .map(|p| {
                    let id: SharedString = format!("mono-connect-profile-{}", p.dir).into();
                    let selected = p.dir == chosen_dir;
                    let dir = p.dir.clone();
                    div()
                        .id(id.clone())
                        .debug_selector({
                            let id = id.clone();
                            move || id.to_string()
                        })
                        .role(Role::Button)
                        .aria_label(p.name.clone())
                        .px_2p5()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(content(if selected { 0.3 } else { 0.1 }))
                        .text_size(px(12.))
                        .text_color(content(if selected { 0.95 } else { 0.7 }))
                        .cursor_pointer()
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.browser_connect_pick(&dir, cx)),
                        )
                        .child(p.name.clone())
                        .into_any_element()
                })
                .collect();
            if rows.is_empty() {
                card_ = card_.child(div().px_3().py_2().child(note(
                    "mono-connect-profiles-note",
                    "Chrome's profile list is not readable. The default profile is used.",
                    0.6,
                )));
            } else {
                card_ = card_.child(
                    div()
                        .px_3()
                        .py_2()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(rows),
                );
            }
        }
        for l in &status.links {
            let id = l.instance_id.clone();
            let row_id: SharedString = format!("mono-connect-disconnect-{}", l.instance_id).into();
            card_ = card_.child(setting(
                &link_label(l),
                None,
                crate::modal::ghost_button(row_id.clone(), "Disconnect")
                    .debug_selector({
                        let row_id = row_id.clone();
                        move || row_id.to_string()
                    })
                    .border_1()
                    .border_color(content(0.1))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.browser_connect_disconnect(&id, cx)),
                    )
                    .into_any_element(),
            ));
        }
        let label = match phase {
            Phase::Connected => "Connect another profile",
            Phase::WaitingForInstall | Phase::ProtocolMismatch => "Try again",
            _ => "Connect extension",
        };
        let mut buttons = div().px_3().py_2().flex().items_center().gap_2();
        if phase != Phase::NoBrowser {
            buttons = buttons.child(
                action("mono-connect-start", label)
                    .on_click(cx.listener(|this, _, _, cx| this.browser_connect_start(cx))),
            );
        }
        if phase == Phase::Pairing {
            buttons = buttons.child(
                action("mono-connect-cancel", "Cancel")
                    .on_click(cx.listener(|this, _, _, cx| this.browser_connect_cancel(cx))),
            );
        }
        card_ = card_.child(buttons);
        if status.keys_in_fallback {
            card_ = card_.child(div().px_3().pb_2().child(note(
                "mono-connect-fallback",
                "The OS credential store is not available, so the pairing key is kept in zptah's encrypted storage instead.",
                0.6,
            )));
        }
        for (id, text) in [
            ("mono-connect-server-error", status.server_error.clone()),
            ("mono-connect-alert", status.alert.clone()),
            ("mono-connect-error", self.browser_ui.connect.error.clone()),
        ] {
            if let Some(text) = text {
                card_ = card_.child(div().px_3().pb_2().child(alert(id, text)));
            }
        }
        Some(card_.into_any_element())
    }
}
