//! Pure-Rust integration tests: build binary streams in Rust, decode them with
//! the library, and verify rendered output.  No subprocesses needed.

use tempfile::TempDir;
use zfmt_host::{
    db::Db,
    decode::decode_stream,
    elf::{EventEntry, StringEntry},
};

// ---------------------------------------------------------------------------
// Known constants for well-known events (§7)

// DebugMessage: UTF8_BYTE/var-length + END
const DM_TAG:   u32    = 0xa1a6a340;
const DM_FH:    u64    = 0xcef2c6c3a1a6a340;
const DM_FMTH:  u32    = 0x524fb994;
const DM_BC:    &[u8]  = &[0x4b, 0x00];

// EventHeader: U64/single, U8/single, SKIP/fa 7, END
const HDR_TAG:  u32    = 0x640003d2;
const HDR_FH:   u64    = 0xfb7e523c640003d2;
const HDR_FMTH: u32    = 0x112d69b2;
const HDR_BC:   &[u8]  = &[0x20, 0x08, 0x51, 0x07, 0x00];

// ---------------------------------------------------------------------------
// Helpers

/// Encode a single wire frame: tag(4) | LEB128(payload_len) | payload.
fn frame(tag: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = tag.to_le_bytes().to_vec();
    let mut n = payload.len() as u64;
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 { v.push(b); break; } else { v.push(b | 0x80); }
    }
    v.extend_from_slice(payload);
    v
}

/// Create a temporary on-disk Db with the given events and strings ingested.
fn make_db(
    dir: &TempDir,
    events:  &[(u32, u64, u32, &[u8])],
    strings: &[(u32, &str)],
) -> Db {
    let path = dir.path().join("test.db");
    let mut db = Db::create(&path).unwrap();
    let evts: Vec<EventEntry> = events.iter().map(|(tag, fh, fmth, bc)| EventEntry {
        tag: *tag, full_hash: *fh, format_hash: *fmth, bytecode: bc.to_vec(),
    }).collect();
    let strs: Vec<StringEntry> = strings.iter().map(|(h, c)| StringEntry {
        hash: *h, content: (*c).to_owned(),
    }).collect();
    db.ingest(&evts, &strs, 0).unwrap();
    db
}

