//! Asking for administrator rights at startup.
//!
//! Administrator rights let Diskie read the NTFS Master File Table (whole drives in seconds)
//! and folders a standard user can't open. Instead of a `requireAdministrator` manifest, which
//! refuses to start at all when the prompt is declined, Diskie relaunches itself through the
//! UAC prompt. If that is declined, it simply carries on as a standard user.

/// Passed to the elevated copy so it never asks again.
#[cfg_attr(not(windows), allow(dead_code))]
pub const ELEVATED_ARG: &str = "--elevated";
/// Skips the prompt (also the `run_as_admin` setting).
pub const NO_ELEVATE_ARG: &str = "--no-elevate";

/// Returns `true` when an elevated copy was started and this process should exit.
#[cfg(windows)]
pub fn relaunch_elevated(args: &[String]) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    if diskie_scan::volumes::is_elevated() || args.iter().any(|a| a == ELEVATED_ARG || a == NO_ELEVATE_ARG) {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain([0]).collect::<Vec<u16>>();
    let mut params = String::from(ELEVATED_ARG);
    for a in args {
        params.push(' ');
        params.push_str(&quote(a));
    }
    let verb = wide("runas".as_ref());
    let file = wide(exe.as_os_str());
    let params = wide(params.as_ref());
    let cwd = std::env::current_dir().ok().map(|d| wide(d.as_os_str()));
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            cwd.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success; anything else (including a declined prompt) means carry on here.
    r as usize > 32
}

#[cfg(not(windows))]
pub fn relaunch_elevated(_: &[String]) -> bool {
    false
}

/// Quotes one argument for the Windows command line.
#[cfg_attr(not(windows), allow(dead_code))]
fn quote(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '"']) {
        return a.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in a.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn quoting() {
        assert_eq!(quote(r"C:\Users"), r"C:\Users");
        assert_eq!(quote(r"C:\Program Files\"), r#""C:\Program Files\\""#);
        assert_eq!(quote(r"\\fs01\home share"), r#""\\fs01\home share""#);
    }
}
