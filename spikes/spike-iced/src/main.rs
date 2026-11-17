//! Week-1 spike: the Diskie main view in iced 0.14.
//!
//! iced has no built-in virtual list, so the tree is a `scrollable` whose
//! content is a top spacer, only the visible rows, and a bottom spacer.
//! The treemap is a `canvas` whose rectangles are tessellated once into a
//! cached mesh (`SPIKE_MAP_MODE=mesh`, the default) or drawn as one cached
//! bitmap (`raster`); hover is a separate, uncached layer.
//! `--bench` runs the scripted benchmark and writes `spike-results/iced-*.json`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::scrollable::{AbsoluteOffset, Viewport};
use iced::widget::{Space, canvas, column, container, row, scrollable, text};
use iced::{Color, Element, Font, Length, Point, Rectangle, Size, Subscription, Task, Theme, mouse};
use spike_data::color::{dark, kind_dark_ink, kind_rgb, rgb_f32};
use spike_data::{BenchDriver, BenchReport, Dataset, FrameStats, MAP_H, MAP_W, ProcSample, fmt};

const ROW_H: f32 = 24.0;
const LIST_H: f32 = 300.0;

fn col(c: u32) -> Color {
    let [r, g, b] = rgb_f32(c);
    Color::from_rgb(r, g, b)
}

fn raster_mode() -> bool {
    std::env::var("SPIKE_MAP_MODE").is_ok_and(|v| v == "raster")
}

struct Spike {
    data: Arc<Dataset>,
    offset_y: f32,
    viewport_h: f32,
    hovered: Option<usize>,
    stats: FrameStats,
    map_cache: canvas::Cache,
    raster: Option<iced::widget::image::Handle>,
    bench: Option<BenchDriver>,
    bench_begin: Option<ProcSample>,
    started: Instant,
    first_frame_ms: Option<f64>,
    reported: bool,
    last_view: std::cell::Cell<Duration>,
}

#[derive(Debug, Clone)]
enum Message {
    Scrolled(Viewport),
    Frame,
    Hover(Option<usize>),
}

