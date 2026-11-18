//! Squarified treemap layout (Bruls, Huizing & van Wijk) with folder header strips, and a CPU
//! rasterizer for it.
//!
//! The UI draws the rasterized bitmap once per layout and paints only labels, hover and
//! selection live on top. In the week-1 spike, painting each rectangle every frame cost 124 ms
//! at 144k rectangles, while rasterizing them took about 5 ms.

use crate::kind::KIND_COUNT;
use crate::tree::{Tree, flag};

#[derive(Clone, Copy, Debug)]
pub struct MapRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub node: u32,
    pub depth: u16,
    /// A folder drawn as a frame around its children.
    pub group: bool,
    /// Height of the group's header strip (0 when it has none).
    pub header: f32,
}

#[derive(Clone, Copy)]
pub struct Options {
    pub header: f32,
    pub pad: f32,
    /// Smallest side a folder needs before it is opened up into its children.
    pub min_group: f32,
    /// Rectangles under this many square pixels are left out (their area stays with the parent).
    pub min_area: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self { header: 16.0, pad: 1.0, min_group: 10.0, min_area: 2.0 }
    }
}

/// Lays out `root`'s contents in a `w` x `h` pixel rectangle. Rectangles are in paint order:
/// every group comes before the rectangles inside it.
pub fn layout(tree: &Tree, root: u32, w: f32, h: f32, o: &Options) -> Vec<MapRect> {
    let mut out = Vec::with_capacity(16_384);
    if tree.is_dir(root) {
        lay(tree, root, 0.0, 0.0, w, h, 0, &mut out, o);
    } else {
        out.push(MapRect { x: 0.0, y: 0.0, w, h, node: root, depth: 0, group: false, header: 0.0 });
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn lay(t: &Tree, node: u32, x: f32, y: f32, w: f32, h: f32, depth: u16, out: &mut Vec<MapRect>, o: &Options) {
    if w <= 0.5 || h <= 0.5 {
        return;
    }
    let kids = t.kids(node);
    let vals: Vec<f64> = kids.iter().map(|&k| t.size[k as usize] as f64).take_while(|&v| v > 0.0).collect();
    let rects = squarify(&vals, x, y, w, h);
    for (&k, (rx, ry, rw, rh)) in kids.iter().zip(rects) {
        if rw * rh < o.min_area {
            break; // children are sorted largest first, so the rest are smaller still
        }
        let opens = t.is_dir(k) && !t.kids(k).is_empty() && !t.has(k, flag::SYNTHETIC) && rw >= o.min_group && rh >= o.min_group;
        if opens {
            let header = if rh >= o.min_group + o.header && rw >= 40.0 { o.header } else { 0.0 };
            out.push(MapRect { x: rx, y: ry, w: rw, h: rh, node: k, depth, group: true, header });
            let top = if header > 0.0 { header } else { o.pad };
            lay(t, k, rx + o.pad, ry + top, rw - 2.0 * o.pad, rh - top - o.pad, depth + 1, out, o);
        } else {
            out.push(MapRect { x: rx, y: ry, w: rw, h: rh, node: k, depth, group: false, header: 0.0 });
        }
    }
}

/// Lays `vals` (sorted largest first, all positive) out in the rectangle; returns (x, y, w, h) per value.
pub fn squarify(vals: &[f64], x: f32, y: f32, w: f32, h: f32) -> Vec<(f32, f32, f32, f32)> {
    let total: f64 = vals.iter().sum();
    let mut out = vec![(x, y, 0.0, 0.0); vals.len()];
    if total <= 0.0 || w <= 0.0 || h <= 0.0 {
        return out;
    }
    let scale = (w as f64 * h as f64) / total;
    let areas: Vec<f64> = vals.iter().map(|v| v * scale).collect();
    let (mut rx, mut ry, mut rw, mut rh) = (x as f64, y as f64, w as f64, h as f64);
    let mut row_start = 0;
    let mut i = 0;

    let worst = |sum: f64, mx: f64, mn: f64, side: f64| {
        let (s2, sum2) = (side * side, sum * sum);
        (s2 * mx / sum2).max(sum2 / (s2 * mn))
    };

    let mut place = |from: usize, to: usize, rx: &mut f64, ry: &mut f64, rw: &mut f64, rh: &mut f64| {
        let sum: f64 = areas[from..to].iter().sum();
        if *rw >= *rh {
            let cw = if *rh > 0.0 { sum / *rh } else { 0.0 };
            let mut cy = *ry;
            for k in from..to {
                let hh = if cw > 0.0 { areas[k] / cw } else { 0.0 };
                out[k] = (*rx as f32, cy as f32, cw as f32, hh as f32);
                cy += hh;
            }
            *rx += cw;
            *rw -= cw;
        } else {
            let ch = if *rw > 0.0 { sum / *rw } else { 0.0 };
            let mut cx = *rx;
            for k in from..to {
                let ww = if ch > 0.0 { areas[k] / ch } else { 0.0 };
                out[k] = (cx as f32, *ry as f32, ww as f32, ch as f32);
                cx += ww;
            }
            *ry += ch;
            *rh -= ch;
        }
    };

    let (mut sum, mut mx, mut mn) = (0.0f64, 0.0f64, f64::INFINITY);
    while i < areas.len() {
        let a = areas[i];
        let side = rw.min(rh);
        if i == row_start || worst(sum, mx, mn, side) >= worst(sum + a, mx.max(a), mn.min(a), side) {
            sum += a;
            mx = mx.max(a);
            mn = mn.min(a);
            i += 1;
        } else {
            place(row_start, i, &mut rx, &mut ry, &mut rw, &mut rh);
            row_start = i;
            (sum, mx, mn) = (0.0, 0.0, f64::INFINITY);
        }
    }
    if row_start < i {
        place(row_start, i, &mut rx, &mut ry, &mut rw, &mut rh);
    }
    out
}

/// Colours for the rasterizer, 0xRRGGBB.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub background: u32,
    /// Group fills by depth, cycling.
    pub groups: [u32; 3],
    pub kinds: [u32; KIND_COUNT],
}

/// Rasterizes the rectangles into a `w` x `h` RGBA (or BGRA) buffer at `scale` device pixels
/// per layout pixel. Leaves get a 1-pixel gap where they are big enough to keep one.
pub fn rasterize(tree: &Tree, rects: &[MapRect], pal: &Palette, w: u32, h: u32, scale: f32, bgra: bool) -> Vec<u8> {
    let px = |c: u32| -> [u8; 4] {
        let (r, g, b) = (((c >> 16) & 0xff) as u8, ((c >> 8) & 0xff) as u8, (c & 0xff) as u8);
        if bgra { [b, g, r, 255] } else { [r, g, b, 255] }
    };
    let mut buf: Vec<u8> = px(pal.background).repeat((w as usize) * (h as usize));
    for r in rects {
        let mut x0 = (r.x * scale).round() as i64;
        let mut y0 = (r.y * scale).round() as i64;
        let mut x1 = ((r.x + r.w) * scale).round() as i64;
        let mut y1 = ((r.y + r.h) * scale).round() as i64;
        if !r.group && x1 - x0 > 3 && y1 - y0 > 3 {
            x1 -= 1;
            y1 -= 1;
        }
        x0 = x0.clamp(0, w as i64);
        x1 = x1.clamp(0, w as i64);
        y0 = y0.clamp(0, h as i64);
        y1 = y1.clamp(0, h as i64);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let color = if r.group { pal.groups[r.depth as usize % 3] } else { pal.kinds[tree.kind[r.node as usize] as usize % KIND_COUNT] };
        let c = px(color);
        for y in y0..y1 {
            let start = (y as usize * w as usize + x0 as usize) * 4;
            let end = (y as usize * w as usize + x1 as usize) * 4;
            for p in buf[start..end].chunks_exact_mut(4) {
                p.copy_from_slice(&c);
            }
        }
    }
    buf
}

/// Index of the innermost rectangle containing the point.
pub fn hit_test(rects: &[MapRect], x: f32, y: f32) -> Option<usize> {
    rects.iter().rposition(|r| x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Chunk, NO_KEY};

    #[test]
    fn squarify_fills_the_rectangle() {
        let vals = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let r = squarify(&vals, 0.0, 0.0, 600.0, 400.0);
        let area: f32 = r.iter().map(|(_, _, w, h)| w * h).sum();
        assert!((area - 240_000.0).abs() < 1.0, "area {area}");
        for (x, y, w, h) in r {
            assert!(x >= -0.01 && y >= -0.01 && x + w <= 600.01 && y + h <= 400.01);
        }
    }

    #[test]
    fn layout_nests_groups_inside_their_parents() {
        let mut c = Chunk::default();
        c.push(1, 0, "dir", 0, 0, flag::DIR);
        for i in 0..20 {
            c.push(NO_KEY, 1, &format!("f{i}.mkv"), 1000 + i, 0, 0);
            c.push(NO_KEY, 0, &format!("g{i}.zip"), 500 + i, 0, 0);
        }
        let t = Tree::build("C:\\", 0, vec![c], 4);
        let rects = layout(&t, 0, 800.0, 500.0, &Options::default());
        let g = rects.iter().find(|r| r.group).expect("the folder opens");
        let inner = rects.iter().filter(|r| t.parent[r.node as usize] == g.node).count();
        assert_eq!(inner, 20);
        for r in rects.iter().filter(|r| t.parent[r.node as usize] == g.node) {
            assert!(r.x >= g.x && r.y >= g.y + g.header - 0.01 && r.x + r.w <= g.x + g.w + 0.01);
        }
        let pal = Palette { background: 0, groups: [1, 2, 3], kinds: [0xff0000; KIND_COUNT] };
        let buf = rasterize(&t, &rects, &pal, 800, 500, 1.0, false);
        assert_eq!(buf.len(), 800 * 500 * 4);
        assert!(hit_test(&rects, 799.0, 499.0).is_some());
    }
}
