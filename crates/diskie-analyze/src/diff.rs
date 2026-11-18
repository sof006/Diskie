//! What changed between an older scan and the current one of the same place.
//!
//! Entries are matched by path. The result is kept per node of the *current* tree, as the size
//! each had back then, so changes made after the scan (deletes, live updates) still show
//! correctly: the change is always `now - then`.

use std::collections::HashMap;

use diskie_model::{NONE, Tree};

/// `old_size` value for an entry that didn't exist in the older scan.
pub const NEW: u64 = u64::MAX;

pub struct Diff {
    /// Per current node: its size in the older scan, or [`NEW`]. Nodes added after the
    /// comparison was made are past the end, and new as well.
    pub old_size: Vec<u64>,
    /// Entries in the older scan that are gone now, largest first. Only the topmost of a
    /// removed folder is listed.
    pub removed: Vec<Removed>,
}

pub struct Removed {
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    pub mtime: u32,
}

impl Diff {
    /// Size now minus size then; a new entry's whole size.
    pub fn change(&self, tree: &Tree, n: u32) -> i64 {
        let now = tree.size[n as usize] as i64;
        match self.old_size.get(n as usize) {
            Some(&old) if old != NEW => now - old as i64,
            _ => now,
        }
    }

    pub fn is_new(&self, n: u32) -> bool {
        self.old_size.get(n as usize).is_none_or(|&o| o == NEW)
    }
}

/// Compares `old` with `new`. Their roots are taken to be the same place.
pub fn compare(new: &Tree, old: &Tree) -> Diff {
    let mut old_size = vec![NEW; new.len()];
    let mut removed = Vec::new();
    let fold = new.name(Tree::ROOT).contains('\\');
    let key = |s: &str| if fold { s.to_lowercase() } else { s.to_string() };
    let mut stack = vec![(Tree::ROOT, Tree::ROOT)];
    while let Some((n, o)) = stack.pop() {
        old_size[n as usize] = old.size[o as usize];
        if !new.is_dir(n) {
            continue;
        }
        let mut was: HashMap<String, u32> = old.kids(o).iter().map(|&k| (key(old.name(k)), k)).collect();
        for &k in new.kids(n) {
            match was.remove(&key(new.name(k))) {
                Some(ok) if old.is_dir(ok) == new.is_dir(k) => stack.push((k, ok)),
                Some(ok) => removed.push(gone(old, ok)),
                None => {}
            }
        }
        removed.extend(was.into_values().map(|ok| gone(old, ok)));
    }
    removed.sort_unstable_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    Diff { old_size, removed }
}

fn gone(old: &Tree, n: u32) -> Removed {
    Removed { path: old.path(n), size: old.size[n as usize], is_dir: old.is_dir(n), mtime: old.mtime[n as usize] }
}

/// What to list as changed at or below `under`: files that are new or changed size, folders
/// that are new as a whole (listed once, not their contents), largest change first.
pub fn changed(tree: &Tree, diff: &Diff, under: u32, limit: usize) -> Vec<u32> {
    let mut out: Vec<(u64, u32)> = Vec::new();
    let mut stack = vec![under];
    while let Some(n) = stack.pop() {
        let parent_new = tree.parent[n as usize] != NONE && diff.is_new(tree.parent[n as usize]);
        if n != under && diff.is_new(n) && !parent_new {
            out.push((tree.size[n as usize], n));
            continue;
        }
        if tree.is_dir(n) {
            stack.extend_from_slice(tree.kids(n));
        } else {
            let c = diff.change(tree, n);
            if c != 0 {
                out.push((c.unsigned_abs(), n));
            }
        }
        if out.len() > limit * 4 {
            out.sort_unstable_by_key(|b| std::cmp::Reverse(b.0));
            out.truncate(limit);
        }
    }
    out.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    out.truncate(limit);
    out.into_iter().map(|(_, n)| n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use diskie_model::tree::flag;
    use diskie_model::{Chunk, NO_KEY};

    fn old() -> Tree {
        let mut c = Chunk::default();
        c.push(1, 0, "a", 0, 0, flag::DIR);
        c.push(NO_KEY, 1, "keep.txt", 10, 0, 0);
        c.push(NO_KEY, 1, "grow.log", 100, 0, 0);
        c.push(2, 0, "gone", 0, 0, flag::DIR);
        c.push(NO_KEY, 2, "big.bin", 5000, 0, 0);
        c.push(NO_KEY, 0, "was-file", 1, 0, 0);
        Tree::build("C:\\", 0, vec![c], 4)
    }

    fn new() -> Tree {
        let mut c = Chunk::default();
        c.push(1, 0, "A", 0, 0, flag::DIR); // case changed on Windows: same folder
        c.push(NO_KEY, 1, "keep.txt", 10, 0, 0);
        c.push(NO_KEY, 1, "grow.log", 700, 0, 0);
        c.push(2, 0, "fresh", 0, 0, flag::DIR);
        c.push(NO_KEY, 2, "inner.bin", 300, 0, 0);
        c.push(3, 0, "was-file", 0, 0, flag::DIR);
        Tree::build("C:\\", 0, vec![c], 4)
    }

    fn by(t: &Tree, name: &str) -> u32 {
        (0..t.len() as u32).find(|&n| t.name(n) == name).unwrap()
    }

    #[test]
    fn growth_new_and_removed() {
        let (n, o) = (new(), old());
        let d = compare(&n, &o);
        assert_eq!(d.change(&n, Tree::ROOT), 1010 - 5111);
        assert_eq!(d.change(&n, by(&n, "A")), 600);
        assert_eq!(d.change(&n, by(&n, "keep.txt")), 0);
        assert!(d.is_new(by(&n, "fresh")) && d.is_new(by(&n, "inner.bin")));
        assert!(d.is_new(by(&n, "was-file")), "a file that became a folder is new");
        let gone: Vec<&str> = d.removed.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(gone, ["C:\\gone", "C:\\was-file"]);
        let listed: Vec<&str> = changed(&n, &d, Tree::ROOT, 10).iter().map(|&k| n.name(k)).collect();
        assert_eq!(listed, ["grow.log", "fresh", "was-file"], "new folders are listed once, unchanged files not at all");
    }

    #[test]
    fn nodes_added_later_count_as_new() {
        let (mut n, o) = (new(), old());
        let d = compare(&n, &o);
        let a = by(&n, "A");
        let late = n.add(a, "late.txt", 5, 0, 0);
        assert!(d.is_new(late));
        assert_eq!(d.change(&n, a), 605);
    }
}
