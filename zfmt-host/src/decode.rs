//! Binary stream decoder (§6, Phase 8 full implementation).
//!
//! Phase 7 skeleton: parses the wire framing and looks up each tag in the
//! database.  Bytecode interpretation and field decoding are Phase 8.

use anyhow::{Context, Result};

use crate::db::Db;

/// Decode a binary stream, printing one line per event to stdout.
///
/// Unknown tags are skipped (their payload bytes are consumed and a warning
/// is printed).
pub fn decode_stream(data: &[u8], databases: &[Db]) -> Result<()> {
    let mut pos = 0usize;

    while pos < data.len() {
        // Minimum: tag(4) + at least one LEB128 byte
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
                "warn: truncated stream: need {payload_len} bytes at offset {pos} but only {} remain",
                data.len() - pos
            );
            break;
        }
        let payload = &data[pos..pos + payload_len];
        pos += payload_len;

        // Look up the tag in the provided databases (first match wins).
        let entry = databases.iter().find_map(|db| {
            db.all_events().ok().and_then(|evts| {
                evts.into_iter().find(|e| e.tag == tag)
            })
        });

        match entry {
            Some(e) => {
                // Phase 7: emit a minimal decoded line.
                // Phase 8 will add full bytecode interpretation.
                let fmt = databases.iter().find_map(|db| {
                    db.lookup_string(e.format_hash).ok().flatten()
                });
                let tag_hex = format!("{:08x}", tag);
                if let Some(f) = fmt {
                    println!("[{tag_hex}] {f} ({payload_len}B payload)");
                } else {
                    println!(
                        "[{tag_hex}] <no format string> ({payload_len}B payload)"
                    );
                }
                // Suppress unused variable warning — payload will be decoded in Phase 8.
                let _ = payload;
            }
            None => {
                eprintln!(
                    "warn: unknown tag {:08x} at offset {} ({payload_len}B skipped)",
                    tag,
                    pos - 4 - leb_n - payload_len
                );
            }
        }
    }

    Ok(())
}

/// Decode an unsigned LEB128 integer from `buf`.
/// Returns `(value, bytes_consumed)`.
fn decode_leb128(buf: &[u8]) -> Result<(u64, usize)> {
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

    #[test]
    fn decode_leb128_single_byte() {
        assert_eq!(decode_leb128(&[0x00]).unwrap(), (0, 1));
        assert_eq!(decode_leb128(&[0x10]).unwrap(), (16, 1));
        assert_eq!(decode_leb128(&[0x7f]).unwrap(), (127, 1));
    }

    #[test]
    fn decode_leb128_multibyte() {
        // 128 = 0x80 0x01
        assert_eq!(decode_leb128(&[0x80, 0x01]).unwrap(), (128, 2));
        // 300 = 0xAC 0x02
        assert_eq!(decode_leb128(&[0xAC, 0x02]).unwrap(), (300, 2));
    }

    #[test]
    fn decode_leb128_truncated() {
        assert!(decode_leb128(&[0x80]).is_err());
    }

    #[test]
    fn decode_empty_stream() {
        let db = Db::memory().unwrap();
        assert!(decode_stream(&[], &[db]).is_ok());
    }

    #[test]
    fn decode_known_event() {
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
        )
        .unwrap();

        // Build a minimal stream: tag(4) + LEB128(0) = 5 bytes
        let mut stream = tag.to_le_bytes().to_vec();
        stream.push(0x00); // payload length = 0
        assert!(decode_stream(&stream, &[db]).is_ok());
    }
}
