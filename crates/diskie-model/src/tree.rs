//! A struct-of-arrays file tree.
//!
//! Scanners don't build the tree directly. They push flat entries into [`Chunk`]s (one per
//! worker thread, so there is no locking), each naming its parent by a *key*: a directory id
//! for the directory walker, an MFT record number for the NTFS reader. [`Tree::build`] then
//! resolves keys to indices, groups children, aggregates sizes bottom-up and sorts every
//! folder's children by size, largest first.
//!
//! About 50 bytes per node plus the name, so ten million files fit in roughly 650 MB.

use crate::kind::{KIND_COUNT, Kind};

/// "No node": the root's parent, and the parent of entries whose parent was never seen.
pub const NONE: u32 = u32::MAX;
/// The key of an entry that nothing can be a child of (a file).
pub const NO_KEY: u64 = u64::MAX;

pub mod flag {
    pub const DIR: u8 = 1;
    /// A folder Diskie could not list (access denied, gone, I/O error).
    pub const UNREADABLE: u8 = 2;
    /// Removed after the scan (sent to the Recycle Bin or deleted).
    pub const DELETED: u8 = 4;
    /// A junction or symbolic link. Never followed, so it counts as empty.
    pub const REPARSE: u8 = 8;
    /// Not a real file system entry, such as the NTFS metadata total.
    pub const SYNTHETIC: u8 = 16;
    /// A folder where another file system is mounted (Linux). Not entered, so it counts as empty.
    pub const MOUNT: u8 = 32;
}

#[derive(Clone, Copy)]
struct Raw {
    key: u64,
    parent: u64,
    name_off: u32,
    name_len: u32,
    size: u64,
    mtime: u32,
    flags: u8,
}

/// Scan output from one worker: entries plus their names in one string pool.
#[derive(Default)]
pub struct Chunk {
    entries: Vec<Raw>,
    names: Vec<u8>,
    unreadable: Vec<u64>,
}

impl Chunk {
    pub fn with_capacity(n: usize) -> Self {
        Self { entries: Vec::with_capacity(n), names: Vec::with_capacity(n * 16), unreadable: Vec::new() }
    }

    /// Records that the folder with this key could not be listed.
    pub fn mark_unreadable(&mut self, key: u64) {
        self.unreadable.push(key);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds one entry. `key` is [`NO_KEY`] for files; directories get a unique key that their
    /// children use as `parent`. `mtime` is Unix seconds (0 when unknown).
    pub fn push(&mut self, key: u64, parent: u64, name: &str, size: u64, mtime: u32, flags: u8) {
        let name_off = self.names.len() as u32;
        self.names.extend_from_slice(name.as_bytes());
        self.entries.push(Raw { key, parent, name_off, name_len: name.len() as u32, size, mtime, flags });
    }
}

#[derive(Clone, Default)]
pub struct Tree {
    names: Vec<u8>,
    name_off: Vec<u32>,
    name_len: Vec<u32>,
    pub parent: Vec<u32>,
    child_start: Vec<u32>,
    child_count: Vec<u32>,
    children: Vec<u32>,
    /// Logical size in bytes; for folders, the total of everything below.
    pub size: Vec<u64>,
    /// Files at or below the node (1 for a file).
    pub files: Vec<u32>,
    /// Folders below the node, not counting itself.
    pub dirs: Vec<u32>,
    /// Last modified, Unix seconds; for folders, the newest file below.
    pub mtime: Vec<u32>,
    pub flags: Vec<u8>,
    /// File type; for folders, the type holding the most bytes below.
    pub kind: Vec<u8>,
    pub depth: Vec<u16>,
    /// Number of folders flagged [`flag::UNREADABLE`].
    pub unreadable: u32,
}

impl Tree {
    pub const ROOT: u32 = 0;

