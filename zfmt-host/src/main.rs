//! `zfmt` CLI — host tooling (§15).

use std::path::PathBuf;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use zfmt_host::{db::Db, decode, elf};

// ---------------------------------------------------------------------------
// CLI definition

#[derive(Parser)]
#[command(name = "zfmt", about = "zfmt host tooling — ELF ingest, stream decode, database management")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Extract events from an ELF and ingest them into the database (§15.1)
    Ingest {
        /// Path to the SQLite database
        #[arg(long, default_value = "zfmt/events.db")]
        database: PathBuf,
        /// Firmware ELF to parse
        elf: PathBuf,
        /// firmware_build_id to record (hex u64); defaults to 0
        #[arg(long, default_value = "0")]
        build_id: String,
    },

    /// Read a binary event stream and print human-readable output (§15.2)
    Decode {
        /// Database(s) to search for event definitions (may be repeated)
        #[arg(long = "database")]
        databases: Vec<PathBuf>,
        /// Binary stream file to decode
        stream: PathBuf,
        /// Fallback tick rate in Hz for timestamp scaling, used when the stream
        /// contains no StreamStart frame (or the StreamStart was not captured).
        #[arg(long)]
        tick_rate_hz: Option<u64>,
        /// Fallback protocol version, used when the stream contains no StreamStart
        /// frame. 1 = no sequence tracking (default); 2 = sequence gap detection.
        #[arg(long, default_value = "1")]
        protocol_version: u16,
    },

    /// Verify every event in an ELF is present in the database (§15.3)
    Verify {
        /// Path to the SQLite database
        #[arg(long, default_value = "zfmt/events.db")]
        database: PathBuf,
        /// Firmware ELF to check
        elf: PathBuf,
    },

    /// Database management subcommands
    Db {
        #[command(subcommand)]
        cmd: DbCommand,
    },
}

#[derive(Subcommand)]
enum DbCommand {
    /// Create a new empty database (§15.4)
    Create {
        /// Path for the new database file
        path: PathBuf,
    },

    /// Merge all entries from src into dst (§15.5)
    Merge {
        /// Source database
        src: PathBuf,
        /// Destination database
        dst: PathBuf,
    },

    /// Print all events in the database in companion-export format (§15.6)
    List {
        /// Path to the SQLite database
        #[arg(long, default_value = "zfmt/events.db")]
        database: PathBuf,
    },
}

// ---------------------------------------------------------------------------
// Entry point

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Ingest { database, elf: elf_path, build_id } => {
            cmd_ingest(database, elf_path, build_id)
        }
        Command::Decode { databases, stream, tick_rate_hz, protocol_version } => {
            cmd_decode(databases, stream, tick_rate_hz, protocol_version)
        }
        Command::Verify { database, elf: elf_path } => {
            cmd_verify(database, elf_path)
        }
        Command::Db { cmd } => match cmd {
            DbCommand::Create { path } => cmd_db_create(path),
            DbCommand::Merge  { src, dst } => cmd_db_merge(src, dst),
            DbCommand::List   { database } => cmd_db_list(database),
        },
    }
}

// ---------------------------------------------------------------------------
// Command implementations

fn cmd_ingest(db_path: PathBuf, elf_path: PathBuf, build_id_hex: String) -> Result<()> {
    let elf_data = std::fs::read(&elf_path)
        .with_context(|| format!("read {}", elf_path.display()))?;

    let events  = elf::parse_events(&elf_data)
        .with_context(|| format!("parse events from {}", elf_path.display()))?;
    let strings = elf::parse_strings(&elf_data)
        .with_context(|| format!("parse strings from {}", elf_path.display()))?;

    eprintln!(
        "parsed {} event(s) and {} string(s) from {}",
        events.len(), strings.len(), elf_path.display()
    );

    let build_id = u64::from_str_radix(build_id_hex.trim_start_matches("0x"), 16)
        .with_context(|| format!("parse build-id '{build_id_hex}'"))?;

    let mut db = Db::open(&db_path)?;
    let stats = db.ingest(&events, &strings, build_id)?;

    eprintln!(
        "events: {} added, {} skipped  |  strings: {} added, {} skipped",
        stats.events_added, stats.events_skipped,
        stats.strings_added, stats.strings_skipped
    );

    // Regenerate companion export.
    let export_path = db_path.with_extension("db.txt");
    db.write_export(&export_path)?;
    eprintln!("companion export written to {}", export_path.display());

    Ok(())
}

fn cmd_decode(db_paths: Vec<PathBuf>, stream_path: PathBuf, tick_rate_hz: Option<u64>, protocol_version: u16) -> Result<()> {
    let dbs: Vec<Db> = if db_paths.is_empty() {
        // Default database.
        let p = PathBuf::from("zfmt/events.db");
        if p.exists() {
            vec![Db::open(&p)?]
        } else {
            vec![]
        }
    } else {
        db_paths.iter().map(|p| Db::open(p)).collect::<Result<_>>()?
    };

    let data = std::fs::read(&stream_path)
        .with_context(|| format!("read {}", stream_path.display()))?;

    let config = decode::DecodeConfig {
        tick_rate_hz: tick_rate_hz.unwrap_or(0),
        protocol_version,
    };
    decode::decode_stream(&data, &dbs, &mut std::io::stdout(), &config)
}

fn cmd_verify(db_path: PathBuf, elf_path: PathBuf) -> Result<()> {
    let elf_data = std::fs::read(&elf_path)
        .with_context(|| format!("read {}", elf_path.display()))?;
    let events = elf::parse_events(&elf_data)?;
    let db = Db::open(&db_path)?;
    let missing = db.verify(&events)?;
    if missing.is_empty() {
        eprintln!("ok: all {} event(s) verified", events.len());
        Ok(())
    } else {
        for tag in &missing {
            eprintln!("missing: tag {tag}");
        }
        anyhow::bail!("{} event(s) missing from database", missing.len())
    }
}

fn cmd_db_create(path: PathBuf) -> Result<()> {
    Db::create(&path)?;
    eprintln!("created {}", path.display());
    Ok(())
}

fn cmd_db_merge(src_path: PathBuf, dst_path: PathBuf) -> Result<()> {
    let src = Db::open(&src_path)?;
    let mut dst = Db::open(&dst_path)?;
    let stats = dst.merge_from(&src)?;
    eprintln!(
        "events: {} added, {} skipped  |  strings: {} added, {} skipped",
        stats.events_added, stats.events_skipped,
        stats.strings_added, stats.strings_skipped
    );
    Ok(())
}

fn cmd_db_list(db_path: PathBuf) -> Result<()> {
    let db = Db::open(&db_path)?;
    let events  = db.all_events()?;
    let strings = db.all_strings()?;
    let text = zfmt_host::export::render(&events, &strings, &db)?;
    print!("{text}");
    Ok(())
}
