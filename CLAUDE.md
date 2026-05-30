# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
# Build everything
cargo build --workspace

# Run all tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p zfmt
cargo test -p zfmt-macro
cargo test -p zfmt-host

# Run a single test by name
cargo test -p zfmt -- tier1::test_name

# Build/test with constraint features
cargo test -p zfmt --features no-float
cargo test -p zfmt --features no-64bit

# Build the CLI tool
cargo build -p zfmt-host

# Run the CLI
cargo run -p zfmt-host -- check --database events.db firmware.elf
cargo run -p zfmt-host -- ingest --database events.db firmware.elf
cargo run -p zfmt-host -- decode --database events.db stream.bin
cargo run -p zfmt-host -- decode --database events.db --tick-rate-hz 1000000 --protocol-version 2 stream.bin
cargo run -p zfmt-host -- verify --database events.db firmware.elf
```

MSRV is **1.77** (required for `core::mem::offset_of!`).

## Architecture

This is a `no_std`-compatible binary event logging system for embedded firmware with a host-side decoder. The workspace has four crates:

- **`zfmt-macro`** — proc-macro crate; implements `#[derive(Zfmt)]`, `zfmt_str!`, and `__zfmt_log_text!`
- **`zfmt`** — firmware library (`no_std`); re-exports the macro; provides `Write`, `Format`, `Logger`, `FlatSend`, `FlatAdapter`, `ZfmtU64`, and the `log_info!`/`log_warn!` etc. macros
- **`zfmt-host`** — host tooling (`std`); ELF parser, SQLite database, bytecode interpreter, stream decoder; exposes both a library API and the `zfmt` CLI binary
- **`zfmt-testfw`** — a fake firmware binary used by the integration test suite

### How the derive macro works

`#[derive(Zfmt)]` on a struct or enum generates three things at compile time:

1. **Impl block** — `ZFMT_TAG` (u32), `ZFMT_FULL_HASH` (u64), `payload_size()`, `serialize_into()`, `format_into()` (if `#[zfmt(format = "...")]` is present), and `ZfmtEvent` trait impl.
2. **Linker section entry** — a `#[used] static` byte array placed in `.zfmt_events.<hex-tag>` (sorted by tag for binary search). Entry layout: `tag(4) + pad(4) + full_hash(8) + format_hash(4) + pad(4) + bytecode_len(4) + bytecode[padded]`.
3. **String section entry** — the format string is interned into `.zfmt_strings.<hex-hash>` with layout `hash(4) + len(2) + pad(2) + bytes[padded]`.

Event identity is a 64-bit FNV-1a hash of a canonical text description of the struct/variant (name, format string, field names, field canonical types). The wire tag is the lower 32 bits. See §3 of `SPEC.md`.

### Tier-1 vs Tier-2 events

- **Tier-1** — all fields are fixed-size (integers, bool, floats, fixed arrays). `payload_size` is a compile-time constant (`size_of::<Self>()`). `with_payload_bytes` uses a zero-copy `from_raw_parts` of the `repr(C)` struct.
- **Tier-2** — contains at least one variable-length field (`&str`, `String`). `payload_size()` is computed at runtime. Variable-length fields serialize as `[LEB128 count][bytes]`.
- **Nested structs** — Tier-1 structs that embed another `#[derive(Zfmt)]` struct generate bytecode with a `CALL` opcode. Because the inner tag is unknown at proc-macro time, the linker section static uses a const-expression array rather than a byte literal, letting the Rust const evaluator resolve `InnerType::ZFMT_TAG`.

### Bytecode

Opcodes are a single byte: `(item_type << 3) | operand_type`. Key item types: `u8`–`u64`, `i8`–`i64`, `f32`, `f64`, `bool`, `utf8-byte`, `skip`, `dispatch` (inline enum), `call` (nested struct), `string-ref`, `u64-pair` (ZfmtU64). Operand types: `single`, `fixed-array` (LEB128 count in bytecode), `zero-term`, `var-length` (LEB128 count in stream). See §4 of `SPEC.md`.

### Format specifiers

The `c` format type displays integers as FourCC character codes. Bytes are extracted in little-endian order (LSB-first), matching in-memory layout on LE targets. Printable ASCII (0x20–0x7E) is emitted as-is; all other bytes are escaped as `\xNN`. Example: `0x46464952u32` with `:c` → `"RIFF"`. Works on `u8`, `u16`, `u32`, `u64` and their signed counterparts; most useful on `u32`. Newtype wrappers around `u32` can derive `Zfmt` with `#[zfmt(format = "{0:c}")]` and use `:c` in parent event format strings for consistent host-side display.

### Tag collision handling

Tag collisions cannot be detected by the proc-macro (per-item, no cross-type visibility). Detection happens at **`zfmt ingest` time**: same-build collisions are caught because the linker concatenates duplicate-tagged entries into the same ELF section; cross-build collisions are caught against the accumulated database. The recommended resolution is to rename the conflicting type. A `#[zfmt(salt = "...")]` attribute (not yet implemented) is the planned escape hatch for cases where renaming is not feasible.

