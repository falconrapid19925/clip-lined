use clip_line_core::capture::{Block, ClipEvent, Format};
use clip_line_core::store::{Filter, Store, StoreOpts};

fn text_event(s: &str) -> ClipEvent {
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut bytes = Vec::new();
    for u in units {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    bytes.push(0);
    bytes.push(0);
    ClipEvent {
        blocks: vec![Block {
            format: Format::UnicodeText,
            bytes,
        }],
        source_app: "demo".into(),
        source_title: "Demo Window".into(),
    }
}

fn temp_store(tag: &str) -> (Store, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("clipline-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("t.db");
    (Store::open(&p, StoreOpts::default()).unwrap(), p)
}

#[test]
fn append_load_roundtrip() {
    let (store, _p) = temp_store("roundtrip");
    let id = store.append(&text_event("hello clipLine")).unwrap();
    assert!(id > 0);
    let blocks = store.load_data(id as i64).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].format, Format::UnicodeText);
    // decode back
    let units: Vec<u16> = blocks[0]
        .bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let s = String::from_utf16(&units).unwrap();
    assert_eq!(s.trim_end_matches('\0'), "hello clipLine");
}

#[test]
fn dedup_identical_consecutive() {
    let (store, _p) = temp_store("dedup");
    assert!(store.append(&text_event("same")).unwrap() > 0);
    assert_eq!(store.append(&text_event("same")).unwrap(), 0);
    assert!(store.append(&text_event("different")).unwrap() > 0);
}

#[test]
fn classify_url_text_image() {
    let (store, _p) = temp_store("classify");
    let url_id = store
        .append(&text_event("https://example.com/page"))
        .unwrap();
    let rows = store
        .list(&Filter {
            kind: Some(3),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id as u64, url_id);
    assert_eq!(rows[0].url, "https://example.com/page");

    let _txt = store.append(&text_event("plain words here")).unwrap();
    let rows = store
        .list(&Filter {
            kind: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);

    let dib = ClipEvent {
        blocks: vec![Block {
            format: Format::ImageDib,
            bytes: vec![1, 2, 3, 4],
        }],
        source_app: "demo".into(),
        source_title: String::new(),
    };
    let img_id = store.append(&dib).unwrap();
    let rows = store
        .list(&Filter {
            kind: Some(5),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows[0].id as u64, img_id);
}

#[test]
fn title_truncated_to_55() {
    let (store, _p) = temp_store("title55");
    let long = "x".repeat(200);
    store.append(&text_event(&long)).unwrap();
    let rows = store.list(&Filter::default()).unwrap();
    assert_eq!(rows[0].title.chars().count(), 55);
}

#[test]
fn star_search_delete() {
    let (store, _p) = temp_store("ops");
    let a = store.append(&text_event("alpha needle")).unwrap();
    let _b = store.append(&text_event("beta hay")).unwrap();
    store.set_star(a as i64, true).unwrap();

    let f = Filter {
        query: Some("needle".into()),
        ..Default::default()
    };
    assert_eq!(store.list(&f).unwrap().len(), 1);

    let f = Filter {
        starred_only: true,
        ..Default::default()
    };
    assert_eq!(store.list(&f).unwrap().len(), 1);

    store.delete(&[a as i64]).unwrap();
    assert_eq!(store.list(&Filter::default()).unwrap().len(), 1);
}

#[test]
fn import_legacy_preserves_rows() {
    let (src_dir, _) = (
        std::env::temp_dir().join(format!("clipline-legacy-{}", std::process::id())),
        (),
    );
    let _ = std::fs::remove_dir_all(&src_dir);
    std::fs::create_dir_all(&src_dir).unwrap();
    let legacy = src_dir.join("legacy.db");
    {
        let s = Store::open(&legacy, StoreOpts::default()).unwrap();
        s.append(&text_event("legacy row one")).unwrap();
        s.append(&text_event("https://legacy.example.com")).unwrap();
    }
    let (store, _p) = temp_store("import");
    let n = store.import_legacy(&legacy).unwrap();
    assert_eq!(n, 2);
    assert_eq!(store.list(&Filter::default()).unwrap().len(), 2);
}

#[test]
fn gzip_envelope_shape() {
    let bytes = clip_line_core::envelope::pack(&[Block {
        format: Format::UnicodeText,
        bytes: vec![0x41, 0x00],
    }])
    .unwrap();
    assert_eq!(&bytes[..3], &[0x1f, 0x8b, 0x08], "gzip magic");
    let blocks = clip_line_core::envelope::unpack(&bytes).unwrap();
    assert_eq!(blocks[0].bytes, vec![0x41, 0x00]);
}