    /// Builds the tree. The root is a folder called `root_name` (a full path such as `C:\` or
    /// `\\server\share`) with key `root_key`. Directory keys must be below `key_limit`.
    /// Entries whose parent never appears are kept but not attached, so they are not counted.
    pub fn build(root_name: &str, root_key: u64, chunks: Vec<Chunk>, key_limit: u64) -> Tree {
        let total: usize = 1 + chunks.iter().map(|c| c.entries.len()).sum::<usize>();
        let mut t = Tree {
            names: Vec::with_capacity(root_name.len() + chunks.iter().map(|c| c.names.len()).sum::<usize>()),
            name_off: Vec::with_capacity(total),
            name_len: Vec::with_capacity(total),
            parent: vec![NONE; total],
            child_start: vec![0; total],
            child_count: vec![0; total],
            children: Vec::new(),
            size: Vec::with_capacity(total),
            files: vec![0; total],
            dirs: vec![0; total],
            mtime: Vec::with_capacity(total),
            flags: Vec::with_capacity(total),
            kind: vec![Kind::Other as u8; total],
            depth: vec![0; total],
            unreadable: 0,
        };

        // Indices and names. Root first.
        let mut key_to_idx = vec![NONE; key_limit as usize];
        let mut parent_keys: Vec<u64> = Vec::with_capacity(total);
        t.names.extend_from_slice(root_name.as_bytes());
        t.name_off.push(0);
        t.name_len.push(root_name.len() as u32);
        t.size.push(0);
        t.mtime.push(0);
        t.flags.push(flag::DIR);
        parent_keys.push(NO_KEY);
        if let Some(slot) = key_to_idx.get_mut(root_key as usize) {
            *slot = 0;
        }
        for chunk in &chunks {
            let base = t.names.len() as u32;
            t.names.extend_from_slice(&chunk.names);
            for e in &chunk.entries {
                let idx = t.name_off.len() as u32;
                t.name_off.push(base + e.name_off);
                t.name_len.push(e.name_len);
                t.size.push(e.size);
                t.mtime.push(e.mtime);
                t.flags.push(e.flags);
                parent_keys.push(if e.parent == e.key { NO_KEY } else { e.parent });
                if e.key != NO_KEY
                    && let Some(slot) = key_to_idx.get_mut(e.key as usize)
                {
                    *slot = idx;
                }
            }
        }
        for key in chunks.iter().flat_map(|c| c.unreadable.iter()) {
            if let Some(&i) = key_to_idx.get(*key as usize)
                && i != NONE
            {
                t.flags[i as usize] |= flag::UNREADABLE;
            }
        }
        drop(chunks);
        for (i, &pk) in parent_keys.iter().enumerate() {
            if pk != NO_KEY
                && let Some(&p) = key_to_idx.get(pk as usize)
                && p != i as u32
            {
                t.parent[i] = p;
            }
        }
        drop(parent_keys);
        drop(key_to_idx);

        // Group children by parent (counting sort).
        for &p in &t.parent {
            if p != NONE {
                t.child_count[p as usize] += 1;
            }
        }
        let mut acc = 0u32;
        for i in 0..total {
            t.child_start[i] = acc;
            acc += t.child_count[i];
        }
        t.children = vec![0; acc as usize];
        let mut fill = t.child_start.clone();
        for i in 0..total {
            let p = t.parent[i];
            if p != NONE {
                t.children[fill[p as usize] as usize] = i as u32;
                fill[p as usize] += 1;
            }
        }
        drop(fill);

        // Pre-order from the root (ignores anything unreachable, including parent cycles).
        let mut order: Vec<u32> = Vec::with_capacity(total);
        let mut stack = vec![Tree::ROOT];
        while let Some(n) = stack.pop() {
            order.push(n);
            let d = t.depth[n as usize];
            let (s, c) = (t.child_start[n as usize] as usize, t.child_count[n as usize] as usize);
            for &k in &t.children[s..s + c] {
                t.depth[k as usize] = d.saturating_add(1);
                stack.push(k);
            }
        }

        // Aggregate bottom-up, tracking bytes per file type for each folder.
        let mut dir_slot = vec![NONE; total];
        let mut kind_bytes: Vec<[u64; KIND_COUNT]> = Vec::new();
        for &n in &order {
            if t.flags[n as usize] & flag::DIR != 0 {
                dir_slot[n as usize] = kind_bytes.len() as u32;
                kind_bytes.push([0; KIND_COUNT]);
            }
        }
        for &n in order.iter().rev() {
            let i = n as usize;
            let is_dir = t.flags[i] & flag::DIR != 0;
            if is_dir {
                if t.flags[i] & flag::UNREADABLE != 0 {
                    t.unreadable += 1;
                }
                let kb = &kind_bytes[dir_slot[i] as usize];
                let (best, bytes) = kb.iter().enumerate().max_by_key(|&(_, b)| *b).unwrap();
                t.kind[i] = if *bytes == 0 { Kind::Other as u8 } else { best as u8 };
            } else {
                t.files[i] = 1;
                t.kind[i] = Kind::of_name(t.name(n)) as u8;
            }
            let p = t.parent[i];
            if p == NONE {
                continue;
            }
            let p = p as usize;
            t.size[p] += t.size[i];
            t.files[p] += t.files[i];
            t.dirs[p] += t.dirs[i] + is_dir as u32;
            t.mtime[p] = t.mtime[p].max(t.mtime[i]);
            let ps = dir_slot[p] as usize;
            if is_dir {
                let child = kind_bytes[dir_slot[i] as usize];
                for (a, b) in kind_bytes[ps].iter_mut().zip(child) {
                    *a += b;
                }
            } else {
                kind_bytes[ps][t.kind[i] as usize] += t.size[i];
            }
        }

        // Children largest first, ties by name for a stable order.
        for &n in &order {
            let (s, c) = (t.child_start[n as usize] as usize, t.child_count[n as usize] as usize);
            if c > 1 {
                let mut kids = std::mem::take(&mut t.children);
                kids[s..s + c].sort_unstable_by(|&a, &b| t.size[b as usize].cmp(&t.size[a as usize]).then_with(|| t.name(a).cmp(t.name(b))));
                t.children = kids;
            }
        }
        t
    }

