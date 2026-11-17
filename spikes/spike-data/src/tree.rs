//! A struct-of-arrays file tree, generated deterministically.
//!
//! Nodes are created breadth-first, so every directory's children occupy
//! one contiguous index range and every child index is greater than its
//! parent's. That makes aggregation a single reverse pass.

use crate::color::KIND_COUNT;

pub struct Tree {
    pub parent: Vec<u32>,
    pub child_start: Vec<u32>,
    pub child_count: Vec<u32>,
    /// `sorted[child_start[d]..][..child_count[d]]` lists d's children by size, largest first.
    pub sorted: Vec<u32>,
    /// Logical size in bytes (aggregated for directories).
    pub size: Vec<u64>,
    /// Number of files at or below this node.
    pub files: Vec<u32>,
    pub depth: Vec<u16>,
    /// File-type category; for directories, the category most of their files were drawn from.
    pub kind: Vec<u8>,
    pub is_dir: Vec<bool>,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn f32(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }
}

impl Tree {
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    pub fn generate(target: usize, seed: u64) -> Self {
        let mut rng = Rng(seed | 1);
        let mut t = Tree {
            parent: Vec::with_capacity(target + 64),
            child_start: Vec::with_capacity(target + 64),
            child_count: Vec::with_capacity(target + 64),
            sorted: Vec::new(),
            size: Vec::with_capacity(target + 64),
            files: Vec::with_capacity(target + 64),
            depth: Vec::with_capacity(target + 64),
            kind: Vec::with_capacity(target + 64),
            is_dir: Vec::with_capacity(target + 64),
        };
        t.push(u32::MAX, 0, true, 0, 0);

        let mut queue = std::collections::VecDeque::from([0u32]);
        while let Some(d) = queue.pop_front() {
            if t.len() >= target {
                break;
            }
            let depth = t.depth[d as usize];
            let theme = rng.below(KIND_COUNT as u32) as u8;
            t.kind[d as usize] = theme;
            let ndirs = match depth {
                0 => 14,
                1 => 3 + rng.below(8),
                d if d >= 14 => 0,
                _ => rng.below(5),
            };
            let nfiles = if depth == 0 { 6 } else { rng.below(32) };
            t.child_start[d as usize] = t.len() as u32;
            t.child_count[d as usize] = ndirs + nfiles;
            for _ in 0..ndirs {
                let id = t.push(d, depth + 1, true, theme, 0);
                queue.push_back(id);
            }
            for _ in 0..nfiles {
                let kind = if rng.f32() < 0.7 { theme } else { rng.below(KIND_COUNT as u32) as u8 };
                let size = file_size(&mut rng);
                t.push(d, depth + 1, false, kind, size);
            }
        }

        // Aggregate sizes and file counts bottom-up.
        for i in (1..t.len()).rev() {
            let p = t.parent[i] as usize;
            t.size[p] += t.size[i];
            t.files[p] += if t.is_dir[i] { t.files[i] } else { 1 };
        }

        // Children sorted by size, largest first.
        t.sorted = (0..t.len() as u32).collect();
        for d in 0..t.len() {
            let (s, n) = (t.child_start[d] as usize, t.child_count[d] as usize);
            if n > 1 {
                let size = &t.size;
                t.sorted[s..s + n].sort_unstable_by(|a, b| size[*b as usize].cmp(&size[*a as usize]));
            }
        }
        t
    }

    fn push(&mut self, parent: u32, depth: u16, is_dir: bool, kind: u8, size: u64) -> u32 {
        let id = self.parent.len() as u32;
        self.parent.push(parent);
        self.child_start.push(0);
        self.child_count.push(0);
        self.size.push(size);
        self.files.push(0);
        self.depth.push(depth);
        self.kind.push(kind);
        self.is_dir.push(is_dir);
        id
    }

    pub fn children(&self, d: u32) -> &[u32] {
        let s = self.child_start[d as usize] as usize;
        &self.sorted[s..s + self.child_count[d as usize] as usize]
    }

    /// Pre-order list of every node, children by size: the tree fully expanded.
    pub fn flatten_expanded(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.len());
        let mut stack = vec![0u32];
        while let Some(n) = stack.pop() {
            out.push(n);
            stack.extend(self.children(n).iter().rev());
        }
        out
    }

    pub fn name(&self, i: u32) -> String {
        const WORDS: [&str; 24] = [
            "assets", "build", "cache", "data", "docs", "export", "fixtures", "images", "logs", "media",
            "models", "node_modules", "output", "packages", "projects", "raw", "release", "renders",
            "scans", "shared", "source", "target", "temp", "vendor",
        ];
        let w = WORDS[i as usize % WORDS.len()];
        if self.is_dir[i as usize] {
            format!("{w}-{}", i / WORDS.len() as u32)
        } else {
            format!("{w}_{:06}.{}", i, crate::color::EXT[self.kind[i as usize] as usize])
        }
    }

    pub fn percent_of_parent(&self, i: u32) -> f32 {
        let p = self.parent[i as usize];
        if p == u32::MAX {
            return 100.0;
        }
        let ps = self.size[p as usize].max(1);
        self.size[i as usize] as f32 / ps as f32 * 100.0
    }
}

fn file_size(rng: &mut Rng) -> u64 {
    if rng.below(200_000) == 0 {
        // The occasional huge file: disk image, video, VM.
        return (1u64 << 32) + (rng.next() % (1u64 << 34));
    }
    let e = rng.f32() * 14.0 + 10.0; // log-uniform, 1 KB .. 16 MB
    (2f32.powf(e) * (1.0 + rng.f32())) as u64
}
