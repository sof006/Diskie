//! Drives, drive types and the process's elevation.

use std::path::Path;

#[derive(Clone, Debug)]
pub struct Volume {
    /// `C:\`
    pub root: String,
    pub label: String,
    pub filesystem: String,
    pub total: u64,
    pub free: u64,
    pub kind: VolumeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeKind {
    Fixed,
    Removable,
    Network,
    Optical,
    Other,
}

/// A `\\server\share` path, or a path on a mapped network drive letter. On Linux, a path on a
/// network mount (CIFS, NFS, SSHFS, GVFS…).
pub fn is_network_path(p: &Path) -> bool {
    #[cfg(unix)]
    if p.is_absolute() {
        return imp::is_network(p);
    }
    let s = p.to_string_lossy();
    if s.starts_with(r"\\") {
        return !s.starts_with(r"\\?\") && !s.starts_with(r"\\.\");
    }
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && imp::drive_is_remote(b[0] as char)
}

/// `Some('C')` when `p` is exactly a drive root such as `C:\`.
pub fn drive_root_letter(p: &Path) -> Option<char> {
    let s = p.to_string_lossy();
    let b = s.as_bytes();
    (b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')).then(|| (b[0] as char).to_ascii_uppercase())
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
        GetVolumePathNameW, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{
        DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_PROPERTY_QUERY,
        StorageDeviceSeekPenaltyProperty,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain([0]).collect()
    }

    fn from_wide(buf: &[u16]) -> String {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    }

    pub fn list_volumes() -> Vec<Volume> {
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let root = format!("{}:\\", (b'A' + i as u8) as char);
            let w = wide(&root);
            let kind = match unsafe { GetDriveTypeW(w.as_ptr()) } {
                DRIVE_FIXED => VolumeKind::Fixed,
                DRIVE_REMOVABLE => VolumeKind::Removable,
                DRIVE_REMOTE => VolumeKind::Network,
                DRIVE_CDROM => VolumeKind::Optical,
                _ => VolumeKind::Other,
            };
            let (mut free, mut total) = (0u64, 0u64);
            if unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) } == 0 {
                continue; // no media, or not reachable
            }
            let mut label = [0u16; 261];
            let mut fs = [0u16; 261];
            unsafe {
                GetVolumeInformationW(
                    w.as_ptr(),
                    label.as_mut_ptr(),
                    label.len() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    fs.as_mut_ptr(),
                    fs.len() as u32,
                )
            };
            out.push(Volume { root, label: from_wide(&label), filesystem: from_wide(&fs), total, free, kind });
        }
        out
    }

    pub fn drive_is_remote(letter: char) -> bool {
        let root = wide(&format!("{letter}:\\"));
        unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
    }

