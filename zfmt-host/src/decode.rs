//! Binary stream decoder (§6) — wire framing, tag lookup, bytecode interpretation.

use std::io;

use anyhow::{Context, Result};

use crate::db::Db;
use crate::interpret;

// Well-known event tags (§7) — FNV-1a hashes of the canonical struct definitions.
const TAG_STREAM_START:  u32 = 0x0ef1ba00;
const TAG_EVENT_HEADER:  u32 = 0xe43ae42d;

/// Fallback decoder configuration for streams that may not contain a `StreamStart` frame.
///
/// When a `StreamStart` frame is encountered in the stream it always takes precedence,
/// overriding whatever was set here.  These values are used as initial state only
/// when no `StreamStart` has been seen yet.
#[derive(Debug, Clone)]
pub struct DecodeConfig {
    /// Tick rate in Hz for timestamp scaling.  0 = unknown (timestamps shown as raw ticks).
    pub tick_rate_hz: u64,
    /// Protocol version for feature detection.  1 = no seq tracking; 2 = seq tracking.
    pub protocol_version: u16,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self { tick_rate_hz: 0, protocol_version: 1 }
    }
}

/// Decode a binary stream, writing one line per frame to `out`.
///
/// Each frame is: `tag(u32 LE) | LEB128(payload_len) | payload[payload_len]`.
/// Unknown tags are warned to stderr and skipped; decode errors are warned
/// and a placeholder line is written so the rest of the stream continues.
///
/// `config` provides initial state used when the stream does not begin with a
/// `StreamStart` frame.  When a `StreamStart` is encountered it overrides these
/// values for all subsequent frames.
pub fn decode_stream(data: &[u8], databases: &[Db], out: &mut dyn io::Write, config: &DecodeConfig) -> Result<()> {
    let mut pos = 0usize;
    let mut tick_rate_hz: u64 = config.tick_rate_hz;
    // Sequence tracking: active when protocol_version >= 2.
    let mut seq_enabled = config.protocol_version >= 2;
    let mut prev_seq: Option<u32> = None;

    while pos < data.len() {
        if data.len() - pos < 5 {
            eprintln!(
                "warn: {} trailing bytes at offset {pos} — not enough for a frame header",
                data.len() - pos
            );
            break;
        }

        let tag = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        pos += 4;

        let (payload_len, leb_n) = decode_leb128(&data[pos..])
            .with_context(|| format!("LEB128 length at offset {}", pos - 4))?;
        pos += leb_n;

        let payload_len = payload_len as usize;
        if data.len() - pos < payload_len {
            eprintln!(
                "warn: truncated stream: need {payload_len} bytes at offset {pos} \
                 but only {} remain",
                data.len() - pos
            );
            break;
        }
        let payload = &data[pos..pos + payload_len];
        pos += payload_len;

        // Find the event entry (first database wins).
        let result = databases.iter().find_map(|db| {
            db.all_events().ok().and_then(|evts| {
                evts.into_iter().find(|e| e.tag == tag).map(|e| (e, db))
            })
        });

        // Extract metadata from StreamStart before generic decode.
        // StreamStart layout: u16(2) + _pad0(2) + ZfmtU64{lo,hi}(8) + ZfmtU64{lo,hi}(8)
        // protocol_version at [0..2]; tick_rate_hz lo at [4..8], hi at [8..12].
        if tag == TAG_STREAM_START && payload.len() >= 12 {
            let protocol_version = u16::from_le_bytes(payload[0..2].try_into().unwrap());
            seq_enabled = protocol_version >= 2;
            if seq_enabled { prev_seq = None; } // reset on new stream
            let lo = u32::from_le_bytes(payload[4..8].try_into().unwrap()) as u64;
            let hi = u32::from_le_bytes(payload[8..12].try_into().unwrap()) as u64;
            tick_rate_hz = (hi << 32) | lo;
        }

        // Sequence gap detection: EventHeader.seq lives at bytes [9..12].
        // Emit a gap annotation line before the header that follows a drop.
        if tag == TAG_EVENT_HEADER && seq_enabled && payload.len() >= 12 {
            let cur_seq = u32::from_le_bytes([payload[9], payload[10], payload[11], 0]);
            if let Some(prev) = prev_seq {
                let expected = prev.wrapping_add(1) & 0x00FF_FFFF;
                if cur_seq != expected {
                    let dropped = cur_seq.wrapping_sub(prev.wrapping_add(1)) & 0x00FF_FFFF;
                    writeln!(out, "[seq gap: ~{dropped} events dropped]")?;
                }
            }
            prev_seq = Some(cur_seq);
        }

        match result {
            Some((e, db)) => {
                let tag_hex = format!("{:08x}", tag);
                match interpret::interpret(&e.bytecode, payload, db) {
                    Ok(mut values) => {
                        // Scale EventHeader timestamp from ticks to seconds.
                        if tag == TAG_EVENT_HEADER && tick_rate_hz > 0 {
                            if let Some(interpret::Value::U64(ticks)) = values.first() {
                                let secs = *ticks as f64 / tick_rate_hz as f64;
                                values[0] = interpret::Value::F64(secs);
                            }
                        }
                        let fmt_opt = databases.iter().find_map(|d| {
                            d.lookup_string(e.format_hash).ok().flatten()
                        });
                        let line = match fmt_opt {
                            Some(fmt) => match interpret::render(&fmt, &values) {
                                Ok(s) => s,
                                Err(err) => {
                                    eprintln!("warn: render error for {tag_hex}: {err}");
                                    fallback_join(&values)
                                }
                            },
                            None => fallback_join(&values),
                        };
                        writeln!(out, "[{tag_hex}] {line}")?;
                    }
                    Err(err) => {
                        eprintln!("warn: interpret error for {tag_hex}: {err}");
                        writeln!(out, "[{tag_hex}] <decode error> ({payload_len}B payload)")?;
                    }
                }
            }
            None => {
                let frame_start = pos - 4 - leb_n - payload_len;
                eprintln!(
                    "warn: unknown tag {:08x} at offset {frame_start} ({payload_len}B skipped)",
                    tag
                );
            }
        }
    }

    Ok(())
}

