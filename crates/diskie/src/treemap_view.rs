//! The treemap pane: layout and rasterizing happen on a background thread; each frame paints
//! the cached bitmap, then labels, hover and selection on top.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use diskie_model::treemap::{self, MapRect, Options, Palette};
use diskie_model::{Tree, fmt};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MapKey {
    pub zoom: u32,
    pub w: u32,
    pub h: u32,
    pub device_w: u32,
    pub device_h: u32,
    pub generation: u64,
    pub dark: bool,
}

pub struct MapLayout {
    pub key: MapKey,
    pub rects: Vec<MapRect>,
    /// Node → index of its rectangle.
    pub index: HashMap<u32, usize>,
    pub image: Arc<RenderImage>,
}

/// What the background job hands back.
pub struct Built {
    key: MapKey,
    rects: Vec<MapRect>,
    index: HashMap<u32, usize>,
    pixels: Vec<u8>,
}

#[derive(Default)]
pub struct MapState {
    pub current: Option<Rc<MapLayout>>,
    pub pending: Option<MapKey>,
    /// Where the treemap was last painted, and at what scale.
    pub bounds: Bounds<Pixels>,
    pub scale: f32,
    /// Images to release from the GPU atlas on the next paint.
    pub stale: Vec<Arc<RenderImage>>,
}

impl MapState {
    /// The layout the current bounds call for, if the pane has been painted yet.
    pub fn wanted(&self, zoom: u32, generation: u64, dark: bool) -> Option<MapKey> {
        let (w, h) = (self.bounds.size.width.as_f32(), self.bounds.size.height.as_f32());
        if w < 8.0 || h < 8.0 || self.scale <= 0.0 {
            return None;
        }
        Some(MapKey {
            zoom,
            w: w.round() as u32,
            h: h.round() as u32,
            device_w: (w * self.scale).round() as u32,
            device_h: (h * self.scale).round() as u32,
            generation,
            dark,
        })
    }

    pub fn install(&mut self, b: Built) {
        let img = image::RgbaImage::from_raw(b.key.device_w, b.key.device_h, b.pixels).expect("buffer size");
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(img)]));
        if let Some(old) = self.current.take() {
            self.stale.push(old.image.clone());
        }
        self.current = Some(Rc::new(MapLayout { key: b.key, rects: b.rects, index: b.index, image }));
        if self.pending == Some(b.key) {
            self.pending = None;
        }
    }

    /// Converts a window position to layout coordinates and finds the innermost rectangle.
    pub fn hit(&self, pos: Point<Pixels>) -> Option<u32> {
        let l = self.current.as_ref()?;
        let b = self.bounds;
        let (bw, bh) = (b.size.width.as_f32(), b.size.height.as_f32());
        if bw <= 0.0 || bh <= 0.0 {
            return None;
        }
        let x = (pos.x.as_f32() - b.origin.x.as_f32()) * l.key.w as f32 / bw;
        let y = (pos.y.as_f32() - b.origin.y.as_f32()) * l.key.h as f32 / bh;
        if x < 0.0 || y < 0.0 || x >= l.key.w as f32 || y >= l.key.h as f32 {
            return None;
        }
        treemap::hit_test(&l.rects, x, y).map(|i| l.rects[i].node)
    }
}

/// Runs off the UI thread.
pub fn build(tree: Arc<Tree>, key: MapKey, palette: Palette) -> Built {
    let rects = treemap::layout(&tree, key.zoom, key.w as f32, key.h as f32, &Options::default());
    let scale = key.device_w as f32 / key.w.max(1) as f32;
    let pixels = treemap::rasterize(&tree, &rects, &palette, key.device_w, key.device_h, scale, true);
    let index = rects.iter().enumerate().map(|(i, r)| (r.node, i)).collect();
    Built { key, rects, index, pixels }
}

pub struct Paint {
    pub tree: Arc<Tree>,
    pub theme: Theme,
    pub hovered: Option<u32>,
    pub selected: Option<u32>,
}

