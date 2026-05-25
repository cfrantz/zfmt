//! Tests for well-known events (§7) and logging macros (§13).

use zfmt::events::{DebugMessage, DroppedEvents, EventHeader, Severity, StreamStart};
use zfmt::{Format, FormatInto, FormatSpec, Write, Error, ZfmtU64};

// ---------------------------------------------------------------------------
// Writer helper

struct Buf(std::string::String);
impl Write for Buf {
    fn write_str(&mut self, s: &str) -> Result<(), Error> { self.0.push_str(s); Ok(()) }
}
fn render<T: Fn(&mut Buf) -> Result<(), Error>>(f: T) -> std::string::String {
    let mut w = Buf(std::string::String::new());
    f(&mut w).unwrap();
    w.0
}

// ---------------------------------------------------------------------------
// §7.1 Severity

#[test]
fn severity_ordering() {
    assert!(Severity::Trace < Severity::Debug);
    assert!(Severity::Debug < Severity::Info);
    assert!(Severity::Info  < Severity::Warn);
    assert!(Severity::Warn  < Severity::Error);
    assert!(Severity::Error < Severity::Fatal);
}

#[test]
fn severity_discriminants() {
    assert_eq!(Severity::Trace as u8, 0);
    assert_eq!(Severity::Debug as u8, 1);
    assert_eq!(Severity::Info  as u8, 2);
    assert_eq!(Severity::Warn  as u8, 3);
    assert_eq!(Severity::Error as u8, 4);
    assert_eq!(Severity::Fatal as u8, 5);
}

#[test]
fn severity_format() {
    let spec = FormatSpec::default();
    assert_eq!(render(|w| Severity::Trace.fmt(w, spec)), "TRACE");
    assert_eq!(render(|w| Severity::Debug.fmt(w, spec)), "DEBUG");
    assert_eq!(render(|w| Severity::Info .fmt(w, spec)), "INFO");
    assert_eq!(render(|w| Severity::Warn .fmt(w, spec)), "WARN");
    assert_eq!(render(|w| Severity::Error.fmt(w, spec)), "ERROR");
    assert_eq!(render(|w| Severity::Fatal.fmt(w, spec)), "FATAL");
}

// ---------------------------------------------------------------------------
// §7.2 EventHeader

#[test]
fn event_header_tag_nonzero() {
    assert_ne!(EventHeader::ZFMT_TAG, 0);
}

#[test]
fn event_header_full_hash_lower32_is_tag() {
    assert_eq!(EventHeader::ZFMT_FULL_HASH as u32, EventHeader::ZFMT_TAG);
}

#[test]
fn event_header_size() {
    // §7.2: ZfmtU64(8) + severity(1) + _pad(3) = 12
    assert_eq!(core::mem::size_of::<EventHeader>(), 12);
    let hdr = EventHeader::new(ZfmtU64::default(), Severity::Info);
    assert_eq!(hdr.payload_size(), 12);
}

#[test]
fn event_header_zfmt_tag_method() {
    let hdr = EventHeader::new(ZfmtU64::default(), Severity::Info);
    assert_eq!(hdr.zfmt_tag(), EventHeader::ZFMT_TAG);
}

#[test]
fn event_header_serialize_roundtrip() {
    let ts = ZfmtU64::from_u64(0xdeadbeefcafe1234);
    let hdr = EventHeader::new(ts, Severity::Warn);
    let mut buf = [0u8; 12];
    hdr.serialize_into(&mut buf);
    assert_eq!(&buf[..4], &ts.lo.to_le_bytes());
    assert_eq!(&buf[4..8], &ts.hi.to_le_bytes());
    assert_eq!(buf[8], Severity::Warn as u8);
    assert_eq!(&buf[9..12], &[0u8; 3]);
}

#[test]
fn event_header_format_into() {
    let hdr = EventHeader::new(ZfmtU64::from_u64(1000), Severity::Info);
    assert_eq!(render(|w| hdr.format_into(w)), "1000 INFO");
}

#[test]
fn event_header_severity_field() {
    let hdr = EventHeader::new(ZfmtU64::default(), Severity::Fatal);
    assert_eq!(hdr.severity, Severity::Fatal as u8);
}

// ---------------------------------------------------------------------------
// §7.3 StreamStart

#[test]
fn stream_start_tag_nonzero() {
    assert_ne!(StreamStart::ZFMT_TAG, 0);
}

