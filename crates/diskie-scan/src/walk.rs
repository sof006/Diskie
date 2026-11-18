//! Parallel directory walk with a shared work stack.
//!
//! Each worker pops a folder, lists it with `std::fs::read_dir` (on Windows that is
//! `FindFirstFileExW` underneath, and the size and type come with each entry, so there is no
//! extra call per file), records every entry in its own [`Chunk`] and pushes the subfolders back
//! on the stack. The stack is LIFO, so workers go depth-first, which keeps a hard disk's head
//! near where it already is. Junctions and symbolic links are recorded but never followed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
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
    let q = Queue { stack: Mutex::new(vec![Job { path: root.to_path_buf(), key: ROOT_KEY }]), ready: Condvar::new(), pending: AtomicUsize::new(1) };

    let chunks: Vec<Chunk> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads.max(1)).map(|_| s.spawn(|| worker(&q, &next_key, progress))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
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
    loop {
        let job = {
            let mut stack = q.stack.lock().unwrap();
            loop {
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
            list(&job, &mut chunk, &mut found, next_key, progress);
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

fn list(job: &Job, chunk: &mut Chunk, found: &mut Vec<Job>, next_key: &AtomicU64, progress: &Progress) {
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
        } else if ft.is_dir() {
            let key = next_key.fetch_add(1, Relaxed);
            chunk.push(key, job.key, &name, 0, mtime, flag::DIR);
            found.push(Job { path: entry.path(), key });
            dirs += 1;
        } else {
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            chunk.push(NO_KEY, job.key, &name, size, mtime, 0);
            files += 1;
            bytes += size;
        }
    }
    progress.files.fetch_add(files, Relaxed);
    progress.dirs.fetch_add(dirs, Relaxed);
    progress.bytes.fetch_add(bytes, Relaxed);
}