/// The canvas element. `state` is shared with the view, which schedules layouts.
pub fn element(state: Rc<RefCell<MapState>>, p: Paint) -> impl IntoElement {
    let st = state.clone();
    canvas(
        move |bounds, window, _| {
            let mut s = st.borrow_mut();
            let changed = s.bounds.size != bounds.size || s.scale != window.scale_factor();
            s.bounds = bounds;
            s.scale = window.scale_factor();
            if changed {
                // The view decides on a new layout during its next render.
                window.request_animation_frame();
            }
        },
        move |bounds, _, window, cx| {
            let mut s = state.borrow_mut();
            for old in s.stale.drain(..) {
                let _ = window.drop_image(old);
            }
            let t = &p.theme;
            window.paint_quad(fill(bounds, rgb(t.map_bg)));
            let Some(l) = s.current.clone() else { return };
            drop(s);
            let _ = window.paint_image(bounds, bounds, Corners::default(), l.image.clone(), 0, false);

            let (ox, oy) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
            let sx = bounds.size.width.as_f32() / l.key.w.max(1) as f32;
            let sy = bounds.size.height.as_f32() / l.key.h.max(1) as f32;
            let rect = |r: &MapRect| Bounds::new(point(px(ox + r.x * sx), px(oy + r.y * sy)), size(px(r.w * sx), px(r.h * sy)));

            // Labels: folder names in header strips, and names (plus size) on big leaves.
            let font = window.text_style().font();
            let mut bold = font.clone();
            bold.weight = FontWeight::SEMIBOLD;
            let tree = &p.tree;
            for r in &l.rects {
                let (w, h) = (r.w * sx, r.h * sy);
                if r.group {
                    if r.header <= 0.0 || w < 48.0 {
                        continue;
                    }
                    let size_text = fmt::size(tree.size[r.node as usize]);
                    let max_chars = ((w - 10.0) / CHAR_W).floor() as usize;
                    let text = if max_chars >= size_text.len() + 6 {
                        format!("{}  {size_text}", fmt::middle_truncate(tree.name(r.node), max_chars - size_text.len() - 2))
                    } else {
                        tree.name(r.node).to_string()
                    };
                    label(window, cx, &bold, &text, rgb(t.head), ox + r.x * sx + 5.0, oy + r.y * sy + 1.0, w - 10.0);
                } else if w >= 56.0 && h >= 18.0 {
                    let k = tree.kind[r.node as usize];
                    let ink = if t.kind_dark_ink(k) { rgba(0x141210e6) } else { rgba(0xffffffeb) };
                    label(window, cx, &bold, tree.name(r.node), ink, ox + r.x * sx + 5.0, oy + r.y * sy + 3.0, w - 10.0);
                    if h >= 34.0 {
                        label(window, cx, &font, &fmt::size(tree.size[r.node as usize]), ink, ox + r.x * sx + 5.0, oy + r.y * sy + 17.0, w - 10.0);
                    }
                }
            }

            // Hover and selection: the node's own rectangle, or its nearest visible ancestor.
            let find = |n: u32| -> Option<&MapRect> {
                let mut a = n;
                while a != diskie_model::NONE {
                    if let Some(&i) = l.index.get(&a) {
                        return Some(&l.rects[i]);
                    }
                    a = tree.parent[a as usize];
                }
                None
            };
            if let Some(r) = p.hovered.and_then(find) {
                window.paint_quad(outline(rect(r), rgba(t.ring << 8 | 0xb0), BorderStyle::Solid));
            }
            if let Some(r) = p.selected.and_then(find) {
                let b = rect(r);
                window.paint_quad(fill(b, rgba(t.accent << 8 | 0x30)));
                window.paint_quad(quad(b, px(0.), transparent_black(), px(2.), rgb(t.accent), BorderStyle::Solid));
            }
        },
    )
    .size_full()
}

/// Average advance of an 11 px Segoe UI character, for fitting labels without measuring.
const CHAR_W: f32 = 6.4;

#[allow(clippy::too_many_arguments)]
fn label(window: &mut Window, cx: &mut App, font: &Font, text: &str, color: Rgba, x: f32, y: f32, max_w: f32) {
    let max_chars = (max_w / CHAR_W).floor() as usize;
    if max_chars < 3 {
        return;
    }
    let text = fmt::middle_truncate(text, max_chars);
    let run = TextRun { len: text.len(), font: font.clone(), color: color.into(), background_color: None, underline: None, strikethrough: None };
    let line = window.text_system().shape_line(text.into(), px(11.), &[run], None);
    let _ = line.paint(point(px(x), px(y)), px(14.), TextAlign::Left, None, window, cx);
}
