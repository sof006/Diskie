//! Files with identical contents.
//!
//! 1. Files of the same size are candidates (anything else can't be identical).
//! 2. Candidates are compared by a hash of their first and last 64 KB (xxh3), which rules out
//!    almost everything without reading whole files.
//! 3. What's left is compared by a BLAKE3 hash of the whole file.
//! 4. Two names for the same file on disk (hard links) are one file, not duplicates.
//!
//! Online-only cloud files are never read (that would download them), and neither are links.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering::Relaxed};

use diskie_model::Tree;
use diskie_model::tree::flag;

const EDGE: u64 = 64 * 1024;

#[derive(Default)]
pub struct Progress {
    /// Files that share their size with another file.
    pub candidates: AtomicU64,
    /// Files hashed so far in the current stage.
    pub hashed: AtomicU64,
    /// Files to hash in the current stage.
    pub to_hash: AtomicU64,
    pub bytes_read: AtomicU64,
    /// 0 grouping by size, 1 quick hash, 2 full hash.
    pub stage: AtomicU8,
    /// Files that couldn't be read (in use, no access).
    pub unreadable: AtomicU64,
    cancel: AtomicBool,
}

impl Progress {
    pub fn cancel(&self) {
        self.cancel.store(true, Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Relaxed)
    }
}

/// Copies of one file.
pub struct Group {
    pub size: u64,
    /// Largest-path-first is meaningless here, so by path, which keeps copies in one folder together.
    pub nodes: Vec<u32>,
}

impl Group {
    /// What deleting all but one copy would free.
    pub fn wasted(&self) -> u64 {
        self.size * (self.nodes.len() as u64).saturating_sub(1)
    }
}

/// Finds duplicate files of at least `min_size` bytes at or below `under`, most space wasted
/// first. `threads` readers run at once (use 1–2 on a hard disk). `None` when cancelled.
pub fn find(tree: &Tree, under: u32, min_size: u64, threads: usize, progress: &Progress) -> Option<Vec<Group>> {
    // 1. Same size.
    let mut by_size: HashMap<u64, Vec<u32>> = HashMap::new();
    for n in tree.subtree(under) {
        if tree.is_dir(n) || tree.has(n, flag::CLOUD | flag::REPARSE | flag::SYNTHETIC) {
            continue;
        }
        let size = tree.size[n as usize];
        if size >= min_size.max(1) {
            by_size.entry(size).or_default().push(n);
        }
    }
    by_size.retain(|_, v| v.len() > 1);
    let candidates: Vec<(u64, u32)> = by_size.iter().flat_map(|(&s, v)| v.iter().map(move |&n| (s, n))).collect();
    progress.candidates.store(candidates.len() as u64, Relaxed);
    if progress.cancelled() {
        return None;
    }

    // 2. First and last 64 KB.
    progress.stage.store(1, Relaxed);
    let quick = hash_all(tree, &candidates, threads, progress, |path, size, p| quick_hash(path, size, p).map(|h| h as u128 | (size as u128) << 64))?;
    let mut groups: Vec<Vec<(u64, u32)>> = group(quick);

    // 3. Whole files, for anything bigger than the two ends already covered.
    let needs_full: Vec<(u64, u32)> = groups.iter().flatten().copied().filter(|&(s, _)| s > 2 * EDGE).collect();
    if !needs_full.is_empty() {
        progress.stage.store(2, Relaxed);
        let full = hash_all(tree, &needs_full, threads, progress, |path, _, p| full_hash(path, p))?;
        let small: Vec<Vec<(u64, u32)>> = groups.into_iter().filter(|g| g[0].0 <= 2 * EDGE).collect();
        groups = small;
        groups.extend(group(full));
    }

    // 4. Hard links to one file are the same file.
    let mut out: Vec<Group> = Vec::new();
    for g in groups {
        let mut seen: Vec<same_file::Handle> = Vec::new();
        let mut nodes = Vec::new();
        for &(_, n) in &g {
            match same_file::Handle::from_path(tree.path(n)) {
                Ok(h) if seen.contains(&h) => {}
                Ok(h) => {
                    seen.push(h);
                    nodes.push(n);
                }
                Err(_) => nodes.push(n),
            }
        }
        if nodes.len() > 1 {
            nodes.sort_by_cached_key(|&n| tree.path(n));
            out.push(Group { size: g[0].0, nodes });
        }
    }
    out.sort_unstable_by(|a, b| b.wasted().cmp(&a.wasted()).then(a.nodes[0].cmp(&b.nodes[0])));
    Some(out)
}

