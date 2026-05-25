//! Binary stream decoder (§6) — wire framing, tag lookup, bytecode interpretation.

use std::io;

use anyhow::{Context, Result};

use crate::db::Db;
use crate::interpret;

// Well-known event tags (§7) — stable, spec-computed FNV-1a hashes.
const TAG_STREAM_START:  u32 = 0x9e106a38;
const TAG_EVENT_HEADER:  u32 = 0x640003d2;

/// Decode a binary stream, writing one line per frame to `out`.
///
/// Each frame is: `tag(u32 LE) | LEB128(payload_len) | payload[payload_len]`.
/// Unknown tags are warned to stderr and skipped; decode errors are warned
/// and a placeholder line is written so the rest of the stream continues.
///
/// When a `StreamStart` frame is encountered its `tick_rate_hz` field is used
/// to scale subsequent `EventHeader` timestamps from firmware ticks to seconds.
pub fn decode_stream(data: &[u8], databases: &[Db], out: &mut dyn io::Write) -> Result<()> {
    let mut pos = 0usize;
    // Updated when a StreamStart frame is parsed; 0 means "rate unknown".
    let mut tick_rate_hz: u64 = 0;

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

        // Extract tick_rate_hz from StreamStart before generic decode.
        if tag == TAG_STREAM_START && payload.len() >= 16 {
            tick_rate_hz = u64::from_le_bytes(payload[8..16].try_into().unwrap());
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
        assert!(decode_stream(&[], &[db], &mut sink()).is_ok());
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
        assert!(decode_stream(&stream, &[db], &mut sink()).is_ok());
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
        decode_stream(&stream, &[db], &mut out).unwrap();
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
        decode_stream(&stream, &[db], &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s.trim(), "[11111111] world");
    }

    // Helper: build a minimal StreamStart payload (24 bytes, §7.3).
    fn stream_start_payload(tick_rate_hz: u64) -> Vec<u8> {
        let mut p = vec![0u8; 24];
        p[..2].copy_from_slice(&1u16.to_le_bytes());       // protocol_version = 1
        p[8..16].copy_from_slice(&tick_rate_hz.to_le_bytes());
        p
    }

    // Helper: build an EventHeader payload (16 bytes, §7.2).
    fn event_header_payload(timestamp_ticks: u64, severity: u8) -> Vec<u8> {
        let mut p = vec![0u8; 16];
        p[..8].copy_from_slice(&timestamp_ticks.to_le_bytes());
        p[8] = severity;
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
        let ss_payload = stream_start_payload(1_000_000);
        let ss_frame = frame(TAG_STREAM_START, &ss_payload);

        // StreamStart is not in the DB — it should be skipped gracefully.
        // The tick_rate_hz is still extracted from the raw payload before the DB lookup.
        // Here we verify that a subsequent EventHeader uses the scaled timestamp.
        use crate::elf::{EventEntry, StringEntry};
        let fmt_hash: u32 = 0xaabb1234;
        db.ingest(&[
            EventEntry { tag: TAG_EVENT_HEADER, full_hash: TAG_EVENT_HEADER as u64,
                format_hash: fmt_hash,
                bytecode: vec![0x20, 0x08, 0x51, 0x07, 0x00] },
        ], &[StringEntry { hash: fmt_hash, content: "{timestamp} {severity}".to_owned() }],
        0).unwrap();

        let hdr_payload = event_header_payload(2_000_000, 2); // 2s at 1MHz
        let mut stream = ss_frame;
        stream.extend_from_slice(&frame(TAG_EVENT_HEADER, &hdr_payload));

        let mut out = Vec::new();
        decode_stream(&stream, &[db], &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        // Timestamp should be scaled: 2_000_000 ticks / 1_000_000 Hz = 2.0 s
        assert!(s.contains("2.000000"), "expected scaled timestamp in: {s:?}");
        assert!(s.contains("2"), "expected severity in: {s:?}");
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

        let hdr_payload = event_header_payload(99_000, 3);
        let stream = frame(TAG_EVENT_HEADER, &hdr_payload);
        let mut out = Vec::new();
        decode_stream(&stream, &[db], &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        // No scaling — raw tick count 99000 should appear.
        assert!(s.contains("99000"), "expected raw ticks in: {s:?}");
    }
}