    pub fn len(&self) -> usize {
        self.parent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    pub fn name(&self, n: u32) -> &str {
        let (o, l) = (self.name_off[n as usize] as usize, self.name_len[n as usize] as usize);
        // Names are only ever pushed as `&str`, so this slice is valid UTF-8.
        std::str::from_utf8(&self.names[o..o + l]).unwrap_or("?")
    }

    /// The node's children, largest first.
    pub fn kids(&self, n: u32) -> &[u32] {
        let (s, c) = (self.child_start[n as usize] as usize, self.child_count[n as usize] as usize);
        &self.children[s..s + c]
    }

    pub fn is_dir(&self, n: u32) -> bool {
        self.flags[n as usize] & flag::DIR != 0
    }

    pub fn has(&self, n: u32, f: u8) -> bool {
        self.flags[n as usize] & f != 0
    }

    /// The full path. The root's name is already a path (`C:\`, `\\server\share`).
    pub fn path(&self, n: u32) -> String {
        let mut parts = vec![n];
        let mut a = self.parent[n as usize];
        while a != NONE {
            parts.push(a);
            a = self.parent[a as usize];
        }
        let sep = if self.name(Tree::ROOT).contains('/') && !self.name(Tree::ROOT).contains('\\') { '/' } else { '\\' };
        let mut out = String::new();
        for (i, &p) in parts.iter().rev().enumerate() {
            if i > 0 && !out.ends_with(sep) {
                out.push(sep);
            }
            out.push_str(self.name(p));
        }
        out
    }

    /// `n` and its ancestors, from the root down to `n`.
    pub fn lineage(&self, n: u32) -> Vec<u32> {
        let mut v = vec![n];
        let mut a = self.parent[n as usize];
        while a != NONE {
            v.push(a);
            a = self.parent[a as usize];
        }
        v.reverse();
        v
    }

    pub fn is_ancestor(&self, a: u32, mut n: u32) -> bool {
        while n != NONE {
            if n == a {
                return true;
            }
            n = self.parent[n as usize];
        }
        false
    }

    /// Share of the parent's size, 0..=100.
    pub fn percent_of_parent(&self, n: u32) -> f64 {
        let p = self.parent[n as usize];
        if p == NONE {
            return 100.0;
        }
        let ps = self.size[p as usize];
        if ps == 0 { 0.0 } else { self.size[n as usize] as f64 * 100.0 / ps as f64 }
    }

    /// Bytes per file type below `n`.
    pub fn kind_totals(&self, n: u32) -> [u64; KIND_COUNT] {
        let mut out = [0u64; KIND_COUNT];
        let mut stack = vec![n];
        while let Some(x) = stack.pop() {
            if self.is_dir(x) {
                stack.extend_from_slice(self.kids(x));
            } else {
                out[self.kind[x as usize] as usize] += self.size[x as usize];
            }
        }
        out
    }

    /// Takes `n` (and everything below it) out of the tree after it was deleted from disk:
    /// subtracts it from every ancestor and keeps each folder's children sorted.
    pub fn remove(&mut self, n: u32) {
        let p = self.parent[n as usize];
        if p == NONE || self.has(n, flag::DELETED) {
            return;
        }
        let (size, files, dirs) = (self.size[n as usize], self.files[n as usize], self.dirs[n as usize] + self.is_dir(n) as u32);
        let (s, c) = (self.child_start[p as usize] as usize, self.child_count[p as usize] as usize);
        if let Some(pos) = self.children[s..s + c].iter().position(|&k| k == n) {
            self.children[s + pos..s + c].rotate_left(1);
            self.child_count[p as usize] -= 1;
        }
        self.flags[n as usize] |= flag::DELETED;
        let mut a = p;
        while a != NONE {
            let i = a as usize;
            self.size[i] -= size;
            self.files[i] -= files;
            self.dirs[i] -= dirs;
            let up = self.parent[i];
            if up != NONE {
                self.resort_child(up, a);
            }
            a = up;
        }
    }

    /// Moves `child` to its place in `parent`'s size-sorted children after its size dropped.
    fn resort_child(&mut self, parent: u32, child: u32) {
        let (s, c) = (self.child_start[parent as usize] as usize, self.child_count[parent as usize] as usize);
        let Some(mut pos) = self.children[s..s + c].iter().position(|&k| k == child) else { return };
        while pos + 1 < c && self.size[self.children[s + pos + 1] as usize] > self.size[child as usize] {
            self.children.swap(s + pos, s + pos + 1);
            pos += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// root ─ a ─ big.mkv (100), small.txt (5)
    ///      └ b ─ c ─ x.zip (40)
    ///      └ top.dll (60)
    fn sample() -> Tree {
        let mut c1 = Chunk::default();
        let mut c2 = Chunk::default();
        c1.push(1, 0, "a", 0, 10, flag::DIR);
        c2.push(3, 2, "c", 0, 0, flag::DIR); // child before its parent, in another chunk
        c1.push(NO_KEY, 1, "big.mkv", 100, 50, 0);
        c1.push(NO_KEY, 1, "small.txt", 5, 70, 0);
        c1.push(2, 0, "b", 0, 0, flag::DIR);
        c2.push(NO_KEY, 3, "x.zip", 40, 20, 0);
        c2.push(NO_KEY, 0, "top.dll", 60, 5, 0);
        c2.push(NO_KEY, 99, "orphan", 1000, 0, 0); // parent never seen
        Tree::build("C:\\", 0, vec![c1, c2], 128)
    }

    fn by_name(t: &Tree, name: &str) -> u32 {
        (0..t.len() as u32).find(|&n| t.name(n) == name).unwrap()
    }

    #[test]
    fn aggregates_and_sorts() {
        let t = sample();
        assert_eq!(t.size[0], 205, "orphan must not be counted");
        assert_eq!(t.files[0], 4);
        assert_eq!(t.dirs[0], 3);
        let names: Vec<&str> = t.kids(0).iter().map(|&k| t.name(k)).collect();
        assert_eq!(names, ["a", "top.dll", "b"]);
        let a = by_name(&t, "a");
        assert_eq!(t.size[a as usize], 105);
        assert_eq!(t.mtime[a as usize], 70);
        assert_eq!(Kind::from_u8(t.kind[a as usize]), Kind::Video);
        assert_eq!(t.path(by_name(&t, "x.zip")), "C:\\b\\c\\x.zip");
        assert_eq!(t.depth[by_name(&t, "x.zip") as usize], 3);
        assert_eq!(t.kind_totals(0)[Kind::Apps as usize], 60);
    }

    #[test]
    fn remove_updates_ancestors_and_order() {
        let mut t = sample();
        t.remove(by_name(&t, "big.mkv"));
        assert_eq!(t.size[0], 105);
        assert_eq!(t.files[0], 3);
        let names: Vec<&str> = t.kids(0).iter().map(|&k| t.name(k)).collect();
        assert_eq!(names, ["top.dll", "b", "a"]);
        let b = by_name(&t, "b");
        t.remove(b);
        assert_eq!(t.size[0], 65);
        assert_eq!(t.dirs[0], 1);
        assert_eq!(t.kids(0).len(), 2);
    }

    #[test]
    fn unix_style_root() {
        let mut c = Chunk::default();
        c.push(1, 0, "home", 0, 0, flag::DIR);
        c.push(NO_KEY, 1, "f", 1, 0, 0);
        let t = Tree::build("/", 0, vec![c], 4);
        assert_eq!(t.path(2), "/home/f");
    }
}
