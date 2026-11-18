//! The largest files below a folder, wherever they are.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use diskie_model::Tree;
use diskie_model::tree::flag;

/// Up to `limit` files at or below `under`, largest first. Links and synthetic entries are left
/// out: they take no space of their own.
pub fn largest_files(tree: &Tree, under: u32, limit: usize) -> Vec<u32> {
    let mut heap: BinaryHeap<Reverse<(u64, u32)>> = BinaryHeap::with_capacity(limit + 1);
    for n in tree.subtree(under) {
        if tree.is_dir(n) || tree.has(n, flag::REPARSE | flag::SYNTHETIC) {
            continue;
        }
        let size = tree.size[n as usize];
        if heap.len() < limit {
            heap.push(Reverse((size, n)));
        } else if heap.peek().is_some_and(|Reverse((s, _))| size > *s) {
            heap.pop();
            heap.push(Reverse((size, n)));
        }
    }
    let mut v: Vec<(u64, u32)> = heap.into_iter().map(|Reverse(x)| x).collect();
    v.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    v.into_iter().map(|(_, n)| n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{find, tree};

    #[test]
    fn largest_first_and_limited() {
        let t = tree();
        let top = largest_files(&t, Tree::ROOT, 3);
        let names: Vec<&str> = top.iter().map(|&n| t.name(n)).collect();
        assert_eq!(names, ["pagefile.sys", "Holiday Video.mkv", "setup.exe"]);
        let users = find(&t, "Users");
        assert_eq!(largest_files(&t, users, 10).len(), 2);
    }
}