#[test]
fn stream_start_full_hash_lower32_is_tag() {
    assert_eq!(StreamStart::ZFMT_FULL_HASH as u32, StreamStart::ZFMT_TAG);
}

#[test]
fn stream_start_size() {
    // §7.3: protocol_version(2) + _pad0(2) + ZfmtU64(8) + ZfmtU64(8) = 20
    assert_eq!(core::mem::size_of::<StreamStart>(), 20);
    let ss = StreamStart {
        protocol_version: 1, _pad0: [0;2],
        tick_rate_hz: ZfmtU64::from_u64(1_000_000),
        firmware_build_id: ZfmtU64::from_u64(42),
    };
    assert_eq!(ss.payload_size(), 20);
}

#[test]
fn stream_start_serialize() {
    let ss = StreamStart {
        protocol_version: 1,
        _pad0: [0; 2],
        tick_rate_hz: ZfmtU64::from_u64(1_000_000u64),
        firmware_build_id: ZfmtU64::from_u64(0xabcdu64),
    };
    let mut buf = [0u8; 20];
    ss.serialize_into(&mut buf);
    assert_eq!(&buf[..2], &1u16.to_le_bytes());
    assert_eq!(&buf[2..4], &[0u8; 2]);
    assert_eq!(&buf[4..8],  &(1_000_000u32).to_le_bytes());   // tick_rate_hz lo
    assert_eq!(&buf[8..12], &0u32.to_le_bytes());              // tick_rate_hz hi
    assert_eq!(&buf[12..16], &(0xabcdu32).to_le_bytes());      // firmware_build_id lo
    assert_eq!(&buf[16..20], &0u32.to_le_bytes());             // firmware_build_id hi
}

// ---------------------------------------------------------------------------
// §7.4 DroppedEvents

#[test]
fn dropped_events_tag_nonzero() {
    assert_ne!(DroppedEvents::ZFMT_TAG, 0);
}

#[test]
fn dropped_events_full_hash_lower32_is_tag() {
    assert_eq!(DroppedEvents::ZFMT_FULL_HASH as u32, DroppedEvents::ZFMT_TAG);
}

#[test]
fn dropped_events_size() {
    // §7.4: count(4) + _pad(4) = 8
    assert_eq!(core::mem::size_of::<DroppedEvents>(), 8);
    let de = DroppedEvents { count: 5, _pad: [0;4] };
    assert_eq!(de.payload_size(), 8);
}

#[test]
fn dropped_events_serialize() {
    let de = DroppedEvents { count: 42u32, _pad: [0;4] };
    let mut buf = [0u8; 8];
    de.serialize_into(&mut buf);
    assert_eq!(&buf[..4], &42u32.to_le_bytes());
    assert_eq!(&buf[4..8], &[0u8; 4]);
}

// ---------------------------------------------------------------------------
// §7.5 DebugMessage

#[test]
fn debug_message_tag_nonzero() {
    assert_ne!(DebugMessage::ZFMT_TAG, 0);
}

#[test]
fn debug_message_full_hash_lower32_is_tag() {
    assert_eq!(DebugMessage::ZFMT_FULL_HASH as u32, DebugMessage::ZFMT_TAG);
}

#[test]
fn debug_message_payload_size() {
    let msg = DebugMessage { message: "hello" };
    // LEB128(5) + 5 bytes = 1 + 5 = 6
    assert_eq!(msg.payload_size(), 6);
}

#[test]
fn debug_message_serialize() {
    let msg = DebugMessage { message: "hi" };
    let mut buf = [0u8; 8];
    msg.serialize_into(&mut buf);
    // LEB128(2) = 0x02, then 'h', 'i'
    assert_eq!(buf[0], 2);
    assert_eq!(&buf[1..3], b"hi");
}

#[test]
fn debug_message_format_into() {
    let msg = DebugMessage { message: "test message" };
    assert_eq!(render(|w| msg.format_into(w)), "test message");
}

// ---------------------------------------------------------------------------
// Tag distinctness across well-known events

#[test]
fn well_known_tags_distinct() {
    let tags = [
        EventHeader::ZFMT_TAG,
        StreamStart::ZFMT_TAG,
        DroppedEvents::ZFMT_TAG,
        DebugMessage::ZFMT_TAG,
    ];
    for i in 0..tags.len() {
        for j in i+1..tags.len() {
            assert_ne!(tags[i], tags[j], "tags[{}] == tags[{}]", i, j);
        }
    }
}

// ---------------------------------------------------------------------------
// §13 Logging macros

