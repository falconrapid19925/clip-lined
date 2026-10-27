//! Conversion between Linux payloads and ClipDiary Windows clipboard blocks.
#![cfg(target_os = "linux")]
use anyhow::{Context, Result, bail};
use std::path::PathBuf;

pub fn files_to_hdrop(paths: &[PathBuf]) -> Result<Vec<u8>> {
    if paths.is_empty() {
        bail!("empty file list");
    }
    let mut data = vec![0u8; 20];
    data[0..4].copy_from_slice(&20u32.to_le_bytes());
    data[16..20].copy_from_slice(&1u32.to_le_bytes()); // DROPFILES.fWide
    for path in paths {
        let text = path.to_str().context("file path is not Unicode")?;
        if text.contains('\0') {
            bail!("file path contains NUL");
        }
        for unit in text.encode_utf16().chain(std::iter::once(0)) {
            data.extend_from_slice(&unit.to_le_bytes());
        }
    }
    data.extend_from_slice(&[0, 0]);
    Ok(data)
}

pub fn hdrop_to_files(data: &[u8]) -> Result<Vec<PathBuf>> {
    if data.len() < 20 {
        bail!("truncated DROPFILES header");
    }
    let off = u32::from_le_bytes(data[0..4].try_into()?) as usize;
    let wide = u32::from_le_bytes(data[16..20].try_into()?);
    if off < 20 || off >= data.len() {
        bail!("invalid DROPFILES offset");
    }
    if wide == 0 {
        bail!("non-Unicode DROPFILES is unsupported");
    }
    if !(data.len() - off).is_multiple_of(2) {
        bail!("invalid DROPFILES length");
    }
    let units: Vec<u16> = data[off..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    if !units.ends_with(&[0, 0]) {
        bail!("unterminated DROPFILES list");
    }
    let mut paths = Vec::new();
    for path in units.split(|&x| x == 0) {
        if path.is_empty() {
            break;
        }
        paths.push(PathBuf::from(
            String::from_utf16(path).context("invalid Unicode file path")?,
        ));
    }
    if paths.is_empty() {
        bail!("empty DROPFILES list");
    }
    Ok(paths)
}

pub fn rgba_to_dib(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>> {
    let width: u32 = width.try_into()?;
    let height: u32 = height.try_into()?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 64_000_000 {
        bail!("invalid or oversized image");
    }
    if (width as usize)
        .checked_mul(height as usize)
        .and_then(|v| v.checked_mul(4))
        != Some(rgba.len())
    {
        bail!("invalid image dimensions");
    }
    let mut bmp = Vec::new();
    image::codecs::bmp::BmpEncoder::new(&mut bmp).encode(
        rgba,
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(bmp[14..].to_vec())
}

pub fn dib_to_rgba(dib: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    if dib.len() < 40 {
        bail!("truncated DIB");
    }
    let header = u32::from_le_bytes(dib[0..4].try_into()?) as usize;
    if header < 40 || header > dib.len() {
        bail!("unsupported DIB header");
    }
    let width = i32::from_le_bytes(dib[4..8].try_into()?);
    let height = i32::from_le_bytes(dib[8..12].try_into()?);
    if width <= 0
        || height == 0
        || u64::from(width as u32) * u64::from(height.unsigned_abs()) > 64_000_000
    {
        bail!("invalid or oversized DIB");
    }
    let bpp = u16::from_le_bytes(dib[14..16].try_into()?);
    let compression = u32::from_le_bytes(dib[16..20].try_into()?);
    let colors = u32::from_le_bytes(dib[32..36].try_into()?) as usize;
    let palette = if colors > 0 {
        colors
    } else if bpp <= 8 {
        1usize << bpp
    } else {
        0
    };
    let masks = if header == 40 && compression == 3 {
        12
    } else if header == 40 && compression == 6 {
        16
    } else {
        0
    };
    let offset = 14usize
        .checked_add(header)
        .and_then(|v| v.checked_add(masks))
        .and_then(|v| v.checked_add(palette.checked_mul(4)?))
        .context("invalid DIB size")?;
    if offset > dib.len() + 14 {
        bail!("truncated DIB palette");
    }
    let size: u32 = (dib.len() + 14).try_into()?;
    let offset: u32 = offset.try_into()?;
    let mut bmp = Vec::with_capacity(size as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&size.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&offset.to_le_bytes());
    bmp.extend_from_slice(dib);
    let image = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp)?.into_rgba8();
    Ok((
        image.width() as usize,
        image.height() as usize,
        image.into_raw(),
    ))
}

/// Translate PNG clipboard data into a ClipDiary-compatible CF_DIB block.
pub fn png_to_dib(data: &[u8]) -> Result<Vec<u8>> {
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(data), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode()?.into_rgba8();
    if u64::from(image.width()) * u64::from(image.height()) > 64_000_000 {
        bail!("oversized image");
    }
    rgba_to_dib(
        image.width() as usize,
        image.height() as usize,
        image.as_raw(),
    )
}

pub fn dib_to_png(data: &[u8]) -> Result<Vec<u8>> {
    use image::ImageEncoder;
    let (width, height, rgba) = dib_to_rgba(data)?;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        &rgba,
        width.try_into()?,
        height.try_into()?,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(png)
}

pub fn uri_list_to_files(data: &[u8]) -> Result<Vec<PathBuf>> {
    let text = std::str::from_utf8(data)?;
    let mut paths = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        let url = url::Url::parse(line).context("invalid file URL")?;
        paths.push(
            url.to_file_path()
                .map_err(|_| anyhow::anyhow!("clipboard URI is not a local file"))?,
        );
    }
    if paths.is_empty() {
        bail!("empty file list");
    }
    Ok(paths)
}

pub fn files_to_uri_list(paths: &[PathBuf]) -> Result<Vec<u8>> {
    if paths.is_empty() {
        bail!("empty file list");
    }
    let mut result = String::new();
    for path in paths {
        let url = url::Url::from_file_path(path)
            .map_err(|_| anyhow::anyhow!("file path must be absolute"))?;
        result.push_str(url.as_str());
        result.push_str("\r\n");
    }
    Ok(result.into_bytes())
}
