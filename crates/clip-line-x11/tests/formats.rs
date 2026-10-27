#![cfg(target_os = "linux")]
use clip_line_x11::formats::{
    dib_to_rgba, files_to_hdrop, files_to_uri_list, hdrop_to_files, rgba_to_dib, uri_list_to_files,
};
use std::path::PathBuf;

#[test]
fn file_drop_round_trip_keeps_unicode_paths() {
    let paths = vec![
        PathBuf::from("/tmp/one file.txt"),
        PathBuf::from("/tmp/文件.png"),
    ];
    let block = files_to_hdrop(&paths).unwrap();
    assert_eq!(hdrop_to_files(&block).unwrap(), paths);
    assert!(hdrop_to_files(&block[..block.len() - 2]).is_err());
}

#[test]
fn dib_round_trip_keeps_pixels() {
    let pixels = [
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];
    let block = rgba_to_dib(2, 2, &pixels).unwrap();
    let (width, height, decoded) = dib_to_rgba(&block).unwrap();
    assert_eq!((width, height), (2, 2));
    assert_eq!(decoded, pixels);
    assert!(dib_to_rgba(&block[..20]).is_err());
}

#[test]
fn uri_list_round_trip_escapes_spaces_and_unicode() {
    let paths = vec![
        PathBuf::from("/tmp/one file.txt"),
        PathBuf::from("/tmp/文件.png"),
    ];
    let list = files_to_uri_list(&paths).unwrap();
    assert_eq!(uri_list_to_files(&list).unwrap(), paths);
    assert!(uri_list_to_files(b"https://example.org/file").is_err());
}
