//! Pan and zoom math of the diagram viewer, shared in spirit with the web
//! viewer (same constants, same formulas). `x` and `y` are the screen offset of
//! the content origin; a content point `p` is shown at `p * scale + offset`.
pub const MIN_SCALE: f32 = 0.25;
pub const MAX_SCALE: f32 = 4.0;
/// Fit never enlarges beyond this, so a tiny diagram is not blown up.
pub const FIT_MAX: f32 = 1.6;
pub const FIT_MARGIN: f32 = 20.;
pub const BUTTON_STEP: f32 = 1.2;
pub const DOT_SPACING: f32 = 18.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub scale: f32,
    pub x: f32,
    pub y: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            scale: 1.,
            x: 0.,
            y: 0.,
        }
    }
}

impl Viewport {
    /// Centre `content` in `board` at the largest scale that fits with a margin.
    pub fn fit(content: [f32; 2], board: [f32; 2]) -> Self {
        let cw = content[0].max(1.);
        let ch = content[1].max(1.);
        let scale = ((board[0] - 2. * FIT_MARGIN) / cw)
            .min((board[1] - 2. * FIT_MARGIN) / ch)
            .min(FIT_MAX)
            .clamp(MIN_SCALE, MAX_SCALE);
        Self {
            scale,
            x: (board[0] - cw * scale) / 2.,
            y: (board[1] - ch * scale) / 2.,
        }
    }

    /// Multiply the scale by `factor` keeping the content point under the
    /// screen position `(px, py)` fixed.
    pub fn zoom_at(&mut self, factor: f32, px: f32, py: f32) {
        let next = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        self.x = px - (px - self.x) * next / self.scale;
        self.y = py - (py - self.y) * next / self.scale;
        self.scale = next;
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.x += dx;
        self.y += dy;
    }

    pub fn percent(&self) -> u32 {
        (self.scale * 100.).round() as u32
    }

    /// The content point shown at a screen position.
    pub fn to_content(&self, px: f32, py: f32) -> [f32; 2] {
        [(px - self.x) / self.scale, (py - self.y) / self.scale]
    }

    pub fn to_screen(&self, cx: f32, cy: f32) -> [f32; 2] {
        [cx * self.scale + self.x, cy * self.scale + self.y]
    }
}

/// Zoom factor for a wheel scroll of `delta_y` pixels (positive scrolls down,
/// which zooms out). One 20px wheel notch is about 10%.
pub fn wheel_factor(delta_y: f32) -> f32 {
    (-delta_y * 0.005).exp().clamp(0.5, 2.)
}

/// Zoom factor for a pinch delta (0.1 means 10% in).
pub fn pinch_factor(delta: f32) -> f32 {
    (1. + delta).clamp(0.5, 2.)
}
