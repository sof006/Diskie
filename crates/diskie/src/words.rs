//! Words, fonts and folders that differ between Windows and Linux.

#[cfg(windows)]
mod imp {
    pub const BIN: &str = "Recycle Bin";
    pub const SHOW_IN_FILES: &str = "Show in Explorer";
    pub const MOVE_TO_BIN: &str = "Move to Recycle Bin…";
    pub const KEYS_HINT: &str = "Arrows move, Enter zooms in, Backspace goes up, Del moves to the Recycle Bin, Shift+Del deletes permanently.";
    pub const NO_BIN: &str =
        "This location has no Recycle Bin (network share or removable drive). To delete it permanently, press Shift+Del or right-click it.";
    pub const SYSTEM_WARNING: &str = "This is inside a Windows system folder. Removing it can stop Windows or installed programs from working.";
    pub const NETWORK_HINT: &str = r"Type a \\server\share path into the folder box to scan a network share.";
    pub const UI_FONT: &str = "Segoe UI";
    pub const MONO_FONT: &str = "Cascadia Mono";

    pub fn mode(elevated: bool) -> (&'static str, &'static str) {
        if elevated { ("shield", "Administrator · fast MFT scans") } else { ("lock", "Standard user · folder walk") }
    }

    pub fn is_system_path(p: &str) -> bool {
        let lower = p.to_ascii_lowercase();
        let win = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_ascii_lowercase();
        [win.as_str(), r"c:\program files", r"c:\program files (x86)", r"c:\programdata"]
            .iter()
            .any(|s| lower == *s || lower.starts_with(&format!("{s}\\")))
            || lower.ends_with(r"\pagefile.sys")
            || lower.ends_with(r"\hiberfil.sys")
            || lower.ends_with(r"\swapfile.sys")
    }
}

#[cfg(not(windows))]
mod imp {
    pub const BIN: &str = "Trash";
    pub const SHOW_IN_FILES: &str = "Show in file manager";
    pub const MOVE_TO_BIN: &str = "Move to Trash…";
    pub const KEYS_HINT: &str = "Arrows move, Enter zooms in, Backspace goes up, Del moves to the Trash, Shift+Del deletes permanently.";
    pub const NO_BIN: &str =
        "This location has no Trash (it is a network share). To delete it permanently, press Shift+Del or right-click it.";
    pub const SYSTEM_WARNING: &str = "This is inside a system folder. Removing it can stop Linux or installed programs from working.";
    pub const NETWORK_HINT: &str = "Network shares appear under Drives once mounted, for example by opening them in your file manager.";
    /// The desktop's own interface font.
    pub const UI_FONT: &str = ".SystemUIFont";
    pub const MONO_FONT: &str = "DejaVu Sans Mono";

    pub fn mode(elevated: bool) -> (&'static str, &'static str) {
        if elevated { ("shield", "Root · folder walk") } else { ("lock", "Normal user · folder walk") }
    }

    /// Folders the system and packages own. Deleting from them as a normal user mostly fails
    /// anyway, but as root it would break the system.
    pub fn is_system_path(p: &str) -> bool {
        p == "/"
            || ["/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32", "/etc", "/boot", "/var/lib", "/opt", "/snap", "/proc", "/sys", "/dev"]
                .iter()
                .any(|s| p == *s || p.starts_with(&format!("{s}/")))
    }
}

pub use imp::*;

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    #[::core::prelude::v1::test]
    fn linux_system_folders() {
        assert!(super::is_system_path("/usr/lib/x86_64-linux-gnu"));
        assert!(super::is_system_path("/etc"));
        assert!(!super::is_system_path("/home/me/usr"));
        assert!(!super::is_system_path("/usrdata"));
    }
}
