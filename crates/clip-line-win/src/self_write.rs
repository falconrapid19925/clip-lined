//! Shared between the paste writer and the clipboard listener. The lock is
//! held until the writer records the final sequence, so the listener cannot
//! race ahead of the write's WM_CLIPBOARDUPDATE notification.
use std::sync::Mutex;

pub(crate) static SEQUENCE: Mutex<Option<u32>> = Mutex::new(None);

pub(crate) fn is_self_write() -> bool {
    let mut expected = SEQUENCE.lock().unwrap_or_else(|e| e.into_inner());
    // Read while holding the writer's lock, after its clipboard transaction.
    let seq = unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
    let matched = *expected == Some(seq);
    // A different sequence means another process has already replaced our
    // clipboard contents; the old sequence must not suppress future writes.
    if !matched {
        *expected = None;
    }
    matched
}
