use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::{
    Mutex,
    atomic::{AtomicIsize, Ordering},
};
use std::time::Duration;

use crate::dib_to_bmp;
use clip_line_core::capture::{Block, Capture, ClipEvent, Format};
use clip_line_core::paste::{ClipData, PasteMode, Paster};

// Serialize our own reads and writes so a paste's changeCount is registered
// before this process's capture loop can consume it.
static PASTEBOARD_LOCK: Mutex<()> = Mutex::new(());
static SELF_CHANGE: AtomicIsize = AtomicIsize::new(-1);

type Obj = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Obj;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
}
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn CGEventCreateKeyboardEvent(source: Obj, key: u16, down: bool) -> Obj;
    fn CGEventSetFlags(event: Obj, flags: u64);
    fn CGEventKeyboardSetUnicodeString(event: Obj, length: usize, text: *const u16);
    fn CGEventPost(tap: u32, event: Obj);
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(obj: Obj);
}

// Objective-C dispatch uses the ABI of each actual method. These helpers keep
// the signatures explicit at each call site and all objects inside a pool.
unsafe fn class(name: &CStr) -> Obj {
    unsafe { objc_getClass(name.as_ptr()) }
}
unsafe fn sel(name: &CStr) -> Sel {
    unsafe { sel_registerName(name.as_ptr()) }
}
unsafe fn send0<R>(o: Obj, s: &CStr) -> R {
    unsafe {
        let f: unsafe extern "C" fn(Obj, Sel) -> R = std::mem::transmute(objc_msgSend as *const ());
        f(o, sel(s))
    }
}
unsafe fn send1<A, R>(o: Obj, s: &CStr, a: A) -> R {
    unsafe {
        let f: unsafe extern "C" fn(Obj, Sel, A) -> R =
            std::mem::transmute(objc_msgSend as *const ());
        f(o, sel(s), a)
    }
}
unsafe fn send2<A, B, R>(o: Obj, s: &CStr, a: A, b: B) -> R {
    unsafe {
        let f: unsafe extern "C" fn(Obj, Sel, A, B) -> R =
            std::mem::transmute(objc_msgSend as *const ());
        f(o, sel(s), a, b)
    }
}
unsafe fn send3<A, B, C, R>(o: Obj, s: &CStr, a: A, b: B, c: C) -> R {
    unsafe {
        let f: unsafe extern "C" fn(Obj, Sel, A, B, C) -> R =
            std::mem::transmute(objc_msgSend as *const ());
        f(o, sel(s), a, b, c)
    }
}
struct Pool(Obj);
impl Pool {
    fn new() -> Self {
        unsafe {
            let pool: Obj = send0(class(c"NSAutoreleasePool"), c"new");
            Self(pool)
        }
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        unsafe {
            send0::<()>(self.0, c"drain");
        }
    }
}

unsafe fn nsstring(s: &str) -> Obj {
    // NSUTF8StringEncoding = 4. NSString copies the bytes and the pool
    // releases this temporary after the pasteboard operation.
    unsafe {
        let allocated: Obj = send0(class(c"NSString"), c"alloc");
        let initialized: Obj = send3(
            allocated,
            c"initWithBytes:length:encoding:",
            s.as_ptr(),
            s.len(),
            4usize,
        );
        send0(initialized, c"autorelease")
    }
}
unsafe fn string(obj: Obj) -> Option<String> {
    if obj.is_null() {
        return None;
    }
    unsafe {
        let p: *const u8 = send0(obj, c"UTF8String");
        if p.is_null() {
            return None;
        }
        let n: usize = send1(obj, c"lengthOfBytesUsingEncoding:", 4usize);
        Some(String::from_utf8_lossy(std::slice::from_raw_parts(p, n)).into_owned())
    }
}
unsafe fn board() -> Obj {
    unsafe { send0(class(c"NSPasteboard"), c"generalPasteboard") }
}
unsafe fn count(board: Obj) -> isize {
    unsafe { send0(board, c"changeCount") }
}

