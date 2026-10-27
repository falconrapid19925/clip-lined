#![cfg(windows)]

use clip_line_core::capture::Capture;
use clip_line_win::WinCapture;

#[test]
fn listener_starts_and_reads_one_clipboard_change() {
    let mut cap = WinCapture::new().expect("listener window");
    // Put text on the clipboard ourselves — that alone fires WM_CLIPBOARDUPDATE.
    let units: Vec<u8> = "clipline-e2e"
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .flat_map(|u| u.to_le_bytes())
        .collect();
    // Clipboard open can race with other processes; retry a few times.
    let mut set = false;
    for _ in 0..20 {
        unsafe {
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::System::DataExchange::*;
            use windows::Win32::System::Memory::*;
            if OpenClipboard(None).is_err() {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            let _ = EmptyClipboard();
            let h = GlobalAlloc(GMEM_MOVEABLE, units.len()).unwrap();
            let p = GlobalLock(h);
            assert!(!p.is_null());
            std::ptr::copy_nonoverlapping(units.as_ptr(), p as *mut u8, units.len());
            let _ = GlobalUnlock(h);
            // SetClipboardData returns null HGLOBAL when the system takes a copy;
            // windows 0.62 models null as Err(HRESULT 0) — treat both as ok.
            let r = SetClipboardData(13, Some(HANDLE(h.0)));
            let _ = CloseClipboard();
            if r.is_ok() || r.unwrap_err().code().0 == 0 {
                set = true;
                break;
            }
        }
    }
    assert!(set, "could not set clipboard text after retries");
    let ev = cap.next_event().expect("event");
    let text = ev
        .blocks
        .iter()
        .any(|b| b.format == clip_line_core::capture::Format::UnicodeText);
    assert!(text, "expected a unicode text block, got {:?}", ev.blocks);
}
