//! Showing a file in the system's file manager, and deleting for good.

use std::io;
use std::path::Path;

/// Opens the file manager at `path`'s folder with `path` selected. Blocks briefly: run it off
/// the UI thread.
#[cfg(windows)]
pub fn reveal(path: &Path) -> io::Result<()> {
    // The shell call needs COM, set up on the calling thread: give it a thread of its own.
    let owned = path.to_path_buf();
    let selected = std::thread::spawn(move || select_in_explorer(&owned)).join().unwrap_or(false);
    if selected {
        return Ok(());
    }
    // Starting Explorer works from any process, but doesn't always select the item.
    use std::os::windows::process::CommandExt;
    let explorer = std::env::var_os("SystemRoot").map(|r| Path::new(&r).join("explorer.exe")).unwrap_or_else(|| "explorer.exe".into());
    let p = path.to_string_lossy();
    if p.contains('"') {
        return Err(io::Error::other("the path contains a quote"));
    }
    // Explorer's own argument parsing wants exactly `/select,"<path>"`.
    std::process::Command::new(explorer).raw_arg(format!("/select,\"{p}\"")).spawn().map(drop)
}

/// What Explorer itself does for "Open file location": opens the folder, or reuses a window
/// already showing it, and selects the item.
#[cfg(windows)]
fn select_in_explorer(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
    use windows_sys::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        let init = CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32);
        let pidl = ILCreateFromPathW(wide.as_ptr());
        // With no child items, the one fully specified item is selected in its folder.
        let ok = !pidl.is_null() && SHOpenFolderAndSelectItems(pidl, 0, std::ptr::null(), 0) >= 0;
        if !pidl.is_null() {
            ILFree(pidl);
        }
        if init >= 0 {
            CoUninitialize();
        }
        ok
    }
}

/// Asks the desktop's file manager to show `path` selected (the `org.freedesktop.FileManager1`
/// interface that Files, Dolphin, Nemo, Caja and Thunar provide), or else opens its folder.
#[cfg(not(windows))]
pub fn reveal(path: &Path) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let selected = Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--reply-timeout=5000",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{}", file_uri(path)))
        .arg("string:")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if selected {
        return Ok(());
    }
    let folder = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
    Command::new("xdg-open").arg(folder).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(drop).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            io::Error::other("no file manager answered, and xdg-open is not installed")
        } else {
            e
        }
    })
}

/// `file:///home/me/My%20Files`. Everything but unreserved characters and `/` is escaped, which
/// also keeps commas out of `dbus-send`'s array syntax.
#[cfg(not(windows))]
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Deletes `path` without the Recycle Bin or Trash: a folder with everything in it, a file, or a
/// link (only the link itself, never what it points to).
pub fn delete_permanently(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    let ft = meta.file_type();
    if ft.is_symlink() {
        #[cfg(windows)]
        if std::os::windows::fs::FileTypeExt::is_symlink_dir(&ft) {
            // Directory links and junctions are removed as folders, without entering them.
            return std::fs::remove_dir(path);
        }
        return std::fs::remove_file(path);
    }
    if ft.is_dir() {
        // Doesn't follow links or junctions inside the folder.
        return std::fs::remove_dir_all(path);
    }
    match std::fs::remove_file(path) {
        // A read-only file: Explorer deletes those too, so clear the flag and try again.
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied && meta.permissions().readonly() => {
            let mut perms = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            std::fs::set_permissions(path, perms)?;
            std::fs::remove_file(path)
        }
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("diskie-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[::core::prelude::v1::test]
    fn deletes_folders_and_read_only_files() {
        let d = temp("delete");
        let inner = d.join("folder").join("deeper");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("a.txt"), b"x").unwrap();
        let ro = inner.join("read-only.txt");
        std::fs::write(&ro, b"x").unwrap();
        let mut p = std::fs::metadata(&ro).unwrap().permissions();
        p.set_readonly(true);
        std::fs::set_permissions(&ro, p.clone()).unwrap();
        let lone = d.join("lone-read-only.txt");
        std::fs::write(&lone, b"x").unwrap();
        std::fs::set_permissions(&lone, p).unwrap();

        delete_permanently(&lone).unwrap();
        assert!(!lone.exists());
        delete_permanently(&d.join("folder")).unwrap();
        assert!(!d.join("folder").exists());
        assert!(delete_permanently(&d.join("missing")).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[::core::prelude::v1::test]
    fn deleting_a_link_keeps_what_it_points_to() {
        let d = temp("link");
        let target = d.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(&target, d.join("link")).unwrap();
        delete_permanently(&d.join("link")).unwrap();
        assert!(!d.join("link").exists());
        assert!(target.join("keep.txt").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(windows)]
    #[::core::prelude::v1::test]
    fn deleting_a_junction_keeps_what_it_points_to() {
        let d = temp("junction");
        let target = d.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep.txt"), b"x").unwrap();
        let link = d.join("junction");
        let made = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(&target).output().unwrap();
        assert!(made.status.success(), "{made:?}");
        delete_permanently(&link).unwrap();
        assert!(!link.exists());
        assert!(target.join("keep.txt").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(not(windows))]
    #[::core::prelude::v1::test]
    fn file_uris_escape_spaces_and_commas() {
        assert_eq!(file_uri(Path::new("/home/me/A b,c")), "file:///home/me/A%20b%2Cc");
    }
}