pub struct MacCapture {
    seen: isize,
}
impl MacCapture {
    pub fn new() -> anyhow::Result<Self> {
        let _pool = Pool::new();
        let b = unsafe { board() };
        if b.is_null() {
            anyhow::bail!("macOS pasteboard is unavailable");
        }
        Ok(Self {
            seen: unsafe { count(b) },
        })
    }
}
impl Capture for MacCapture {
    fn next_event(&mut self) -> anyhow::Result<ClipEvent> {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let _guard = PASTEBOARD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let _pool = Pool::new();
            unsafe {
                let b = board();
                if b.is_null() {
                    anyhow::bail!("macOS pasteboard is unavailable");
                }
                let now = count(b);
                if now == self.seen {
                    continue;
                }
                self.seen = now;
                if SELF_CHANGE
                    .compare_exchange(now, -1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    continue;
                }
                let mut blocks = Vec::new();
                let items: Obj = send0(b, c"pasteboardItems");
                let len: usize = if items.is_null() {
                    0
                } else {
                    send0(items, c"count")
                };
                let mut paths = Vec::new();
                for i in 0..len {
                    let item: Obj = send1(items, c"objectAtIndex:", i);
                    let url_string: Obj =
                        send1(item, c"stringForType:", nsstring("public.file-url"));
                    if !url_string.is_null() {
                        let url: Obj = send1(class(c"NSURL"), c"URLWithString:", url_string);
                        let path: Obj = send0(url, c"path");
                        if let Some(path) = string(path) {
                            paths.push(path);
                        }
                    }
                }
                if !paths.is_empty() {
                    blocks.push(Block {
                        format: Format::FileDrop,
                        bytes: encode_paths(&paths),
                    });
                }
                let text: Obj = send1(b, c"stringForType:", nsstring("public.utf8-plain-text"));
                if let Some(text) = string(text) {
                    blocks.push(Block {
                        format: Format::UnicodeText,
                        bytes: encode_text(&text),
                    });
                }
                if let Some(image) = read_image(b) {
                    blocks.push(Block {
                        format: Format::ImageDibV5,
                        bytes: image,
                    });
                }
                if blocks.is_empty() {
                    continue;
                }
                let workspace: Obj = send0(class(c"NSWorkspace"), c"sharedWorkspace");
                let app: Obj = send0(workspace, c"frontmostApplication");
                let name: Obj = send0(app, c"localizedName");
                return Ok(ClipEvent {
                    blocks,
                    source_app: string(name).unwrap_or_default(),
                    source_title: String::new(),
                });
            }
        }
    }
}

