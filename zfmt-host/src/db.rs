//! SQLite database for accumulated event and string metadata (§9).

use std::path::Path;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};

use crate::elf::{EventEntry, StringEntry};
use crate::export;

// ---------------------------------------------------------------------------
// Schema

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS events (
    tag         TEXT NOT NULL PRIMARY KEY,  -- hex u32
    full_hash   TEXT NOT NULL UNIQUE,       -- hex u64
    format_hash TEXT NOT NULL,              -- hex u32, FK into strings
    bytecode    BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS strings (
    hash    TEXT NOT NULL PRIMARY KEY,      -- hex u32
    content TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS ingested_builds (
    build_id    TEXT NOT NULL PRIMARY KEY,  -- hex u64
    ingested_at TEXT NOT NULL               -- ISO 8601
);
"#;

// ---------------------------------------------------------------------------
// Database handle

pub struct Db {
    conn: Connection,
}

impl Db {
    /// Create a new empty database at `path`.  Errors if the file already exists.
    pub fn create(path: &Path) -> Result<Self> {
        if path.exists() {
            bail!("database already exists: {}", path.display());
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create directory {}", parent.display()))?;
            }
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open {}", path.display()))?;
        conn.execute_batch(SCHEMA).context("create schema")?;
        Ok(Self { conn })
    }

    /// Open an existing database at `path`, or create one if it doesn't exist.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create directory {}", parent.display()))?;
            }
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open {}", path.display()))?;
        conn.execute_batch(SCHEMA).context("ensure schema")?;
        Ok(Self { conn })
    }

    /// Open an in-memory database (for tests).
    #[cfg(test)]
    pub fn memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("open :memory:")?;
        conn.execute_batch(SCHEMA).context("create schema")?;
        Ok(Self { conn })
    }

    // -----------------------------------------------------------------------
    // Ingest

    /// Ingest `events` and `strings` from an ELF identified by `build_id`
    /// (typically `StreamStart.firmware_build_id`).
    ///
    /// Uses a single transaction so the database is either fully updated or
    /// left unchanged on any error.  Re-ingesting an identical entry is
    /// silently skipped.
    pub fn ingest(
        &mut self,
        events: &[EventEntry],
        strings: &[StringEntry],
        build_id: u64,
    ) -> Result<IngestStats> {
        let tx = self.conn.transaction().context("begin transaction")?;
        let mut stats = IngestStats::default();

        // Strings first (events reference them via format_hash).
        for s in strings {
            let hash_hex = format!("{:08x}", s.hash);
            let existing: Option<String> = tx
                .query_row(
                    "SELECT content FROM strings WHERE hash = ?1",
                    params![hash_hex],
                    |row| row.get(0),
                )
                .optional()
                .context("query string")?;
            match existing {
                Some(c) if c == s.content => {
                    stats.strings_skipped += 1;
                }
                Some(c) => {
                    bail!(
                        "collision: string hash {hash_hex} already exists with different content\
                        \n  existing: {c:?}\
                        \n  new:      {:?}",
                        s.content
                    );
                }
                None => {
                    tx.execute(
                        "INSERT INTO strings (hash, content) VALUES (?1, ?2)",
                        params![hash_hex, s.content],
                    )
                    .context("insert string")?;
                    stats.strings_added += 1;
                }
            }
        }

        // Events.
        for e in events {
            let tag_hex  = format!("{:08x}", e.tag);
            let fh_hex   = format!("{:016x}", e.full_hash);
            let fmt_hex  = format!("{:08x}", e.format_hash);

            // Check by full_hash first (idempotent re-ingest check).
            let by_hash: Option<String> = tx
                .query_row(
                    "SELECT tag FROM events WHERE full_hash = ?1",
                    params![fh_hex],
                    |row| row.get(0),
                )
                .optional()
                .context("query by full_hash")?;

            if let Some(existing_tag) = by_hash {
                if existing_tag == tag_hex {
                    stats.events_skipped += 1;
                    continue;
                }
                bail!(
                    "collision: full_hash {fh_hex} exists with tag {existing_tag} but new entry has tag {tag_hex}"
                );
            }

            // Check tag uniqueness (different full_hash → wire collision).
            let by_tag: Option<String> = tx
                .query_row(
                    "SELECT full_hash FROM events WHERE tag = ?1",
                    params![tag_hex],
                    |row| row.get(0),
                )
                .optional()
                .context("query by tag")?;

            if let Some(existing_fh) = by_tag {
                bail!(
                    "wire collision: tag {tag_hex} already mapped to full_hash {existing_fh} \
                     but new entry has full_hash {fh_hex}"
                );
            }

            tx.execute(
                "INSERT INTO events (tag, full_hash, format_hash, bytecode) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![tag_hex, fh_hex, fmt_hex, e.bytecode],
            )
            .context("insert event")?;
            stats.events_added += 1;
        }

        // Record the build.
        let build_hex = format!("{:016x}", build_id);
        let now = now_iso8601();
        tx.execute(
            "INSERT OR IGNORE INTO ingested_builds (build_id, ingested_at) VALUES (?1, ?2)",
            params![build_hex, now],
        )
        .context("insert build")?;

        tx.commit().context("commit")?;
        Ok(stats)
    }

    // -----------------------------------------------------------------------
    // Check (read-only collision validation)

    /// Validate `events` and `strings` against the database without writing.
    ///
    /// Checks for:
    /// - Same-ELF collisions: two entries in `events` with the same 32-bit tag
    ///   but different full hashes.
    /// - Cross-database wire collisions: an entry's 32-bit tag matches an
    ///   existing database entry with a different full hash.
    /// - Full-hash collisions: an entry's 64-bit full hash matches an existing
    ///   database entry with a different tag.
    /// - String hash collisions: a string's hash matches an existing entry with
    ///   different content.
    ///
    /// Returns `Ok(CheckStats)` if no collisions are found.  Reports how many
    /// entries are new (would be added by `ingest`) vs already present.
    pub fn check(
        &self,
        events:  &[EventEntry],
        strings: &[StringEntry],
    ) -> Result<CheckStats> {
        let mut stats = CheckStats::default();

        // Same-ELF collision check: scan the input slice for duplicate tags.
        let mut seen: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
        for e in events {
            match seen.get(&e.tag) {
                Some(&fh) if fh != e.full_hash => bail!(
                    "same-build collision in ELF: tag {:08x} appears with \
                     full_hashes {:016x} and {:016x}",
                    e.tag, fh, e.full_hash
                ),
                Some(_) => {} // identical duplicate — treat as one entry
                None => { seen.insert(e.tag, e.full_hash); }
            }
        }

        // String collision check against database.
        for s in strings {
            let hash_hex = format!("{:08x}", s.hash);
            let existing: Option<String> = self.conn
                .query_row(
                    "SELECT content FROM strings WHERE hash = ?1",
                    params![hash_hex],
                    |row| row.get(0),
                )
                .optional()
                .context("query string")?;
            match existing {
                Some(c) if c == s.content => stats.strings_existing += 1,
                Some(c) => bail!(
                    "string hash collision: {hash_hex} already exists with different content\
                    \n  existing: {c:?}\
                    \n  new:      {:?}",
                    s.content
                ),
                None => stats.strings_new += 1,
            }
        }

        // Event collision check against database.
        for e in events {
            let tag_hex = format!("{:08x}", e.tag);
            let fh_hex  = format!("{:016x}", e.full_hash);

            // Check by full_hash (idempotent re-ingest path).
            let by_hash: Option<String> = self.conn
                .query_row(
                    "SELECT tag FROM events WHERE full_hash = ?1",
                    params![fh_hex],
                    |row| row.get(0),
                )
                .optional()
                .context("query by full_hash")?;

            if let Some(existing_tag) = by_hash {
                if existing_tag == tag_hex {
                    stats.events_existing += 1;
                    continue;
                }
                bail!(
                    "full-hash collision: full_hash {fh_hex} exists with tag {existing_tag} \
                     but ELF entry has tag {tag_hex}"
                );
            }

            // Check by tag (wire collision).
            let by_tag: Option<String> = self.conn
                .query_row(
                    "SELECT full_hash FROM events WHERE tag = ?1",
                    params![tag_hex],
                    |row| row.get(0),
                )
                .optional()
                .context("query by tag")?;

            if let Some(existing_fh) = by_tag {
                bail!(
                    "wire collision: tag {tag_hex} already mapped to full_hash {existing_fh} \
                     but ELF entry has full_hash {fh_hex}"
                );
            }

            stats.events_new += 1;
        }

        Ok(stats)
    }

    // -----------------------------------------------------------------------
    // Verify

    /// Check that every event in `events` is already present in the database
    /// with a matching full_hash.  Returns a list of missing tag hex strings.
    pub fn verify(&self, events: &[EventEntry]) -> Result<Vec<String>> {
        let mut missing = Vec::new();
        for e in events {
            let tag_hex = format!("{:08x}", e.tag);
            let fh_hex  = format!("{:016x}", e.full_hash);
            let found: Option<String> = self.conn
                .query_row(
                    "SELECT full_hash FROM events WHERE tag = ?1",
                    params![tag_hex],
                    |row| row.get(0),
                )
                .optional()
                .context("query by tag")?;
            match found {
                Some(existing) if existing == fh_hex => {}
                Some(existing) => {
                    bail!(
                        "tag {tag_hex} found but full_hash mismatch: db={existing} elf={fh_hex}"
                    );
                }
                None => missing.push(tag_hex),
            }
        }
        Ok(missing)
    }

    // -----------------------------------------------------------------------
    // Merge

    /// Copy all entries from `src` into `self`, applying the standard
    /// collision policy.
    pub fn merge_from(&mut self, src: &Db) -> Result<MergeStats> {
        let src_events = src.all_events()?;
        let src_strings = src.all_strings()?;
        let stats = self.ingest(&src_events, &src_strings, 0)?;
        Ok(MergeStats {
            events_added:   stats.events_added,
            events_skipped: stats.events_skipped,
            strings_added:  stats.strings_added,
            strings_skipped: stats.strings_skipped,
        })
    }

    // -----------------------------------------------------------------------
    // Query helpers

    pub fn all_events(&self) -> Result<Vec<EventEntry>> {
        let mut stmt = self.conn
            .prepare("SELECT tag, full_hash, format_hash, bytecode FROM events ORDER BY tag")
            .context("prepare")?;
        let rows = stmt.query_map([], |row| {
            let tag_hex:  String = row.get(0)?;
            let fh_hex:   String = row.get(1)?;
            let fmt_hex:  String = row.get(2)?;
            let bytecode: Vec<u8> = row.get(3)?;
            Ok((tag_hex, fh_hex, fmt_hex, bytecode))
        })
        .context("query")?;
        let mut out = Vec::new();
        for row in rows {
            let (tag_hex, fh_hex, fmt_hex, bytecode) = row.context("row")?;
            out.push(EventEntry {
                tag:         u32::from_str_radix(&tag_hex, 16).context("parse tag")?,
                full_hash:   u64::from_str_radix(&fh_hex, 16).context("parse full_hash")?,
                format_hash: u32::from_str_radix(&fmt_hex, 16).context("parse format_hash")?,
                bytecode,
            });
        }
        Ok(out)
    }

    pub fn all_strings(&self) -> Result<Vec<StringEntry>> {
        let mut stmt = self.conn
            .prepare("SELECT hash, content FROM strings ORDER BY hash")
            .context("prepare")?;
        let rows = stmt.query_map([], |row| {
            let hash_hex: String = row.get(0)?;
            let content:  String = row.get(1)?;
            Ok((hash_hex, content))
        })
        .context("query")?;
        let mut out = Vec::new();
        for row in rows {
            let (hash_hex, content) = row.context("row")?;
            out.push(StringEntry {
                hash: u32::from_str_radix(&hash_hex, 16).context("parse hash")?,
                content,
            });
        }
        Ok(out)
    }

    pub fn lookup_string(&self, hash: u32) -> Result<Option<String>> {
        let hash_hex = format!("{:08x}", hash);
        self.conn
            .query_row(
                "SELECT content FROM strings WHERE hash = ?1",
                params![hash_hex],
                |row| row.get(0),
            )
            .optional()
            .context("lookup string")
    }

    // -----------------------------------------------------------------------
    // Companion export

    /// Write the companion plaintext export to `path` (§9.3).
    pub fn write_export(&self, path: &Path) -> Result<()> {
        let events = self.all_events()?;
        let strings = self.all_strings()?;
        let text = export::render(&events, &strings, self)?;
        std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
    }
}

