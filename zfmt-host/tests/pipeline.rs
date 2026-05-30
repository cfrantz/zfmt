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

// EventHeader: U64_PAIR/single, U8/single, UTF8_BYTE|FIXED_ARRAY/3, END
const HDR_TAG:  u32    = 0xe43ae42d;
const HDR_FH:   u64    = 0x5a19e4cfe43ae42d;
const HDR_FMTH: u32    = 0x112d69b2;
const HDR_BC:   &[u8]  = &[0x88, 0x08, 0x49, 0x03, 0x00];

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

    // EventHeader payload: ZfmtU64{lo=42,hi=0}, severity=2 (Info), seq[3]=0
    let mut hdr_payload = vec![0u8; 12];
    hdr_payload[..4].copy_from_slice(&42u32.to_le_bytes()); // timestamp lo
    // timestamp hi = 0
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
// §9.6c — StreamStart sets tick rate; EventHeader timestamps are scaled

const SS_TAG:  u32   = 0x0ef1ba00;  // StreamStart::ZFMT_TAG
const HDR_TAG2: u32  = HDR_TAG;     // alias for clarity below

#[test]
fn pipeline_stream_start_scales_timestamps() {
    let dir = TempDir::new().unwrap();
    let fmt_hash: u32 = 0x99887766;
    let db = make_db(
        &dir,
        &[
            // StreamStart: U16/single, SKIP/fa 2, U64_PAIR/single, U64_PAIR/single, END
            (SS_TAG,   SS_TAG as u64,   0,         &[0x10u8, 0x51, 0x02, 0x88, 0x88, 0x00]),
            (HDR_TAG2, HDR_FH,          fmt_hash,  HDR_BC),
        ],
        &[(fmt_hash, "{timestamp} {severity}")],
    );

    // StreamStart payload: protocol_version=1 (no seq tracking), tick_rate_hz=ZfmtU64(1_000_000)
    let mut ss_payload = vec![0u8; 20];
    ss_payload[..2].copy_from_slice(&1u16.to_le_bytes());
    ss_payload[4..8].copy_from_slice(&1_000_000u32.to_le_bytes());   // tick_rate_hz lo
    // tick_rate_hz hi = 0 (already zero)

    // EventHeader: timestamp = 500_000 ticks → 0.500000 s
    let mut hdr_payload = vec![0u8; 12];
    hdr_payload[..4].copy_from_slice(&500_000u32.to_le_bytes());  // timestamp lo
    // timestamp hi = 0
    hdr_payload[8] = 2; // Info

    let mut stream = frame(SS_TAG, &ss_payload);
    stream.extend_from_slice(&frame(HDR_TAG2, &hdr_payload));

    let output = decode_to_string(&stream, db);
    assert!(output.contains("0.500000"), "expected scaled timestamp: {output:?}");
}

// ---------------------------------------------------------------------------
// §9.6d — StreamStart → EventHeader + event → DroppedEvents sequence

#[test]
fn pipeline_well_known_event_sequence() {
    let dir = TempDir::new().unwrap();
    const DE_TAG:  u32 = 0xe0ee1b4e;
    const DE_FH:   u64 = 0xcb0b57d1e0ee1b4e;
    const DE_BC:   &[u8] = &[0x18u8, 0x51, 0x04, 0x00]; // U32/single, SKIP/fa 4, END
    let fmt_ss: u32 = 0x1111_0000;
    let fmt_hdr: u32 = 0x2222_0000;
    let fmt_de: u32 = 0x3333_0000;
    let db = make_db(
        &dir,
        &[
            (SS_TAG,   SS_TAG as u64,  fmt_ss,  &[0x10u8, 0x51, 0x02, 0x88, 0x88, 0x00]),
            (HDR_TAG,  HDR_FH,         fmt_hdr, HDR_BC),
            (DM_TAG,   DM_FH,          DM_FMTH, DM_BC),
            (DE_TAG,   DE_FH,          fmt_de,  DE_BC),
        ],
        &[
            (fmt_ss,  "stream start tick_rate={1}"),
            (fmt_hdr, "{timestamp} {severity}"),
            (DM_FMTH, "{message}"),
            (fmt_de,  "dropped count={0}"),
        ],
    );

    let mut ss_payload = vec![0u8; 20];
    ss_payload[..2].copy_from_slice(&1u16.to_le_bytes());
    ss_payload[4..8].copy_from_slice(&1_000u32.to_le_bytes()); // tick_rate_hz lo = 1 kHz

    let mut hdr_payload = vec![0u8; 12];
    hdr_payload[..4].copy_from_slice(&1_000u32.to_le_bytes()); // timestamp lo = 1.0 s at 1kHz
    hdr_payload[8] = 2;

    let mut msg_payload = vec![2u8];
    msg_payload.extend_from_slice(b"hi");

    let mut de_payload = vec![0u8; 8];
    de_payload[..4].copy_from_slice(&3u32.to_le_bytes());

    let mut stream = frame(SS_TAG, &ss_payload);
    stream.extend_from_slice(&frame(HDR_TAG, &hdr_payload));
    stream.extend_from_slice(&frame(DM_TAG, &msg_payload));
    stream.extend_from_slice(&frame(DE_TAG, &de_payload));

    let output = decode_to_string(&stream, db);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 4, "expected 4 lines:\n{output}");
    assert!(lines[0].contains("stream start"), "line 0: {}", lines[0]);
    assert!(lines[1].contains("1.000000"), "EventHeader scaled ts: {}", lines[1]);
    assert!(lines[2].contains("hi"), "DebugMessage: {}", lines[2]);
    assert!(lines[3].contains("dropped count=3"), "DroppedEvents: {}", lines[3]);
}