pub struct MacPaster;
impl Paster for MacPaster {
    fn paste(&self, data: &ClipData, mode: PasteMode) -> anyhow::Result<()> {
        // Reject unsupported formats before clearing the user's clipboard.
        let paths = data
            .blocks
            .iter()
            .find(|b| b.format == Format::FileDrop)
            .map(|b| decode_paths(&b.bytes))
            .transpose()?;
        let text = data
            .blocks
            .iter()
            .find(|b| b.format == Format::UnicodeText)
            .map(|b| decode_text(&b.bytes))
            .transpose()?;
        let image = data
            .blocks
            .iter()
            .find(|b| matches!(b.format, Format::ImageDibV5 | Format::ImageDib))
            .map(|b| dib_to_bmp(&b.bytes))
            .transpose()?;
        let rtf = data.blocks.iter().find_map(|block| match &block.format {
            Format::Registered(_, name)
                if name.eq_ignore_ascii_case("Rich Text Format")
                    || name.eq_ignore_ascii_case("public.rtf") =>
            {
                Some(block.bytes.strip_suffix(&[0]).unwrap_or(&block.bytes))
            }
            _ => None,
        });
        if paths.is_none() && text.is_none() && image.is_none() && rtf.is_none() {
            anyhow::bail!(
                "macOS paste supports text, RTF, file and DIB image clips; the stored formats are unsupported"
            );
        }
        if mode == PasteMode::TypeIn && text.is_none() {
            anyhow::bail!("typing emulation requires text");
        }
        if mode != PasteMode::SetOnly && !unsafe { AXIsProcessTrusted() } {
            anyhow::bail!(
                "Enable Accessibility for clipLine in System Settings > Privacy & Security > Accessibility to allow paste injection"
            );
        }
        {
            let _guard = PASTEBOARD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let _pool = Pool::new();
            unsafe {
                let b = board();
                if b.is_null() {
                    anyhow::bail!("macOS pasteboard is unavailable");
                }
                // Convert image before clearing the user's clipboard.
                let png = if let Some(image) = image.as_ref() {
                    let data: Obj = send2(
                        class(c"NSData"),
                        c"dataWithBytes:length:",
                        image.as_ptr(),
                        image.len(),
                    );
                    let rep: Obj = send1(class(c"NSBitmapImageRep"), c"imageRepWithData:", data);
                    if rep.is_null() {
                        anyhow::bail!("macOS could not decode the stored DIB image");
                    }
                    let options: Obj = send0(class(c"NSDictionary"), c"dictionary");
                    let encoded: Obj =
                        send2(rep, c"representationUsingType:properties:", 4usize, options);
                    if encoded.is_null() {
                        anyhow::bail!("macOS could not encode the stored DIB image");
                    }
                    Some(encoded)
                } else {
                    None
                };
                let _: isize = send0(b, c"clearContents");
                let mut success = true;
                // Files may have multiple pasteboard items; other formats are
                // representations of the first item and remain available too.
                if let Some(paths) = paths {
                    let array: Obj = send0(class(c"NSMutableArray"), c"array");
                    for path in paths {
                        let url: Obj = send1(class(c"NSURL"), c"fileURLWithPath:", nsstring(&path));
                        send1::<Obj, ()>(array, c"addObject:", url);
                    }
                    let written: bool = send1(b, c"writeObjects:", array);
                    success &= written;
                }
                if let Some(png) = png {
                    let written: bool = send2(b, c"setData:forType:", png, nsstring("public.png"));
                    success &= written;
                }
                if let Some(rtf) = rtf {
                    let bytes: Obj = send2(
                        class(c"NSData"),
                        c"dataWithBytes:length:",
                        rtf.as_ptr(),
                        rtf.len(),
                    );
                    let written: bool =
                        send2(b, c"setData:forType:", bytes, nsstring("public.rtf"));
                    success &= written;
                }
                if let Some(text) = text.as_deref() {
                    let written: bool = send2(
                        b,
                        c"setString:forType:",
                        nsstring(text),
                        nsstring("public.utf8-plain-text"),
                    );
                    success &= written;
                }
                SELF_CHANGE.store(count(b), Ordering::Release);
                if !success {
                    anyhow::bail!("macOS pasteboard write failed");
                }
            }
        }
        match mode {
            PasteMode::SetOnly => Ok(()),
            PasteMode::InjectCtrlV => inject_command_v(),
            PasteMode::TypeIn => type_text(text.as_deref().unwrap_or_default()),
        }
    }
}

