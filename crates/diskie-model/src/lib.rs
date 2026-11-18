//! The data side of Diskie, independent of the UI and of how a volume was read.
//!
//! - [`tree`]: a struct-of-arrays file tree built from scan output, with aggregated sizes.
//! - [`kind`]: file-type categories by extension.
//! - [`treemap`]: squarified treemap layout with folder header strips, and a CPU rasterizer.
//! - [`fmt`]: number, size and age formatting.

pub mod fmt;
pub mod kind;
pub mod tree;
pub mod treemap;

pub use kind::{KIND_COUNT, Kind};
pub use tree::{Chunk, NO_KEY, NONE, Tree};