impl Spike {
    fn new(data: Arc<Dataset>, started: Instant) -> Self {
        let bench = BenchDriver::from_args(data.rows.len(), data.rects.len());
        let raster = raster_mode().then(|| {
            // Rasterized once at the expected on-screen size; the image scales on resize.
            let (w, h) = (976, 320);
            let t0 = Instant::now();
            let px = spike_data::treemap::rasterize(&data.rects, MAP_W, MAP_H, w, h, false);
            eprintln!("treemap rasterized at {w}x{h} in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
            iced::widget::image::Handle::from_rgba(w, h, px)
        });
        Self {
            data,
            offset_y: 0.0,
            viewport_h: LIST_H,
            hovered: None,
            stats: FrameStats::default(),
            map_cache: canvas::Cache::new(),
            raster,
            bench,
            bench_begin: None,
            started,
            first_frame_ms: None,
            reported: false,
            last_view: Default::default(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Scrolled(v) => {
                self.offset_y = v.absolute_offset().y;
                self.viewport_h = v.bounds().height;
                Task::none()
            }
            Message::Hover(h) => {
                self.hovered = h;
                Task::none()
            }
            Message::Frame => {
                self.stats.frame();
                self.stats.build(self.last_view.get());
                if self.first_frame_ms.is_none() {
                    self.first_frame_ms = Some(self.started.elapsed().as_secs_f64() * 1000.0);
                }
                let Some(bench) = self.bench.as_mut() else { return Task::none() };
                let begin = *self.bench_begin.get_or_insert_with(ProcSample::now);
                match bench.step() {
                    Some(step) => {
                        self.hovered = Some(step.hover_rect);
                        let y = step.scroll_row as f32 * ROW_H;
                        iced::widget::operation::scroll_to("tree", AbsoluteOffset { x: 0.0, y })
                    }
                    None if !self.reported => {
                        self.reported = true;
                        BenchReport {
                            framework: if raster_mode() { "iced-raster" } else { "iced-mesh" },
                            nodes: self.data.tree.len(),
                            rows: self.data.rows.len(),
                            rects: self.data.rects.len(),
                            data_build_ms: self.data.build_ms,
                            startup_ms: self.first_frame_ms.unwrap_or(0.0),
                            stats: &self.stats,
                            begin,
                            end: ProcSample::now(),
                        }
                        .write();
                        iced::exit()
                    }
                    None => Task::none(),
                }
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        // Only the benchmark listens to every frame: in iced each message re-renders,
        // so an always-on frame subscription would redraw continuously.
        if self.bench.is_some() { iced::window::frames().map(|_| Message::Frame) } else { Subscription::none() }
    }

    fn view(&self) -> Element<'_, Message> {
        let t0 = Instant::now();
        let t = &self.data.tree;
        let total = self.data.rows.len();

        // Visible rows plus a little overscan.
        let first = ((self.offset_y / ROW_H) as usize).saturating_sub(4).min(total);
        let last = (((self.offset_y + self.viewport_h) / ROW_H) as usize + 4).min(total);
        let mut rows = column![Space::new().height(first as f32 * ROW_H)];
        for i in first..last {
            let n = self.data.rows[i];
            let pct = t.percent_of_parent(n);
            let depth = t.depth[n as usize] as f32;
            let bar_color = col(kind_rgb(t.kind[n as usize]));
            let bar = container(
                container(Space::new())
                    .width(120.0 * pct / 100.0)
                    .height(3)
                    .style(move |_| container::background(bar_color)),
            )
            .width(120)
            .height(3)
            .style(|_| container::background(col(dark::TRACK)));
            rows = rows.push(
                row![
                    container(text(t.name(n)).wrapping(text::Wrapping::None))
                        .padding(iced::Padding { left: depth * 14.0, ..Default::default() })
                        .width(Length::Fill)
                        .clip(true),
                    text(fmt::size(t.size[n as usize])).width(84).align_x(iced::alignment::Horizontal::Right),
                    bar,
                    text(fmt::percent(pct)).width(52).color(col(dark::MUTED)).align_x(iced::alignment::Horizontal::Right),
                    text(fmt::count(t.files[n as usize])).width(84).color(col(dark::MUTED)).align_x(iced::alignment::Horizontal::Right),
                ]
                .spacing(12)
                .padding([0, 12])
                .height(ROW_H)
                .align_y(iced::alignment::Vertical::Center),
            );
        }
        rows = rows.push(Space::new().height((total - last) as f32 * ROW_H));

        let list = scrollable(rows).id("tree").height(LIST_H).on_scroll(Message::Scrolled);

        let hover_text = self
            .hovered
            .map(|i| {
                let n = self.data.rects[i].node;
                format!("{} · {}", t.name(n), fmt::size(t.size[n as usize]))
            })
            .unwrap_or_else(|| "Hover the treemap".into());

        let header = container(
            row![
                text("Diskie · iced spike").font(Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT }),
                Space::new().width(Length::Fill),
                text(format!(
                    "{} rows · {} rects · {}",
                    fmt::count(total as u32),
                    fmt::count(self.data.rects.len() as u32),
                    self.stats.live()
                ))
                .size(12)
                .color(col(dark::MUTED)),
            ]
            .align_y(iced::alignment::Vertical::Center),
        )
        .padding([0, 12])
        .height(32)
        .align_y(iced::alignment::Vertical::Center)
        .style(|_| container::background(col(dark::CHROME)));

        let map = canvas(MapProgram { data: &self.data, cache: &self.map_cache, raster: self.raster.as_ref(), hovered: self.hovered })
            .width(Length::Fill)
            .height(Length::Fill);

        let body = column![
            header,
            list,
            container(text(hover_text).size(12).color(col(dark::MUTED))).padding([6, 12]),
            container(map).padding(iced::Padding { top: 0.0, right: 12.0, bottom: 12.0, left: 12.0 }).height(Length::Fill),
        ];

        let el = container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(col(dark::CANVAS).into()),
                text_color: Some(col(dark::TEXT)),
                ..Default::default()
            })
            .into();
        self.last_view.set(t0.elapsed());
        el
    }
}

