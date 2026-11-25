//! The QR code the web app pairing dialog shows, and the page-level hooks
//! (polling, leaving, dismissing, the window overlay) that share the
//! Connections section with the web app page.
use crate::settings::{Section, SettingsEvent, SettingsView};
use gpui::{AnyElement, Context, Task, Window, canvas, prelude::*, px, rgb};
use std::{sync::Arc, time::Duration};

/// The QR tile's size and padding, for the test of its pixel edges.
#[cfg(test)]
const QR_SIDE: f32 = 384.;
#[cfg(test)]
const QR_PADDING: f32 = 14.;

/// What the Connections page remembers between frames.
#[derive(Default)]
pub(in crate::settings) struct MobileState {
    pub(in crate::settings) poll: Option<Task<()>>,
}

/// A QR code's modules, row by row, without the quiet zone.
#[derive(Debug, PartialEq)]
pub(in crate::settings) struct Qr {
    width: usize,
    dark: Vec<bool>,
}

/// The QR code the browser scans; low error correction keeps the modules large.
pub(in crate::settings) fn qr(text: &str) -> Option<Qr> {
    let code =
        qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L).ok()?;
    Some(Qr {
        width: code.width(),
        dark: code
            .to_colors()
            .into_iter()
            .map(|color| color == qrcode::Color::Dark)
            .collect(),
    })
}

impl SettingsView {
    /// Web pairing status changes on its own: read it every second while the
    /// page is open.
    pub(in crate::settings) fn watch_mobile(&mut self, cx: &mut Context<Self>) {
        if self.section != Section::Connections {
            self.mobile.poll = None;
            return;
        }
        cx.emit(SettingsEvent::MobileRefresh);
        self.mobile.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| {
                    if this.ctx.web_pairing.is_some() {
                        cx.notify();
                    }
                    cx.emit(SettingsEvent::MobileRefresh);
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    /// Leaving the page closes the web pairing code.
    pub(in crate::settings) fn leave_mobile(&mut self, cx: &mut Context<Self>) {
        if self.section != Section::Connections {
            return;
        }
        self.leave_web_app(cx);
    }

    /// Escape closes the web app dialogs.
    pub(in crate::settings) fn dismiss_mobile(&mut self, cx: &mut Context<Self>) -> bool {
        if self.section != Section::Connections {
            return false;
        }
        self.dismiss_web_app(cx)
    }

    /// The web app dialogs, presented by Shell alongside the other window
    /// overlays so their scrim and centering cover the entire viewport.
    pub(crate) fn mobile_overlay_open(&self) -> bool {
        self.section == Section::Connections && self.web_overlay_open()
    }

    pub(crate) fn mobile_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.mobile_overlay_open() {
            return None;
        }
        self.web_overlay(window, cx)
    }
}

/// A module edge in physical pixels. Snapping each edge independently fills
/// the image area, like React's pixelated QR image, without shaving an entire
/// logical pixel from every module on Retina displays.
fn qr_edge(index: usize, modules: usize, side: f32, scale: f32) -> f32 {
    (index as f32 * side * scale / modules as f32).round() / scale
}

/// Dark modules on white with the same two-module quiet zone as React's
/// QRCode.toDataURL. All edges land on device pixels so cameras see crisp ink.
pub(super) fn qr_canvas(qr: Arc<Qr>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let modules = qr.width + 4;
            let side = bounds.size.width.min(bounds.size.height).as_f32();
            let scale = window.scale_factor();
            let origin = gpui::point(
                px((bounds.origin.x.as_f32() * scale).round() / scale),
                px((bounds.origin.y.as_f32() * scale).round() / scale),
            );
            for (y, row) in qr.dark.chunks(qr.width).enumerate() {
                let top = qr_edge(y + 2, modules, side, scale);
                let bottom = qr_edge(y + 3, modules, side, scale);
                let mut x = 0;
                while x < row.len() {
                    if !row[x] {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < row.len() && row[x] {
                        x += 1;
                    }
                    let left = qr_edge(start + 2, modules, side, scale);
                    let right = qr_edge(x + 2, modules, side, scale);
                    window.paint_quad(gpui::fill(
                        gpui::Bounds::new(
                            origin + gpui::point(px(left), px(top)),
                            gpui::size(px(right - left), px(bottom - top)),
                        ),
                        rgb(0x000000),
                    ));
                }
            }
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_qr_codes_fill_the_image_with_crisp_device_pixel_edges() {
        let side = QR_SIDE - QR_PADDING * 2.;
        // A dense code used to lose almost a third of its side because each
        // module was rounded down to a whole logical pixel.
        let modules = 153;
        for scale in [1., 1.5, 2.] {
            assert_eq!(qr_edge(0, modules, side, scale), 0.);
            assert_eq!(qr_edge(modules, modules, side, scale), side);
            let left = qr_edge(2, modules, side, scale);
            let right = qr_edge(modules - 2, modules, side, scale);
            assert!(right - left > side * 0.96);
            for index in 0..=modules {
                let edge = qr_edge(index, modules, side, scale) * scale;
                assert!((edge - edge.round()).abs() < 0.001);
                if index > 0 {
                    assert!(
                        qr_edge(index, modules, side, scale)
                            > qr_edge(index - 1, modules, side, scale)
                    );
                }
            }
        }
    }

    #[test]
    fn pairing_links_fit_a_scannable_code() {
        let url = format!("monocode://pair?i={}", "A".repeat(600));
        let code = qr(&url).unwrap();
        assert_eq!(code.dark.len(), code.width * code.width);
        assert!(code.dark.iter().any(|&dark| dark));
        // The three finder patterns start with a dark corner.
        assert!(code.dark[0] && code.dark[code.width - 1]);
        assert!(code.dark[code.width * (code.width - 1)]);
    }
}
