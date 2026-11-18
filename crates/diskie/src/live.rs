//! Live updates: the scanned folder is watched (ReadDirectoryChangesW on Windows, inotify on
//! Linux), and changed paths are looked at again and patched into the tree, so the numbers stay
//! true without a rescan.
//!
//! The watcher only collects paths. Every couple of seconds the window takes them, works out off
//! the UI thread what each one is now ([`resolve`]), and applies that ([`apply`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use diskie_model::Tree;
use diskie_model::tree::flag;
use notify::{RecursiveMode, Watcher};

/// On Linux every folder needs a watch of its own; beyond this many it isn't worth it.
#[cfg(not(windows))]
pub const MAX_LINUX_FOLDERS: u32 = 50_000;

#[derive(Default)]
struct Pending {
    paths: HashSet<PathBuf>,
    /// The system dropped events (too many at once): the tree may be out of date.
    missed: bool,
}

pub struct Live {
    _watcher: notify::RecommendedWatcher,
    pending: Arc<Mutex<Pending>>,
}

impl Live {
    /// Starts watching `root` and everything below it. Can take a while on Linux: run it off
    /// the UI thread.
    pub fn start(root: &Path) -> Result<Live, String> {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let p2 = pending.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(mut p) = p2.lock() else { return };
            match res {
                Ok(ev) => {
                    if ev.need_rescan() {
                        p.missed = true;
                    }
                    if !matches!(ev.kind, notify::EventKind::Access(_)) {
                        p.paths.extend(ev.paths);
                    }
                }
                Err(_) => p.missed = true,
            }
        })
        .map_err(|e| e.to_string())?;
        watcher.watch(root, RecursiveMode::Recursive).map_err(|e| e.to_string())?;
        Ok(Live { _watcher: watcher, pending })
    }

    /// The paths that changed since the last call, and whether any changes were lost.
    pub fn take(&self) -> (Vec<PathBuf>, bool) {
        let Ok(mut p) = self.pending.lock() else { return (Vec::new(), false) };
        let missed = std::mem::take(&mut p.missed);
        (p.paths.drain().collect(), missed)
    }
}

#[derive(Debug, PartialEq)]
pub enum Change {
    Gone(String),
    File { path: String, size: u64, mtime: u32, flags: u8 },
    /// A folder that isn't in the tree yet, with everything in it.
    Dir { path: String, mtime: u32, entries: Vec<Entry> },
}

/// An entry inside a new folder. `parent` indexes earlier entries; `None` is the folder itself.
#[derive(Debug, PartialEq)]
pub struct Entry {
    parent: Option<usize>,
    name: String,
    size: u64,
    mtime: u32,
    flags: u8,
}

/// Most entries listed for one new folder; past this, a rescan is the better tool.
const MAX_NEW_ENTRIES: usize = 200_000;

/// What each changed path is now, parents before children. Paths outside the tree's root are
/// ignored, and so are folders already in the tree (their own changes arrive as their
/// children's paths).
pub fn resolve(tree: &Tree, mut paths: Vec<PathBuf>) -> Vec<Change> {
    paths.sort_by_key(|p| p.as_os_str().len());
    let mut out = Vec::new();
    for p in paths {
        let s = p.to_string_lossy().to_string();
        let known = tree.find_path(&s);
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            if known.is_some() {
                out.push(Change::Gone(s));
            }
            continue;
        };
        if known.is_none() && p.parent().is_none_or(|parent| tree.find_path(&parent.to_string_lossy()).is_none()) {
            // Outside the root, or inside a folder that's new too (which lists it).
            continue;
        }
        let mtime = mtime_of(&meta);
        if meta.is_dir() && !meta.file_type().is_symlink() {
            if known.is_none() {
                let mut entries = Vec::new();
                list_new(&p, None, &mut entries);
                out.push(Change::Dir { path: s, mtime, entries });
            }
        } else {
            out.push(Change::File { path: s, size: if meta.file_type().is_symlink() { 0 } else { meta.len() }, mtime, flags: flags_of(&meta) });
        }
    }
    out
}

fn list_new(dir: &Path, parent: Option<usize>, out: &mut Vec<Entry>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if out.len() >= MAX_NEW_ENTRIES {
            return;
        }
        let Ok(meta) = e.metadata() else { continue };
        let link = meta.file_type().is_symlink();
        let is_dir = meta.is_dir() && !link;
        out.push(Entry {
            parent,
            name: e.file_name().to_string_lossy().to_string(),
            size: if is_dir || link { 0 } else { meta.len() },
            mtime: mtime_of(&meta),
            flags: if is_dir { flag::DIR | (flags_of(&meta) & flag::SYNCED) } else { flags_of(&meta) },
        });
        if is_dir {
            let me = out.len() - 1;
            list_new(&e.path(), Some(me), out);
        }
    }
}

fn mtime_of(m: &std::fs::Metadata) -> u32 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs().min(u32::MAX as u64) as u32).unwrap_or(0)
}

