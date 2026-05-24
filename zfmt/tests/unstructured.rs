//! Integration tests for Phase 11: unstructured text logging and zfmt_str!.

use std::sync::{Arc, Mutex};
use zfmt::events::{DebugMessage, EventHeader, Severity};
use zfmt::{Logger, log_fatal};
// log_info/log_warn/log_error are imported below only in tests that use them.

// ---------------------------------------------------------------------------
// Test logger

struct VecLogger {
    ts: u64,
    packets: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Logger for VecLogger {
    fn timestamp(&self) -> u64 { self.ts }
    fn send_vectored(&mut self, bufs: &[&[u8]]) {
        let mut data = vec![];
        for b in bufs { data.extend_from_slice(b); }
        self.packets.lock().unwrap().push(data);
    }
}

fn make_logger(ts: u64) -> (VecLogger, Arc<Mutex<Vec<Vec<u8>>>>) {
    let packets = Arc::new(Mutex::new(vec![]));
    (VecLogger { ts, packets: packets.clone() }, packets)
}

/// Returns (tag, payload, frame_end_offset).
fn parse_frame(data: &[u8]) -> (u32, Vec<u8>, usize) {
    let tag = u32::from_le_bytes(data[..4].try_into().unwrap());
    let (payload_len, leb_n) = zfmt::leb128::decode(&data[4..]).unwrap();
    let end = 4 + leb_n + payload_len as usize;
    (tag, data[4 + leb_n..end].to_vec(), end)
}

/// Decode the text from a DebugMessage payload (LEB128 length + UTF-8 bytes).
fn debug_msg_text(payload: &[u8]) -> String {
    let (msg_len, leb_n) = zfmt::leb128::decode(payload).unwrap();
    std::str::from_utf8(&payload[leb_n..leb_n + msg_len as usize])
        .unwrap()
        .to_owned()
}

// ---------------------------------------------------------------------------
// §4.7 zfmt_str!

#[test]
fn zfmt_str_returns_u32() {
    let h: u32 = zfmt::zfmt_str!("hello");
    assert_ne!(h, 0, "hash should be non-zero");
}

#[test]
fn zfmt_str_same_string_same_hash() {
    let h1: u32 = zfmt::zfmt_str!("same string");
    let h2: u32 = zfmt::zfmt_str!("same string");
    assert_eq!(h1, h2);
}

#[test]
fn zfmt_str_different_strings_different_hash() {
    let h1: u32 = zfmt::zfmt_str!("alpha");
    let h2: u32 = zfmt::zfmt_str!("beta");
    assert_ne!(h1, h2);
}

// ---------------------------------------------------------------------------
// §13.3 Unstructured text events — log_fatal! is always enabled (no cfg gate).
// Uses #[allow(deprecated)] at the function level to suppress the intentional
// deprecation warning for using unstructured logging at fatal severity.

#[allow(deprecated)]
#[test]
fn unstructured_sends_debug_message() {
    let (mut logger, packets) = make_logger(42);
    log_fatal!(logger, "hello world");
    let pkts = packets.lock().unwrap();
    assert_eq!(pkts.len(), 1, "expected exactly one packet");
    let pkt = &pkts[0];
    let (hdr_tag, hdr_payload, hdr_end) = parse_frame(pkt);
    assert_eq!(hdr_tag, EventHeader::ZFMT_TAG);
    assert_eq!(hdr_payload[8], Severity::Fatal as u8, "severity should be Fatal");
    let (evt_tag, evt_payload, _) = parse_frame(&pkt[hdr_end..]);
    assert_eq!(evt_tag, DebugMessage::ZFMT_TAG, "second frame should be DebugMessage");
    assert_eq!(debug_msg_text(&evt_payload), "hello world");
}

#[allow(deprecated)]
#[test]
fn unstructured_with_placeholder() {
    let (mut logger, packets) = make_logger(0);
    let x: u32 = 42;
    log_fatal!(logger, "x={x}");
    let pkts = packets.lock().unwrap();
    let pkt = &pkts[0];
    let (_, _, hdr_end) = parse_frame(pkt);
    let (_, evt_payload, _) = parse_frame(&pkt[hdr_end..]);
    assert_eq!(debug_msg_text(&evt_payload), "x=42");
}

#[allow(deprecated)]
#[test]
fn unstructured_with_named_binding() {
    let (mut logger, packets) = make_logger(0);
    log_fatal!(logger, "val={v} ok", v = 7u32);
    let pkts = packets.lock().unwrap();
    let pkt = &pkts[0];
    let (_, _, hdr_end) = parse_frame(pkt);
    let (_, evt_payload, _) = parse_frame(&pkt[hdr_end..]);
    assert_eq!(debug_msg_text(&evt_payload), "val=7 ok");
}

#[allow(deprecated)]
#[test]
fn unstructured_with_hex_spec() {
    let (mut logger, packets) = make_logger(0);
    let addr: u32 = 0xDEAD_BEEF;
    log_fatal!(logger, "addr={addr:#010x}");
    let pkts = packets.lock().unwrap();
    let pkt = &pkts[0];
    let (_, _, hdr_end) = parse_frame(pkt);
    let (_, evt_payload, _) = parse_frame(&pkt[hdr_end..]);
    assert_eq!(debug_msg_text(&evt_payload), "addr=0xdeadbeef");
}

#[allow(deprecated)]
#[test]
fn unstructured_timestamp_forwarded() {
    let (mut logger, packets) = make_logger(77777);
    log_fatal!(logger, "ts test");
    let pkts = packets.lock().unwrap();
    let (_, hdr_payload, _) = parse_frame(&pkts[0]);
    let ts = u64::from_le_bytes(hdr_payload[..8].try_into().unwrap());
    assert_eq!(ts, 77777);
}

// ---------------------------------------------------------------------------
// log_info! / log_warn! / log_error! unstructured — always enabled by default.

#[allow(deprecated)]
#[test]
fn log_info_literal_sends_debug_message() {
    use zfmt::log_info;
    let (mut logger, packets) = make_logger(0);
    log_info!(logger, "info text");
    let pkts = packets.lock().unwrap();
    let (_, hdr_payload, hdr_end) = parse_frame(&pkts[0]);
    assert_eq!(hdr_payload[8], Severity::Info as u8);
    let (evt_tag, evt_payload, _) = parse_frame(&pkts[0][hdr_end..]);
    assert_eq!(evt_tag, DebugMessage::ZFMT_TAG);
    assert_eq!(debug_msg_text(&evt_payload), "info text");
}

#[allow(deprecated)]
#[test]
fn log_warn_literal_sends_debug_message() {
    use zfmt::log_warn;
    let (mut logger, packets) = make_logger(0);
    log_warn!(logger, "warn text");
    let pkts = packets.lock().unwrap();
    let (_, hdr_payload, hdr_end) = parse_frame(&pkts[0]);
    assert_eq!(hdr_payload[8], Severity::Warn as u8);
    let (_, evt_payload, _) = parse_frame(&pkts[0][hdr_end..]);
    assert_eq!(debug_msg_text(&evt_payload), "warn text");
}

#[allow(deprecated)]
#[test]
fn log_error_literal_sends_debug_message() {
    use zfmt::log_error;
    let (mut logger, packets) = make_logger(0);
    log_error!(logger, "error text");
    let pkts = packets.lock().unwrap();
    let (_, hdr_payload, hdr_end) = parse_frame(&pkts[0]);
    assert_eq!(hdr_payload[8], Severity::Error as u8);
    let (_, evt_payload, _) = parse_frame(&pkts[0][hdr_end..]);
    assert_eq!(debug_msg_text(&evt_payload), "error text");
}

// ---------------------------------------------------------------------------
// log_debug! unstructured — requires log-level-debug feature.

#[cfg(feature = "log-level-debug")]
#[test]
fn log_debug_literal_sends_debug_message() {
    use zfmt::log_debug;
    let (mut logger, packets) = make_logger(0);
    log_debug!(logger, "debug msg");
    let pkts = packets.lock().unwrap();
    assert_eq!(pkts.len(), 1);
    let pkt = &pkts[0];
    let (_, hdr_payload, hdr_end) = parse_frame(pkt);
    assert_eq!(hdr_payload[8], Severity::Debug as u8);
    let (evt_tag, evt_payload, _) = parse_frame(&pkt[hdr_end..]);
    assert_eq!(evt_tag, DebugMessage::ZFMT_TAG);
    assert_eq!(debug_msg_text(&evt_payload), "debug msg");
}

#[cfg(feature = "log-level-debug")]
#[test]
fn log_debug_literal_with_placeholder() {
    use zfmt::log_debug;
    let (mut logger, packets) = make_logger(0);
    let x: u32 = 42;
    log_debug!(logger, "x={x}");
    let pkts = packets.lock().unwrap();
    let pkt = &pkts[0];
    let (_, _, hdr_end) = parse_frame(pkt);
    let (_, evt_payload, _) = parse_frame(&pkt[hdr_end..]);
    assert_eq!(debug_msg_text(&evt_payload), "x=42");
}

#[cfg(feature = "log-level-debug")]
#[test]
fn log_debug_literal_with_named_binding() {
    use zfmt::log_debug;
    let (mut logger, packets) = make_logger(0);
    log_debug!(logger, "val={v} ok", v = 7u32);
    let pkts = packets.lock().unwrap();
    let pkt = &pkts[0];
    let (_, _, hdr_end) = parse_frame(pkt);
    let (_, evt_payload, _) = parse_frame(&pkt[hdr_end..]);
    assert_eq!(debug_msg_text(&evt_payload), "val=7 ok");
}