### Logger trait design

`Logger` and `FlatSend` both use `&self` (not `&mut self`) for all send methods. IPC sends are inherently shared operations; task-local statics guarantee exclusive access by construction. Implementations that need internal mutation (e.g. a software ring-buffer) use interior mutability (`UnsafeCell`, atomics, `RefCell`).

The trait has three methods:
- `timestamp(&self) -> ZfmtU64` — current tick count
- `next_seq(&self) -> u32` — 24-bit sequence counter for gap detection; **default returns 0** (disables sequencing). Override only in the central log-handling task; IPC client loggers leave the default.
- `send_vectored(&self, bufs: &[&[u8]])` — scatter-gather send; `send` defaults to a single-slice call.

`FlatAdapter<L: FlatSend, const N: usize>` assembles scattered slices into an N-byte stack buffer and forwards to `FlatSend::send`. N is the maximum wire size of any event the task may log.

### Wire stream format

```
item = tag(u32 LE) + length(LEB128) + payload
```

`log_info!` always emits an `EventHeader` item immediately before the event item. `EventHeader` layout (12 bytes, `repr(C)`):

```
timestamp: ZfmtU64   // bytes 0–7   (two u32 halves, LE)
severity:  u8        // byte  8
seq:       [u8; 3]   // bytes 9–11  (24-bit LE counter; zero when sequencing disabled)
```

Well-known tags (§7 of `SPEC.md`):

| Event | Tag |
|-------|-----|
| `EventHeader` | `0xe43ae42d` |
| `StreamStart` | `0x0ef1ba00` |
| `DroppedEvents` | `0xe0ee1b4e` |
| `DebugMessage` | `0xa1a6a340` |

`StreamStart.protocol_version = 2` signals that `seq` is in use. Version 1 leaves `seq` zeroed and the host ignores it.

### Host-side decoding (`zfmt-host`)

- **`elf.rs`** — parses `.zfmt_events` and `.zfmt_strings` ELF sections via the `object` crate
- **`db.rs`** — SQLite store (via `rusqlite`) with tables `events`, `strings`, `ingested_builds`; hashes stored as hex text to avoid i64 truncation of u64 values
- **`interpret.rs`** — bytecode interpreter that reads payload bytes and produces typed field values
- **`decode.rs`** — walks the binary stream, dispatches by tag, calls the interpreter, formats output; on `protocol_version >= 2` streams tracks `EventHeader.seq` and emits `[seq gap: ~N events dropped]` annotations before headers where the counter skips. `decode_stream()` takes a `&DecodeConfig` (fallback `tick_rate_hz` and `protocol_version` used when no `StreamStart` is present; a `StreamStart` in the stream always overrides). Use `DecodeConfig::default()` for normal operation.
- **`db.rs`** — also exposes `Db::check(&self, events, strings)` for read-only collision validation (same logic as `ingest` but never writes; returns `CheckStats`). Used by `zfmt check`.
- **`export.rs`** — renders the database as human-readable companion text (`events.db.txt`)

### Feature flags

| Feature | Effect |
|---------|--------|
| `no-float` | Proc-macro rejects `f32`/`f64` fields; `Format` impls compiled out |
| `no-64bit` | Proc-macro rejects `u64`/`i64` fields; `ZfmtU64::from_u64()` and 64-bit arithmetic compiled out; `ZfmtU64` still available, formats as 16 hex digits using 32-bit ops |
| `log-level-*` | Compile-time severity filter; default is `log-level-info` |
| `output-binary` / `output-text` / `output-both` | Controls whether macros emit binary wire format, text via `DebugMessage`, or both |

`ZfmtU64` is a 4-byte-aligned `[u32; 2]` alternative to `u64`, useful on 32-bit targets where a native `u64` field would insert 4 bytes of `repr(C)` padding. Wire encoding is identical to a native little-endian u64.

## Key source files for the proc-macro (`zfmt-macro/src/`)

| File | Purpose |
|------|---------|
| `lib.rs` | Entry points: `derive_zfmt`, `zfmt_str`, `__zfmt_log_text` |
| `parse.rs` | Field parsing, canonical type strings, padding-field detection |
| `hash.rs` | FNV-1a implementation, canonical hash input construction |
| `bytecode.rs` | Opcode constants, LEB128 encoding helpers, `size_of_canonical` |
| `tier1.rs` | Struct derive: bytecode generation, linker section statics, nested-struct `CALL` opcode |
| `tier2.rs` | Variable-length field handling (`payload_size`, `serialize_into` for Tier-2) |
| `enum_derive.rs` | Enum derive: per-variant tags, inline enum `dispatch` opcode generation |
| `format_into.rs` | `format_into` code generation from `#[zfmt(format = "...")]` |
| `fmtstr.rs` | Format string parser (placeholders, specifiers) |
| `codegen.rs` | Shared linker section helpers (`gen_string_section`) |
| `log_text.rs` | `__zfmt_log_text!` macro body: parses format string, generates `FixedBuf` + `DebugMessage` emit |
