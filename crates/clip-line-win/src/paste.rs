//! SetClipboardData + SendInput Ctrl+V.

use clip_line_core::capture::{Block, Format};
use clip_line_core::paste::{ClipData, PasteMode, Paster};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

/// GlobalFree is not exposed by the windows 0.62 crate; declare it directly.
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
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
    VK_CONTROL, VK_V,
};

const CF_UNICODETEXT_U32: u32 = 13;

pub struct WinPaster;

impl Paster for WinPaster {
    fn paste(&self, data: &ClipData, mode: PasteMode) -> anyhow::Result<()> {
        set_clipboard(&data.blocks)?;
        match mode {
            PasteMode::SetOnly => Ok(()),
            PasteMode::InjectCtrlV => inject_ctrl_v(),
            PasteMode::TypeIn => anyhow::bail!("TypeIn not implemented yet (phase 2)"),
        }
    }
}

fn set_clipboard(blocks: &[Block]) -> anyhow::Result<()> {
    let text = blocks.iter().find(|b| b.format == Format::UnicodeText);
    // ponytail: text-only set for phase 1; image/file formats phase 2
    let Some(tb) = text else {
        anyhow::bail!("phase 1 paste supports text clips only");
    };
    unsafe {
        if OpenClipboard(None).is_err() {
            anyhow::bail!("OpenClipboard failed");
        }
        let _ = EmptyClipboard();
        let units = decode_utf16(&tb.bytes);
        let mut bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
        bytes.extend_from_slice(&[0, 0]); // terminating NUL
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let p = GlobalLock(h);
        if p.is_null() {
            let _ = CloseClipboard();
            let _ = GlobalFree(h);
            anyhow::bail!("GlobalLock failed");
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        let _ = GlobalUnlock(h);
        if SetClipboardData(CF_UNICODETEXT_U32, Some(HANDLE(h.0))).is_err() {
            let _ = CloseClipboard();
            let _ = GlobalFree(h); // system did not take ownership on failure
            anyhow::bail!("SetClipboardData failed");
        }
        let _ = CloseClipboard();
        Ok(())
    }
}

fn decode_utf16(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .take_while(|c| !(c[0] == 0 && c[1] == 0))
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

fn inject_ctrl_v() -> anyhow::Result<()> {
    let down = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let seq = [
        down(VK_CONTROL, false),
        down(VK_V, false),
        down(VK_V, true),
        down(VK_CONTROL, true),
    ];
    std::thread::sleep(std::time::Duration::from_millis(30));
    let n = unsafe { SendInput(&seq, std::mem::size_of::<INPUT>() as i32) };
    if n != seq.len() as u32 {
        anyhow::bail!(
            "SendInput blocked (UIPI?): {} of {} events delivered",
            n,
            seq.len()
        );
    }
    Ok(())
}
