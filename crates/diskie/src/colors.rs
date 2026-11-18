//! What the treemap's colours mean: file type (the default), age, or change since an earlier
//! scan. The right-hand panel shows the matching legend.

use std::sync::Arc;

use diskie_analyze::age;
use diskie_analyze::diff::Diff;
use diskie_model::Tree;
use serde::{Deserialize, Serialize};

use crate::theme::{Theme, oklch_to_rgb};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorBy {
    #[default]
    Type,
    Age,
    Change,
}

impl ColorBy {
    pub const ALL: [ColorBy; 3] = [ColorBy::Type, ColorBy::Age, ColorBy::Change];

    pub fn label(self) -> &'static str {
        match self {
            ColorBy::Type => "Type",
            ColorBy::Age => "Age",
            ColorBy::Change => "Change",
        }
    }
}

/// A leaf's colour, 0xRRGGBB.
pub type LeafColor = Arc<dyn Fn(&Tree, u32) -> u32 + Send + Sync>;

/// Newest first, warm to cool and bright to dim, so old data recedes.
pub fn age_colors() -> [u32; age::BUCKETS] {
    let mut out = [0; age::BUCKETS];
    for (i, c) in out.iter_mut().enumerate() {
        let t = i as f32 / (age::BUCKETS - 1) as f32;
        *c = oklch_to_rgb(0.80 - 0.34 * t, 0.15 - 0.10 * t, 75.0 + 175.0 * t);
    }
    out
}

pub fn unknown_color(t: &Theme) -> u32 {
    if t.dark { 0x4a4744 } else { 0xb9b4ad }
}

pub struct ChangeColors {
    pub new: u32,
    pub grew: u32,
    pub shrank: u32,
    pub same: u32,
}

pub fn change_colors(t: &Theme) -> ChangeColors {
    ChangeColors {
        new: oklch_to_rgb(0.66, 0.19, 32.0),
        grew: oklch_to_rgb(0.76, 0.15, 65.0),
        shrank: oklch_to_rgb(0.72, 0.12, 165.0),
        same: if t.dark { 0x3d3a37 } else { 0xcdc8c1 },
    }
}

/// The colour function for `by`, or `None` for file types (the rasterizer's default).
pub fn leaf(by: ColorBy, theme: &Theme, now: u64, diff: Option<Arc<Diff>>) -> Option<LeafColor> {
    match by {
        ColorBy::Type => None,
        ColorBy::Age => {
            let colors = age_colors();
            let unknown = unknown_color(theme);
            Some(Arc::new(move |tree: &Tree, n: u32| age::bucket(tree.mtime[n as usize], now).map(|b| colors[b]).unwrap_or(unknown)))
        }
        ColorBy::Change => {
            let c = change_colors(theme);
            Some(Arc::new(move |tree: &Tree, n: u32| {
                let Some(d) = &diff else { return c.same };
                if d.is_new(n) {
                    return c.new;
                }
                let delta = d.change(tree, n);
                let size = tree.size[n as usize].max(1) as f32;
                // Strength by how much of it changed: a folder that doubled is vivid, one that
                // grew by a percent barely tinted.
                let strength = (delta.unsigned_abs() as f32 / size).min(1.0).sqrt();
                match delta {
                    0 => c.same,
                    d if d > 0 => mix(c.same, c.grew, strength.max(0.25)),
                    _ => mix(c.same, c.shrank, strength.max(0.25)),
                }
            }))
        }
    }
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xff) as f32;
    let m = |s: u32| ((ch(a, s) * (1.0 - t) + ch(b, s) * t).round() as u32) << s;
    m(16) | m(8) | m(0)
}

/// Whether text on this colour should be dark rather than white.
pub fn dark_ink(rgb: u32) -> bool {
    let lin = |s: u32| {
        let v = ((rgb >> s) & 0xff) as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(16) + 0.7152 * lin(8) + 0.0722 * lin(0) > 0.28
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn ink_and_mix() {
        assert!(dark_ink(0xffffff));
        assert!(!dark_ink(0x202020));
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        let a = age_colors();
        assert_ne!(a[0], a[6]);
    }
}
