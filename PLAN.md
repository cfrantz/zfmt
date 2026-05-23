# zfmt Implementation Plan

## Crate Overview

```
zfmt-macro    proc-macro crate; no runtime deps on other zfmt crates
zfmt          firmware library (no_std); depends on zfmt-macro
zfmt-host     host tooling (std); independent of the other two
```

---

## Phase 1 — Workspace and Core Traits

**Goal:** Establish the crate layout and implement all traits and types that
have no dependency on the proc-macro.  These are independently testable and
unblock all later phases.

### Tasks

- [ ] Create Cargo workspace with the three crates
- [ ] `zfmt`: `Write` trait and `Error` type (§11.1)
- [ ] `zfmt`: `FormatType`, `Align`, `FormatSpec` (§11.2)
- [ ] `zfmt`: `Format` trait (§11.3)
- [ ] `zfmt`: `Format` impls for all §3.3 primitives (`u8`–`u64`, `i8`–`i64`,
  `f32`, `f64`, `bool`, `char`, `&str`) covering every specifier in §10.2
- [ ] `zfmt`: alignment helpers — counting writer for left-align; arithmetic
  width computation for right-align / zero-pad (§11.5)
- [ ] `zfmt`: `Logger` trait, `FlatSend` trait, `FlatAdapter<L, N>` (§12)

### Validation

Unit tests for every `Format` impl: verify each type against each applicable
specifier (`#`, `+`, `0N`, `<N`, `>N`, `x`, `X`, `b`, `o`) and alignment
edge cases (value wider than field width, exact width, zero width).

---

## Phase 2 — Proc-Macro: Tier-1 Structs

**Goal:** Generate correct linker section entries, `payload_size`, and
`serialize_into` for fixed-size `repr(C)` structs.  Establishes all
proc-macro infrastructure that every later phase builds on.

### Tasks

- [ ] `zfmt-macro`: attribute parsing — `#[derive(zfmt)]`, `#[zfmt(format = "...")]`
- [ ] `zfmt-macro`: canonical hash input construction for structs (§3.2)
- [ ] `zfmt-macro`: FNV-1a 64-bit hash (§3.1)
- [ ] `zfmt-macro`: canonical type name mapping (§3.3)
- [ ] `zfmt-macro`: Tier-1 bytecode generation — integer/bool/float/fixed-array
  item types, `skip` instructions for `repr(C)` padding, fixed-array operand (§4)
- [ ] `zfmt-macro`: `.zfmt_events` linker section entry emission (§8.1)
- [ ] `zfmt-macro`: `.zfmt_strings` linker section entry emission for format
  strings (§8.2)
- [ ] `zfmt-macro`: `payload_size(&self) -> usize` generation — compile-time
  constant for Tier-1 (§5.1)
- [ ] `zfmt-macro`: `serialize_into(&self, buf: &mut [u8])` generation

### Validation

- Unit-test hash values against the §3.6 worked example
- Unit-test generated `payload_size` matches `core::mem::size_of` for sample
  structs
- Unit-test `serialize_into` output matches a hand-computed byte sequence
- Link a minimal test binary; confirm `.zfmt_events` section is present and
  parseable

---

## Phase 3 — Proc-Macro: Format Strings and `format_into`

**Goal:** Parse format strings at macro-expansion time and generate
`format_into` methods, making firmware-side text formatting available for
all Tier-1 structs.

### Tasks

- [ ] `zfmt-macro`: format string lexer — split literal segments from
  `{name}` / `{name:spec}` placeholders
- [ ] `zfmt-macro`: specifier parser — sign, alternate, zero-pad, align,
  precision, type; emit compile errors for invalid combinations
- [ ] `zfmt-macro`: `format_into<W: Write>` method generation (§11.4)
- [ ] `zfmt-macro`: `zfmt_str!("literal")` macro — hash string, emit
  `.zfmt_strings` entry, evaluate to `u32` hash (§4.7)
- [ ] `zfmt-macro`: `string-ref` bytecode emission for `zfmt_str!` fields

### Validation

- Test every specifier combination in §10.2 against known formatted output
- Test format strings with no placeholders, all-placeholders, and mixed
- Test compile-error diagnostics for unknown field names and invalid specifiers

---

## Phase 4 — Proc-Macro: Tier-2 Events

**Goal:** Support variable-length fields (`&str`, `String`) and the
LEB128-prefixed serialization they require.

