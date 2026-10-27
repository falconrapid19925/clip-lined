//! AddClipboardFormatListener + WM_CLIPBOARDUPDATE on a hidden message window.

use clip_line_core::capture::{Block, Capture, ClipEvent, Format};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE,
};
use windows::core::PCWSTR;

pub struct WinCapture {
    hwnd: isize,
}

impl WinCapture {
    pub fn new() -> anyhow::Result<Self> {
        unsafe {
            let class_name: Vec<u16> = "clipLineListener\0".encode_utf16().collect();
            let class_name = PCWSTR(class_name.as_ptr());
            let wc = windows::Win32::UI::WindowsAndMessaging::WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                lpszClassName: class_name,
                ..Default::default()
            };
            windows::Win32::UI::WindowsAndMessaging::RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                None,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )?;
            AddClipboardFormatListener(hwnd)?;
            Ok(WinCapture {
                hwnd: hwnd.0 as isize,
            })
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

impl Capture for WinCapture {
    fn next_event(&mut self) -> anyhow::Result<ClipEvent> {
        unsafe {
            let mut msg = MSG::default();
            loop {
                let r = GetMessageW(&mut msg, Some(HWND(self.hwnd as *mut _)), 0, 0);
                if r.0 <= 0 {
                    anyhow::bail!("message loop ended: {}", r.0);
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                if msg.message == WM_CLIPBOARDUPDATE {
                    return read_clipboard();
                }
            }
        }
    }
}

unsafe fn read_clipboard() -> anyhow::Result<ClipEvent> {
    unsafe {
        let (app, title) = foreground_app();
        let mut blocks = Vec::new();
        if OpenClipboard(None).is_err() {
            return Ok(ClipEvent {
                blocks,
                source_app: app,
                source_title: title,
            });
        }
        let mut fmt = EnumClipboardFormats(0);
        while fmt != 0 {
            if let Some(b) = read_format(fmt) {
                blocks.push(b);
            }
            fmt = EnumClipboardFormats(fmt);
        }
        let _ = CloseClipboard();
        Ok(ClipEvent {
            blocks,
            source_app: app,
            source_title: title,
        })
    }
}

unsafe fn read_format(fmt: u32) -> Option<Block> {
    unsafe {
        let format = match fmt {
            13 => Format::UnicodeText,
            8 | 17 => Format::ImageDib, // CF_DIB | CF_DIBV5
            15 => Format::FileDrop,
            other => {
                let mut name = [0u16; 128];
                let n = GetClipboardFormatNameW(other, &mut name);
                if n == 0 {
                    return None;
                }
                let s = String::from_utf16_lossy(&name[..n as usize]);
                Format::Registered(other, s)
            }
        };
        let handle: HANDLE = GetClipboardData(fmt).ok()?; // ponytail: delayed-render sources can fail here; skip not crash
        let h = HGLOBAL(handle.0);
        let p = GlobalLock(h);
        if p.is_null() {
            return None;
        }
        let len = GlobalSize(h);
        let bytes = std::slice::from_raw_parts(p as *const u8, len).to_vec();
        let _ = GlobalUnlock(h);
        Some(Block { format, bytes })
    }
}

unsafe fn foreground_app() -> (String, String) {
    // ponytail: process-name + window-title lookup lands with paste polish (phase 2)
    (String::new(), String::new())
}
