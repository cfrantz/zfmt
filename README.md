# zfmt: Low-Overhead Binary Event Logging for Embedded Systems

`zfmt` is a Rust library for structured event logging on resource-constrained
embedded targets.  Firmware emits compact binary frames — just a tag and raw
field bytes.  All human-readable content (format strings, field names, type
layouts) lives in the ELF file and is never needed at runtime.  A host-side
CLI decodes the binary stream into readable text using a SQLite database built
from the ELF's linker sections.

---

## Table of Contents

1. [Introduction](#introduction)
2. [Architecture](#architecture)
3. [Use Cases](#use-cases)
4. [Host Tooling](#host-tooling)
5. [Quick Start](#quick-start)

---

## Introduction

Logging on deeply embedded systems is hard.  Flash is scarce, RAM is scarcer,
and `printf` allocates.  Most solutions either waste space storing format
strings on the device or require a proprietary binary protocol that ties the
host decoder to a specific firmware version.

`zfmt` takes a different approach:

- **Define events as Rust structs or enums** annotated with `#[derive(Zfmt)]`.
  The proc-macro generates a stable 32-bit tag, a compact bytecode description
  of the field layout, and the human-readable format string — all compiled into
  dedicated ELF linker sections.
- **Firmware emits only raw bytes** — a tag, a length, and the serialized field
  values.  No strings, no format logic, no heap allocation.
- **Host tooling decodes everything.**  The `zfmt` CLI ingests ELF files into a
  SQLite database and uses that database to reconstruct human-readable output
  from any binary stream, even streams captured from older firmware versions.

The linker sections can be stripped from the final firmware image with no
effect on runtime behaviour.  The host database accumulates entries across
every firmware version ever shipped, so historical events are always decodable.

---

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                     Firmware                        │
│                                                     │
│  #[derive(Zfmt)]                                    │
│  #[zfmt(format = "temp celsius_x10={c} sensor={s}")]│
│  pub struct TempReading { c: i16, s: u8 }           │
│                  │                                  │
│          compile time                               │
│                  │                                  │
│                  ├─► .zfmt_events section ──────────┼──► ELF file
│                  │     tag, bytecode, format hash   │
│                  └─► .zfmt_strings section ─────────┼──► ELF file
│                        hash → "temp celsius_x10=…"  │
│                                                     │
│  log_info!(logger, TempReading { c: 215, s: 3 });   │
│                  │                                  │
│              run time                               │
│                  │                                  │
│                  ▼                                  │
│  [tag u32][len LEB128][0xD7 0x00 0x03]  ◄── stream  │
└──────────────────┬──────────────────────────────────┘
                   │ binary stream (UART / USB / buffer)
                   ▼
┌─────────────────────────────────────────────────────┐
│                  Host (zfmt CLI)                    │
│                                                     │
│  $ zfmt ingest firmware.elf                         │
│           └─► reads .zfmt_events / .zfmt_strings    │
│               writes tag → bytecode + format string │
│               into events.db (SQLite)               │
│                                                     │
│  $ zfmt decode stream.bin                           │
│           └─► for each frame:                       │
│               1. look up tag in events.db           │
│               2. run bytecode interpreter on payload│
│               3. render with stored format string   │
│               4. print one line per event           │
│                                                     │
│  [INFO] temp celsius_x10=215 sensor=3               │
└─────────────────────────────────────────────────────┘
```

### Key design properties

**Stable tags.**  Each event type gets a 64-bit FNV-1a hash of its canonical
description (name, format string, field names, and types).  The lower 32 bits
are the wire tag.  Renaming a module or reorganising packages does not change
the tag; only a semantic change to the event definition does.

**Forward-compatible decoding.**  A host with an incomplete database can skip
any frame whose tag is absent — the LEB128 length field allows safe byte-exact
skipping — and continue decoding the rest of the stream without losing sync.

**Strippable metadata.**  Linker sections `.zfmt_events` and `.zfmt_strings`
can be discarded from the final firmware image using a linker script, saving
flash with no runtime impact.

**Bytecode interpreter.**  The host decoder uses a small stack-based bytecode
interpreter to consume payload bytes and produce typed values.  Bytecode
handles fixed integers, floats, booleans, UTF-8 strings (fixed-length,
zero-terminated, or variable-length), compile-time interned string references,
and inline enum dispatch — covering the full range of `repr(C)` struct layouts.

---

## Use Cases

### Production telemetry from constrained MCUs

A target with 64 KB of flash and no heap can emit a high-rate binary stream
over UART.  Because firmware never touches format strings, the per-event cost
is just a tag write (4 bytes), a length write (1–2 bytes), and a memcpy of the
struct payload.  A background host process decodes the stream in real time.

### Post-mortem analysis of fault logs

Firmware can log events to a circular buffer in retained RAM.  After a reset,
the buffer is flushed to the host over the debug interface.  Because the
database persists across firmware updates, even a stream captured weeks before
the current firmware was released decodes correctly.

### Mixed-version device fleets

Running multiple firmware versions simultaneously?  Run `zfmt ingest` for
every ELF you have ever shipped.  The database accumulates entries for all
versions; decoding a stream from any device in the fleet uses the same command
regardless of which firmware version it is running.

### CI / release gating

Add `zfmt verify` to your CI pipeline.  It exits non-zero if any event defined
in the new firmware ELF is absent from the database, catching the case where a
developer forgot to run `zfmt ingest` before shipping.

---

## Host Tooling

The `zfmt` CLI is built from the `zfmt-host` crate.  All subcommands use a
SQLite database (`zfmt/events.db` by default).

### `zfmt ingest`

```
zfmt ingest [--database <path>] <elf>
```

Parses `.zfmt_events` and `.zfmt_strings` sections from `<elf>` and upserts
them into the database.  Ingest is **idempotent** — running it twice on the
same ELF is safe.  After every successful ingest a companion text export is
written alongside the database (e.g. `events.db.txt`) so the database contents
are human-inspectable without the CLI.

### `zfmt decode`

```
zfmt decode [--database <path>]... <stream>
```

Reads the binary event stream from `<stream>` and prints one decoded line per
event to stdout.  Multiple `--database` flags are accepted; the first database
that contains a given tag is used (useful when events are split across
per-subsystem databases).

Each output line is prefixed with the hex tag:

```
[640003d2] 0.001000 INFO
[a1b2c3d4] heartbeat ts=1000 up=5000ms
```

Unknown tags produce a warning on stderr and are skipped; decoding continues
with the next frame.  If `--database` is omitted, `zfmt/events.db` in the
current directory is used.

### `zfmt verify`

```
zfmt verify [--database <path>] <elf>
```

Checks that every event tag found in `<elf>` is present in the database.
Exits 0 if all events are present, non-zero (with a list of missing tags on
stderr) otherwise.  Intended for use in CI.

### `zfmt db create`

```
zfmt db create <path>
```

Creates a new, empty database at `<path>`.

### `zfmt db merge`

```
zfmt db merge <src> <dst>
```

Copies all events and strings from `<src>` into `<dst>`.  Idempotent; entries
already present in `<dst>` are skipped.  Useful for combining per-subsystem
databases into a single fleet-wide store.

### `zfmt db list`

```
zfmt db list [--database <path>]
```

Prints all events and strings stored in the database in companion-export text
format.  Useful for inspection and diffing database contents.

---

## Quick Start

### 1. Add dependencies

In your firmware's `Cargo.toml`, enable the log levels you want:

```toml
[dependencies]
zfmt = { path = "../zfmt", features = ["log-level-info", "log-level-warn", "log-level-error"] }
```

### 2. Define your events

```rust
use zfmt::ZfmtStr;

/// A periodic heartbeat.
#[derive(zfmt::Zfmt)]
#[zfmt(format = "heartbeat ts={timestamp} up={uptime_ms}ms")]
pub struct Heartbeat {
    pub timestamp: u64,
    pub uptime_ms: u32,
}

/// Temperature from a sensor.
#[derive(zfmt::Zfmt)]
#[zfmt(format = "temp celsius_x10={celsius_x10} sensor={sensor_id}")]
pub struct TempReading {
    pub celsius_x10: i16,
    pub sensor_id:   u8,
}

/// An alert — each variant is a separate event on the wire.
#[derive(zfmt::Zfmt)]
pub enum Alert {
    #[zfmt(format = "alert critical code={code}")]
    Critical { code: u32 },
    #[zfmt(format = "alert warning")]
    Warning,
}
```

### 3. Implement a logger

`zfmt` ships with `FlatAdapter`, which buffers events into a fixed-size stack
array and flushes them via a user-supplied `FlatSend` implementation:

```rust
use zfmt::{FlatAdapter, FlatSend};

struct UartLogger;

impl FlatSend for UartLogger {
    fn timestamp(&self) -> u64 {
        read_tick_counter()   // your hardware timer
    }
    fn send(&mut self, data: &[u8]) {
        uart_write(data);     // UART / USB / DMA ring buffer / etc.
    }
}

static mut LOGGER: FlatAdapter<UartLogger, 256> = FlatAdapter::new(UartLogger);
```

### 4. Emit the stream header and log events

```rust
use zfmt::events::StreamStart;

// Emit StreamStart once at boot — required for timestamp scaling on the host.
zfmt::log_bare_event!(unsafe { &mut LOGGER }, StreamStart {
    protocol_version: StreamStart::PROTOCOL_VERSION,
    _pad0: [0; 6],
    tick_rate_hz: 1_000_000,   // your hardware tick rate in Hz
    firmware_build_id: 0,
});

// Log structured events at the appropriate severity.
zfmt::log_info!(unsafe { &mut LOGGER }, Heartbeat { timestamp: 1000, uptime_ms: 5000 });
zfmt::log_warn!(unsafe { &mut LOGGER }, TempReading { celsius_x10: 215, sensor_id: 3 });
zfmt::log_error!(unsafe { &mut LOGGER }, Alert::Critical { code: 42 });
```

### 5. Ingest the ELF on the host

After building your firmware, ingest it once (and re-run whenever the firmware
changes):

```
$ zfmt ingest --database events.db target/thumbv7em-none-eabi/release/firmware.elf
parsed 14 event(s) and 8 string(s) from firmware.elf
events: 14 added, 0 skipped  |  strings: 8 added, 0 skipped
companion export written to events.db.txt
```

### 6. Decode a captured stream

Capture the binary stream from your device (UART dump, USB capture, etc.) and
save it to a file, then:

```
$ zfmt decode --database events.db stream.bin
[640003d2] 0.001000 INFO
[a1b2c3d4] heartbeat ts=1000 up=5000ms
[640003d2] 0.002500 WARN
[deadbeef] temp celsius_x10=215 sensor=3
[640003d2] 0.003000 ERROR
[cafebabe] alert critical code=42
```

`EventHeader` frames (tag `640003d2`) carry the timestamp (scaled to seconds
using `tick_rate_hz` from `StreamStart`) and severity.  Application event
frames follow immediately after each header.

### 7. Add verify to CI

```
$ zfmt verify --database events.db target/thumbv7em-none-eabi/release/firmware.elf
ok: all 14 event(s) verified
$ echo $?
0
```

If a new event is added to firmware but `zfmt ingest` has not been re-run,
`verify` exits non-zero and lists the missing tags.

---

## Crate Layout

| Crate | Role |
|-------|------|
| `zfmt` | Core firmware library: traits, macros, serialization, `no_std` |
| `zfmt-macro` | Proc-macro: `#[derive(Zfmt)]`, `zfmt_str!`, log-text macros |
| `zfmt-host` | Host library: ELF parsing, SQLite database, bytecode interpreter, stream decoder |
| `zfmt-testfw` | Test firmware binary used by the integration test suite |

## Specification

The full wire protocol, bytecode encoding, ELF section layout, and database
schema are documented in [`SPEC.md`](SPEC.md).
