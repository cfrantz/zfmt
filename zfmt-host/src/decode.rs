//! Binary stream decoder (§6) — wire framing, tag lookup, bytecode interpretation.

use std::io;

use anyhow::{Context, Result};

use crate::db::Db;
use crate::interpret;

/// Decode a binary stream, writing one line per frame to `out`.
///
/// Each frame is: `tag(u32 LE) | LEB128(payload_len) | payload[payload_len]`.
/// Unknown tags are warned to stderr and skipped; decode errors are warned
/// and a placeholder line is written so the rest of the stream continues.
pub fn decode_stream(data: &[u8], databases: &[Db], out: &mut dyn io::Write) -> Result<()> {
    let mut pos = 0usize;

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

        match result {
            Some((e, db)) => {
                let tag_hex = format!("{:08x}", tag);
                match interpret::interpret(&e.bytecode, payload, db) {
                    Ok(values) => {
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
}
