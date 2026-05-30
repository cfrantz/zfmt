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
cargo run -p zfmt-host -- ingest --database events.db firmware.elf
cargo run -p zfmt-host -- decode --database events.db stream.bin
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

### Wire stream format

```
item = tag(u32 LE) + length(LEB128) + payload
```

`log_info!` always emits an `EventHeader` item (timestamp + severity) immediately before the event item. The `EventHeader` tag is a well-known spec-computed constant (§7.2).

### Host-side decoding (`zfmt-host`)

- **`elf.rs`** — parses `.zfmt_events` and `.zfmt_strings` ELF sections via the `object` crate
- **`db.rs`** — SQLite store (via `rusqlite`) with tables `events`, `strings`, `ingested_builds`; hashes stored as hex text to avoid i64 truncation of u64 values
- **`interpret.rs`** — bytecode interpreter that reads payload bytes and produces typed field values
- **`decode.rs`** — walks the binary stream, dispatches by tag, calls the interpreter, formats output
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
