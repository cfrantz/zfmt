//! End-to-end CLI tests: build zfmt-testfw, ingest its ELF, generate a
//! binary stream, decode it, and verify the rendered output.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR has no parent")
        .to_owned()
}

fn zfmt() -> &'static str {
    env!("CARGO_BIN_EXE_zfmt")
}

static BUILD_TESTFW: Once = Once::new();

/// Return the path to the compiled `zfmt-testfw` binary, building it first if
/// necessary.  Uses `std::sync::Once` so the build runs at most once per test
/// binary invocation.
fn testfw() -> PathBuf {
    let ws = workspace_root();
    let path = ws.join("target/debug/zfmt-testfw");
    BUILD_TESTFW.call_once(|| {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
        let status = Command::new(&cargo)
            .args(["build", "-p", "zfmt-testfw"])
            .current_dir(&ws)
            .status()
            .expect("could not run `cargo build -p zfmt-testfw`");
        assert!(status.success(), "`cargo build -p zfmt-testfw` failed");
    });
    path
}

/// Run `zfmt ingest` and assert success.
fn run_ingest(db: &Path, elf: &Path) {
    let out = Command::new(zfmt())
        .args(["ingest", "--database", db.to_str().unwrap(), elf.to_str().unwrap()])
        .output()
        .expect("failed to run `zfmt ingest`");
    assert!(
        out.status.success(),
        "`zfmt ingest` failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------
// §15.1 — ingest + companion export

#[test]
fn cli_ingest_companion_export() {
    let fw = testfw();
    let dir = TempDir::new().unwrap();
    let db_path  = dir.path().join("events.db");
    let txt_path = dir.path().join("events.db.txt");

    run_ingest(&db_path, &fw);

    assert!(txt_path.exists(), "companion export not written");
    let text = std::fs::read_to_string(&txt_path).unwrap();
    // Format strings from Heartbeat and TempReading should appear.
    assert!(
        text.contains("heartbeat ts="),
        "Heartbeat format missing from export:\n{text}"
    );
    assert!(
        text.contains("temp celsius_x10="),
        "TempReading format missing from export:\n{text}"
    );
    assert!(
        text.contains("alert critical code="),
        "Alert::Critical format missing from export:\n{text}"
    );
    assert!(
        text.contains("named label="),
        "NamedEvent format missing from export:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// §15.1 — ingest is idempotent (run twice, no error)

#[test]
fn cli_ingest_idempotent() {
    let fw = testfw();
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("events.db");

    run_ingest(&db_path, &fw);
    run_ingest(&db_path, &fw); // second ingest must also succeed
}

// ---------------------------------------------------------------------------
// §15.2 — decode: rendered format strings appear in stdout

#[test]
fn cli_decode_rendered_output() {
    let fw = testfw();
    let dir = TempDir::new().unwrap();
    let db_path     = dir.path().join("events.db");
    let stream_path = dir.path().join("stream.bin");

    // Ingest ELF.
    run_ingest(&db_path, &fw);

    // Generate the test stream.
    let out = Command::new(&fw)
        .arg(stream_path.to_str().unwrap())
        .output()
        .expect("failed to run `zfmt-testfw`");
    assert!(out.status.success(), "zfmt-testfw failed");

    // Decode.
    let out = Command::new(zfmt())
        .args(["decode", "--database", db_path.to_str().unwrap(),
               stream_path.to_str().unwrap()])
        .output()
        .expect("failed to run `zfmt decode`");
    assert!(
        out.status.success(),
        "`zfmt decode` failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let decoded = String::from_utf8_lossy(&out.stdout);
    // Heartbeat timestamp is 1000 ticks at 1 MHz = 0.001000 s in EventHeader.
    // The Heartbeat format string uses its own `timestamp` field (raw ticks).
    assert!(
        decoded.contains("heartbeat ts=1000 up=5000ms"),
        "Heartbeat not rendered:\n{decoded}"
    );
    assert!(
        decoded.contains("temp celsius_x10=215 sensor=3"),
        "TempReading not rendered:\n{decoded}"
    );
    assert!(
        decoded.contains("alert critical code=42"),
        "Alert::Critical not rendered:\n{decoded}"
    );
    assert!(
        decoded.contains("alert warning"),
        "Alert::Warning not rendered:\n{decoded}"
    );
    assert!(
        decoded.contains("firmware node"),
        "NamedEvent label not resolved:\n{decoded}"
    );
    assert!(
        decoded.contains("debug note seq=7"),
        "unstructured log not rendered:\n{decoded}"
    );
}

// ---------------------------------------------------------------------------
// §15.3 — verify: all events present → exit 0

#[test]
fn cli_verify_present() {
    let fw = testfw();
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("events.db");

    run_ingest(&db_path, &fw);

    let out = Command::new(zfmt())
        .args(["verify", "--database", db_path.to_str().unwrap(), fw.to_str().unwrap()])
        .output()
        .expect("failed to run `zfmt verify`");
    assert!(
        out.status.success(),
        "`zfmt verify` reported missing events:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------
// §15.3 — verify: missing events → exit non-zero

#[test]
fn cli_verify_missing_events() {
    let fw = testfw();
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("events.db");

    // Create an empty database — do NOT ingest.
    let out = Command::new(zfmt())
        .args(["db", "create", db_path.to_str().unwrap()])
        .output()
        .expect("failed to run `zfmt db create`");
    assert!(out.status.success());

    let out = Command::new(zfmt())
        .args(["verify", "--database", db_path.to_str().unwrap(), fw.to_str().unwrap()])
        .output()
        .expect("failed to run `zfmt verify`");
    assert!(
        !out.status.success(),
        "`zfmt verify` should have failed but exited 0"
    );
}

// ---------------------------------------------------------------------------
// §15.5 — db merge: events from src appear in dst; verify passes after merge

#[test]
fn cli_db_merge_and_verify() {
    let fw = testfw();
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("src.db");
    let dst_path = dir.path().join("dst.db");

    // Ingest into src.
    run_ingest(&src_path, &fw);

    // Create an empty dst.
    let out = Command::new(zfmt())
        .args(["db", "create", dst_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());

    // Merge src → dst.
    let out = Command::new(zfmt())
        .args(["db", "merge",
               src_path.to_str().unwrap(),
               dst_path.to_str().unwrap()])
        .output()
        .expect("failed to run `zfmt db merge`");
    assert!(
        out.status.success(),
        "`zfmt db merge` failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Verify passes against dst.
    let out = Command::new(zfmt())
        .args(["verify", "--database", dst_path.to_str().unwrap(), fw.to_str().unwrap()])
        .output()
        .expect("failed to run `zfmt verify`");
    assert!(
        out.status.success(),
        "`zfmt verify` after merge failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------
// §15.6 — db list: output contains expected event and string stanzas

#[test]
fn cli_db_list_output() {
    let fw = testfw();
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("events.db");

    run_ingest(&db_path, &fw);

    let out = Command::new(zfmt())
        .args(["db", "list", "--database", db_path.to_str().unwrap()])
        .output()
        .expect("failed to run `zfmt db list`");
    assert!(out.status.success());

    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("[event "), "no event stanzas: {text}");
    assert!(text.contains("[string "), "no string stanzas: {text}");
    assert!(text.contains("heartbeat ts="), "Heartbeat format missing: {text}");
}

// ---------------------------------------------------------------------------
// §15.2 — decode: multi-database first-match fallback

#[test]
fn cli_decode_multi_database_fallback() {
    let fw = testfw();
    let dir = TempDir::new().unwrap();
    let db_full  = dir.path().join("full.db");
    let db_empty = dir.path().join("empty.db");
    let stream_path = dir.path().join("stream.bin");

    // Ingest into the full database only.
    run_ingest(&db_full, &fw);

    // Create an empty database (no events registered).
    let out = Command::new(zfmt())
        .args(["db", "create", db_empty.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());

    // Generate the test stream.
    let out = Command::new(&fw)
        .arg(stream_path.to_str().unwrap())
        .output()
        .expect("failed to run `zfmt-testfw`");
    assert!(out.status.success(), "zfmt-testfw failed");

    // Decode with empty db first, full db second — events must still resolve.
    let out = Command::new(zfmt())
        .args([
            "decode",
            "--database", db_empty.to_str().unwrap(),
            "--database", db_full.to_str().unwrap(),
            stream_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run `zfmt decode`");
    assert!(
        out.status.success(),
        "`zfmt decode` failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let decoded = String::from_utf8_lossy(&out.stdout);
    assert!(
        decoded.contains("heartbeat ts=1000 up=5000ms"),
        "Heartbeat not rendered (fallback db):\n{decoded}"
    );
    assert!(
        decoded.contains("temp celsius_x10=215 sensor=3"),
        "TempReading not rendered (fallback db):\n{decoded}"
    );
    assert!(
        decoded.contains("alert critical code=42"),
        "Alert::Critical not rendered (fallback db):\n{decoded}"
    );
}