// ---------------------------------------------------------------------------
// §9.6e — DISPATCH instruction: inline enum field decoded end-to-end
//
// The outer event's format string references the dispatch-produced values
// positionally.  Both variants here produce exactly one U32 value so the
// same format string works for both.

#[test]
fn pipeline_dispatch_inline_enum() {
    // Inline enum (repr u8), both variants produce a single U32 value:
    //   Variant 0 → subroutine 0xAABB: U32/single + END  (e.g., "ok code")
    //   Variant 1 → subroutine 0xCCDD: U32/single + END  (e.g., "fault code")
    let ok_tag:    u32 = 0xAABB;
    let fault_tag: u32 = 0xCCDD;
    let fmt_ok:    u32 = 0x1001;
    let fmt_fault: u32 = 0x1002;
    let fmt_outer: u32 = 0x2000;

    let leb = |n: u32| -> Vec<u8> {
        let mut v = Vec::new();
        let mut x = n as u64;
        loop { let b = (x & 0x7f) as u8; x >>= 7;
               if x == 0 { v.push(b); break; } else { v.push(b | 0x80); } }
        v
    };

    // DISPATCH bytecode: opcode=0x70, discrim=U8(1), padding=0, count=2,
    //                    (0, ok_tag), (1, fault_tag), END
    let mut dispatch_bc = vec![0x70u8, 0x01, 0x00, 0x02];
    dispatch_bc.push(0x00); dispatch_bc.extend(leb(ok_tag));
    dispatch_bc.push(0x01); dispatch_bc.extend(leb(fault_tag));
    dispatch_bc.push(0x00); // END

    let outer_tag: u32 = 0x5566;

    // Build and test Ok path.
    let dir = TempDir::new().unwrap();
    let db = make_db(
        &dir,
        &[
            (outer_tag, outer_tag as u64, fmt_outer, dispatch_bc.as_slice()),
            (ok_tag,    ok_tag as u64,    fmt_ok,    &[0x18u8, 0x00]), // U32/single + END
            (fault_tag, fault_tag as u64, fmt_fault, &[0x18u8, 0x00]), // U32/single + END
        ],
        &[
            (fmt_outer, "result={0}"),
            (fmt_ok,    "ok val={0}"),
            (fmt_fault, "fault code={0}"),
        ],
    );
    // Discriminant=0 (ok_tag), then u32 payload = 7
    let mut ok_payload = vec![0u8];
    ok_payload.extend_from_slice(&7u32.to_le_bytes());
    let out_ok = decode_to_string(&frame(outer_tag, &ok_payload), db);
    // The outer format "result={0}" references the single U32 value from the
    // dispatch subroutine (7).
    assert!(out_ok.contains("result=7"), "Ok dispatch: {out_ok:?}");

    // Build and test Fault path.
    let dir2 = TempDir::new().unwrap();
    let db2 = make_db(
        &dir2,
        &[
            (outer_tag, outer_tag as u64, fmt_outer, dispatch_bc.as_slice()),
            (ok_tag,    ok_tag as u64,    fmt_ok,    &[0x18u8, 0x00]),
            (fault_tag, fault_tag as u64, fmt_fault, &[0x18u8, 0x00]),
        ],
        &[
            (fmt_outer, "result={0}"),
            (fmt_ok,    "ok val={0}"),
            (fmt_fault, "fault code={0}"),
        ],
    );
    let mut fault_payload = vec![1u8];
    fault_payload.extend_from_slice(&99u32.to_le_bytes());
    let out_fault = decode_to_string(&frame(outer_tag, &fault_payload), db2);
    assert!(out_fault.contains("result=99"), "Fault dispatch: {out_fault:?}");
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
