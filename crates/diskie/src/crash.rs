//! Leaving a trace when Diskie crashes.
//!
//! Release builds have no console, so a panic would otherwise vanish without a word. Panics
//! (and, on Windows, fatal exceptions such as an access violation or a stack overflow) are
//! appended to `crash.log` in Diskie's local data folder, and the user is told where it is.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;

/// `%LOCALAPPDATA%\Diskie\crash.log` on Windows, `~/.local/state/diskie/crash.log` elsewhere.
pub fn log_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let dir = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Diskie"));
    #[cfg(not(windows))]
    let dir = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))
        .map(|d| d.join("diskie"));
    dir.map(|d| d.join("crash.log"))
}

static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();

/// Installs the panic hook (and the Windows exception filter). Call first thing in `main`.
///
/// Every panic is logged. Only a panic on the main thread is fatal for certain, so only that
/// one tells the user straight away; background work runs under [`guard`], which turns a panic
/// into an error the window shows, and a panic that escapes anyway reaches the exception filter.
pub fn install() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let backtrace = std::backtrace::Backtrace::force_capture();
        let report = format!(
            "panic on thread '{}' at {location}\n{}\n\nbacktrace:\n{backtrace}",
            thread.name().unwrap_or("<unnamed>"),
            message(info.payload())
        );
        write_log(&report);
        if MAIN_THREAD.get() == Some(&thread.id()) {
            tell_once();
        }
        default(info);
    }));
    #[cfg(windows)]
    windows::install();
}

/// Runs `f`, turning a panic into an error message for the window. The panic hook has already
/// logged the details by the time this returns.
pub fn guard<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| {
        let saved = log_path().map(|p| format!(" Details were saved to {}.", p.display())).unwrap_or_default();
        format!("Diskie hit an internal error: {}.{saved}", message(&*p))
    })
}

fn message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into())
}

static TOLD: AtomicBool = AtomicBool::new(false);

/// Appends one report to the log and, the first time, tells the user.
fn record(report: &str) {
    write_log(report);
    tell_once();
}

fn tell_once() {
    if !TOLD.swap(true, Ordering::SeqCst) {
        let where_ = log_path().map(|p| p.display().to_string()).unwrap_or_else(|| "(no log folder)".into());
        tell(&format!(
            "Diskie ran into a problem and has to close.\n\nDetails were saved to:\n{where_}\n\n\
             Please attach that file to an issue at github.com/sof006/Diskie/issues."
        ));
    }
}

fn write_log(report: &str) {
    let path = log_path();
    if let Some(path) = &path {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(
                f,
                "==== Diskie {} ({} {}), {} seconds since 1970\n{report}\n",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            );
        }
    }
}

#[cfg(windows)]
fn tell(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (text, title) = (wide(text), wide("Diskie"));
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR | MB_SETFOREGROUND) };
}

#[cfg(not(windows))]
fn tell(text: &str) {
    eprintln!("{text}");
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::EXCEPTION_STACK_OVERFLOW;
    use windows_sys::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

    pub fn install() {
        unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    }

    /// Fatal exceptions that aren't panics: log the code and where in Diskie.exe it happened.
    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        let (code, address) = unsafe {
            match info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) {
                Some(r) => (r.ExceptionCode, r.ExceptionAddress as usize),
                None => (0, 0),
            }
        };
        let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
        let what = match code {
            EXCEPTION_STACK_OVERFLOW => "stack overflow",
            c if c as u32 == 0xC000_0005 => "access violation",
            // How a Rust panic travels on Windows: one escaped a thread Rust didn't start.
            c if c as u32 == 0xE06D_7363 => "uncaught panic (logged above)",
            _ => "unhandled exception",
        };
        let place = if address >= base { format!("Diskie.exe+0x{:x}", address - base) } else { format!("0x{address:x}") };
        // A stack overflow leaves little room: only try the log if this isn't one.
        if code != EXCEPTION_STACK_OVERFLOW {
            super::record(&format!("{what} (0x{:08X}) at {place}", code as u32));
        } else {
            super::tell(&format!(
                "Diskie ran into a problem (a stack overflow at {place}) and has to close.\n\n\
                 Please report it at github.com/sof006/Diskie/issues."
            ));
        }
        EXCEPTION_CONTINUE_SEARCH
    }
}
