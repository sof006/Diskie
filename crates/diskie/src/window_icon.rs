//! Gives Diskie's window its icon directly, in the large and small sizes.
//!
//! GPUI only sets the icon on its window class. Windows can find the icon that way, but Task
//! Manager, Alt+Tab and the taskbar ask the window itself first (`WM_GETICON`), and some of
//! them fall back to a blank icon when it has none.

/// Sets the icon on each of this thread's GPUI windows. Call it after opening a window.
#[cfg(windows)]
pub fn set() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, TRUE};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumThreadWindows;

    unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> windows_sys::core::BOOL {
        unsafe { set_on(hwnd) };
        TRUE
    }
    unsafe { EnumThreadWindows(GetCurrentThreadId(), Some(each), 0) };
}

#[cfg(windows)]
unsafe fn set_on(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    let mut class = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    if String::from_utf16_lossy(&class[..len.max(0) as usize]) != "Zed::Window" {
        return;
    }
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    for (which, cx, cy) in [(ICON_BIG, SM_CXICON, SM_CYICON), (ICON_SMALL, SM_CXSMICON, SM_CYSMICON)] {
        // Resource 1 is the icon build.rs embeds. LR_SHARED icons are owned by the system.
        let icon = unsafe { LoadImageW(module, 1 as _, IMAGE_ICON, GetSystemMetrics(cx), GetSystemMetrics(cy), LR_SHARED) };
        if !icon.is_null() {
            unsafe { SendMessageW(hwnd, WM_SETICON, which as usize, icon as isize) };
        }
    }
}

#[cfg(not(windows))]
pub fn set() {}