struct MapProgram<'a> {
    data: &'a Dataset,
    cache: &'a canvas::Cache,
    raster: Option<&'a iced::widget::image::Handle>,
    hovered: Option<usize>,
}

impl canvas::Program<Message> for MapProgram<'_> {
    type State = ();

    fn update(&self, _: &mut (), event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<Message>> {
        if let canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) = event {
            let hit = cursor.position_in(bounds).and_then(|p| {
                self.data.hit_test(p.x / bounds.width * MAP_W, p.y / bounds.height * MAP_H)
            });
            if hit != self.hovered {
                return Some(canvas::Action::publish(Message::Hover(hit)));
            }
        }
        None
    }

    fn draw(&self, _: &(), renderer: &iced::Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<canvas::Geometry> {
        let (sx, sy) = (bounds.width / MAP_W, bounds.height / MAP_H);
        let base = self.cache.draw(renderer, bounds.size(), |frame| {
            if let Some(handle) = self.raster {
                frame.draw_image(
                    Rectangle::with_size(frame.size()),
                    canvas::Image::new(handle.clone()).filter_method(iced::widget::image::FilterMethod::Nearest),
                );
            } else {
                frame.fill_rectangle(Point::ORIGIN, frame.size(), col(dark::MAP_BG));
                for r in &self.data.rects {
                    let inset = if !r.group && r.w * sx > 3.0 && r.h * sy > 3.0 { 0.5 } else { 0.0 };
                    frame.fill_rectangle(
                        Point::new(r.x * sx + inset, r.y * sy + inset),
                        Size::new((r.w * sx - 2.0 * inset).max(0.0), (r.h * sy - 2.0 * inset).max(0.0)),
                        col(r.color),
                    );
                }
            }
            for r in self.data.rects.iter().filter(|r| r.label) {
                let (w, h) = (r.w * sx, if r.group { r.header } else { r.h } * sy);
                if w < 48.0 || h < 13.0 {
                    continue;
                }
                let ink = if r.group {
                    col(dark::HEAD)
                } else if kind_dark_ink(self.data.tree.kind[r.node as usize]) {
                    Color::from_rgba8(0x14, 0x12, 0x10, 0.88)
                } else {
                    Color::from_rgba8(0xff, 0xff, 0xff, 0.92)
                };
                let mut name = self.data.tree.name(r.node);
                let max_chars = ((w - 10.0) / 6.2) as usize;
                if name.len() > max_chars {
                    name.truncate(max_chars.saturating_sub(1));
                    name.push('…');
                }
                frame.fill_text(canvas::Text {
                    content: name,
                    position: Point::new(r.x * sx + 5.0, r.y * sy + 1.0),
                    color: ink,
                    size: 11.0.into(),
                    ..Default::default()
                });
            }
        });

        let mut overlay = canvas::Frame::new(renderer, bounds.size());
        if let Some(i) = self.hovered {
            let r = &self.data.rects[i];
            overlay.stroke_rectangle(
                Point::new(r.x * sx, r.y * sy),
                Size::new(r.w * sx, r.h * sy),
                canvas::Stroke::default().with_color(col(dark::ACCENT)).with_width(1.5),
            );
        }
        vec![base, overlay.into_geometry()]
    }
}

fn main() -> iced::Result {
    let started = Instant::now();
    let data = Arc::new(Dataset::from_env());
    eprintln!(
        "dataset: {} nodes, {} rows, {} rects in {:.0} ms",
        data.tree.len(),
        data.rows.len(),
        data.rects.len(),
        data.build_ms
    );
    iced::application(move || Spike::new(data.clone(), started), Spike::update, Spike::view)
        .title("Diskie · iced spike")
        .subscription(Spike::subscription)
        .theme(|_: &Spike| Theme::Dark)
        .window_size(Size::new(1000.0, 720.0))
        .default_font(Font::with_name("Segoe UI"))
        .run()
}
