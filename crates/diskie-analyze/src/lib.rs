//! Analyses over a finished [`Tree`]. All of them run off the UI thread and only read the tree,
//! except [`dupes`], which reads file contents.
//!
//! - [`top`]: the largest files below a folder.
//! - [`search`]: a small query language over names, sizes, ages and types.
//! - [`dupes`]: files with identical contents.
//! - [`cleanup`]: well-known space users (build output, caches, old Windows) with advice.
//! - [`diff`]: what changed between two scans of the same place.
//! - [`export`]: CSV and JSON.
//! - [`age`]: bytes by last-modified age.

pub mod age;
pub mod cleanup;
pub mod diff;
pub mod dupes;
pub mod export;
pub mod search;
pub mod top;

pub use diskie_model::Tree;

#[cfg(test)]
pub(crate) mod fixture {
    use diskie_model::tree::flag;
    use diskie_model::{Chunk, NO_KEY, Tree};

    /// `C:\` with `proj` (a Rust project), `Users\me\Downloads` and a few loose files.
    pub fn tree() -> Tree {
        let mut c = Chunk::default();
        c.push(1, 0, "proj", 0, 1_000, flag::DIR);
        c.push(NO_KEY, 1, "Cargo.toml", 1_000, 1_000, 0);
        c.push(2, 1, "target", 0, 1_000, flag::DIR);
        c.push(NO_KEY, 2, "app.exe", 5_000_000, 1_000, 0);
        c.push(3, 0, "Users", 0, 0, flag::DIR);
        c.push(4, 3, "me", 0, 0, flag::DIR);
        c.push(5, 4, "Downloads", 0, 0, flag::DIR);
        c.push(NO_KEY, 5, "setup.exe", 50_000_000, 10, 0);
        c.push(NO_KEY, 5, "Holiday Video.mkv", 900_000_000, 2_000_000_000, 0);
        c.push(NO_KEY, 0, "notes.txt", 120, 2_000_000_000, 0);
        c.push(NO_KEY, 0, "pagefile.sys", 4_000_000_000, 0, 0);
        Tree::build("C:\\", 0, vec![c], 8)
    }

    pub fn find(t: &Tree, name: &str) -> u32 {
        (0..t.len() as u32).find(|&n| t.name(n) == name).unwrap()
    }
}
