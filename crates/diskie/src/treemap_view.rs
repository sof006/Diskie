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

use crate::colors::{self, ColorBy, LeafColor};
use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MapKey {
    /// Which scan the layout is for. Node numbers only mean something within one scan's tree.
    pub scan: u64,
    pub zoom: u32,
    pub w: u32,
    pub h: u32,
    pub device_w: u32,
    pub device_h: u32,
    pub generation: u64,
    pub dark: bool,
    pub color: ColorBy,
    /// Which comparison colours by change, when [`ColorBy::Change`].
    pub diff: u64,
}

pub struct MapLayout {
    pub key: MapKey,
    pub rects: Vec<MapRect>,
    /// Node → index of its rectangle.
    pub index: HashMap<u32, usize>,
    pub image: Arc<RenderImage>,
    /// Each rectangle's colour when not coloured by type (empty when by type).
    pub colors: Vec<u32>,
}

/// What the background job hands back.
pub struct Built {
    key: MapKey,
    rects: Vec<MapRect>,
    index: HashMap<u32, usize>,
    pixels: Vec<u8>,
    colors: Vec<u32>,
}

#[derive(Default)]
pub struct MapState {
    pub current: Option<Rc<MapLayout>>,
    pub pending: Option<MapKey>,
    /// A layout whose build panicked, so it isn't retried every frame.
    pub failed: Option<MapKey>,
    /// Where the treemap was last painted, and at what scale.
    pub bounds: Bounds<Pixels>,
    pub scale: f32,
    /// Images to release from the GPU atlas on the next paint.
    pub stale: Vec<Arc<RenderImage>>,
}

impl MapState {
    /// The layout the current bounds call for, if the pane has been painted yet.
    pub fn wanted(&self, scan: u64, zoom: u32, generation: u64, dark: bool, color: ColorBy, diff: u64) -> Option<MapKey> {
        let (w, h) = (self.bounds.size.width.as_f32(), self.bounds.size.height.as_f32());
        if w < 8.0 || h < 8.0 || self.scale <= 0.0 {
            return None;
        }
        Some(MapKey {
            scan,
            zoom,
            w: w.round() as u32,
            h: h.round() as u32,
            device_w: (w * self.scale).round() as u32,
            device_h: (h * self.scale).round() as u32,
            generation,
            dark,
            color,
            diff: if color == ColorBy::Change { diff } else { 0 },
        })
    }

    pub fn install(&mut self, b: Built) {
        let img = image::RgbaImage::from_raw(b.key.device_w, b.key.device_h, b.pixels).expect("buffer size");
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(img)]));
        if let Some(old) = self.current.take() {
            self.stale.push(old.image.clone());
        }
        self.current = Some(Rc::new(MapLayout { key: b.key, rects: b.rects, index: b.index, image, colors: b.colors }));
        if self.pending == Some(b.key) {
            self.pending = None;
        }
    }

    /// Drops the layout of a previous scan, so it is never drawn or hit-tested against a new tree.
    pub fn forget(&mut self) {
        if let Some(old) = self.current.take() {
            self.stale.push(old.image.clone());
        }
        self.pending = None;
        self.failed = None;
    }

    /// Converts a window position to layout coordinates and finds the innermost rectangle of
    /// `scan`'s layout.
    pub fn hit(&self, pos: Point<Pixels>, scan: u64) -> Option<u32> {
        let l = self.current.as_ref().filter(|l| l.key.scan == scan)?;
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
pub fn build(tree: Arc<Tree>, key: MapKey, palette: Palette, leaf: Option<LeafColor>) -> Built {
    let rects = treemap::layout(&tree, key.zoom, key.w as f32, key.h as f32, &Options::default());
    let scale = key.device_w as f32 / key.w.max(1) as f32;
    let (pixels, colors) = match leaf {
        None => (treemap::rasterize(&tree, &rects, &palette, key.device_w, key.device_h, scale, true), Vec::new()),
        Some(f) => {
            let colors: Vec<u32> = rects.iter().map(|r| if r.group { 0 } else { f(&tree, r.node) }).collect();
            let by_node: HashMap<u32, u32> = rects.iter().zip(&colors).map(|(r, &c)| (r.node, c)).collect();
            let pick = |n: u32| by_node.get(&n).copied().unwrap_or(palette.background);
            (treemap::rasterize_with(&tree, &rects, &palette, &pick, key.device_w, key.device_h, scale, true), colors)
        }
    };
    let index = rects.iter().enumerate().map(|(i, r)| (r.node, i)).collect();
    Built { key, rects, index, pixels, colors }
}

pub struct Paint {
    pub tree: Arc<Tree>,
    /// The scan `tree` came from: a layout for any other scan is not drawn.
    pub scan: u64,
    pub theme: Theme,
    pub hovered: Option<u32>,
    pub selected: Option<u32>,
    /// Also selected (Ctrl or Shift click), drawn lighter.
    pub marked: Vec<u32>,
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
            let Some(l) = s.current.clone().filter(|l| l.key.scan == p.scan) else { return };
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
            for (i, r) in l.rects.iter().enumerate() {
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
                    let dark = match l.colors.get(i) {
                        Some(&c) => colors::dark_ink(c),
                        None => t.kind_dark_ink(tree.kind[r.node as usize]),
                    };
                    let ink = if dark { rgba(0x141210e6) } else { rgba(0xffffffeb) };
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
            for &m in &p.marked {
                if let Some(r) = find(m) {
                    let b = rect(r);
                    window.paint_quad(fill(b, rgba(t.accent << 8 | 0x24)));
                    window.paint_quad(outline(b, rgb(t.accent), BorderStyle::Solid));
                }
            }
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

/// Average advance of an 11 px UI-font character, for fitting labels without measuring.
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

#[cfg(test)]
mod tests {
    use super::*;
    use diskie_model::{Chunk, NO_KEY};

    fn tree(files: u64) -> Arc<Tree> {
        let mut c = Chunk::default();
        for i in 0..files {
            c.push(NO_KEY, 0, &format!("f{i}.bin"), 1000 + i, 0, 0);
        }
        Arc::new(Tree::build("root", 0, vec![c], 1))
    }

    fn state() -> MapState {
        MapState { bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(300.))), scale: 1.0, ..Default::default() }
    }

    /// A bigger scan's layout once outlived it: the next scan asked for an identical key, so the
    /// old rectangles were drawn against the new, smaller tree and indexed past its end.
    #[::core::prelude::v1::test]
    fn a_layout_never_carries_over_to_another_scan() {
        let pal = Theme::DARK.palette();
        let mut s = state();
        let big = s.wanted(1, Tree::ROOT, 0, true, ColorBy::Type, 0).unwrap();
        s.install(build(tree(5000), big, pal, None));
        let centre = point(px(200.), px(150.));
        assert!(s.hit(centre, 1).is_some_and(|n| n >= 1000), "the first scan's layout covers its big nodes");

        // The next scan: same pane, same zoom, same generation, different tree.
        let next = s.wanted(2, Tree::ROOT, 0, true, ColorBy::Type, 0).unwrap();
        assert_ne!(next, big, "the old layout must not count as up to date");
        assert_eq!(s.hit(centre, 2), None, "the old layout must not be hit-tested for the new scan");
        s.forget();
        assert!(s.current.is_none());
        assert_eq!(s.stale.len(), 1, "its image is released from the GPU on the next paint");
    }
}