fn inject_command_v() -> anyhow::Result<()> {
    // ANSI virtual key 9 is V on macOS; flags specify Command held.
    unsafe {
        let down = CGEventCreateKeyboardEvent(ptr::null_mut(), 9, true);
        let up = CGEventCreateKeyboardEvent(ptr::null_mut(), 9, false);
        if down.is_null() || up.is_null() {
            if !down.is_null() {
                CFRelease(down);
            }
            if !up.is_null() {
                CFRelease(up);
            }
            anyhow::bail!("could not create macOS keyboard events");
        }
        CGEventSetFlags(down, 1 << 20);
        CGEventSetFlags(up, 1 << 20);
        CGEventPost(1, down);
        CGEventPost(1, up);
        CFRelease(down);
        CFRelease(up);
    }
    Ok(())
}
fn type_text(text: &str) -> anyhow::Result<()> {
    // Unicode chunks avoid splitting surrogate pairs.
    for c in text.chars() {
        let mut buf = [0u16; 2];
        let units = c.encode_utf16(&mut buf);
        unsafe {
            for down in [true, false] {
                let event = CGEventCreateKeyboardEvent(ptr::null_mut(), 0, down);
                if event.is_null() {
                    anyhow::bail!("could not create macOS keyboard event");
                }
                CGEventKeyboardSetUnicodeString(event, units.len(), units.as_ptr());
                CGEventPost(1, event);
                CFRelease(event);
            }
        }
    }
    Ok(())
}
fn encode_text(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}
fn decode_text(bytes: &[u8]) -> anyhow::Result<String> {
    if bytes.len() % 2 != 0 {
        anyhow::bail!("text contains a truncated UTF-16 code unit");
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|v| u16::from_le_bytes([v[0], v[1]]))
        .take_while(|&u| u != 0)
        .collect();
    Ok(String::from_utf16(&units)?)
}
fn encode_paths(paths: &[String]) -> Vec<u8> {
    let mut out = vec![0; 20];
    out[0..4].copy_from_slice(&20u32.to_le_bytes());
    out[16..20].copy_from_slice(&1u32.to_le_bytes());
    for p in paths {
        out.extend(encode_text(p));
    }
    out.extend_from_slice(&[0, 0]);
    out
}
fn decode_paths(bytes: &[u8]) -> anyhow::Result<Vec<String>> {
    if bytes.len() < 24 {
        anyhow::bail!("invalid file-drop payload");
    }
    let offset = u32::from_le_bytes(bytes[0..4].try_into()?) as usize;
    let wide = u32::from_le_bytes(bytes[16..20].try_into()?);
    if offset < 20
        || offset > bytes.len()
        || wide != 1
        || offset % 2 != 0
        || (bytes.len() - offset) % 2 != 0
    {
        anyhow::bail!("unsupported or truncated file-drop payload");
    }
    let units: Vec<u16> = bytes[offset..]
        .chunks_exact(2)
        .map(|v| u16::from_le_bytes([v[0], v[1]]))
        .collect();
    if !units.ends_with(&[0, 0]) {
        anyhow::bail!("file-drop path list is not terminated");
    }
    let mut paths = Vec::new();
    for path in units.split(|u| *u == 0) {
        if path.is_empty() {
            break;
        }
        paths.push(String::from_utf16(path)?);
    }
    if paths.is_empty() {
        anyhow::bail!("file-drop contains no paths");
    }
    Ok(paths)
}

/// Identify the frontmost application before the history popup takes focus.
pub fn foreground_target() -> Option<isize> {
    let _pool = Pool::new();
    unsafe {
        let workspace: Obj = send0(class(c"NSWorkspace"), c"sharedWorkspace");
        let app: Obj = send0(workspace, c"frontmostApplication");
        if app.is_null() {
            return None;
        }
        let pid: i32 = send0(app, c"processIdentifier");
        (pid > 0).then_some(pid as isize)
    }
}

/// Bring the previously active application forward before injecting keys.
pub fn activate_target(target: isize) -> anyhow::Result<()> {
    let pid = i32::try_from(target)?;
    if pid <= 0 {
        anyhow::bail!("invalid target application");
    }
    let _pool = Pool::new();
    unsafe {
        let app: Obj = send1(
            class(c"NSRunningApplication"),
            c"runningApplicationWithProcessIdentifier:",
            pid,
        );
        if app.is_null() {
            anyhow::bail!("target application has exited");
        }
        // NSApplicationActivateIgnoringOtherApps = 1 << 1.
        let ok: bool = send1(app, c"activateWithOptions:", 2usize);
        if !ok {
            anyhow::bail!("could not activate target application");
        }
    }
    Ok(())
}

