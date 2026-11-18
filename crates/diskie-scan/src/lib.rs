//! Turning a folder, drive or network share into a [`Tree`].
//!
//! [`scan`] picks the backend:
//! - **MFT** (Windows, NTFS volume root, administrator): reads the Master File Table in one
//!   sequential pass. Seconds for a whole drive, and the only fast option on a hard disk.
//! - **Walk** (everything else): a parallel directory walk. On a hard disk it uses two workers,
//!   because many concurrent seeks made the week-1 spike more than 100× slower.
//!
//! Progress and cancellation go through a shared [`Progress`].

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

pub use diskie_model::Tree;

#[cfg(windows)]
mod mft;
pub mod volumes;
mod walk;

pub use volumes::{Volume, list_volumes};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Mft,
    Walk,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Mft => "MFT",
            Backend::Walk => "folder walk",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Starting = 0,
    ReadingMft = 1,
    Walking = 2,
    Building = 3,
}

/// Live counters, shared between the scanner and the UI.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub unreadable: AtomicU64,
    phase: AtomicU8,
    cancel: AtomicBool,
    current: Mutex<String>,
}

impl Progress {
    pub fn phase(&self) -> Phase {
        match self.phase.load(Relaxed) {
            1 => Phase::ReadingMft,
            2 => Phase::Walking,
            3 => Phase::Building,
            _ => Phase::Starting,
        }
    }

    fn set_phase(&self, p: Phase) {
        self.phase.store(p as u8, Relaxed);
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Relaxed)
    }

    /// The folder being listed right now (sampled, not every folder).
    pub fn current(&self) -> String {
        self.current.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set_current(&self, s: &str) {
        if let Ok(mut c) = self.current.try_lock() {
            c.clear();
            c.push_str(s);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Use the MFT when possible. On by default.
    pub mft: bool,
    /// Worker threads for the directory walk; `None` picks by drive type.
    pub threads: Option<usize>,
}

impl Default for Options {
    fn default() -> Self {
        Self { mft: true, threads: None }
    }
}

pub struct Scan {
    pub tree: Tree,
    pub backend: Backend,
    pub elapsed: Duration,
    /// Why the MFT was not used, when it was tried and failed.
    pub fallback: Option<String>,
    pub threads: usize,
}

#[derive(Debug)]
pub enum Error {
    Cancelled,
    /// The root itself could not be listed.
    Root(std::io::Error),
    /// A bug: the scan panicked. The message is already user-facing.
    Internal(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Cancelled => f.write_str("scan cancelled"),
            Error::Root(e) => write!(f, "cannot read the folder: {e}"),
            Error::Internal(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

/// Scans `root` (a folder, drive root or UNC share) and returns the finished tree.
pub fn scan(root: &Path, opts: Options, progress: &Progress) -> Result<Scan, Error> {
    let t0 = Instant::now();
    let root = normalize(root);
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut fallback = None;

    #[cfg(windows)]
    if opts.mft
        && let Some(letter) = volumes::drive_root_letter(&root)
    {
        if volumes::is_network_path(&root) {
            // A mapped network drive can report NTFS, but its MFT is on the server; the walk is
            // the right method there, so there is nothing to explain.
        } else if !volumes::is_elevated() {
            fallback = Some("Diskie is not running as administrator".to_string());
        } else if volumes::filesystem(&root).as_deref() != Some("NTFS") {
            fallback = Some("the drive is not NTFS".to_string());
        } else {
            progress.set_phase(Phase::ReadingMft);
            match mft::scan(letter, progress) {
                Ok(tree) => {
                    return Ok(Scan { tree, backend: Backend::Mft, elapsed: t0.elapsed(), fallback: None, threads: 1 });
                }
                Err(e) if progress.is_cancelled() => {
                    let _ = e;
                    return Err(Error::Cancelled);
                }
                Err(e) => fallback = Some(format!("reading the MFT failed: {e}")),
            }
        }
    }

    let threads = opts.threads.unwrap_or_else(|| walk::default_threads(&root));
    progress.set_phase(Phase::Walking);
    let tree = walk::scan(&root, threads, progress)?;
    Ok(Scan { tree, backend: Backend::Walk, elapsed: t0.elapsed(), fallback, threads })
}

/// Cloud state from Windows file attributes: [`flag::CLOUD`] for a file that is online only
/// (OneDrive and Dropbox placeholders: reading it would download it), [`flag::SYNCED`] for one
/// kept in sync and stored here, 0 otherwise. Links must be ruled out before calling this.
///
/// [`flag::CLOUD`]: diskie_model::tree::flag::CLOUD
/// [`flag::SYNCED`]: diskie_model::tree::flag::SYNCED
pub fn cloud_flag(attributes: u32) -> u8 {
    use diskie_model::tree::flag;
    const REPARSE_POINT: u32 = 0x400;
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_OPEN: u32 = 0x4_0000;
    const PINNED: u32 = 0x8_0000;
    const UNPINNED: u32 = 0x10_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    if attributes & (RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN | OFFLINE) != 0 {
        flag::CLOUD
    } else if attributes & (REPARSE_POINT | PINNED | UNPINNED) != 0 {
        flag::SYNCED
    } else {
        0
    }
}

/// `C:` → `C:\`; strips a trailing separator from anything but a drive root.
fn normalize(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s.len() == 2 && s.ends_with(':') {
        return PathBuf::from(format!("{s}\\"));
    }
    if s.len() > 3 && (s.ends_with('\\') || s.ends_with('/')) && !s.ends_with(":\\") {
        return PathBuf::from(s.trim_end_matches(['\\', '/']).to_string());
    }
    p.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_a_temporary_folder() {
        let dir = std::env::temp_dir().join(format!("diskie-scan-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/movie.mkv"), vec![0u8; 3000]).unwrap();
        std::fs::write(dir.join("a/notes.txt"), b"hello").unwrap();
        std::fs::write(dir.join("top.bin"), vec![0u8; 100]).unwrap();

        let p = Progress::default();
        let s = scan(&dir, Options { mft: false, threads: Some(3) }, &p).unwrap();
        let t = &s.tree;
        assert_eq!(s.backend, Backend::Walk);
        assert_eq!(t.size[0], 3105);
        assert_eq!(t.files[0], 3);
        assert_eq!(t.dirs[0], 2);
        assert_eq!(t.name(t.kids(0)[0]), "a");
        let a = t.kids(0)[0];
        assert!(t.path(a).ends_with("\\a") || t.path(a).ends_with("/a"));
        assert_eq!(p.files.load(Relaxed), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_root_is_an_error() {
        let p = Progress::default();
        let r = scan(Path::new("Z:\\definitely\\not\\here"), Options { mft: false, threads: Some(1) }, &p);
        assert!(matches!(r, Err(Error::Root(_))));
    }
}