### Tasks

- [ ] `zfmt-macro`: detect Tier-2 fields (`&str`, `String` → canonical `str`)
- [ ] `zfmt-macro`: `var-length` operand type (3) in bytecode (§4.3)
- [ ] `zfmt-macro`: `payload_size` for Tier-2 — runtime traversal via
  `payload_size(&self) -> usize` (§5.2)
- [ ] `zfmt-macro`: `serialize_into` for Tier-2 — emit LEB128 element count
  then bytes for each variable-length field
- [ ] `zfmt`: unsigned LEB128 encode/decode helpers

### Validation

- Round-trip tests: serialize a Tier-2 event, verify the byte sequence, then
  parse it back and confirm lengths match `payload_size`
- Test events mixing fixed and variable-length fields in various orderings

---

## Phase 5 — Proc-Macro: Enum Events and Inline Enums

**Goal:** Support top-level enum variant events (§5.3) and `repr(C, uN)`
inline enums decoded via the `dispatch` instruction (§4.5).

### Tasks

- [ ] `zfmt-macro`: canonical hash input for enum variants (§3.2 variant form)
- [ ] `zfmt-macro`: top-level enum — emit one `.zfmt_events` entry per
  variant; `payload_size` / `serialize_into` / `format_into` per variant
- [ ] `zfmt-macro`: inline enum detection via `#[repr(C, uN)]`
- [ ] `zfmt-macro`: `dispatch` bytecode generation — discriminant type,
  padding, dispatch table of (value, tag) pairs (§4.5)
- [ ] `zfmt-macro`: variant subroutine generation — each variant consumes
  exactly the union size; smaller variants padded with `skip` instructions
- [ ] `zfmt-macro`: `call` bytecode emission for nested struct fields (§4.6)

### Validation

- Test top-level enum: each variant produces its own tag and correct bytecode
- Test inline enum: dispatch table entries match discriminant values
- Test `repr(C, u8)` through `repr(C, u64)` discriminant sizes
- Test mixed struct containing an inline enum field

---

## Phase 6 — Well-Known Events and Logging Macros

**Goal:** Define the spec-mandated well-known events and implement the full
logging macro family.

### Tasks

**Well-known events** (§7)

- [ ] `zfmt`: `Severity` enum (`repr(C, u8)`) with display strings
- [ ] `zfmt`: `EventHeader` struct (`repr(C)`, `#[derive(zfmt)]`)
- [ ] `zfmt`: `StreamStart` struct (`repr(C)`, `#[derive(zfmt)]`)
- [ ] `zfmt`: `DroppedEvents` struct (`repr(C)`, `#[derive(zfmt)]`)
- [ ] `zfmt`: `DebugMessage` struct (`#[derive(zfmt)]`, Tier-2)

**Logging macros** (§13)

- [ ] `zfmt`: `log_trace!` / `log_debug!` / `log_info!` / `log_warn!` /
  `log_error!` / `log_fatal!` — structured event path: get timestamp,
  serialize `EventHeader` + event, call `LOGGER.send_vectored()` (§13.2)
- [ ] `zfmt`: unstructured text path — format into stack buffer, emit as
  `DebugMessage` (§13.3)
- [ ] `zfmt`: named binding syntax (`name = expr`) in unstructured macros
- [ ] `zfmt`: compile-time severity filtering via `log-level-*` features (§13.4)
- [ ] `zfmt`: output mode selection via `output-binary` / `output-text` /
  `output-both` features (§13.5)
- [ ] `zfmt`: deprecation warning when unstructured text is used in
  `log_info!` or above (§13.3)

### Validation

- Test logging macros against a mock `Logger` that captures `send_vectored`
  calls; inspect the raw bytes for correct wire format
- Test severity filtering: events below threshold produce no code (verify with
  `assert_eq!(0, call_count)` after filtered call sites)
- Test deprecation warning fires for `log_info!("...")` and is suppressed by
  `#[allow(deprecated)]`
- Test `output-text` mode produces a `DebugMessage` with correctly formatted
  content

---

## Phase 7 — Host Tooling: ELF, Database, and CLI (except `decode`)

**Goal:** Implement the ingestion pipeline and all CLI commands except
`decode`, which requires the bytecode interpreter (Phase 8).

### Tasks

