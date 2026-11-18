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

/// A `\\server\share` path, or a path on a mapped network drive letter.
pub fn is_network_path(p: &Path) -> bool {
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

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn list_volumes() -> Vec<Volume> {
        vec![Volume { root: "/".into(), label: "File system".into(), filesystem: String::new(), total: 0, free: 0, kind: VolumeKind::Fixed }]
    }

    pub fn drive_is_remote(_: char) -> bool {
        false
    }

    pub fn filesystem(_: &Path) -> Option<String> {
        None
    }

    pub fn has_seek_penalty(_: &Path) -> Option<bool> {
        None
    }

    pub fn is_elevated() -> bool {
        false
    }

    pub fn has_recycle_bin(p: &Path) -> bool {
        p.is_absolute()
    }
}

pub use imp::{filesystem, has_recycle_bin, has_seek_penalty, is_elevated, list_volumes};

/// Total and free bytes on the volume holding `p`, when known.
#[cfg(windows)]
pub fn space(p: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    let w: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let (mut free, mut total) = (0u64, 0u64);
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    (ok != 0).then_some((total, free))
}

#[cfg(not(windows))]
pub fn space(_: &Path) -> Option<(u64, u64)> {
    None
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