/// Run decode_stream, return stdout as a String.
fn decode_to_string(stream: &[u8], db: Db) -> String {
    let mut out = Vec::new();
    decode_stream(stream, &[db], &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

// ---------------------------------------------------------------------------
// §9.1 — single DebugMessage frame

#[test]
fn pipeline_debug_message_rendered() {
    let dir = TempDir::new().unwrap();
    let db = make_db(
        &dir,
        &[(DM_TAG, DM_FH, DM_FMTH, DM_BC)],
        &[(DM_FMTH, "{message}")],
    );

    // Payload: LEB128(11) + "hello world"
    let mut payload = vec![11u8];
    payload.extend_from_slice(b"hello world");

    let output = decode_to_string(&frame(DM_TAG, &payload), db);
    assert!(output.contains("[a1a6a340]"), "missing tag: {output:?}");
    assert!(output.contains("hello world"), "missing message: {output:?}");
}

// ---------------------------------------------------------------------------
// §9.2 — EventHeader + DebugMessage pair, both frames decoded

#[test]
fn pipeline_header_and_event_pair() {
    let dir = TempDir::new().unwrap();
    let db = make_db(
        &dir,
        &[
            (HDR_TAG,  HDR_FH,  HDR_FMTH, HDR_BC),
            (DM_TAG,   DM_FH,   DM_FMTH,  DM_BC),
        ],
        &[
            (HDR_FMTH, "{timestamp} {severity}"),
            (DM_FMTH,  "{message}"),
        ],
    );

    // EventHeader payload: timestamp=42 u64 LE, severity=2 (Info), _pad[7]
    let mut hdr_payload = vec![0u8; 16];
    hdr_payload[..8].copy_from_slice(&42u64.to_le_bytes());
    hdr_payload[8] = 2; // Info

    // DebugMessage payload: LEB128(2) + "hi"
    let mut msg_payload = vec![2u8];
    msg_payload.extend_from_slice(b"hi");

    let mut stream = frame(HDR_TAG, &hdr_payload);
    stream.extend_from_slice(&frame(DM_TAG, &msg_payload));

    let output = decode_to_string(&stream, db);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 2, "expected 2 lines:\n{output}");
    assert!(lines[0].contains("42"), "EventHeader should have timestamp: {}", lines[0]);
    assert!(lines[1].contains("hi"), "DebugMessage should have message: {}", lines[1]);
}

// ---------------------------------------------------------------------------
// §9.3 — unknown tag produces no output lines (warning goes to stderr only)

#[test]
fn pipeline_unknown_tag_no_output() {
    let dir = TempDir::new().unwrap();
    let db = make_db(&dir, &[], &[]);

    let stream = frame(0xdeadbeef, &[1u8, 2, 3, 4]);
    let mut out = Vec::new();
    decode_stream(&stream, &[db], &mut out).unwrap();
    assert!(out.is_empty(), "unknown tag should produce no stdout: {:?}", out);
}

// ---------------------------------------------------------------------------
// §9.4 — truncated stream is handled gracefully (no panic, returns Ok)

#[test]
fn pipeline_truncated_stream_graceful() {
    let dir = TempDir::new().unwrap();
    let db = make_db(&dir, &[], &[]);

    let mut payload = vec![20u8];
    payload.extend_from_slice(b"this will be cut off");
    let mut stream = frame(DM_TAG, &payload);
    stream.truncate(stream.len() - 8); // lop off the tail

    let mut out = Vec::new();
    assert!(decode_stream(&stream, &[db], &mut out).is_ok());
}

// ---------------------------------------------------------------------------
// §9.5 — multi-field Tier-1 event with format string

#[test]
fn pipeline_multi_field_render() {
    let dir = TempDir::new().unwrap();
    // Synthetic event: U64/single + U32/single + END = [0x20, 0x18, 0x00]
    // Use an arbitrary but consistent format_hash.
    let tag:  u32  = 0x11223344;
    let fmth: u32  = 0xaabbccdd;
    let bc:   &[u8] = &[0x20, 0x18, 0x00];
    let db = make_db(
        &dir,
        &[(tag, tag as u64, fmth, bc)],
        &[(fmth, "ts={ts} val={v}")],
    );

    let mut payload = Vec::new();
    payload.extend_from_slice(&1000u64.to_le_bytes());
    payload.extend_from_slice(&42u32.to_le_bytes());

    let output = decode_to_string(&frame(tag, &payload), db);
    assert!(output.contains("ts=1000"), "got: {output:?}");
    assert!(output.contains("val=42"), "got: {output:?}");
}

// ---------------------------------------------------------------------------
// §9.6 — event with no format string falls back to space-joined values

#[test]
fn pipeline_no_format_string_fallback() {
    let dir = TempDir::new().unwrap();
    let tag: u32 = 0x55667788;
    let bc:  &[u8] = &[0x08, 0x18, 0x00]; // U8/single + U32/single + END
    let db = make_db(
        &dir,
        &[(tag, tag as u64, 0, bc)], // format_hash=0, no format string
        &[],
    );

    let mut payload = Vec::new();
    payload.push(7u8);
    payload.extend_from_slice(&300u32.to_le_bytes());

    let output = decode_to_string(&frame(tag, &payload), db);
    // Should contain both values space-joined
    assert!(output.contains("7"), "got: {output:?}");
    assert!(output.contains("300"), "got: {output:?}");
}

// ---------------------------------------------------------------------------
// §9.6b — STRING_REF field: hash looked up in string table

#[test]
fn pipeline_string_ref_field_rendered() {
    let dir = TempDir::new().unwrap();
    // Bytecode for a one-field event: STRING_REF/single + END
    // STRING_REF opcode = (16 << 3) | 0 = 0x80
    let tag:     u32  = 0xaabbcc11;
    let fmth:    u32  = 0x12345678;
    let bc:      &[u8] = &[0x80, 0x00];
    let str_hash: u32 = 0xdeadbeef;
    let db = make_db(
        &dir,
        &[(tag, tag as u64, fmth, bc)],
        &[
            (fmth,     "{label}"),
            (str_hash, "my interned string"),
        ],
    );

    // Payload: u32 LE hash
    let output = decode_to_string(&frame(tag, &str_hash.to_le_bytes()), db);
    assert!(output.contains("my interned string"), "got: {output:?}");
}

// ---------------------------------------------------------------------------
// §9.7 — multiple events in one stream, all rendered

#[test]
fn pipeline_multiple_frames_in_stream() {
    let dir = TempDir::new().unwrap();
    let db = make_db(
        &dir,
        &[(DM_TAG, DM_FH, DM_FMTH, DM_BC)],
        &[(DM_FMTH, "{message}")],
    );

    let mut stream = Vec::new();
    for msg in &["first", "second", "third"] {
        let mut payload = Vec::new();
        let b = msg.as_bytes();
        payload.push(b.len() as u8);
        payload.extend_from_slice(b);
        stream.extend_from_slice(&frame(DM_TAG, &payload));
    }

    let output = decode_to_string(&stream, db);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 3, "expected 3 lines:\n{output}");
    assert!(output.contains("first"));
    assert!(output.contains("second"));
    assert!(output.contains("third"));
}