- [ ] `zfmt-host`: ELF section reader — locate and parse `.zfmt_events` and
  `.zfmt_strings` sections from an object file (§8)
- [ ] `zfmt-host`: database module — create schema, `ingest` with collision
  policy (§9.1, §9.2), idempotent re-ingest
- [ ] `zfmt-host`: companion plaintext export generation after every
  successful ingest (§9.3)
- [ ] `zfmt-host`: CLI skeleton with `--database` flag
- [ ] `zfmt-host`: `zfmt ingest` (§15.1)
- [ ] `zfmt-host`: `zfmt verify` (§15.3)
- [ ] `zfmt-host`: `zfmt db create` / `db merge` / `db list` (§15.4–15.6)

### Validation

- Ingest a test ELF (built from Phase 2–5 test events); verify database rows
  match expected tags, full hashes, and bytecode
- Test idempotency: ingest same ELF twice, confirm no change
- Test all three collision cases from §9.2 (identical = skip; same hash
  different content = error; same tag different hash = error)
- Test `verify` exits non-zero when a known event is absent from the database

---

## Phase 8 — Host Tooling: Stream Decoder

**Goal:** Implement the bytecode interpreter and wire-format parser, enabling
`zfmt decode` to produce human-readable output from a binary stream.

### Tasks

- [ ] `zfmt-host`: wire format parser — read `[tag u32][LEB128 length][payload]`
  items; skip unknown tags using the length field (§6.1)
- [ ] `zfmt-host`: bytecode interpreter:
  - Basic item types: `u8`–`u64`, `i8`–`i64`, `f32`, `f64`, `bool`, `char` (§4.2)
  - Operand types: `single`, `fixed-array`, `zero-term`, `var-length` (§4.3)
  - `skip` instruction
  - `call` instruction with depth limit of 4 (§4.6)
  - `dispatch` instruction (§4.5)
  - `string-ref` instruction (§4.7)
- [ ] `zfmt-host`: format string renderer — interpolate field values into
  format string placeholders using the database
- [ ] `zfmt-host`: `EventHeader` decoding — scale timestamp to wall-clock
  time using `StreamStart.tick_rate_hz`
- [ ] `zfmt-host`: `zfmt decode` command (§15.2)

### Validation

- Unit-test each bytecode instruction in isolation
- Test `call` depth limit enforcement (depth > 4 returns error)
- Test `dispatch` with missing discriminant value (implementation-defined
  placeholder path)
- Test the forward-compatible skip path: a stream with an unknown tag is
  decoded up to and after the unknown event without error
- Test `EventHeader` timestamp scaling with known tick rate

---

## Phase 9 — Integration Testing

**Goal:** Validate the full pipeline end-to-end across all event kinds.

### Tasks

- [ ] Build a test firmware crate that exercises every event kind: Tier-1
  structs, Tier-2 events, top-level enum variants, inline enum fields,
  `zfmt_str!`, `DebugMessage`
- [ ] Capture the binary stream produced by the test firmware
- [ ] Ingest the test ELF; decode the stream; compare output against golden
  fixtures
- [ ] Test well-known event sequence: `StreamStart` → `EventHeader` +
  event pairs → `DroppedEvents`
- [ ] Test `StreamStart` emitted without a preceding `EventHeader` (bare
  event, §6.3)
- [ ] Test `zfmt decode` with multiple `--database` flags; confirm first-match
  semantics
- [ ] Test the developer workflow (§17): `db merge`, local ingest, multi-DB
  decode
- [ ] Test the release pipeline (§16): ingest → verify → companion export diff

---

## Critical Path

```
Phase 1 (traits)
  └─► Phase 2 (Tier-1 proc-macro)
        ├─► Phase 3 (format_into)
        │     └─► Phase 6 (macros, output-text mode)
        ├─► Phase 4 (Tier-2)
        │     └─► Phase 6
        ├─► Phase 5 (enums)
        │     └─► Phase 6
        └─► Phase 6 (macros, output-binary mode)  ◄── unblocks Phase 7 & 8
              ├─► Phase 7 (ELF + DB + CLI)
              └─► Phase 8 (decoder)
                    └─► Phase 9 (integration)
```

Phases 3, 4, and 5 extend the proc-macro independently and can be developed
in parallel once Phase 2 is complete.  Phase 7 (ELF parsing and database) can
begin alongside Phase 3–5 using hand-crafted test fixtures before real ELFs
are available.