/// Groups `(size, node)` by hash, keeping groups of two or more.
fn group(hashed: Vec<(u128, u64, u32)>) -> Vec<Vec<(u64, u32)>> {
    let mut m: HashMap<(u128, u64), Vec<(u64, u32)>> = HashMap::new();
    for (h, s, n) in hashed {
        m.entry((h, s)).or_default().push((s, n));
    }
    m.into_values().filter(|v| v.len() > 1).collect()
}

/// Hashes every file on `threads` threads. Files that can't be read are left out.
fn hash_all(
    tree: &Tree,
    files: &[(u64, u32)],
    threads: usize,
    progress: &Progress,
    hash: impl Fn(&Path, u64, &Progress) -> io::Result<u128> + Sync,
) -> Option<Vec<(u128, u64, u32)>> {
    progress.hashed.store(0, Relaxed);
    progress.to_hash.store(files.len() as u64, Relaxed);
    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(files.len()));
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Relaxed);
                    if i >= files.len() || progress.cancelled() {
                        break;
                    }
                    let (size, n) = files[i];
                    match hash(Path::new(&tree.path(n)), size, progress) {
                        Ok(h) => out.lock().unwrap().push((h, size, n)),
                        Err(_) => {
                            progress.unreadable.fetch_add(1, Relaxed);
                        }
                    }
                    progress.hashed.fetch_add(1, Relaxed);
                }
            });
        }
    });
    if progress.cancelled() { None } else { Some(out.into_inner().unwrap()) }
}

fn quick_hash(path: &Path, size: u64, p: &Progress) -> io::Result<u64> {
    let mut f = File::open(path)?;
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    let mut buf = vec![0u8; EDGE as usize];
    let first = read_up_to(&mut f, &mut buf)?;
    h.update(&buf[..first]);
    if size > 2 * EDGE {
        f.seek(SeekFrom::End(-(EDGE as i64)))?;
        let last = read_up_to(&mut f, &mut buf)?;
        h.update(&buf[..last]);
        p.bytes_read.fetch_add((first + last) as u64, Relaxed);
    } else {
        // Small enough that the "two ends" are the whole file.
        let rest = io::copy(&mut f, &mut HashWriter(&mut h))?;
        p.bytes_read.fetch_add(first as u64 + rest, Relaxed);
    }
    Ok(h.digest())
}

fn full_hash(path: &Path, p: &Progress) -> io::Result<u128> {
    let mut f = File::open(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if p.cancelled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        p.bytes_read.fetch_add(k as u64, Relaxed);
    }
    Ok(u128::from_le_bytes(h.finalize().as_bytes()[..16].try_into().unwrap()))
}

fn read_up_to(f: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..])? {
            0 => break,
            k => got += k,
        }
    }
    Ok(got)
}

struct HashWriter<'a>(&'a mut xxhash_rust::xxh3::Xxh3);

impl io::Write for HashWriter<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.update(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_copies_but_not_lookalikes_or_links() {
        let d = std::env::temp_dir().join(format!("diskie-dupes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(d.join("a.bin"), &big).unwrap();
        std::fs::write(d.join("sub").join("copy.bin"), &big).unwrap();
        // Same size, same ends, different middle: only the full hash tells them apart.
        let mut other = big.clone();
        other[150_000] ^= 1;
        std::fs::write(d.join("lookalike.bin"), &other).unwrap();
        std::fs::write(d.join("small1.txt"), b"same small text").unwrap();
        std::fs::write(d.join("small2.txt"), b"same small text").unwrap();
        std::fs::hard_link(d.join("small1.txt"), d.join("link-to-small1.txt")).unwrap();

        let mut c = diskie_model::Chunk::default();
        c.push(1, 0, "sub", 0, 0, flag::DIR);
        for (parent, name) in [(0, "a.bin"), (1, "copy.bin"), (0, "lookalike.bin"), (0, "small1.txt"), (0, "small2.txt"), (0, "link-to-small1.txt")] {
            let p = if parent == 0 { d.join(name) } else { d.join("sub").join(name) };
            c.push(diskie_model::NO_KEY, parent, name, std::fs::metadata(p).unwrap().len(), 0, 0);
        }
        let t = Tree::build(&d.to_string_lossy(), 0, vec![c], 2);
        let p = Progress::default();
        let groups = find(&t, Tree::ROOT, 1, 2, &p).unwrap();
        let named: Vec<Vec<&str>> = groups.iter().map(|g| g.nodes.iter().map(|&n| t.name(n)).collect()).collect();
        assert_eq!(named.len(), 2, "{named:?}");
        assert_eq!(named[0], ["a.bin", "copy.bin"]);
        assert_eq!(groups[0].wasted(), 300_000);
        assert_eq!(named[1].len(), 2, "a hard link isn't a second copy: {named:?}");
        assert!(named[1].contains(&"small2.txt"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
