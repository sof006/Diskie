//! Week-1 spike: the Diskie main view in GPUI.
//!
//! A uniform (virtualized) list over all ~2M rows of the expanded tree, and
//! a canvas that paints every treemap rectangle (~150k) as GPU quads on each
//! frame, with hover hit-testing and labels. `--bench` runs the scripted
//! scroll-and-hover benchmark and writes `spike-results/gpui.json`.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::*;
use spike_data::color::{dark, kind_dark_ink, kind_rgb};
use spike_data::{BenchDriver, BenchReport, Dataset, FrameStats, MAP_H, MAP_W, ProcSample, fmt};

const ROW_H: f32 = 24.0;

struct Spike {
    data: Arc<Dataset>,
    scroll: UniformListScrollHandle,
    hovered: Option<usize>,
    stats: Rc<RefCell<FrameStats>>,
    frame_start: Rc<Cell<Instant>>,
    map_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Cached treemap bitmap: device width, height, image.
    raster: Rc<RefCell<Option<(u32, u32, Arc<RenderImage>)>>>,
    bench: Option<BenchDriver>,
    bench_begin: Option<ProcSample>,
    started: Instant,
    first_frame_ms: Option<f64>,
    reported: bool,
}

impl Spike {
    fn new(data: Arc<Dataset>, started: Instant) -> Self {
        let bench = BenchDriver::from_args(data.rows.len(), data.rects.len());
        Self {
            data,
            scroll: UniformListScrollHandle::new(),
            hovered: None,
            stats: Rc::default(),
            frame_start: Rc::new(Cell::new(Instant::now())),
            map_bounds: Rc::new(Cell::new(Bounds::default())),
            raster: Rc::default(),
            bench,
            bench_begin: None,
            started,
            first_frame_ms: None,
            reported: false,
        }
    }