    pub fn filesystem(root: &Path) -> Option<String> {
        let w = wide(&root.to_string_lossy());
        let mut fs = [0u16; 64];
        let ok = unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        };
        (ok != 0).then(|| from_wide(&fs))
    }

    /// Whether the volume holding `p` sits on a spinning disk. `None` when Windows won't say.
    pub fn has_seek_penalty(p: &Path) -> Option<bool> {
        let w = wide(&p.to_string_lossy());
        let mut vol = [0u16; 300];
        if unsafe { GetVolumePathNameW(w.as_ptr(), vol.as_mut_ptr(), vol.len() as u32) } == 0 {
            return None;
        }
        let vol = from_wide(&vol);
        let letter = vol.strip_suffix('\\').unwrap_or(&vol);
        if letter.len() != 2 {
            return None;
        }
        let device = wide(&format!(r"\\.\{letter}"));
        // Access 0 is enough for this query, and works without administrator rights.
        let h: HANDLE = unsafe {
            CreateFileW(device.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
        };
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let query = STORAGE_PROPERTY_QUERY { PropertyId: StorageDeviceSeekPenaltyProperty, QueryType: PropertyStandardQuery, AdditionalParameters: [0] };
        let mut desc: DEVICE_SEEK_PENALTY_DESCRIPTOR = unsafe { std::mem::zeroed() };
        let mut got = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&query as *const STORAGE_PROPERTY_QUERY).cast(),
                std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                (&mut desc as *mut DEVICE_SEEK_PENALTY_DESCRIPTOR).cast(),
                std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
                &mut got,
                std::ptr::null_mut(),
            )
        };
        unsafe { CloseHandle(h) };
        (ok != 0 && got as usize >= std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>()).then_some(desc.IncursSeekPenalty)
    }

    /// Whether `p` is on a fixed local drive, where deleting goes to the Recycle Bin. Network
    /// shares and most removable drives have no Recycle Bin, so a "delete" there is permanent.
    pub fn has_recycle_bin(p: &Path) -> bool {
        if is_network_path(p) {
            return false;
        }
        let s = p.to_string_lossy();
        let b = s.as_bytes();
        if b.len() < 2 || b[1] != b':' || !b[0].is_ascii_alphabetic() {
            return false;
        }
        let root = wide(&format!("{}:\\", b[0] as char));
        unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_FIXED }
    }

    pub fn is_elevated() -> bool {
        let mut token: HANDLE = std::ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return false;
        }
        let mut elev = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                (&mut elev as *mut TOKEN_ELEVATION).cast(),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
        };
        unsafe { CloseHandle(token) };
        ok != 0 && elev.TokenIsElevated != 0
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::path::PathBuf;

    /// One line of `/proc/self/mounts`.
    #[derive(Clone, Debug)]
    pub struct Mount {
        pub source: String,
        pub point: PathBuf,
        pub fstype: String,
    }

    /// File systems that hold files people put there. The rest of the mount table (`proc`,
    /// `sysfs`, `tmpfs`, `overlay`, snap `squashfs` images…) is the system's own plumbing.
    const LOCAL: &[&str] = &[
        "ext2", "ext3", "ext4", "xfs", "btrfs", "f2fs", "jfs", "reiserfs", "zfs", "bcachefs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk",
        "hfsplus", "apfs", "iso9660", "udf",
    ];
    const NETWORK: &[&str] = &["cifs", "smb3", "smbfs", "nfs", "nfs4", "fuse.sshfs", "fuse.rclone", "fuse.gvfsd-fuse", "davfs", "fuse.davfs2"];
    /// Mounts at or under these belong to the system, not to the user.
    const HIDDEN: &[&str] = &["/proc", "/sys", "/dev", "/run", "/snap", "/var/lib/docker", "/var/snap", "/boot/efi", "/usr/lib/wsl", "/mnt/wsl", "/mnt/wslg"];

    /// `/proc/self/mounts` writes spaces, tabs and backslashes as `\040`-style octal.
    fn unescape(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
                out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
                i += 4;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    pub fn parse_mounts(text: &str) -> Vec<Mount> {
        text.lines()
            .filter_map(|l| {
                let mut f = l.split_whitespace();
                let (source, point, fstype) = (f.next()?, f.next()?, f.next()?);
                Some(Mount { source: unescape(source), point: PathBuf::from(unescape(point)), fstype: fstype.to_string() })
            })
            .collect()
    }

    pub fn mounts() -> Vec<Mount> {
        std::fs::read_to_string("/proc/self/mounts").map(|t| parse_mounts(&t)).unwrap_or_default()
    }

    pub fn is_network_type(fstype: &str) -> bool {
        NETWORK.contains(&fstype)
    }

    /// The mount that holds `p`: the deepest mount point `p` lies under (the last one listed,
    /// when two are mounted at the same place).
    pub fn mount_of(p: &Path, mounts: &[Mount]) -> Option<Mount> {
        let mut best: Option<&Mount> = None;
        for m in mounts.iter().filter(|m| p.starts_with(&m.point)) {
            if best.is_none_or(|b| m.point.as_os_str().len() >= b.point.as_os_str().len()) {
                best = Some(m);
            }
        }
        best.cloned()
    }

    /// Whether a mount is a place worth offering in the sidebar.
    pub fn is_user_mount(m: &Mount) -> bool {
        let p = m.point.to_string_lossy();
        // Removable media and GVFS network folders live under /run, but they are the user's.
        let users_under_run = p.starts_with("/run/media/") || (p.starts_with("/run/user/") && p.ends_with("/gvfs"));
        if !users_under_run && HIDDEN.iter().any(|h| p == *h || p.starts_with(&format!("{h}/"))) {
            return false;
        }
        // WSL mounts the Windows drives at /mnt/<letter>.
        let wsl_drive = (m.fstype == "9p" || m.fstype == "drvfs") && p.starts_with("/mnt/");
        LOCAL.contains(&m.fstype.as_str()) || is_network_type(&m.fstype) || wsl_drive
    }

    #[allow(clippy::unnecessary_cast)] // the field types differ between targets
    pub fn space(p: &Path) -> Option<(u64, u64)> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        let unit = st.f_frsize as u64;
        let total = st.f_blocks as u64 * unit;
        (total > 0).then_some((total, st.f_bavail as u64 * unit))
    }

    pub fn list_volumes() -> Vec<Volume> {
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<Volume> = Vec::new();
        // The last mount at a point is the one in effect.
        for m in mounts().into_iter().rev() {
            if !seen.insert(m.point.clone()) || !is_user_mount(&m) {
                continue;
            }
            let Some((total, free)) = space(&m.point) else { continue };
            let root = m.point.to_string_lossy().into_owned();
            let kind = if is_network_type(&m.fstype) {
                VolumeKind::Network
            } else if m.fstype == "iso9660" || m.fstype == "udf" {
                VolumeKind::Optical
            } else if root.starts_with("/media/") || root.starts_with("/run/media/") {
                VolumeKind::Removable
            } else {
                VolumeKind::Fixed
            };
            let name = m.point.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let label = match root.as_str() {
                "/" => "File system".to_string(),
                "/home" => "Home".to_string(),
                _ if m.fstype == "9p" || m.fstype == "drvfs" => format!("Windows {}:", name.to_uppercase()),
                _ => name,
            };
            out.push(Volume { root, label, filesystem: m.fstype.clone(), total, free, kind });
        }
        out.sort_by(|a, b| (a.root != "/", &a.root).cmp(&(b.root != "/", &b.root)));
        out
    }

    pub fn is_network(p: &Path) -> bool {
        let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        mount_of(&p, &mounts()).is_some_and(|m| is_network_type(&m.fstype))
    }

    pub fn drive_is_remote(_: char) -> bool {
        false
    }

    pub fn filesystem(p: &Path) -> Option<String> {
        mount_of(p, &mounts()).map(|m| m.fstype)
    }

    /// Whether the disk under `p` spins, from `/sys/class/block/<dev>/queue/rotational`.
    pub fn has_seek_penalty(p: &Path) -> Option<bool> {
        let m = mount_of(p, &mounts())?;
        let dev = std::fs::canonicalize(&m.source).ok()?;
        let name = dev.strip_prefix("/dev").ok()?.file_name()?.to_owned();
        let sys = std::fs::canonicalize(Path::new("/sys/class/block").join(&name)).ok()?;
        // A partition has no queue of its own: its disk is the folder above it.
        let text = [sys.join("queue/rotational"), sys.parent()?.join("queue/rotational")].iter().find_map(|q| std::fs::read_to_string(q).ok())?;
        Some(text.trim() == "1")
    }

    pub fn is_elevated() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    /// Local folders have a Trash (the `trash` crate finds the right one for each mount). Network
    /// mounts usually don't, so a "delete" there would be permanent.
    pub fn has_recycle_bin(p: &Path) -> bool {
        p.is_absolute() && !is_network(p)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const SAMPLE: &str = "\
/dev/sdd / ext4 rw,relatime 0 0
proc /proc proc rw 0 0
tmpfs /run tmpfs rw 0 0
C:\\134 /mnt/c 9p rw 0 0
/dev/nvme0n1p3 /home btrfs rw 0 0
//nas/media /mnt/nas\\040media cifs rw 0 0
/dev/sdb1 /run/media/me/USB\\040STICK vfat rw 0 0
/dev/loop3 /snap/core/1 squashfs ro 0 0
none /mnt/wslg tmpfs rw 0 0
";

        #[test]
        fn picks_the_mounts_people_use() {
            let all = parse_mounts(SAMPLE);
            assert_eq!(all[3].source, "C:\\");
            let shown: Vec<_> = all.iter().filter(|m| is_user_mount(m)).map(|m| m.point.to_string_lossy().into_owned()).collect();
            assert_eq!(shown, ["/", "/mnt/c", "/home", "/mnt/nas media", "/run/media/me/USB STICK"]);
        }

        #[test]
        fn finds_the_mount_holding_a_path() {
            let all = parse_mounts(SAMPLE);
            assert_eq!(mount_of(Path::new("/home/me/x"), &all).unwrap().fstype, "btrfs");
            assert_eq!(mount_of(Path::new("/mnt/nas media/films"), &all).unwrap().fstype, "cifs");
            assert_eq!(mount_of(Path::new("/usr/bin"), &all).unwrap().point, Path::new("/"));
            // A name that merely starts the same is not a parent folder.
            assert_eq!(mount_of(Path::new("/homework"), &all).unwrap().point, Path::new("/"));
        }

        #[test]
        fn the_real_root_has_space() {
            assert!(space(Path::new("/")).is_some_and(|(t, f)| t > 0 && f <= t));
            assert!(list_volumes().iter().any(|v| v.root == "/"));
        }
    }
}

pub use imp::{filesystem, has_recycle_bin, has_seek_penalty, is_elevated, list_volumes};
#[cfg(unix)]
pub use imp::{Mount, is_network_type, mount_of, mounts, space};

/// Total and free bytes on the volume holding `p`, when known.
#[cfg(windows)]
pub fn space(p: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    let w: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let (mut free, mut total) = (0u64, 0u64);
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    (ok != 0).then_some((total, free))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_shapes() {
        assert_eq!(drive_root_letter(Path::new("c:\\")), Some('C'));
        assert_eq!(drive_root_letter(Path::new("C:\\Users")), None);
        assert!(is_network_path(Path::new(r"\\fs01\home$")));
        assert!(!is_network_path(Path::new(r"\\?\C:\x")));
    }

    #[cfg(windows)]
    #[test]
    fn lists_the_system_drive() {
        let v = list_volumes();
        assert!(v.iter().any(|v| v.root == "C:\\" && v.total > 0), "{v:?}");
    }
}
