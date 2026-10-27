//! Windows adapters behind cfg(windows); empty stubs elsewhere so the
//! workspace always compiles.

#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod paste;
#[cfg(windows)]
mod self_write;
#[cfg(windows)]
pub use capture::WinCapture;
#[cfg(windows)]
pub use paste::WinPaster;

/// Remember the foreground window before showing clipLine's popup.
#[cfg(windows)]
pub fn foreground_target() -> Option<isize> {
    let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    (!hwnd.is_invalid()).then_some(hwnd.0 as isize)
}

/// Position of the foreground thread's caret in screen coordinates; fall
/// back to the pointer when the application does not expose a system caret.
#[cfg(windows)]
pub fn caret_position() -> Option<(f32, f32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo,
        GetWindowThreadProcessId,
    };
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let thread = GetWindowThreadProcessId(hwnd, None);
        if thread != 0
            && GetGUIThreadInfo(thread, &mut info).is_ok()
            && !info.hwndCaret.is_invalid()
        {
            let mut point = POINT {
                x: info.rcCaret.left,
                y: info.rcCaret.bottom,
            };
            if ClientToScreen(info.hwndCaret, &mut point).as_bool() {
                return Some((point.x as f32, point.y as f32));
            }
        }
        let mut point = POINT::default();
        GetCursorPos(&mut point).ok()?;
        Some((point.x as f32, point.y as f32))
    }
}

/// Restore focus to the window which was active before the popup opened.
#[cfg(windows)]
pub fn activate_target(target: isize) -> anyhow::Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SetForegroundWindow};
    let hwnd = HWND(target as *mut _);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            anyhow::bail!("The target window no longer exists");
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            anyhow::bail!("Windows could not restore focus to the target window");
        }
    }
    Ok(())
}