    fn rows(&self, range: Range<usize>) -> Vec<Div> {
        let t = &self.data.tree;
        range
            .map(|i| {
                let n = self.data.rows[i];
                let pct = t.percent_of_parent(n);
                let depth = t.depth[n as usize] as f32;
                div()
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .px_3()
                    .gap_3()
                    .hover(|s| s.bg(rgb(0x24221f)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pl(px(depth * 14.0))
                            .truncate()
                            .child(t.name(n)),
                    )
                    .child(div().w(px(84.)).flex().justify_end().child(fmt::size(t.size[n as usize])))
                    .child(
                        div()
                            .w(px(120.))
                            .h(px(3.))
                            .rounded_sm()
                            .bg(rgb(dark::TRACK))
                            .child(div().h_full().rounded_sm().w(relative(pct / 100.0)).bg(rgb(kind_rgb(t.kind[n as usize])))),
                    )
                    .child(div().w(px(52.)).flex().justify_end().text_color(rgb(dark::MUTED)).child(fmt::percent(pct)))
                    .child(div().w(px(84.)).flex().justify_end().text_color(rgb(dark::MUTED)).child(fmt::count(t.files[n as usize])))
            })
            .collect()
    }

    fn treemap(&self) -> impl IntoElement {
        let data = self.data.clone();
        let hovered = self.hovered;
        let stats = self.stats.clone();
        let frame_start = self.frame_start.clone();
        let map_bounds = self.map_bounds.clone();
        let raster = self.raster.clone();
        canvas(
            move |bounds, _, _| {
                map_bounds.set(bounds);
                bounds
            },
            move |bounds, _, window, cx| {
                let (ox, oy) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
                let sx = bounds.size.width.as_f32() / MAP_W;
                let sy = bounds.size.height.as_f32() / MAP_H;
                let rect = |x: f32, y: f32, w: f32, h: f32| {
                    Bounds::new(point(px(ox + x * sx), px(oy + y * sy)), size(px(w * sx), px(h * sy)))
                };

                if raster_mode() {
                    // Rasterize once per size into a bitmap; each frame then paints one image.
                    let scale = window.scale_factor();
                    let dw = (bounds.size.width.as_f32() * scale).round().max(1.0) as u32;
                    let dh = (bounds.size.height.as_f32() * scale).round().max(1.0) as u32;
                    let mut cache = raster.borrow_mut();
                    if cache.as_ref().is_none_or(|(w, h, _)| (*w, *h) != (dw, dh)) {
                        let t0 = Instant::now();
                        let bytes = spike_data::treemap::rasterize(&data.rects, MAP_W, MAP_H, dw, dh, true);
                        let img = image::RgbaImage::from_raw(dw, dh, bytes).expect("buffer size");
                        let render = Arc::new(RenderImage::new(vec![image::Frame::new(img)]));
                        if let Some((_, _, old)) = cache.take() {
                            let _ = window.drop_image(old);
                        }
                        eprintln!("treemap rasterized at {dw}x{dh} in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
                        *cache = Some((dw, dh, render));
                    }
                    let (_, _, img) = cache.as_ref().unwrap();
                    let _ = window.paint_image(bounds, bounds, Corners::default(), img.clone(), 0, false);
                } else {
                    window.paint_quad(fill(bounds, rgb(dark::MAP_BG)));
                    for r in &data.rects {
                        // A hairline gap between leaves wherever there is room for one.
                        let inset = if !r.group && r.w * sx > 3.0 && r.h * sy > 3.0 { 0.5 / sx } else { 0.0 };
                        window.paint_quad(fill(
                            rect(r.x + inset, r.y + inset, r.w - 2.0 * inset, r.h - 2.0 * inset),
                            rgb(r.color),
                        ));
                    }
                }

                // Labels: folder headers and big leaves.
                let font = window.text_style().font();
                let line_h = px(14.);
                for r in data.rects.iter().filter(|r| r.label && env_on("SPIKE_LABELS")) {
                    let (w, h) = (r.w * sx, if r.group { r.header } else { r.h } * sy);
                    if w < 48.0 || h < 13.0 {
                        continue;
                    }
                    let ink = if r.group {
                        rgb(dark::HEAD)
                    } else if kind_dark_ink(data.tree.kind[r.node as usize]) {
                        rgba(0x141210e0)
                    } else {
                        rgba(0xffffffeb)
                    };
                    let mut name = data.tree.name(r.node);
                    let max_chars = ((w - 10.0) / 6.2) as usize;
                    if name.len() > max_chars {
                        name.truncate(max_chars.saturating_sub(1));
                        name.push('…');
                    }
                    let run = TextRun {
                        len: name.len(),
                        font: font.clone(),
                        color: ink.into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window.text_system().shape_line(name.into(), px(11.), &[run], None);
                    let origin = point(px(ox + r.x * sx + 5.0), px(oy + r.y * sy + 1.0));
                    let _ = line.paint(origin, line_h, TextAlign::Left, None, window, cx);
                }

                if let Some(i) = hovered {
                    let r = &data.rects[i];
                    window.paint_quad(outline(rect(r.x, r.y, r.w, r.h), rgb(dark::ACCENT), BorderStyle::Solid));
                }

                stats.borrow_mut().build(frame_start.get().elapsed());
            },
        )
        .size_full()
    }
}

impl Render for Spike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame_start.set(Instant::now());
        self.stats.borrow_mut().frame();
        if self.first_frame_ms.is_none() {
            self.first_frame_ms = Some(self.started.elapsed().as_secs_f64() * 1000.0);
        }

        if let Some(bench) = self.bench.as_mut() {
            let begin = *self.bench_begin.get_or_insert_with(ProcSample::now);
            match bench.step() {
                Some(step) => {
                    self.scroll.scroll_to_item(step.scroll_row, ScrollStrategy::Top);
                    self.hovered = Some(step.hover_rect);
                    window.request_animation_frame();
                }
                None if !self.reported => {
                    self.reported = true;
                    BenchReport {
                        framework: if raster_mode() { "gpui-raster" } else { "gpui-quads" },
                        nodes: self.data.tree.len(),
                        rows: self.data.rows.len(),
                        rects: self.data.rects.len(),
                        data_build_ms: self.data.build_ms,
                        startup_ms: self.first_frame_ms.unwrap_or(0.0),
                        stats: &self.stats.borrow(),
                        begin,
                        end: ProcSample::now(),
                    }
                    .write();
                    cx.quit();
                }
                None => {}
            }
        }

        let hover_text = self
            .hovered
            .map(|i| {
                let n = self.data.rects[i].node;
                format!("{} · {}", self.data.tree.name(n), fmt::size(self.data.tree.size[n as usize]))
            })
            .unwrap_or_else(|| "Hover the treemap".into());

        let header_cell = |label: &'static str, w: f32| div().w(px(w)).flex().justify_end().child(label);

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(dark::CANVAS))
            .text_color(rgb(dark::TEXT))
            .font_family("Segoe UI")
            .text_size(px(13.))
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(dark::CHROME))
                    .border_b_1()
                    .border_color(rgb(dark::LINE))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Diskie · GPUI spike"))
                    .child(div().text_color(rgb(dark::MUTED)).text_size(px(12.)).child(format!(
                        "{} rows · {} rects · {}",
                        fmt::count(self.data.rows.len() as u32),
                        fmt::count(self.data.rects.len() as u32),
                        self.stats.borrow().live()
                    ))),
            )
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .px_3()
                    .gap_3()
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(rgb(dark::MUTED))
                    .border_b_1()
                    .border_color(rgb(dark::LINE))
                    .child(div().flex_1().child("Name"))
                    .child(header_cell("Size", 84.))
                    .child(div().w(px(120.)).child("Share of parent"))
                    .child(header_cell("", 52.))
                    .child(header_cell("Files", 84.)),
            )
            .child(
                uniform_list("tree", self.data.rows.len(), cx.processor(|this: &mut Self, range, _, _| this.rows(range)))
                    .track_scroll(&self.scroll)
                    .h(px(300.))
                    .flex_none(),
            )
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(rgb(dark::MUTED))
                    .border_t_1()
                    .border_color(rgb(dark::LINE))
                    .child(hover_text),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_3()
                    .pb_3()
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        let b = this.map_bounds.get();
                        let w = b.size.width.as_f32();
                        let h = b.size.height.as_f32();
                        if w <= 0.0 || h <= 0.0 {
                            return;
                        }
                        let x = (ev.position.x.as_f32() - b.origin.x.as_f32()) / w * MAP_W;
                        let y = (ev.position.y.as_f32() - b.origin.y.as_f32()) / h * MAP_H;
                        let hit = if (0.0..MAP_W).contains(&x) && (0.0..MAP_H).contains(&y) { this.data.hit_test(x, y) } else { None };
                        if hit != this.hovered {
                            this.hovered = hit;
                            cx.notify();
                        }
                    }))
                    .child(self.treemap()),
            )
    }
}