/// Convert common 8-bit RGB pasteboard images to a real CF_DIBV5 block.
/// Unusual or planar bitmap layouts are skipped rather than mislabelled.
unsafe fn read_image(board: Obj) -> Option<Vec<u8>> {
    unsafe {
        let mut data: Obj = send1(board, c"dataForType:", nsstring("public.png"));
        if data.is_null() {
            data = send1(board, c"dataForType:", nsstring("public.tiff"));
        }
        if data.is_null() {
            return None;
        }
        let rep: Obj = send1(class(c"NSBitmapImageRep"), c"imageRepWithData:", data);
        if rep.is_null() {
            return None;
        }
        let width: isize = send0(rep, c"pixelsWide");
        let height: isize = send0(rep, c"pixelsHigh");
        let bits: isize = send0(rep, c"bitsPerSample");
        let samples: isize = send0(rep, c"samplesPerPixel");
        let row_bytes: isize = send0(rep, c"bytesPerRow");
        let bits_per_pixel: isize = send0(rep, c"bitsPerPixel");
        let planar: bool = send0(rep, c"isPlanar");
        let alpha: bool = send0(rep, c"hasAlpha");
        let flags: usize = send0(rep, c"bitmapFormat");
        let colourspace: Obj = send0(rep, c"colorSpaceName");
        let colourspace = string(colourspace)?;
        if !(colourspace.contains("RGB")
            && bits == 8
            && !planar
            && ((samples == 3 && !alpha) || (samples == 4 && alpha))
            && flags & (0b1111 << 8) == 0
            && bits_per_pixel == samples * 8)
        {
            return None;
        }
        let (w, h, stride) = (
            usize::try_from(width).ok()?,
            usize::try_from(height).ok()?,
            usize::try_from(row_bytes).ok()?,
        );
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return None;
        }
        let samples = samples as usize;
        if stride < w.checked_mul(samples)? {
            return None;
        }
        let raw_len = stride.checked_mul(h)?;
        let pixel_bytes = w.checked_mul(h)?.checked_mul(4)?;
        if pixel_bytes > 128 * 1024 * 1024 {
            return None;
        }
        let raw: *const u8 = send0(rep, c"bitmapData");
        if raw.is_null() {
            return None;
        }
        let raw = std::slice::from_raw_parts(raw, raw_len);
        let mut dib = vec![0u8; 124 + pixel_bytes];
        dib[0..4].copy_from_slice(&124u32.to_le_bytes());
        dib[4..8].copy_from_slice(&(w as i32).to_le_bytes());
        dib[8..12].copy_from_slice(&(h as i32).to_le_bytes()); // bottom-up
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[16..20].copy_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS
        dib[20..24].copy_from_slice(&(pixel_bytes as u32).to_le_bytes());
        for (offset, mask) in [
            (40, 0x00ff0000u32),
            (44, 0x0000ff00),
            (48, 0x000000ff),
            (52, 0xff000000),
            (56, 0x73524742),
        ] {
            dib[offset..offset + 4].copy_from_slice(&mask.to_le_bytes());
        }
        let alpha_first = alpha && flags & 1 != 0;
        let premultiplied = alpha && flags & 2 == 0;
        for y in 0..h {
            let source = &raw[y * stride..y * stride + w * samples];
            let dest = &mut dib[124 + (h - 1 - y) * w * 4..124 + (h - y) * w * 4];
            for (src, dst) in source.chunks_exact(samples).zip(dest.chunks_exact_mut(4)) {
                let a = if alpha {
                    if alpha_first { src[0] } else { src[3] }
                } else {
                    255
                };
                let rgb = if alpha_first { &src[1..4] } else { &src[0..3] };
                let unpremultiply = |v: u8| -> u8 {
                    if premultiplied && a > 0 {
                        ((u32::from(v) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
                    } else {
                        v
                    }
                };
                dst.copy_from_slice(&[
                    unpremultiply(rgb[2]),
                    unpremultiply(rgb[1]),
                    unpremultiply(rgb[0]),
                    a,
                ]);
            }
        }
        Some(dib)
    }
}
