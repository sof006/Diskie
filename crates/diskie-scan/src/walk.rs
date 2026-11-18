//! Parallel directory walk with a shared work stack.
//!
//! Each worker pops a folder, lists it with `std::fs::read_dir` (on Windows that is
//! `FindFirstFileExW` underneath, and the size and type come with each entry, so there is no
//! extra call per file), records every entry in its own [`Chunk`] and pushes the subfolders back
//! on the stack. The stack is LIFO, so workers go depth-first, which keeps a hard disk's head
//! near where it already is. Junctions and symbolic links are recorded but never followed.
//!
//! On Linux the walk stays on the file system it started on, like `du -x`: a folder where
//! another file system is mounted (`/proc`, a USB stick, a network share) is recorded with
//! [`flag::MOUNT`] and not entered. A file with several hard links is counted once.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Condvar, Mutex};
use std::time::UNIX_EPOCH;

use diskie_model::tree::flag;
use diskie_model::{Chunk, NO_KEY, Tree};

use crate::{Error, Progress};

const ROOT_KEY: u64 = 0;

struct Job {
    path: PathBuf,
    key: u64,
}

struct Queue {
    stack: Mutex<Vec<Job>>,
    ready: Condvar,
    /// Jobs queued or being worked on. The walk is finished when this reaches zero.
    pending: AtomicUsize,
    /// A worker panicked: the others stop instead of waiting for its jobs forever.
    failed: AtomicBool,
    /// The device the walk started on; folders on any other are mount points.
    #[cfg(unix)]
    root_dev: u64,
    /// (device, inode) of every file with more than one hard link seen so far.
    #[cfg(unix)]
    linked: Mutex<std::collections::HashSet<(u64, u64)>>,
}

/// Wakes the other workers if this one panics.
struct WakeOnPanic<'a>(&'a Queue);

impl Drop for WakeOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.failed.store(true, Relaxed);
            let _guard = self.0.stack.lock();
            self.0.ready.notify_all();
        }
    }
}

pub fn default_threads(root: &Path) -> usize {
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    if crate::volumes::is_network_path(root) {
        // Network round trips dominate; many requests in flight hide the latency.
        32
    } else if crate::volumes::has_seek_penalty(root) == Some(true) {
        2
    } else {
        cpus.clamp(2, 16)
    }
}

pub fn scan(root: &Path, threads: usize, progress: &Progress) -> Result<Tree, Error> {
    // Fail early, with the real error, when the root itself cannot be listed.
    std::fs::read_dir(root).map_err(Error::Root)?;

    let next_key = AtomicU64::new(ROOT_KEY + 1);
    let q = Queue {
        stack: Mutex::new(vec![Job { path: root.to_path_buf(), key: ROOT_KEY }]),
        ready: Condvar::new(),
        pending: AtomicUsize::new(1),
        failed: AtomicBool::new(false),
        #[cfg(unix)]
        root_dev: std::os::unix::fs::MetadataExt::dev(&std::fs::metadata(root).map_err(Error::Root)?),
        #[cfg(unix)]
        linked: Mutex::new(Default::default()),
    };

    let chunks: Vec<Chunk> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads.max(1)).map(|_| s.spawn(|| worker(&q, &next_key, progress))).collect();
        // A worker that panicked would leave a hole in the tree; pass the panic on instead.
        handles.into_iter().map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p))).collect()
    });
    if progress.is_cancelled() {
        return Err(Error::Cancelled);
    }
    progress.set_phase(crate::Phase::Building);
    let root_name = root.to_string_lossy();
    Ok(Tree::build(&root_name, ROOT_KEY, chunks, next_key.load(Relaxed)))
}

fn worker(q: &Queue, next_key: &AtomicU64, progress: &Progress) -> Chunk {
    let mut chunk = Chunk::with_capacity(16_384);
    let mut found: Vec<Job> = Vec::new();
    let mut listed = 0u32;
    let _wake = WakeOnPanic(q);
    loop {
        let job = {
            let mut stack = q.stack.lock().unwrap();
            loop {
                if q.failed.load(Relaxed) {
                    break None;
                }
                if let Some(j) = stack.pop() {
                    break Some(j);
                }
                if q.pending.load(Relaxed) == 0 || progress.is_cancelled() {
                    break None;
                }
                stack = q.ready.wait(stack).unwrap();
            }
        };
        let Some(job) = job else {
            q.ready.notify_all();
            return chunk;
        };

        if !progress.is_cancelled() {
            listed = listed.wrapping_add(1);
            if listed % 64 == 1 {
                progress.set_current(&job.path.to_string_lossy());
            }
            list(q, &job, &mut chunk, &mut found, next_key, progress);
        }

        let added = found.len();
        if added > 0 {
            q.pending.fetch_add(added, Relaxed);
            q.stack.lock().unwrap().append(&mut found);
            if added == 1 {
                q.ready.notify_one();
            } else {
                q.ready.notify_all();
            }
        }
        if q.pending.fetch_sub(1, Relaxed) == 1 {
            // That was the last job anywhere: wake everyone so they can exit.
            let _guard = q.stack.lock().unwrap();
            q.ready.notify_all();
        }
    }
}