fn main() {
    let started = Instant::now();
    let data = Arc::new(Dataset::from_env());
    eprintln!(
        "dataset: {} nodes, {} rows, {} rects in {:.0} ms",
        data.tree.len(),
        data.rows.len(),
        data.rects.len(),
        data.build_ms
    );
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1000.), px(720.)), cx))),
            titlebar: Some(TitlebarOptions { title: Some("Diskie · GPUI spike".into()), ..Default::default() }),
            inactive_frame_interval: None,
            low_power_frame_interval: None,
            ..Default::default()
        };
        let data = data.clone();
        gpui_kit::open_window(options, cx, move |_, cx| cx.new(|_| Spike::new(data, started)))
            .expect("failed to open window");
        cx.on_window_closed(|cx, _| cx.quit()).detach();
    });
}

/// Benchmark toggles: `SPIKE_QUADS=0`, `SPIKE_LABELS=0` switch a layer off.
fn env_on(key: &str) -> bool {
    std::env::var(key).map(|v| v != "0").unwrap_or(true)
}

/// `SPIKE_MAP_MODE=quads` paints every rectangle as a GPUI quad each frame;
/// the default `raster` paints a cached bitmap plus labels and hover on top.
fn raster_mode() -> bool {
    std::env::var("SPIKE_MAP_MODE").map(|v| v != "quads").unwrap_or(true)
}
