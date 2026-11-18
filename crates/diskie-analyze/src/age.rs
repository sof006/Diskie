//! Bytes by how long ago files were last modified.

use diskie_model::Tree;

pub const BUCKETS: usize = 7;

/// Upper bounds in days for each bucket; the last is open-ended.
const DAYS: [u64; BUCKETS - 1] = [7, 30, 182, 365, 730, 1825];

pub const LABELS: [&str; BUCKETS] = ["This week", "This month", "Last 6 months", "Last year", "1–2 years", "2–5 years", "Over 5 years"];

/// Which bucket a modification time (Unix seconds) falls in, or `None` when unknown.
pub fn bucket(mtime: u32, now: u64) -> Option<usize> {
    if mtime == 0 {
        return None;
    }
    let days = now.saturating_sub(mtime as u64) / 86_400;
    Some(DAYS.iter().position(|&d| days < d).unwrap_or(BUCKETS - 1))
}

/// Bytes of files at or below `under` per age bucket, plus bytes whose age is unknown.
pub fn histogram(tree: &Tree, under: u32, now: u64) -> ([u64; BUCKETS], u64) {
    let mut out = [0u64; BUCKETS];
    let mut unknown = 0;
    for n in tree.subtree(under) {
        if tree.is_dir(n) {
            continue;
        }
        let size = tree.size[n as usize];
        match bucket(tree.mtime[n as usize], now) {
            Some(b) => out[b] += size,
            None => unknown += size,
        }
    }
    (out, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets() {
        let now = 1_000 * 86_400;
        assert_eq!(bucket(0, now), None);
        assert_eq!(bucket((now - 86_400) as u32, now), Some(0));
        assert_eq!(bucket((now - 40 * 86_400) as u32, now), Some(2));
        assert_eq!(bucket(1, now), Some(5), "999 days: 2–5 years");
        let t = crate::fixture::tree();
        let (h, unknown) = histogram(&t, Tree::ROOT, 2_000_000_100);
        assert_eq!(h[0], 900_000_120);
        assert_eq!(unknown, 4_000_000_000);
    }
}