// ---------------------------------------------------------------------------
// Stats

#[derive(Debug, Default)]
pub struct CheckStats {
    pub events_new:       usize,
    pub events_existing:  usize,
    pub strings_new:      usize,
    pub strings_existing: usize,
}

#[derive(Debug, Default)]
pub struct IngestStats {
    pub events_added:    usize,
    pub events_skipped:  usize,
    pub strings_added:   usize,
    pub strings_skipped: usize,
}

#[derive(Debug, Default)]
pub struct MergeStats {
    pub events_added:    usize,
    pub events_skipped:  usize,
    pub strings_added:   usize,
    pub strings_skipped: usize,
}

// ---------------------------------------------------------------------------
// Timestamp helper

fn now_iso8601() -> String {
    // Compute approximate ISO 8601 UTC from unix epoch without external deps.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    secs_to_iso8601(secs)
}

pub fn secs_to_iso8601(secs: u64) -> String {
    // Days from unix epoch, then decompose into Gregorian date.
    let days = (secs / 86400) as u32;
    let rem = (secs % 86400) as u32;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;

    // Gregorian calendar computation.
    // Shift epoch to 1 Mar 0000 for simpler leap year handling (Rata Die).
    // Days from 1 Jan 1970 → 1 Mar 0000: 719468
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;        // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y   = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp  = (5 * doy + 2) / 153;
    let d   = doy - (153 * mp + 2) / 5 + 1;
    let m   = if mp < 10 { mp + 3 } else { mp - 9 };
    let y   = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

// -----------------------------------------------------------------------
// rusqlite optional helper

trait OptionalExt<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::{EventEntry, StringEntry};

    fn evt(tag: u32, fh: u64, fmth: u32, bc: &[u8]) -> EventEntry {
        EventEntry { tag, full_hash: fh, format_hash: fmth, bytecode: bc.to_vec() }
    }
    fn str_(hash: u32, content: &str) -> StringEntry {
        StringEntry { hash, content: content.to_owned() }
    }

    #[test]
    fn create_and_ingest() {
        let mut db = Db::memory().unwrap();
        let events  = vec![evt(0xaabb, 0xcafe0000_0000aabb, 0x1234, &[0x20, 0x00])];
        let strings = vec![str_(0x1234, "hello {x}")];
        let stats = db.ingest(&events, &strings, 1).unwrap();
        assert_eq!(stats.events_added, 1);
        assert_eq!(stats.strings_added, 1);
    }

    #[test]
    fn idempotent_reingest() {
        let mut db = Db::memory().unwrap();
        let events  = vec![evt(0x1111, 0xcafe_1111, 0, &[0x00])];
        let strings = vec![];
        db.ingest(&events, &strings, 1).unwrap();
        let stats = db.ingest(&events, &strings, 1).unwrap();
        assert_eq!(stats.events_skipped, 1);
        assert_eq!(stats.events_added, 0);
    }

    #[test]
    fn full_hash_collision_rejected() {
        let mut db = Db::memory().unwrap();
        let e1 = evt(0xaaaa, 0xdeadbeef_deadbeef, 0, &[0x00]);
        let e2 = evt(0xbbbb, 0xdeadbeef_deadbeef, 0, &[0x08]); // same fh, different tag
        db.ingest(&[e1], &[], 1).unwrap();
        assert!(db.ingest(&[e2], &[], 2).is_err());
    }

    #[test]
    fn wire_collision_rejected() {
        let mut db = Db::memory().unwrap();
        let e1 = evt(0xcccc, 0x0000_0001, 0, &[0x00]);
        let e2 = evt(0xcccc, 0x0000_0002, 0, &[0x00]); // same tag, different fh
        db.ingest(&[e1], &[], 1).unwrap();
        assert!(db.ingest(&[e2], &[], 2).is_err());
    }

    #[test]
    fn string_collision_rejected() {
        let mut db = Db::memory().unwrap();
        let s1 = str_(0xffff, "original");
        let s2 = str_(0xffff, "different");
        db.ingest(&[], &[s1], 1).unwrap();
        assert!(db.ingest(&[], &[s2], 2).is_err());
    }

    #[test]
    fn verify_present() {
        let mut db = Db::memory().unwrap();
        let e = evt(0x1234, 0xabcd_1234, 0, &[0x00]);
        db.ingest(&[e.clone()], &[], 1).unwrap();
        let missing = db.verify(&[e]).unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn verify_missing() {
        let db = Db::memory().unwrap();
        let e = evt(0x9999, 0xaaaa_9999, 0, &[0x00]);
        let missing = db.verify(&[e]).unwrap();
        assert_eq!(missing, vec!["00009999".to_owned()]);
    }

    #[test]
    fn merge() {
        let mut src = Db::memory().unwrap();
        let mut dst = Db::memory().unwrap();
        src.ingest(&[evt(0x1, 0x1, 0, &[0x00])], &[str_(0x10, "fmt")], 0).unwrap();
        let stats = dst.merge_from(&src).unwrap();
        assert_eq!(stats.events_added, 1);
        assert_eq!(stats.strings_added, 1);
        // Merge again — should skip.
        let stats2 = dst.merge_from(&src).unwrap();
        assert_eq!(stats2.events_skipped, 1);
    }

    #[test]
    fn check_clean_elf() {
        let mut db = Db::memory().unwrap();
        let e = evt(0x1234, 0xabcd_1234, 0, &[0x00]);
        db.ingest(&[e.clone()], &[], 1).unwrap();

        // New event — not in db yet.
        let new = evt(0x5678, 0xabcd_5678, 0, &[0x00]);
        let stats = db.check(&[e.clone(), new], &[]).unwrap();
        assert_eq!(stats.events_existing, 1);
        assert_eq!(stats.events_new, 1);
    }

    #[test]
    fn check_same_elf_collision() {
        let db = Db::memory().unwrap();
        let e1 = evt(0xaaaa, 0x0000_0001, 0, &[0x00]);
        let e2 = evt(0xaaaa, 0x0000_0002, 0, &[0x00]); // same tag, different fh
        assert!(db.check(&[e1, e2], &[]).is_err());
    }

    #[test]
    fn check_wire_collision_with_db() {
        let mut db = Db::memory().unwrap();
        db.ingest(&[evt(0xbbbb, 0x1111_bbbb, 0, &[0x00])], &[], 1).unwrap();
        // New event with same tag but different full_hash.
        let e = evt(0xbbbb, 0x2222_bbbb, 0, &[0x00]);
        assert!(db.check(&[e], &[]).is_err());
    }

    #[test]
    fn check_does_not_modify_db() {
        let mut db = Db::memory().unwrap();
        let e = evt(0xcccc, 0xabcd_cccc, 0, &[0x00]);
        // check with a new event should succeed but not write to db.
        db.check(&[e.clone()], &[]).unwrap();
        // verify the event is still absent from the db.
        let missing = db.verify(&[e]).unwrap();
        assert_eq!(missing.len(), 1, "check should not have written to the database");
    }

    #[test]
    fn secs_to_iso8601_epoch() {
        assert_eq!(secs_to_iso8601(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn secs_to_iso8601_known() {
        // 2026-05-24T00:00:00Z = 1779580800
        assert_eq!(secs_to_iso8601(1779580800), "2026-05-24T00:00:00Z");
    }
}