fn fallback_join(values: &[interpret::Value]) -> String {
    values.iter().map(|v| v.display_default()).collect::<Vec<_>>().join(" ")
}

/// Decode an unsigned LEB128 integer from `buf`.
/// Returns `(value, bytes_consumed)`.
pub(crate) fn decode_leb128(buf: &[u8]) -> Result<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    for (i, &byte) in buf.iter().enumerate() {
        let low = (byte & 0x7f) as u64;
        value |= low << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        if shift >= 64 {
            anyhow::bail!("LEB128 overflow");
        }
    }
    anyhow::bail!("truncated LEB128");
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn sink() -> impl io::Write { io::sink() }

    #[test]
    fn decode_leb128_single_byte() {
        assert_eq!(decode_leb128(&[0x00]).unwrap(), (0, 1));
        assert_eq!(decode_leb128(&[0x10]).unwrap(), (16, 1));
        assert_eq!(decode_leb128(&[0x7f]).unwrap(), (127, 1));
    }

    #[test]
    fn decode_leb128_multibyte() {
        assert_eq!(decode_leb128(&[0x80, 0x01]).unwrap(), (128, 2));
        assert_eq!(decode_leb128(&[0xAC, 0x02]).unwrap(), (300, 2));
    }

    #[test]
    fn decode_leb128_truncated() {
        assert!(decode_leb128(&[0x80]).is_err());
    }

    #[test]
    fn decode_empty_stream() {
        let db = Db::memory().unwrap();
        assert!(decode_stream(&[], &[db], &mut sink(), &DecodeConfig::default()).is_ok());
    }

    #[test]
    fn decode_known_event_end_only() {
        use crate::elf::EventEntry;
        let mut db = Db::memory().unwrap();
        let tag: u32 = 0x12345678;
        db.ingest(
            &[EventEntry {
                tag, full_hash: tag as u64, format_hash: 0,
                bytecode: vec![0x00],
            }],
            &[],
            0,
        ).unwrap();

        let mut stream = tag.to_le_bytes().to_vec();
        stream.push(0x00); // LEB128 payload_len = 0
        assert!(decode_stream(&stream, &[db], &mut sink(), &DecodeConfig::default()).is_ok());
    }

    #[test]
    fn decode_event_with_format_string() {
        use crate::elf::{EventEntry, StringEntry};
        let mut db = Db::memory().unwrap();
        let tag: u32 = 0xaabbccdd;
        let fmt_hash: u32 = 0x11223344;
        db.ingest(
            &[EventEntry { tag, full_hash: tag as u64, format_hash: fmt_hash,
                           bytecode: vec![0x18, 0x00] }],
            &[StringEntry { hash: fmt_hash, content: "val={x}".to_owned() }],
            0,
        ).unwrap();

        let mut stream = tag.to_le_bytes().to_vec();
        stream.push(0x04); // LEB128(4)
        stream.extend_from_slice(&42u32.to_le_bytes());
        let mut out = Vec::new();
        decode_stream(&stream, &[db], &mut out, &DecodeConfig::default()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("val=42"), "got: {s:?}");
    }

    #[test]
    fn decode_captures_rendered_output() {
        use crate::elf::{EventEntry, StringEntry};
        let mut db = Db::memory().unwrap();
        let tag: u32 = 0x11111111;
        let fmt_hash: u32 = 0x22222222;
        // UTF8_BYTE/var-length + END
        db.ingest(
            &[EventEntry { tag, full_hash: tag as u64, format_hash: fmt_hash,
                           bytecode: vec![0x4b, 0x00] }],
            &[StringEntry { hash: fmt_hash, content: "{msg}".to_owned() }],
            0,
        ).unwrap();

        // Payload: LEB128(5) + "world"
        let mut payload = vec![5u8];
        payload.extend_from_slice(b"world");

        let mut stream = tag.to_le_bytes().to_vec();
        stream.push(payload.len() as u8);
        stream.extend_from_slice(&payload);

        let mut out = Vec::new();
        decode_stream(&stream, &[db], &mut out, &DecodeConfig::default()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s.trim(), "[11111111] world");
    }

    // Helper: build a minimal StreamStart payload (20 bytes, §7.3).
    // Layout: u16(2) + _pad0(2) + ZfmtU64{lo,hi}(8) + ZfmtU64{lo,hi}(8) = 20 bytes.
    fn stream_start_payload(tick_rate_hz: u64, protocol_version: u16) -> Vec<u8> {
        let mut p = vec![0u8; 20];
        p[..2].copy_from_slice(&protocol_version.to_le_bytes());
        p[4..8].copy_from_slice(&(tick_rate_hz as u32).to_le_bytes());      // tick_rate_hz lo
        p[8..12].copy_from_slice(&((tick_rate_hz >> 32) as u32).to_le_bytes()); // tick_rate_hz hi
        p
    }

    // Helper: build an EventHeader payload (12 bytes, §7.2).
    // Layout: ZfmtU64{lo,hi}(8) + u8(1) + seq[u8;3](3) = 12 bytes.
    fn event_header_payload(timestamp_ticks: u64, severity: u8, seq: u32) -> Vec<u8> {
        let mut p = vec![0u8; 12];
        p[..4].copy_from_slice(&(timestamp_ticks as u32).to_le_bytes());      // timestamp lo
        p[4..8].copy_from_slice(&((timestamp_ticks >> 32) as u32).to_le_bytes()); // timestamp hi
        p[8] = severity;
        p[9]  = (seq & 0xFF) as u8;
        p[10] = ((seq >> 8) & 0xFF) as u8;
        p[11] = ((seq >> 16) & 0xFF) as u8;
        p
    }

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

    #[test]
    fn decode_stream_start_sets_tick_rate() {
        let mut db = Db::memory().unwrap();
        let ss_payload = stream_start_payload(1_000_000, 1);
        let ss_frame = frame(TAG_STREAM_START, &ss_payload);

        // StreamStart is not in the DB — it should be skipped gracefully.
        // The tick_rate_hz is still extracted from the raw payload before the DB lookup.
        // Here we verify that a subsequent EventHeader uses the scaled timestamp.
        use crate::elf::{EventEntry, StringEntry};
        let fmt_hash: u32 = 0xaabb1234;
        db.ingest(&[
            EventEntry { tag: TAG_EVENT_HEADER, full_hash: TAG_EVENT_HEADER as u64,
                format_hash: fmt_hash,
                bytecode: vec![0x88, 0x08, 0x49, 0x03, 0x00] },
        ], &[StringEntry { hash: fmt_hash, content: "{timestamp} {severity}".to_owned() }],
        0).unwrap();

        let hdr_payload = event_header_payload(2_000_000, 2, 0); // 2s at 1MHz
        let mut stream = ss_frame;
        stream.extend_from_slice(&frame(TAG_EVENT_HEADER, &hdr_payload));

        let mut out = Vec::new();
        decode_stream(&stream, &[db], &mut out, &DecodeConfig::default()).unwrap();
        let s = String::from_utf8(out).unwrap();
        // Timestamp should be scaled: 2_000_000 ticks / 1_000_000 Hz = 2.0 s
        assert!(s.contains("2.000000"), "expected scaled timestamp in: {s:?}");
        assert!(s.contains("2"), "expected severity in: {s:?}");
    }

    #[test]
    fn decode_seq_gap_detected() {
        // v2 stream: two EventHeaders with a gap of 3 in seq.
        let mut db = Db::memory().unwrap();
        let fmt_hash: u32 = 0xbeef0001;
        use crate::elf::{EventEntry, StringEntry};
        db.ingest(&[
            EventEntry { tag: TAG_EVENT_HEADER, full_hash: TAG_EVENT_HEADER as u64,
                format_hash: fmt_hash,
                bytecode: vec![0x88, 0x08, 0x49, 0x03, 0x00] },
        ], &[StringEntry { hash: fmt_hash, content: "{timestamp} {severity}".to_owned() }],
        0).unwrap();

        let ss_payload = stream_start_payload(1_000_000, 2); // protocol_version = 2
        let hdr0 = event_header_payload(0, 2, 0);  // seq = 0
        let hdr1 = event_header_payload(4, 2, 4);  // seq = 4 → gap of 3

        let mut stream = frame(TAG_STREAM_START, &ss_payload);
        stream.extend_from_slice(&frame(TAG_EVENT_HEADER, &hdr0));
        stream.extend_from_slice(&frame(TAG_EVENT_HEADER, &hdr1));

        let mut out = Vec::new();
        decode_stream(&stream, &[db], &mut out, &DecodeConfig::default()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("[seq gap: ~3 events dropped]"),
            "expected gap annotation; got:\n{s}");
    }

    #[test]
    fn decode_seq_no_gap_no_annotation() {
        // v2 stream: two consecutive EventHeaders with no gap.
        let mut db = Db::memory().unwrap();
        let fmt_hash: u32 = 0xbeef0002;
        use crate::elf::{EventEntry, StringEntry};
        db.ingest(&[
            EventEntry { tag: TAG_EVENT_HEADER, full_hash: TAG_EVENT_HEADER as u64,
                format_hash: fmt_hash,
                bytecode: vec![0x88, 0x08, 0x49, 0x03, 0x00] },
        ], &[StringEntry { hash: fmt_hash, content: "{timestamp} {severity}".to_owned() }],
        0).unwrap();

        let ss_payload = stream_start_payload(1_000_000, 2);
        let hdr0 = event_header_payload(0, 2, 0);
        let hdr1 = event_header_payload(1, 2, 1); // seq = 1, no gap

        let mut stream = frame(TAG_STREAM_START, &ss_payload);
        stream.extend_from_slice(&frame(TAG_EVENT_HEADER, &hdr0));
        stream.extend_from_slice(&frame(TAG_EVENT_HEADER, &hdr1));

        let mut out = Vec::new();
        decode_stream(&stream, &[db], &mut out, &DecodeConfig::default()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(!s.contains("seq gap"), "unexpected gap annotation; got:\n{s}");
    }

    #[test]
    fn decode_seq_v1_stream_no_tracking() {
        // v1 stream: seq bytes are present but ignored even if they look like a gap.
        let mut db = Db::memory().unwrap();
        let fmt_hash: u32 = 0xbeef0003;
        use crate::elf::{EventEntry, StringEntry};
        db.ingest(&[
            EventEntry { tag: TAG_EVENT_HEADER, full_hash: TAG_EVENT_HEADER as u64,
                format_hash: fmt_hash,
                bytecode: vec![0x88, 0x08, 0x49, 0x03, 0x00] },
        ], &[StringEntry { hash: fmt_hash, content: "{timestamp} {severity}".to_owned() }],
        0).unwrap();

        let ss_payload = stream_start_payload(1_000_000, 1); // protocol_version = 1
        let hdr0 = event_header_payload(0, 2, 0);
        let hdr1 = event_header_payload(1, 2, 99); // seq = 99, would be a gap in v2

        let mut stream = frame(TAG_STREAM_START, &ss_payload);
        stream.extend_from_slice(&frame(TAG_EVENT_HEADER, &hdr0));
        stream.extend_from_slice(&frame(TAG_EVENT_HEADER, &hdr1));

        let mut out = Vec::new();
        decode_stream(&stream, &[db], &mut out, &DecodeConfig::default()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(!s.contains("seq gap"), "v1 stream should not produce gap annotations; got:\n{s}");
    }

    #[test]
    fn decode_seq_wrap_detected() {
        // v2 stream: seq wraps from 0xFFFFFF to 0 — gap of 0, not a drop.
        let mut db = Db::memory().unwrap();
        let fmt_hash: u32 = 0xbeef0004;
        use crate::elf::{EventEntry, StringEntry};
        db.ingest(&[
            EventEntry { tag: TAG_EVENT_HEADER, full_hash: TAG_EVENT_HEADER as u64,
                format_hash: fmt_hash,
                bytecode: vec![0x88, 0x08, 0x49, 0x03, 0x00] },
        ], &[StringEntry { hash: fmt_hash, content: "{timestamp} {severity}".to_owned() }],
        0).unwrap();

        let ss_payload = stream_start_payload(1_000_000, 2);
        let max_seq: u32 = 0x00FF_FFFF;
        let hdr0 = event_header_payload(0, 2, max_seq);
        let hdr1 = event_header_payload(1, 2, 0); // wraps to 0 — expected, no gap

        let mut stream = frame(TAG_STREAM_START, &ss_payload);
        stream.extend_from_slice(&frame(TAG_EVENT_HEADER, &hdr0));
        stream.extend_from_slice(&frame(TAG_EVENT_HEADER, &hdr1));

        let mut out = Vec::new();
        decode_stream(&stream, &[db], &mut out, &DecodeConfig::default()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(!s.contains("seq gap"), "wrap should not be reported as gap; got:\n{s}");
    }

    #[test]
    fn decode_event_header_no_stream_start_raw_ticks() {
        // Without a prior StreamStart, timestamps are shown as raw tick counts.
        let mut db = Db::memory().unwrap();
        let fmt_hash: u32 = 0xccdd5678;
        use crate::elf::{EventEntry, StringEntry};
        db.ingest(&[EventEntry {
            tag: TAG_EVENT_HEADER, full_hash: TAG_EVENT_HEADER as u64,
            format_hash: fmt_hash,
            bytecode: vec![0x20, 0x08, 0x51, 0x07, 0x00],
        }], &[StringEntry { hash: fmt_hash, content: "{timestamp} {severity}".to_owned() }],
        0).unwrap();

        let hdr_payload = event_header_payload(99_000, 3, 0);
        let stream = frame(TAG_EVENT_HEADER, &hdr_payload);
        let mut out = Vec::new();
        decode_stream(&stream, &[db], &mut out, &DecodeConfig::default()).unwrap();

        let s = String::from_utf8(out).unwrap();
        // No scaling — raw tick count 99000 should appear.
        assert!(s.contains("99000"), "expected raw ticks in: {s:?}");
    }

    #[test]
    fn decode_config_fallback_tick_rate() {
        // Stream has no StreamStart; DecodeConfig supplies tick_rate_hz as fallback.
        let mut db = Db::memory().unwrap();
        let fmt_hash: u32 = 0xfb010001;
        use crate::elf::{EventEntry, StringEntry};
        db.ingest(&[
            EventEntry { tag: TAG_EVENT_HEADER, full_hash: TAG_EVENT_HEADER as u64,
                format_hash: fmt_hash,
                bytecode: vec![0x88, 0x08, 0x49, 0x03, 0x00] },
        ], &[StringEntry { hash: fmt_hash, content: "{timestamp} {severity}".to_owned() }],
        0).unwrap();

        let hdr_payload = event_header_payload(1_000_000, 2, 0); // 1s at 1MHz
        let stream = frame(TAG_EVENT_HEADER, &hdr_payload);
        let mut out = Vec::new();
        let config = DecodeConfig { tick_rate_hz: 1_000_000, protocol_version: 1 };
        decode_stream(&stream, &[db], &mut out, &config).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("1.000000"), "expected scaled timestamp from fallback config; got:\n{s}");
    }

    #[test]
    fn decode_config_fallback_seq_tracking() {
        // Stream has no StreamStart; DecodeConfig enables seq tracking as fallback.
        let mut db = Db::memory().unwrap();
        let fmt_hash: u32 = 0xfb020001;
        use crate::elf::{EventEntry, StringEntry};
        db.ingest(&[
            EventEntry { tag: TAG_EVENT_HEADER, full_hash: TAG_EVENT_HEADER as u64,
                format_hash: fmt_hash,
                bytecode: vec![0x88, 0x08, 0x49, 0x03, 0x00] },
        ], &[StringEntry { hash: fmt_hash, content: "{timestamp} {severity}".to_owned() }],
        0).unwrap();

        let hdr0 = event_header_payload(0, 2, 0);
        let hdr1 = event_header_payload(1, 2, 5); // gap of 4

        let mut stream = frame(TAG_EVENT_HEADER, &hdr0);
        stream.extend_from_slice(&frame(TAG_EVENT_HEADER, &hdr1));
        let mut out = Vec::new();
        let config = DecodeConfig { tick_rate_hz: 0, protocol_version: 2 };
        decode_stream(&stream, &[db], &mut out, &config).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("[seq gap: ~4 events dropped]"),
            "expected gap annotation from fallback config; got:\n{s}");
    }
}
