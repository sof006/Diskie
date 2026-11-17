//! Shared, framework-independent pieces of the week-1 UI spike.
//!
//! Both prototypes (GPUI and iced) render exactly the same data:
//! a deterministic synthetic file tree of ~2M nodes, the same flattened
//! row list, and the same treemap rectangles. They also share the bench
//! driver and the statistics code, so the numbers are comparable.

pub mod bench;
pub mod color;
pub mod fmt;
pub mod tree;
pub mod treemap;

pub use bench::{BenchDriver, BenchReport, FrameStats, ProcSample};
pub use tree::Tree;
pub use treemap::{MapRect, TreemapOptions};

/// Everything a prototype needs, built once at start-up.
pub struct Dataset {
    pub tree: Tree,
    /// Node ids in display order: the whole tree expanded, children by size.
    pub rows: Vec<u32>,
    /// Treemap rectangles in a virtual `MAP_W` x `MAP_H` pixel space, back to front.
    pub rects: Vec<MapRect>,
    pub build_ms: f64,
}

/// Virtual treemap space. Renderers scale it to their actual bounds.
pub const MAP_W: f32 = 1600.0;
pub const MAP_H: f32 = 700.0;

pub const DEFAULT_NODES: usize = 2_000_000;

impl Dataset {
    pub fn build(nodes: usize) -> Self {
        let start = std::time::Instant::now();
        let tree = Tree::generate(nodes, 0x5EED_D15C);
        let rows = tree.flatten_expanded();
        let rects = treemap::layout(&tree, 0, MAP_W, MAP_H, &TreemapOptions::default());
        Self {
            tree,
            rows,
            rects,
            build_ms: start.elapsed().as_secs_f64() * 1000.0,
        }
    }

    /// Node count from `SPIKE_NODES`, else the 2M default.
    pub fn from_env() -> Self {
        let nodes = std::env::var("SPIKE_NODES")
            .ok()
            .and_then(|v| v.replace('_', "").parse().ok())
            .unwrap_or(DEFAULT_NODES);
        Self::build(nodes)
    }

    /// Index of the topmost rectangle containing a point in virtual space.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<usize> {
        self.rects
            .iter()
            .rposition(|r| x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h)
    }
}
