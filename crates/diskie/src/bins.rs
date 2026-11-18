//! How much each drive's Recycle Bin (Trash on Linux) holds, and emptying it.

use std::io;
use std::path::Path;

use diskie_scan::Volume;

#[derive(Clone, Debug)]
pub struct Bin {
    /// The drive's root, as in [`Volume::root`].
    pub root: String,
    pub bytes: u64,
    pub items: u64,
    /// Linux: the trash folders on this drive.
    #[cfg_attr(windows, allow(dead_code))]
    dirs: Vec<std::path::PathBuf>,
}

/// The bins of every drive that has one and isn't empty. Slow-ish (it may add up a big Trash
/// on Linux): run it off the UI thread.
pub fn list(volumes: &[Volume]) -> Vec<Bin> {
    volumes.iter().filter_map(imp::bin).filter(|b| b.items > 0).collect()
}

/// Empties one drive's bin for good.
pub fn empty(bin: &Bin) -> io::Result<()> {
    imp::empty(bin)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW, SHQUERYRBINFO, SHQueryRecycleBinW};

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain([0]).collect()
    }

    pub fn bin(v: &Volume) -> Option<Bin> {
        if !diskie_scan::volumes::has_recycle_bin(Path::new(&v.root)) {
            return None;
        }
        let mut info = SHQUERYRBINFO { cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32, i64Size: 0, i64NumItems: 0 };
        let root = wide(&v.root);
        let hr = unsafe { SHQueryRecycleBinW(root.as_ptr(), &mut info) };
        let (bytes, items) = (info.i64Size, info.i64NumItems);
        (hr >= 0).then(|| Bin { root: v.root.clone(), bytes: bytes.max(0) as u64, items: items.max(0) as u64, dirs: Vec::new() })
    }

    pub fn empty(bin: &Bin) -> io::Result<()> {
        let root = wide(&bin.root);
        let hr = unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), root.as_ptr(), SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND) };
        // E_UNEXPECTED: it was already empty.
        if hr >= 0 || hr as u32 == 0x8000_FFFF { Ok(()) } else { Err(io::Error::from_raw_os_error(hr)) }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;

    use crate::files::{Deletion, delete_permanently};

    /// The freedesktop.org trash folders that live on `v`: the home Trash if home is on it,
    /// and the drive's own `.Trash-<uid>` and `.Trash/<uid>`.
    fn trash_dirs(v: &Volume) -> Vec<PathBuf> {
        let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
        let mut dirs = vec![Path::new(&v.root).join(format!(".Trash-{uid}")), Path::new(&v.root).join(".Trash").join(uid.to_string())];
        let home_trash = std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("Trash"));
        if let Some(h) = home_trash
            && same_device(&h, Path::new(&v.root))
        {
            dirs.push(h);
        }
        dirs.retain(|d| d.join("files").is_dir());
        dirs
    }

    fn same_device(a: &Path, b: &Path) -> bool {
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev(),
            _ => false,
        }
    }

    fn size_of(p: &Path) -> u64 {
        let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
        if !m.is_dir() {
            return m.len();
        }
        std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| size_of(&e.path())).sum()).unwrap_or(0)
    }

    pub fn bin(v: &Volume) -> Option<Bin> {
        let dirs = trash_dirs(v);
        let (mut bytes, mut items) = (0, 0);
        for d in &dirs {
            for e in std::fs::read_dir(d.join("files")).ok()?.flatten() {
                items += 1;
                bytes += size_of(&e.path());
            }
        }
        Some(Bin { root: v.root.clone(), bytes, items, dirs })
    }

    pub fn empty(bin: &Bin) -> io::Result<()> {
        let progress = Deletion::default();
        for d in &bin.dirs {
            for sub in ["files", "info", "expunged"] {
                let Ok(rd) = std::fs::read_dir(d.join(sub)) else { continue };
                for e in rd.flatten() {
                    delete_permanently(&e.path(), &progress)?;
                }
            }
        }
        Ok(())
    }
}
