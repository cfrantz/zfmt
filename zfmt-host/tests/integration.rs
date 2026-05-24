//! Integration tests: entry parsing, database, and companion export round-trips.

use tempfile::TempDir;
use zfmt_host::{
    db::Db,
    elf::{parse_event_entry_bytes, parse_string_entry_bytes},
};

// ---------------------------------------------------------------------------
// Entry byte-level round-trip (no real ELF needed)

#[test]
fn event_and_string_round_trip() {
    // Build raw bytes matching the entry format.
    let bc: &[u8] = &[0x20, 0x08, 0x51, 0x07, 0x00];
    let mut raw = Vec::new();
    raw.extend_from_slice(&0xdeadbeefu32.to_le_bytes()); // tag
    raw.extend_from_slice(&0u32.to_le_bytes());           // _pad
    raw.extend_from_slice(&0xcafe000000000001u64.to_le_bytes()); // full_hash
    raw.extend_from_slice(&0x1234abcdu32.to_le_bytes()); // format_hash
    raw.extend_from_slice(&0u32.to_le_bytes());           // _pad
    raw.extend_from_slice(&(bc.len() as u32).to_le_bytes()); // bc_len
    raw.extend_from_slice(bc);
    while raw.len() % 4 != 0 { raw.push(0); }

    let e = parse_event_entry_bytes(&raw).unwrap();
    assert_eq!(e.tag, 0xdeadbeef);
    assert_eq!(e.full_hash, 0xcafe000000000001);
    assert_eq!(e.format_hash, 0x1234abcd);
    assert_eq!(e.bytecode, bc);

    // String entry.
    let mut sraw = Vec::new();
    let content = b"hello {x}";
    sraw.extend_from_slice(&0xabcd1234u32.to_le_bytes());
    sraw.extend_from_slice(&(content.len() as u16).to_le_bytes());
    sraw.extend_from_slice(&0u16.to_le_bytes()); // _pad
    sraw.extend_from_slice(content);
    while sraw.len() % 4 != 0 { sraw.push(0); }

    let s = parse_string_entry_bytes(&sraw).unwrap();
    assert_eq!(s.hash, 0xabcd1234);
    assert_eq!(s.content, "hello {x}");
}

// ---------------------------------------------------------------------------
// DB + export round-trip (temp file)

#[test]
fn db_ingest_export_roundtrip() {
    use zfmt_host::elf::{EventEntry, StringEntry};

    let dir = TempDir::new().unwrap();
    let mut db = Db::create(&dir.path().join("test.db")).unwrap();
    let fmt_hash = 0x524fb994u32;
    let events = vec![EventEntry {
        tag: 0xa1a6a340,
        full_hash: 0xcef2c6c3a1a6a340,
        format_hash: fmt_hash,
        bytecode: vec![0x4b, 0x00],
    }];
    let strings = vec![StringEntry {
        hash: fmt_hash,
        content: "{message}".to_owned(),
    }];

    db.ingest(&events, &strings, 0).unwrap();

    // Verify round-trip through all_events / all_strings.
    let got_events  = db.all_events().unwrap();
    let got_strings = db.all_strings().unwrap();
    assert_eq!(got_events.len(), 1);
    assert_eq!(got_strings.len(), 1);
    assert_eq!(got_events[0].tag, 0xa1a6a340);
    assert_eq!(got_strings[0].content, "{message}");

    // Verify export contains expected lines.
    let text = zfmt_host::export::render(&got_events, &got_strings, &db).unwrap();
    assert!(text.contains("[event a1a6a340]"));
    assert!(text.contains("full_hash   = cef2c6c3a1a6a340"));
    assert!(text.contains("format      = {message}"));
    assert!(text.contains("bytecode    = 4b 00"));
    assert!(text.contains("[string 524fb994]"));
}

// ---------------------------------------------------------------------------
// DB create/open/merge via on-disk files

#[test]
fn db_create_open_merge() {
    use zfmt_host::elf::EventEntry;

    let dir = TempDir::new().unwrap();
    let db1_path = dir.path().join("db1.db");
    let db2_path = dir.path().join("db2.db");

    let e1 = EventEntry { tag: 0x1111, full_hash: 0x1111, format_hash: 0, bytecode: vec![0x00] };
    let e2 = EventEntry { tag: 0x2222, full_hash: 0x2222, format_hash: 0, bytecode: vec![0x00] };

    {
        let mut db1 = Db::create(&db1_path).unwrap();
        db1.ingest(&[e1.clone()], &[], 1).unwrap();
    }
    {
        let mut db2 = Db::create(&db2_path).unwrap();
        db2.ingest(&[e2.clone()], &[], 2).unwrap();
    }

    // Merge db1 → db2.
    let db1_ro = Db::open(&db1_path).unwrap();
    let mut db2 = Db::open(&db2_path).unwrap();
    let stats = db2.merge_from(&db1_ro).unwrap();
    assert_eq!(stats.events_added, 1);

    let all = db2.all_events().unwrap();
    assert_eq!(all.len(), 2);
}

// ---------------------------------------------------------------------------
// Companion export file on disk

#[test]
fn companion_export_written() {
    use zfmt_host::elf::{EventEntry, StringEntry};

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("events.db");
    let export_path = db_path.with_extension("db.txt");

    let mut db = Db::create(&db_path).unwrap();
    db.ingest(
        &[EventEntry {
            tag: 0x1234, full_hash: 0xabcd_1234, format_hash: 0,
            bytecode: vec![0x18, 0x00],
        }],
        &[StringEntry { hash: 0, content: "unused".to_owned() }],
        0,
    ).unwrap();
    db.write_export(&export_path).unwrap();

    let text = std::fs::read_to_string(&export_path).unwrap();
    assert!(text.contains("[event 00001234]"));
    assert!(text.contains("[string 00000000]"));
    assert!(text.contains("content     = unused"));
}
