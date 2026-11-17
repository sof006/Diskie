//! Squarified treemap layout (Bruls, Huizing & van Wijk) with folder header strips.

use crate::color::{dark, kind_rgb};
use crate::tree::Tree;

#[derive(Clone, Copy, Debug)]
pub struct MapRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Fill colour, 0xRRGGBB.
    pub color: u32,
    pub node: u32,
    pub depth: u8,
    /// A folder drawn as a frame around its children (with an optional header strip).
    pub group: bool,
    /// Height of the header strip for groups, 0 when none.
    pub header: f32,
    /// Big enough to carry a text label.
    pub label: bool,
}

pub struct TreemapOptions {
    pub header: f32,
    pub pad: f32,
    /// Smallest side a folder needs before it is opened up as a group.
    pub min_group: f32,
    /// Rectangles smaller than this many square pixels are not emitted.
    pub min_area: f32,
}

impl Default for TreemapOptions {
    /// The stress setting (~150k rectangles for the 2M-node tree): almost no
    /// padding, folders open down to 3 px, rectangles down to 0.35 px².
    /// `SPIKE_PAD` / `SPIKE_MIN_GROUP` / `SPIKE_MIN_AREA` override it. The
    /// readable setting a real app would use is about 1 / 22 / 3 (~3k rectangles).
    fn default() -> Self {
        let env = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        Self {
            header: 15.0,
            pad: env("SPIKE_PAD", 0.2),
            min_group: env("SPIKE_MIN_GROUP", 3.0),
            min_area: env("SPIKE_MIN_AREA", 0.35),
        }
    }
}

pub fn layout(tree: &Tree, root: u32, w: f32, h: f32, o: &TreemapOptions) -> Vec<MapRect> {
    let mut out = Vec::with_capacity(200_000);
    lay(tree, root, 0.0, 0.0, w, h, 0, &mut out, o);
    out
}

#[allow(clippy::too_many_arguments)]
fn lay(t: &Tree, node: u32, x: f32, y: f32, w: f32, h: f32, depth: u8, out: &mut Vec<MapRect>, o: &TreemapOptions) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let kids = t.children(node);
    let vals: Vec<f32> = kids.iter().map(|&k| t.size[k as usize] as f32).collect();
    let rects = squarify(&vals, x, y, w, h);
    for (&k, r) in kids.iter().zip(rects) {
        let (rx, ry, rw, rh) = r;
        if rw * rh < o.min_area {
            break; // children are sorted by size, so the rest are smaller still
        }
        let ki = k as usize;
        let opens = t.is_dir[ki] && t.child_count[ki] > 0 && rw >= o.min_group && rh >= o.min_group;
        if opens {
            let header = if rh >= o.min_group + o.header && rw >= 40.0 { o.header } else { 0.0 };
            out.push(MapRect {
                x: rx,
                y: ry,
                w: rw,
                h: rh,
                color: dark::GROUPS[depth as usize % 3],
                node: k,
                depth,
                group: true,
                header,
                label: header > 0.0 && rw > 50.0,
            });
            let top = if header > 0.0 { header } else { o.pad };
            lay(t, k, rx + o.pad, ry + top, rw - 2.0 * o.pad, rh - top - o.pad, depth + 1, out, o);
        } else {
            out.push(MapRect {
                x: rx,
                y: ry,
                w: rw,
                h: rh,
                color: kind_rgb(t.kind[ki]),
                node: k,
                depth,
                group: false,
                header: 0.0,
                label: rw > 60.0 && rh > 18.0,
            });
        }
    }
}

/// Lays `vals` (sorted largest first) out in the rectangle; returns (x, y, w, h) per value.
pub fn squarify(vals: &[f32], x: f32, y: f32, w: f32, h: f32) -> Vec<(f32, f32, f32, f32)> {
    let total: f32 = vals.iter().sum();
    let mut out = vec![(x, y, 0.0, 0.0); vals.len()];
    if total <= 0.0 || w <= 0.0 || h <= 0.0 {
        return out;
    }
    let scale = w * h / total;
    let areas: Vec<f32> = vals.iter().map(|v| v * scale).collect();
    let (mut rx, mut ry, mut rw, mut rh) = (x, y, w, h);
    let mut row_start = 0;
    let mut i = 0;

    let worst = |sum: f32, mx: f32, mn: f32, side: f32| {
        let (s2, sum2) = (side * side, sum * sum);
        (s2 * mx / sum2).max(sum2 / (s2 * mn))
    };

    let mut place = |from: usize, to: usize, rx: &mut f32, ry: &mut f32, rw: &mut f32, rh: &mut f32| {
        let sum: f32 = areas[from..to].iter().sum();
        if *rw >= *rh {
            let cw = sum / *rh;
            let mut cy = *ry;
            for k in from..to {
                let hh = areas[k] / cw;
                out[k] = (*rx, cy, cw, hh);
                cy += hh;
            }
            *rx += cw;
            *rw -= cw;
        } else {
            let ch = sum / *rw;
            let mut cx = *rx;
            for k in from..to {
                let ww = areas[k] / ch;
                out[k] = (cx, *ry, ww, ch);
                cx += ww;
            }
            *ry += ch;
            *rh -= ch;
        }
    };

    let (mut sum, mut mx, mut mn) = (0.0f32, 0.0f32, f32::INFINITY);
    while i < areas.len() {
        let a = areas[i];
        if a <= 0.0 {
            i += 1;
            continue;
        }
        let side = rw.min(rh);
        if i == row_start || worst(sum, mx, mn, side) >= worst(sum + a, mx.max(a), mn.min(a), side) {
            sum += a;
            mx = mx.max(a);
            mn = mn.min(a);
            i += 1;
        } else {
            place(row_start, i, &mut rx, &mut ry, &mut rw, &mut rh);
            row_start = i;
            (sum, mx, mn) = (0.0, 0.0, f32::INFINITY);
        }
    }
    if row_start < i {
        place(row_start, i, &mut rx, &mut ry, &mut rw, &mut rh);
    }
    drop(place);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

/// Software-rasterizes the rectangles into a `w` x `h` pixel buffer
/// (4 bytes per pixel, BGRA when `bgra`, else RGBA). Leaves get a 1 px gap
/// where they are big enough. Axis-aligned fills only, so this is cheap.
pub fn rasterize(rects: &[MapRect], virt_w: f32, virt_h: f32, w: u32, h: u32, bgra: bool) -> Vec<u8> {
    let px = |c: u32| -> [u8; 4] {
        let (r, g, b) = (((c >> 16) & 0xff) as u8, ((c >> 8) & 0xff) as u8, (c & 0xff) as u8);
        if bgra { [b, g, r, 255] } else { [r, g, b, 255] }
    };
    let mut buf: Vec<u8> = px(dark::MAP_BG).repeat((w * h) as usize);
    let (sx, sy) = (w as f32 / virt_w, h as f32 / virt_h);
    for r in rects {
        let mut x0 = (r.x * sx).round() as i64;
        let mut y0 = (r.y * sy).round() as i64;
        let mut x1 = ((r.x + r.w) * sx).round() as i64;
        let mut y1 = ((r.y + r.h) * sy).round() as i64;
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
        let c = px(r.color);
        for y in y0..y1 {
            let row = &mut buf[((y as u32 * w + x0 as u32) * 4) as usize..((y as u32 * w + x1 as u32) * 4) as usize];
            for p in row.chunks_exact_mut(4) {
                p.copy_from_slice(&c);
            }
        }
    }
    buf
}
