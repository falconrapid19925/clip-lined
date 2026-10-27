#![cfg(windows)]

use clip_line_core::capture::Capture;
use clip_line_core::capture::{Block, Format};
use clip_line_core::paste::ClipData;
use clip_line_win::{WinCapture, WinPaster};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn listener_starts_and_reads_one_clipboard_change() {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut cap = WinCapture::new().expect("listener window");
        ready_tx.send(()).unwrap();
        event_tx.send(cap.next_event()).unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let self_bytes: Vec<u8> = "clipline-self"
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let data = ClipData {
        blocks: vec![Block {
            format: Format::UnicodeText,
            bytes: self_bytes,
        }],
    };
    let mut set_self = false;
    for _ in 0..20 {
        if WinPaster.set_clipboard(&data).is_ok() {
            set_self = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(set_self, "could not set self-originated clipboard text");
    assert!(
        event_rx.recv_timeout(Duration::from_millis(250)).is_err(),
        "self-originated change was captured"
    );

    // An independent write must still be captured after the suppressed one.
    let units: Vec<u8> = "clipline-foreign"
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .flat_map(|u| u.to_le_bytes())
        .collect();
    // Clipboard open can race with other processes; retry a few times.
    let mut set = false;
    let mut foreign_owner = None;
    for _ in 0..20 {
        unsafe {
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::System::DataExchange::*;
            use windows::Win32::System::Memory::*;
            use windows::Win32::UI::WindowsAndMessaging::*;
            #[allow(non_snake_case)]
            unsafe fn GlobalFree(
                h: windows::Win32::Foundation::HGLOBAL,
            ) -> windows::Win32::Foundation::HGLOBAL {
                unsafe {
                    #[link(name = "kernel32")]
                    unsafe extern "system" {
                        fn GlobalFree(
                            hmem: windows::Win32::Foundation::HGLOBAL,
                        ) -> windows::Win32::Foundation::HGLOBAL;
                    }
                    GlobalFree(h)
                }
            }
            let owner = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::w!("STATIC"),
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
            )
            .unwrap();
            if OpenClipboard(Some(owner)).is_err() {
                let _ = DestroyWindow(owner);
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            let _ = EmptyClipboard();
            let h = GlobalAlloc(GMEM_MOVEABLE, units.len()).unwrap();
            let p = GlobalLock(h);
            assert!(!p.is_null());
            std::ptr::copy_nonoverlapping(units.as_ptr(), p as *mut u8, units.len());
            let _ = GlobalUnlock(h);
            let r = SetClipboardData(13, Some(HANDLE(h.0)));
            let _ = CloseClipboard();
            if r.is_ok() {
                foreign_owner = Some(owner);
                set = true;
                break;
            }
            let _ = DestroyWindow(owner);
            let _ = GlobalFree(h);
        }
    }
    assert!(set, "could not set clipboard text after retries");
    let ev = event_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("foreign event timed out")
        .expect("event");
    if let Some(owner) = foreign_owner {
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(owner) };
    }
    assert!(
        ev.blocks
            .iter()
            .any(|b| b.format == Format::UnicodeText && b.bytes == units),
        "expected foreign unicode text block, got {:?}",
        ev.blocks
    );
    let exe = std::env::current_exe().unwrap();
    let expected = exe.file_stem().unwrap().to_string_lossy();
    assert!(
        ev.source_app.eq_ignore_ascii_case(&expected),
        "expected owner process {expected}, got {:?}",
        ev.source_app
    );
}
