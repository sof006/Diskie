//! Who owns a file: the NTFS owner account on Windows, the user on Linux. Looked up only for
//! the rows on screen, and cached per account.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Owners {
    names: Mutex<HashMap<Vec<u8>, Arc<str>>>,
}

impl Owners {
    /// The owner's account name, or `None` when it can't be read.
    pub fn of(&self, path: &Path) -> Option<Arc<str>> {
        let id = imp::owner_id(path)?;
        if let Some(n) = self.names.lock().ok()?.get(&id) {
            return Some(n.clone());
        }
        let name: Arc<str> = imp::name(&id).into();
        self.names.lock().ok()?.insert(id, name.clone());
        Some(name)
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{GetLengthSid, IsValidSid, LookupAccountSidW, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SID_NAME_USE};

    /// The owner SID's bytes.
    pub fn owner_id(path: &Path) -> Option<Vec<u8>> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut owner: PSID = std::ptr::null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        unsafe {
            let r = GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut sd,
            );
            if r != ERROR_SUCCESS || owner.is_null() {
                if !sd.is_null() {
                    LocalFree(sd);
                }
                return None;
            }
            let out = (IsValidSid(owner) != 0).then(|| std::slice::from_raw_parts(owner as *const u8, GetLengthSid(owner) as usize).to_vec());
            LocalFree(sd);
            out
        }
    }

    /// `DOMAIN\user`, or the SID as text for an account that no longer exists.
    pub fn name(sid: &[u8]) -> String {
        let psid = sid.as_ptr() as PSID;
        let (mut name, mut domain) = ([0u16; 256], [0u16; 256]);
        let (mut nlen, mut dlen) = (name.len() as u32, domain.len() as u32);
        let mut use_: SID_NAME_USE = 0;
        unsafe {
            if LookupAccountSidW(std::ptr::null(), psid, name.as_mut_ptr(), &mut nlen, domain.as_mut_ptr(), &mut dlen, &mut use_) != 0 {
                let n = String::from_utf16_lossy(&name[..nlen as usize]);
                let d = String::from_utf16_lossy(&domain[..dlen as usize]);
                return if d.is_empty() { n } else { format!("{d}\\{n}") };
            }
            let mut s: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(psid, &mut s) != 0 && !s.is_null() {
                let len = (0..).take_while(|&i| *s.add(i) != 0).count();
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(s, len));
                LocalFree(s as _);
                return text;
            }
        }
        "unknown".into()
    }
}

#[cfg(unix)]
mod imp {
    use std::collections::HashMap;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;
    use std::sync::OnceLock;

    pub fn owner_id(path: &Path) -> Option<Vec<u8>> {
        Some(std::fs::symlink_metadata(path).ok()?.uid().to_le_bytes().to_vec())
    }

    /// The user name from /etc/passwd (local accounts), or the number.
    pub fn name(id: &[u8]) -> String {
        static USERS: OnceLock<HashMap<u32, String>> = OnceLock::new();
        let uid = u32::from_le_bytes(id.try_into().unwrap_or([0; 4]));
        let users = USERS.get_or_init(|| {
            std::fs::read_to_string("/etc/passwd")
                .unwrap_or_default()
                .lines()
                .filter_map(|l| {
                    let mut f = l.split(':');
                    let name = f.next()?;
                    let uid = f.nth(1)?.parse().ok()?;
                    Some((uid, name.to_string()))
                })
                .collect()
        });
        users.get(&uid).cloned().unwrap_or_else(|| uid.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn a_file_we_made_is_ours() {
        let f = std::env::temp_dir().join(format!("diskie-owner-{}", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        let o = Owners::default();
        let name = o.of(&f).expect("owner");
        assert!(!name.is_empty());
        assert_eq!(o.of(&f).unwrap(), name, "cached");
        std::fs::remove_file(&f).unwrap();
        assert!(o.of(&f).is_none());
    }
}
