//! The shared diagram model: one graph type for Mermaid flowcharts and the
//! native `visual.flow` SDK call, one pure layout engine, and the pan/zoom math
//! of the diagram viewer. Nothing here knows about GPUI, so every piece is
//! unit-testable and the algorithm can be ported to TypeScript for the web.
use crate::visual::ThemeColor;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub mod layout;
pub mod mermaid;
#[cfg(test)]
mod tests;
pub mod viewport;

pub use layout::{layout, text_width, Layout, PlacedEdge, PlacedLabel, PlacedNode, Rect};

pub const MAX_FLOW_NODES: usize = 120;
pub const MAX_FLOW_EDGES: usize = 400;
const MAX_ID_CHARS: usize = 64;

/// Flow direction. `LR` and `TB` are what `visual.flow` accepts; Mermaid also
/// produces `RL` and `BT`, which are the same layout mirrored.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Direction {
    #[default]
    LR,
    RL,
    TB,
    BT,
}

impl Direction {
    pub fn horizontal(self) -> bool {
        matches!(self, Self::LR | Self::RL)
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "LR" => Some(Self::LR),
            "RL" => Some(Self::RL),
            "TB" | "TD" => Some(Self::TB),
            "BT" => Some(Self::BT),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    #[default]
    Box,
    /// A box with generously rounded corners (Mermaid `(text)`).
    Round,
    Decision,
    /// A pill: the entry point (Mermaid `([text])`).
    Start,
    /// A pill: the exit point.
    End,
    Circle,
}

impl NodeKind {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "box" => Some(Self::Box),
            "decision" => Some(Self::Decision),
            "start" => Some(Self::Start),
            "end" => Some(Self::End),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FlowNode {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub kind: NodeKind,
    #[serde(default)]
    pub color: Option<ThemeColor>,
}

fn arrow_default() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub dashed: bool,
    #[serde(default = "arrow_default")]
    pub arrow: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FlowGraph {
    #[serde(default)]
    pub direction: Direction,
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
}

impl FlowGraph {
    /// Structural validation with errors an author can act on.
    pub fn validate(&self) -> Result<(), String> {
        if self.nodes.is_empty() || self.nodes.len() > MAX_FLOW_NODES {
            return Err(format!("Flow needs 1-{MAX_FLOW_NODES} nodes"));
        }
        if self.edges.len() > MAX_FLOW_EDGES {
            return Err(format!("Flow allows at most {MAX_FLOW_EDGES} edges"));
        }
        let mut seen = HashSet::new();
        for node in &self.nodes {
            let chars = node.id.chars().count();
            if chars == 0 || chars > MAX_ID_CHARS || node.id.chars().any(char::is_control) {
                return Err(format!(
                    "Flow node ids must contain 1-{MAX_ID_CHARS} printable characters, got {:?}",
                    node.id
                ));
            }
            if !seen.insert(node.id.as_str()) {
                return Err(format!("Duplicate flow node id '{}'", node.id));
            }
        }
        for (i, edge) in self.edges.iter().enumerate() {
            for (side, id) in [("from", &edge.from), ("to", &edge.to)] {
                if !seen.contains(id.as_str()) {
                    return Err(format!(
                        "Flow edge {i} has unknown node id '{id}' in {side}; declare it in nodes"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Horizontal anchoring of a canvas label relative to its `x`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    /// `x` is the left edge of the text (the original behaviour).
    #[default]
    Start,
    Center,
    End,
}

/// Vertical anchoring of a canvas label relative to its `y`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Baseline {
    /// `y` is the top of the text line (the original behaviour).
    #[default]
    Top,
    /// `y` is the vertical centre of the text line.
    Middle,
    Bottom,
    /// `y` is the text baseline.
    Alphabetic,
}

/// Canvas label font size and line box, in canvas units.
pub const LABEL_FONT: f32 = 12.;
pub const LABEL_LINE: f32 = LABEL_FONT * 1.4;
/// Distance from the top of a line box to its baseline, as a fraction of the font size.
pub const ASCENT: f32 = 1.0;

/// The top-left corner of a label whose anchor is `(x, y)`.
pub fn label_origin(
    x: f32,
    y: f32,
    text_width: f32,
    font: f32,
    align: TextAlign,
    baseline: Baseline,
) -> [f32; 2] {
    let line = font * 1.4;
    let left = match align {
        TextAlign::Start => x,
        TextAlign::Center => x - text_width / 2.,
        TextAlign::End => x - text_width,
    };
    let top = match baseline {
        Baseline::Top => y,
        Baseline::Middle => y - line / 2.,
        Baseline::Bottom => y - line,
        Baseline::Alphabetic => y - ASCENT * font,
    };
    [left, top]
}

/// WCAG relative luminance of an sRGB colour with components in 0..=1.
pub fn luminance(rgb: [f32; 3]) -> f32 {
    let lin = |c: f32| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

/// Whether dark text reads better than light text on a fill of this luminance.
/// 0.179 is where black and white text have equal WCAG contrast.
pub fn prefers_dark_text(fill_luminance: f32) -> bool {
    fill_luminance > 0.179
}