fn flags_of(m: &std::fs::Metadata) -> u8 {
    if m.file_type().is_symlink() {
        return flag::REPARSE | if m.is_dir() { flag::DIR } else { 0 };
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        diskie_scan::cloud_flag(m.file_attributes())
    }
    #[cfg(not(windows))]
    0
}

/// Patches the tree; returns how many changes took effect.
pub fn apply(tree: &mut Tree, changes: &[Change]) -> usize {
    let mut applied = 0;
    for c in changes {
        match c {
            Change::Gone(p) => {
                if let Some(n) = tree.find_path(p)
                    && n != Tree::ROOT
                {
                    tree.remove(n);
                    applied += 1;
                }
            }
            Change::File { path, size, mtime, flags } => match tree.find_path(path) {
                Some(n) if !tree.is_dir(n) => {
                    let i = n as usize;
                    let cloud = flags & (flag::CLOUD | flag::SYNCED);
                    let now = if tree.has(n, flag::CLOUD) { tree.online_size(n) } else { tree.size[i] };
                    if now != *size || tree.mtime[i] != *mtime || tree.flags[i] & (flag::CLOUD | flag::SYNCED) != cloud {
                        tree.set_file(n, *size, *mtime, cloud);
                        applied += 1;
                    }
                }
                Some(n) => {
                    // A folder replaced by a file of the same name.
                    tree.remove(n);
                    applied += add_at(tree, path, *size, *mtime, *flags).is_some() as usize;
                }
                None => applied += add_at(tree, path, *size, *mtime, *flags).is_some() as usize,
            },
            Change::Dir { path, mtime, entries } => {
                if tree.find_path(path).is_some() {
                    continue;
                }
                let Some(d) = add_at(tree, path, 0, *mtime, flag::DIR) else { continue };
                let mut nodes: Vec<u32> = Vec::with_capacity(entries.len());
                for e in entries {
                    let parent = e.parent.map(|i| nodes[i]).unwrap_or(d);
                    nodes.push(tree.add(parent, &e.name, e.size, e.mtime, e.flags));
                }
                // A folder's type is the type most of its bytes are, as the scan works it out.
                let dirs: Vec<u32> = std::iter::once(d).chain(nodes.iter().copied().filter(|&n| tree.is_dir(n))).collect();
                for n in dirs {
                    let totals = tree.kind_totals(n);
                    if let Some((k, &bytes)) = totals.iter().enumerate().max_by_key(|(_, b)| **b)
                        && bytes > 0
                    {
                        tree.kind[n as usize] = k as u8;
                    }
                }
                applied += 1;
            }
        }
    }
    applied
}

fn add_at(tree: &mut Tree, path: &str, size: u64, mtime: u32, flags: u8) -> Option<u32> {
    let p = Path::new(path);
    let parent = tree.find_path(&p.parent()?.to_string_lossy())?;
    if !tree.is_dir(parent) {
        return None;
    }
    Some(tree.add(parent, &p.file_name()?.to_string_lossy(), size, mtime, flags))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn changes_on_disk_reach_the_tree() {
        let d = std::env::temp_dir().join(format!("diskie-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("keep")).unwrap();
        std::fs::write(d.join("keep").join("grows.log"), b"12345").unwrap();
        std::fs::write(d.join("goes.txt"), b"bye").unwrap();
        let progress = diskie_scan::Progress::default();
        let mut tree = diskie_scan::scan(&d, diskie_scan::Options { mft: false, threads: Some(1) }, &progress).unwrap().tree;
        assert_eq!(tree.size[0], 8);

        std::fs::write(d.join("keep").join("grows.log"), b"1234567890").unwrap();
        std::fs::remove_file(d.join("goes.txt")).unwrap();
        std::fs::create_dir_all(d.join("new").join("deeper")).unwrap();
        std::fs::write(d.join("new").join("deeper").join("x.bin"), vec![0u8; 100]).unwrap();
        std::fs::write(d.join("new").join("y.bin"), vec![0u8; 20]).unwrap();
        let root = d.to_string_lossy().to_string();
        let sep = std::path::MAIN_SEPARATOR;
        let paths = vec![
            d.join("keep").join("grows.log"),
            d.join("goes.txt"),
            d.join("new"),
            d.join("new").join("y.bin"), // inside the new folder: comes with it
            PathBuf::from(format!("{root}-elsewhere{sep}z")),
        ];
        let changes = resolve(&tree, paths);
        assert_eq!(changes.len(), 3, "{changes:?}");
        assert_eq!(apply(&mut tree, &changes), 3);
        assert_eq!(tree.size[0], 130);
        assert_eq!(tree.files[0], 3);
        assert!(tree.find_path(&format!("{root}{sep}new{sep}deeper{sep}x.bin")).is_some());
        assert!(tree.find_path(&format!("{root}{sep}goes.txt")).is_none());
        let again = resolve(&tree, vec![d.join("new")]);
        assert_eq!(apply(&mut tree, &again), 0, "applying again changes nothing");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