fn list(q: &Queue, job: &Job, chunk: &mut Chunk, found: &mut Vec<Job>, next_key: &AtomicU64, progress: &Progress) {
    #[cfg(test)]
    if job.path.file_name().is_some_and(|n| n == tests::PANIC_MARKER) {
        panic!("test panic in a walk worker");
    }
    let rd = match std::fs::read_dir(&job.path) {
        Ok(rd) => rd,
        Err(_) => {
            chunk.mark_unreadable(job.key);
            progress.unreadable.fetch_add(1, Relaxed);
            return;
        }
    };
    let (mut files, mut dirs, mut bytes) = (0u64, 0u64, 0u64);
    for entry in rd {
        let Ok(entry) = entry else { continue };
        let Ok(ft) = entry.file_type() else { continue };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let meta = entry.metadata().ok();
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs().min(u32::MAX as u64) as u32)
            .unwrap_or(0);
        if ft.is_symlink() {
            // Junctions and links: shown, never followed, no size of their own.
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir()) || entry.path().is_dir();
            if is_dir {
                chunk.push(NO_KEY, job.key, &name, 0, mtime, flag::DIR | flag::REPARSE);
                dirs += 1;
            } else {
                chunk.push(NO_KEY, job.key, &name, 0, mtime, flag::REPARSE);
                files += 1;
            }
        } else if ft.is_dir() && is_mount(q, meta.as_ref()) {
            // Another file system: shown, not entered.
            chunk.push(NO_KEY, job.key, &name, 0, mtime, flag::DIR | flag::MOUNT);
            dirs += 1;
        } else if ft.is_dir() {
            let key = next_key.fetch_add(1, Relaxed);
            chunk.push(key, job.key, &name, 0, mtime, flag::DIR | (attributes(meta.as_ref()) & flag::SYNCED));
            found.push(Job { path: entry.path(), key });
            dirs += 1;
        } else {
            let size = if counted_before(q, meta.as_ref()) { 0 } else { meta.as_ref().map(|m| m.len()).unwrap_or(0) };
            let f = attributes(meta.as_ref());
            chunk.push(NO_KEY, job.key, &name, size, mtime, f);
            files += 1;
            if f & flag::CLOUD == 0 {
                bytes += size;
            }
        }
    }
    progress.files.fetch_add(files, Relaxed);
    progress.dirs.fetch_add(dirs, Relaxed);
    progress.bytes.fetch_add(bytes, Relaxed);
}

/// [`flag::CLOUD`] or [`flag::SYNCED`] from a Windows entry's attributes. Links never get here.
#[cfg(windows)]
fn attributes(meta: Option<&std::fs::Metadata>) -> u8 {
    use std::os::windows::fs::MetadataExt;
    meta.map(|m| crate::cloud_flag(m.file_attributes())).unwrap_or(0)
}

#[cfg(not(windows))]
fn attributes(_: Option<&std::fs::Metadata>) -> u8 {
    0
}

#[cfg(unix)]
fn is_mount(q: &Queue, meta: Option<&std::fs::Metadata>) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.is_some_and(|m| m.dev() != q.root_dev)
}

#[cfg(not(unix))]
fn is_mount(_: &Queue, _: Option<&std::fs::Metadata>) -> bool {
    false
}

/// Whether this file is another hard link to one already counted, so its bytes aren't added twice.
#[cfg(unix)]
fn counted_before(q: &Queue, meta: Option<&std::fs::Metadata>) -> bool {
    use std::os::unix::fs::MetadataExt;
    match meta {
        Some(m) if m.nlink() > 1 => !q.linked.lock().unwrap().insert((m.dev(), m.ino())),
        _ => false,
    }
}

#[cfg(not(unix))]
fn counted_before(_: &Queue, _: Option<&std::fs::Metadata>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const PANIC_MARKER: &str = "diskie-test-panic";

    fn tree_with(dir: &Path, marker: bool) {
        for a in 0..8 {
            for b in 0..8 {
                let d = dir.join(format!("a{a}")).join(format!("b{b}"));
                std::fs::create_dir_all(&d).unwrap();
                std::fs::write(d.join("f.txt"), b"hello").unwrap();
            }
        }
        if marker {
            std::fs::create_dir_all(dir.join("a3").join(PANIC_MARKER)).unwrap();
        }
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("diskie-walk-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_count_once() {
        let d = temp("links");
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::write(d.join("a").join("big.bin"), vec![7u8; 10_000]).unwrap();
        std::fs::hard_link(d.join("a").join("big.bin"), d.join("same.bin")).unwrap();
        let t = scan(&d, 4, &Progress::default()).unwrap();
        assert_eq!(t.size[0], 10_000);
        assert_eq!(t.files[0], 2);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn walks_everything() {
        let d = temp("ok");
        tree_with(&d, false);
        let t = scan(&d, 4, &Progress::default()).unwrap();
        assert_eq!(t.files[0], 64);
        assert_eq!(t.dirs[0], 72);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_worker_panic_fails_the_scan_instead_of_hanging_it() {
        let d = temp("panic");
        tree_with(&d, true);
        let (tx, rx) = std::sync::mpsc::channel();
        let d2 = d.clone();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(|| scan(&d2, 4, &Progress::default()));
            let _ = tx.send(r.is_err());
        });
        let panicked = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("the scan hung after a worker panicked");
        assert!(panicked, "the panic should reach the caller");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
