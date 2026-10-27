/// Add a BMP file header to a DIB for NSBitmapImageRep; pixel format and masks
/// remain unchanged. Validate bounds before handing bytes to the OS decoder.
pub fn dib_to_bmp(dib: &[u8]) -> anyhow::Result<Vec<u8>> {
    if dib.len() < 40 {
        anyhow::bail!("DIB header is too short");
    }
    let header = u32::from_le_bytes(dib[0..4].try_into()?) as usize;
    if !matches!(header, 40 | 52 | 56 | 108 | 124) || header > dib.len() {
        anyhow::bail!("unsupported DIB header");
    }
    let bits = u16::from_le_bytes(dib[14..16].try_into()?);
    let compression = u32::from_le_bytes(dib[16..20].try_into()?);
    let colours_used = u32::from_le_bytes(dib[32..36].try_into()?) as usize;
    let colours = if colours_used != 0 {
        colours_used
    } else if bits <= 8 {
        1usize << bits
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
    let pixel_offset = header
        .checked_add(masks)
        .and_then(|v| v.checked_add(colours.checked_mul(4)?))
        .ok_or_else(|| anyhow::anyhow!("DIB offset overflow"))?;
    if pixel_offset > dib.len() {
        anyhow::bail!("DIB pixel offset is out of bounds");
    }
    let file_len = u32::try_from(
        dib.len()
            .checked_add(14)
            .ok_or_else(|| anyhow::anyhow!("DIB size overflow"))?,
    )?;
    let file_offset = u32::try_from(pixel_offset + 14)?;
    let mut bmp = Vec::with_capacity(file_len as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_len.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&file_offset.to_le_bytes());
    bmp.extend_from_slice(dib);
    Ok(bmp)
}
