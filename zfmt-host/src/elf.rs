//! ELF section parser for `.zfmt_events` and `.zfmt_strings` (§8.1, §8.2).

use anyhow::{bail, Context, Result};

// ---------------------------------------------------------------------------
// Entry types

/// Parsed representation of one `.zfmt_events.<hex>` section entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventEntry {
    pub tag: u32,
    pub full_hash: u64,
    pub format_hash: u32,
    pub bytecode: Vec<u8>,
}

/// Parsed representation of one `.zfmt_strings.<hex>` section entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringEntry {
    pub hash: u32,
    pub content: String,
}

// ---------------------------------------------------------------------------
// ELF parsing

/// Parse all `.zfmt_events.*` sections from an ELF binary, returning one
/// `EventEntry` per section found.
pub fn parse_events(data: &[u8]) -> Result<Vec<EventEntry>> {
    use object::{Object, ObjectSection};
    let file = object::File::parse(data)
        .context("failed to parse ELF")?;
    let mut out = Vec::new();
    for section in file.sections() {
        let name = section.name().context("section name")?;
        if name.starts_with(".zfmt_events.") {
            let bytes = section.data().context("section data")?;
            let entry = parse_event_entry_bytes(bytes)
                .with_context(|| format!("section {name}"))?;
            out.push(entry);
        }
    }
    Ok(out)
}

/// Parse all `.zfmt_strings.*` sections from an ELF binary.
pub fn parse_strings(data: &[u8]) -> Result<Vec<StringEntry>> {
    use object::{Object, ObjectSection};
    let file = object::File::parse(data)
        .context("failed to parse ELF")?;
    let mut out = Vec::new();
    for section in file.sections() {
        let name = section.name().context("section name")?;
        if name.starts_with(".zfmt_strings.") {
            let bytes = section.data().context("section data")?;
            let entry = parse_string_entry_bytes(bytes)
                .with_context(|| format!("section {name}"))?;
            out.push(entry);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Raw byte parsers (public for testing)

/// Parse one event entry from its raw section bytes.
///
/// Entry layout (§8.1):
///   tag(u32) _pad(u32) full_hash(u64) format_hash(u32) _pad(u32) bc_len(u32) bytecode[padded]
pub fn parse_event_entry_bytes(data: &[u8]) -> Result<EventEntry> {
    const HDR: usize = 28; // 4+4+8+4+4+4
    if data.len() < HDR {
        bail!("event entry too short: {} bytes (need {})", data.len(), HDR);
    }
    let tag         = u32::from_le_bytes(data[0..4].try_into().unwrap());
    // _pad at [4..8]
    let full_hash   = u64::from_le_bytes(data[8..16].try_into().unwrap());
    let format_hash = u32::from_le_bytes(data[16..20].try_into().unwrap());
    // _pad at [20..24]
    let bc_len      = u32::from_le_bytes(data[24..28].try_into().unwrap()) as usize;
    if data.len() < HDR + bc_len {
        bail!(
            "event entry truncated: section is {} bytes but bc_len={bc_len}",
            data.len()
        );
    }
    Ok(EventEntry { tag, full_hash, format_hash, bytecode: data[HDR..HDR + bc_len].to_vec() })
}

/// Parse one string entry from its raw section bytes.
///
/// Entry layout (§8.2):
///   hash(u32) len(u16) _pad(u16) bytes[padded]
pub fn parse_string_entry_bytes(data: &[u8]) -> Result<StringEntry> {
    const HDR: usize = 8; // 4+2+2
    if data.len() < HDR {
        bail!("string entry too short: {} bytes (need {})", data.len(), HDR);
    }
    let hash    = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let str_len = u16::from_le_bytes(data[4..6].try_into().unwrap()) as usize;
    // _pad at [6..8]
    if data.len() < HDR + str_len {
        bail!(
            "string entry truncated: section is {} bytes but str_len={str_len}",
            data.len()
        );
    }
    let content = std::str::from_utf8(&data[HDR..HDR + str_len])
        .context("string entry content is not valid UTF-8")?
        .to_owned();
    Ok(StringEntry { hash, content })
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    // Build a valid event entry byte slice.
    fn event_bytes(tag: u32, full_hash: u64, format_hash: u32, bytecode: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());      // _pad
        v.extend_from_slice(&full_hash.to_le_bytes());
        v.extend_from_slice(&format_hash.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());      // _pad
        v.extend_from_slice(&(bytecode.len() as u32).to_le_bytes());
        v.extend_from_slice(bytecode);
        // pad to 4-byte boundary (optional — parser uses bc_len)
        while v.len() % 4 != 0 { v.push(0); }
        v
    }

    // Build a valid string entry byte slice.
    fn string_bytes(hash: u32, content: &str) -> Vec<u8> {
        let b = content.as_bytes();
        let mut v = Vec::new();
        v.extend_from_slice(&hash.to_le_bytes());
        v.extend_from_slice(&(b.len() as u16).to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes()); // _pad
        v.extend_from_slice(b);
        while v.len() % 4 != 0 { v.push(0); }
        v
    }

    #[test]
    fn round_trip_event_entry() {
        let bc = &[0x20u8, 0x08, 0x51, 0x07, 0x00];
        let raw = event_bytes(0xdeadbeef, 0xcafe000000000001, 0x1234abcd, bc);
        let e = parse_event_entry_bytes(&raw).unwrap();
        assert_eq!(e.tag, 0xdeadbeef);
        assert_eq!(e.full_hash, 0xcafe000000000001);
        assert_eq!(e.format_hash, 0x1234abcd);
        assert_eq!(e.bytecode, bc);
    }

    #[test]
    fn round_trip_string_entry() {
        let raw = string_bytes(0xabcd1234, "hello world");
        let s = parse_string_entry_bytes(&raw).unwrap();
        assert_eq!(s.hash, 0xabcd1234);
        assert_eq!(s.content, "hello world");
    }

    #[test]
    fn event_entry_too_short() {
        assert!(parse_event_entry_bytes(&[0u8; 4]).is_err());
    }

    #[test]
    fn string_entry_too_short() {
        assert!(parse_string_entry_bytes(&[0u8; 4]).is_err());
    }

    #[test]
    fn event_entry_truncated_bytecode() {
        let bc = &[0x20u8, 0x08, 0x00];
        let mut raw = event_bytes(1, 1, 0, bc);
        // Claim bc_len is larger than what's actually there.
        let bc_len_offset = 24usize;
        raw[bc_len_offset..bc_len_offset + 4].copy_from_slice(&100u32.to_le_bytes());
        assert!(parse_event_entry_bytes(&raw).is_err());
    }

    #[test]
    fn string_entry_invalid_utf8() {
        let mut raw = string_bytes(1, "ok");
        // Corrupt the string bytes.
        raw[8] = 0xff;
        raw[9] = 0xfe;
        assert!(parse_string_entry_bytes(&raw).is_err());
    }

    #[test]
    fn event_entry_ignores_tail_padding() {
        // bc_len = 1, but section has 4 bytes of bytecode (3 are padding)
        let raw = event_bytes(1, 2, 3, &[0x00u8]);
        // pad already at 32 bytes; verify we only get 1 byte of bytecode
        let e = parse_event_entry_bytes(&raw).unwrap();
        assert_eq!(e.bytecode, &[0x00u8]);
    }
}
