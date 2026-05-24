//! Companion plaintext export (§9.3).

use crate::db::Db;
use crate::elf::{EventEntry, StringEntry};
use anyhow::Result;

/// Render the companion export text for a set of events and strings.
///
/// Format (§9.3):
/// ```text
/// # zfmt event database export
/// # generated <ISO 8601>
///
/// [event <tag-hex>]
/// full_hash = <hex u64>
/// format    = <format string, if available>
/// bytecode  = <space-separated hex bytes>
///
/// [string <hash-hex>]
/// content = <string>
/// ```
pub fn render(events: &[EventEntry], strings: &[StringEntry], db: &Db) -> Result<String> {
    let now = crate::db::secs_to_iso8601(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    );

    let mut out = std::string::String::new();
    out.push_str("# zfmt event database export\n");
    out.push_str(&format!("# generated {now}\n"));

    for e in events {
        out.push('\n');
        out.push_str(&format!("[event {:08x}]\n", e.tag));
        out.push_str(&format!("full_hash   = {:016x}\n", e.full_hash));
        if let Ok(Some(fmt)) = db.lookup_string(e.format_hash) {
            out.push_str(&format!("format      = {fmt}\n"));
        }
        let bc_hex: Vec<String> = e.bytecode.iter().map(|b| format!("{b:02x}")).collect();
        out.push_str(&format!("bytecode    = {}\n", bc_hex.join(" ")));
    }

    for s in strings {
        out.push('\n');
        out.push_str(&format!("[string {:08x}]\n", s.hash));
        out.push_str(&format!("content     = {}\n", s.content));
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::{EventEntry, StringEntry};

    #[test]
    fn render_basic() {
        let mut db = Db::memory().unwrap();
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

        let text = render(&events, &strings, &db).unwrap();
        assert!(text.contains("# zfmt event database export"));
        assert!(text.contains("[event a1a6a340]"));
        assert!(text.contains("full_hash   = cef2c6c3a1a6a340"));
        assert!(text.contains("format      = {message}"));
        assert!(text.contains("bytecode    = 4b 00"));
        assert!(text.contains("[string 524fb994]"));
        assert!(text.contains("content     = {message}"));
    }

    #[test]
    fn render_no_format_string() {
        let db = Db::memory().unwrap();
        let events = vec![EventEntry {
            tag: 0x1234,
            full_hash: 0xabcd_1234,
            format_hash: 0, // no format
            bytecode: vec![0x18, 0x00],
        }];
        let text = render(&events, &[], &db).unwrap();
        assert!(!text.contains("format"));
        assert!(text.contains("bytecode    = 18 00"));
    }

    #[test]
    fn render_empty() {
        let db = Db::memory().unwrap();
        let text = render(&[], &[], &db).unwrap();
        assert!(text.starts_with("# zfmt event database export\n"));
    }
}
