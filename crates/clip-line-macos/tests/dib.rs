use clip_line_macos::dib_to_bmp;

#[test]
fn wraps_v5_dib_without_changing_pixels_or_masks() {
    let mut dib = vec![0; 132]; // 124-byte header and two BGRA pixels
    dib[0..4].copy_from_slice(&124u32.to_le_bytes());
    dib[4..8].copy_from_slice(&2i32.to_le_bytes());
    dib[8..12].copy_from_slice(&1i32.to_le_bytes());
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32u16.to_le_bytes());
    dib[16..20].copy_from_slice(&3u32.to_le_bytes());
    dib[40..44].copy_from_slice(&0x00ff0000u32.to_le_bytes());
    dib[124..].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let bmp = dib_to_bmp(&dib).unwrap();
    assert_eq!(&bmp[..2], b"BM");
    assert_eq!(
        u32::from_le_bytes(bmp[2..6].try_into().unwrap()),
        bmp.len() as u32
    );
    assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 138);
    assert_eq!(&bmp[14..], dib);
}

#[test]
fn offsets_account_for_bitfield_masks_and_indexed_palette() {
    let mut bitfields = vec![0; 56];
    bitfields[..4].copy_from_slice(&40u32.to_le_bytes());
    bitfields[14..16].copy_from_slice(&32u16.to_le_bytes());
    bitfields[16..20].copy_from_slice(&3u32.to_le_bytes());
    let bmp = dib_to_bmp(&bitfields).unwrap();
    assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 66);
    let mut indexed = vec![0; 44 + 8];
    indexed[..4].copy_from_slice(&40u32.to_le_bytes());
    indexed[14..16].copy_from_slice(&1u16.to_le_bytes());
    let bmp = dib_to_bmp(&indexed).unwrap();
    assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 62);
}

#[test]
fn rejects_truncated_or_impossible_dib_headers() {
    assert!(dib_to_bmp(&[0; 10]).is_err());
    let mut bad = vec![0; 40];
    bad[..4].copy_from_slice(&124u32.to_le_bytes());
    assert!(dib_to_bmp(&bad).is_err());
    bad[..4].copy_from_slice(&40u32.to_le_bytes());
    bad[14..16].copy_from_slice(&8u16.to_le_bytes());
    assert!(dib_to_bmp(&bad).is_err());
}
