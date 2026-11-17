//! Week-1 spike: how fast can we enumerate a whole volume?
//!
//! ```text
//! spike-scan win32 C:\        parallel FindFirstFileExW (large fetch), no admin needed
//! spike-scan std   C:\        parallel std::fs::read_dir, as a baseline
//! spike-scan mft   C:         read the NTFS $MFT with ntfs-reader (administrator only)
//! ```
//!
//! Every mode prints files, folders, total logical bytes, unreadable folders,
//! elapsed time, throughput and peak memory.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

#[derive(Default)]
struct Totals {
    files: AtomicU64,
    dirs: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(mode), Some(target)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: spike-scan <win32|std|mft> <path or drive>");
        std::process::exit(2);
    };
    let totals = Totals::default();
    let t0 = Instant::now();
    let note = match mode.as_str() {
        "win32" => {
            win32::scan(target, &totals);
            String::new()
        }
        "std" => {
            std_walk(std::path::Path::new(target), &totals);
            String::new()
        }
        "mft" => mft::scan(target, &totals),
        _ => {
            eprintln!("unknown mode {mode}");
            std::process::exit(2);
        }
    };
    let secs = t0.elapsed().as_secs_f64();
    let files = totals.files.load(Relaxed);
    let dirs = totals.dirs.load(Relaxed);
    println!(
        "{mode:>5} {target}: {} files, {} folders, {:.1} GB, {} unreadable, {:.2} s, {:.0}k entries/s, peak {:.0} MB{note}",
        files,
        dirs,
        totals.bytes.load(Relaxed) as f64 / 1e9,
        totals.errors.load(Relaxed),
        secs,
        (files + dirs) as f64 / secs / 1000.0,
        peak_mb(),
    );
}

fn std_walk(dir: &std::path::Path, t: &Totals) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        t.errors.fetch_add(1, Relaxed);
        return;
    };
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            t.dirs.fetch_add(1, Relaxed);
            subdirs.push(e.path());
        } else if let Ok(m) = e.metadata() {
            t.files.fetch_add(1, Relaxed);
            t.bytes.fetch_add(m.len(), Relaxed);
        }
    }
    use rayon::prelude::*;
    subdirs.par_iter().for_each(|d| std_walk(d, t));
}

#[cfg(windows)]
fn peak_mb() -> f64 {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut m: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    m.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut m, m.cb) };
    m.PeakWorkingSetSize as f64 / 1e6
}

#[cfg(not(windows))]
fn peak_mb() -> f64 {
    0.0
}

#[cfg(windows)]
mod win32 {
    use super::Totals;
    use std::sync::atomic::Ordering::Relaxed;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic,
        FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, WIN32_FIND_DATAW,
    };

    pub fn scan(root: &str, t: &Totals) {
        let mut path: Vec<u16> = r"\\?\".encode_utf16().collect();
        path.extend(root.trim_end_matches('\\').encode_utf16());
        rayon::scope(|s| walk(s, path, t));
    }

    fn walk<'s>(s: &rayon::Scope<'s>, dir: Vec<u16>, t: &'s Totals) {
        let mut pattern = dir.clone();
        pattern.extend("\\*".encode_utf16());
        pattern.push(0);
        let mut fd: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
        let h = unsafe {
            FindFirstFileExW(
                pattern.as_ptr(),
                FindExInfoBasic,
                (&mut fd as *mut WIN32_FIND_DATAW).cast(),
                FindExSearchNameMatch,
                std::ptr::null(),
                FIND_FIRST_EX_LARGE_FETCH,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            t.errors.fetch_add(1, Relaxed);
            return;
        }
        loop {
            let name_len = fd.cFileName.iter().position(|&c| c == 0).unwrap_or(fd.cFileName.len());
            let name = &fd.cFileName[..name_len];
            let dot = name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16];
            if !dot {
                if fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                    t.dirs.fetch_add(1, Relaxed);
                    // Never follow junctions or symlinks: no loops, no double counting.
                    if fd.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
                        let mut child = dir.clone();
                        child.push(b'\\' as u16);
                        child.extend_from_slice(name);
                        s.spawn(move |s| walk(s, child, t));
                    }
                } else {
                    t.files.fetch_add(1, Relaxed);
                    t.bytes.fetch_add((fd.nFileSizeHigh as u64) << 32 | fd.nFileSizeLow as u64, Relaxed);
                }
            }
            if unsafe { FindNextFileW(h, &mut fd) } == 0 {
                break;
            }
        }
        unsafe { FindClose(h) };
    }
}

#[cfg(windows)]
mod mft {
    use super::Totals;
    use ntfs_reader::{Mft, Volume};
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Instant;

    /// Loads the $MFT, then walks every in-use record once, the way the real
    /// backend would: name, parent reference, directory flag, default-stream size.
    pub fn scan(drive: &str, t: &Totals) -> String {
        let device = format!(r"\\.\{}", drive.trim_end_matches(['\\', '/']));
        let t0 = Instant::now();
        let volume = match Volume::new(&device) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("cannot open {device}: {e:?} (reading the MFT needs an elevated shell)");
                std::process::exit(1);
            }
        };
        let mft = Mft::new(volume).expect("read $MFT");
        let load = t0.elapsed().as_secs_f64();
        let mut parents: Vec<u64> = Vec::with_capacity(mft.record_count() as usize);
        for file in mft.files() {
            let parent = file.best_name().map(|n| n.parent_number()).unwrap_or(u64::MAX);
            parents.push(parent);
            if file.is_directory() {
                t.dirs.fetch_add(1, Relaxed);
            } else {
                t.files.fetch_add(1, Relaxed);
                let size = file.data_streams().find(|s| s.name.is_none()).map(|s| s.size).unwrap_or(0);
                t.bytes.fetch_add(size, Relaxed);
            }
        }
        format!(
            " · $MFT load {load:.2} s, {} records, {:.0} MB in memory, {} parent links",
            mft.record_count(),
            mft.size_in_memory() as f64 / 1e6,
            parents.len()
        )
    }
}

#[cfg(not(windows))]
mod win32 {
    pub fn scan(_: &str, _: &super::Totals) {
        eprintln!("win32 mode is Windows-only");
    }
}

#[cfg(not(windows))]
mod mft {
    pub fn scan(_: &str, _: &super::Totals) -> String {
        eprintln!("mft mode is Windows-only");
        String::new()
    }
}