use zfmt::{Logger, log_info, log_warn, log_error, log_fatal};
use zfmt::events::Severity as Sev;
use std::sync::{Arc, Mutex};

struct VecLogger {
    ts: u64,
    packets: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Logger for VecLogger {
    fn timestamp(&self) -> ZfmtU64 { ZfmtU64::from_u64(self.ts) }
    fn send_vectored(&mut self, bufs: &[&[u8]]) {
        let mut data = std::vec::Vec::new();
        for b in bufs { data.extend_from_slice(b); }
        self.packets.lock().unwrap().push(data);
    }
}

fn make_logger(ts: u64) -> (VecLogger, Arc<Mutex<std::vec::Vec<std::vec::Vec<u8>>>>) {
    let packets = Arc::new(Mutex::new(std::vec::Vec::new()));
    (VecLogger { ts, packets: packets.clone() }, packets)
}

fn parse_frame(data: &[u8]) -> (u32, Vec<u8>) {
    let tag = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let (payload_len, leb_n) = zfmt::leb128::decode(&data[4..]).unwrap();
    let payload = data[4 + leb_n..4 + leb_n + payload_len as usize].to_vec();
    (tag, payload)
}

#[test]
fn log_info_sends_two_frames() {
    let (mut logger, packets) = make_logger(12345);
    log_info!(logger, DroppedEvents { count: 7, _pad: [0;4] });
    let pkts = packets.lock().unwrap();
    assert_eq!(pkts.len(), 1);
    let pkt = &pkts[0];
    // Packet contains: header frame + event frame
    let (hdr_tag, hdr_payload) = parse_frame(pkt);
    assert_eq!(hdr_tag, EventHeader::ZFMT_TAG);
    assert_eq!(hdr_payload.len(), 12);
    // timestamp as ZfmtU64: lo at 0..4, hi at 4..8
    let lo = u32::from_le_bytes(hdr_payload[..4].try_into().unwrap()) as u64;
    let hi = u32::from_le_bytes(hdr_payload[4..8].try_into().unwrap()) as u64;
    assert_eq!((hi << 32) | lo, 12345);
    // severity at byte 8
    assert_eq!(hdr_payload[8], Sev::Info as u8);
    // second frame starts after header frame
    let hdr_frame_len = 4 + 1 + 12; // tag(4)+LEB128(1)+payload(12)
    let (evt_tag, evt_payload) = parse_frame(&pkt[hdr_frame_len..]);
    assert_eq!(evt_tag, DroppedEvents::ZFMT_TAG);
    assert_eq!(&evt_payload[..4], &7u32.to_le_bytes());
}

#[test]
fn log_warn_uses_warn_severity() {
    let (mut logger, packets) = make_logger(0);
    log_warn!(logger, DroppedEvents { count: 0, _pad: [0;4] });
    let pkts = packets.lock().unwrap();
    let (_, hdr_payload) = parse_frame(&pkts[0]);
    assert_eq!(hdr_payload[8], Sev::Warn as u8);
}

#[test]
fn log_error_uses_error_severity() {
    let (mut logger, packets) = make_logger(0);
    log_error!(logger, DroppedEvents { count: 0, _pad: [0;4] });
    let pkts = packets.lock().unwrap();
    let (_, hdr_payload) = parse_frame(&pkts[0]);
    assert_eq!(hdr_payload[8], Sev::Error as u8);
}

#[test]
fn log_fatal_always_emits() {
    let (mut logger, packets) = make_logger(99);
    log_fatal!(logger, DroppedEvents { count: 1, _pad: [0;4] });
    let pkts = packets.lock().unwrap();
    assert_eq!(pkts.len(), 1);
    let (_, hdr_payload) = parse_frame(&pkts[0]);
    assert_eq!(hdr_payload[8], Sev::Fatal as u8);
}

#[test]
fn log_tier2_debug_message() {
    let (mut logger, packets) = make_logger(0);
    log_info!(logger, DebugMessage { message: "hello world" });
    let pkts = packets.lock().unwrap();
    let pkt = &pkts[0];
    let hdr_frame_len = 4 + 1 + 12;
    let (evt_tag, evt_payload) = parse_frame(&pkt[hdr_frame_len..]);
    assert_eq!(evt_tag, DebugMessage::ZFMT_TAG);
    // LEB128(11) = 0x0b, then "hello world"
    assert_eq!(evt_payload[0], 11);
    assert_eq!(&evt_payload[1..12], b"hello world");
}
