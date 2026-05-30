# zfmt Stream Protocol Specification

## 1. Overview

`zfmt` is a low-overhead event logging protocol for embedded systems.
Firmware emits a stream of typed events; a host tool decodes and displays
them using an ever-growing database of format strings and bytecode derived
from ELF linker sections.  The design allows firmware to discard all display
metadata from its image while retaining the ability to reconstruct human-
readable output on the host, even for events from older firmware versions.

### Goals

- Zero runtime allocation on the firmware side
- Deterministic event size for fixed-field events (zerocopy `repr(C)`)
- A stable, content-derived identifier per event type that survives firmware
  evolution
- A host-side database that accumulates identifiers across all firmware
  versions, enabling display of both current and historical events
- Forward-compatible decoding: a host with an incomplete database can skip
  unknown events and continue decoding without losing stream synchronization
- Optional in-firmware event formatting with no dependency on `core::fmt` or
  external formatting libraries

### Constraints

- All multi-byte values in the stream are **little-endian**
- Events are Rust structs or enum variants annotated with the `zfmt`
  proc-macro
- Variable-length fields are supported but require length-prefixed
  serialization (see §5)

---

## 2. Definitions

| Term | Meaning |
|------|---------|
| **Event** | A Rust struct or enum variant annotated with `#[derive(zfmt)]` |
| **Tag** | The lower 32 bits of the 64-bit FNV-1a hash identifying an event type |
| **Full hash** | The complete 64-bit FNV-1a hash, used for collision detection |
| **Format string** | A Rust-style template string, e.g. `"temp={temp} status={status}"` |
| **Bytecode** | A sequence of instructions describing how to interpret stream bytes |
| **Subroutine** | A bytecode sequence callable from another bytecode sequence |
| **String table** | A linker section mapping string hashes to UTF-8 string content |
| **Event table** | A linker section mapping tags to format string hashes and bytecode |
| **Tier-1 event** | An event whose serialized form is a fixed-size `repr(C)` struct |
| **Tier-2 event** | An event containing one or more variable-length fields |
| **Inline enum** | A `repr(uN)` (or `repr(C, uN)` for data-carrying) enum used as a field within a struct; decoded via the `dispatch` opcode; not independently logged |
| **DebugMessage** | Well-known Tier-2 event carrying a pre-formatted UTF-8 string; emitted by the unstructured logging path (§7.5, §13.3) |

---

## 3. Event Identification

### 3.1 Hash Algorithm

Event types are identified by a **64-bit FNV-1a hash** of a canonical
description of the event.

```
FNV offset basis: 0xcbf29ce484222325
FNV prime:        0x00000100000001b3

hash = offset_basis
for each byte in input:
    hash = hash XOR byte
    hash = hash * prime  (wrapping 64-bit multiplication)
```

The **tag** used in the event stream is the lower 32 bits of the full hash.
The full 64-bit hash is stored in the ELF linker section and in the host
database for collision detection.

### 3.2 Canonical Hash Input

The hash is computed over a canonical UTF-8 string constructed as follows.
Each component occupies exactly one line (terminated by `\n`).

**For a struct:**

```
struct <Name>\n
format <format-string>\n
field <name> <canonical-type>\n
field <name> <canonical-type>\n
...
```

**For an enum variant:**

```
variant <EnumName>::<VariantName>\n
format <format-string>\n
field <name> <canonical-type>\n
...
```

For tuple variants, fields are named by their zero-based index (`0`, `1`, …).
If a struct or variant has no format string, the `format` line is omitted.
If a struct or variant has no fields, no `field` lines are emitted.

### 3.3 Canonical Type Names

| Rust type | Canonical name |
|-----------|---------------|
| `u8` | `u8` |
| `u16` | `u16` |
| `u32` | `u32` |
| `u64` | `u64` |
| `i8` | `i8` |
| `i16` | `i16` |
| `i32` | `i32` |
| `i64` | `i64` |
| `f32` | `f32` |
| `f64` | `f64` |
| `bool` | `bool` |
| `char` | `char` |
| `&str`, `String` | `str` |
| `[T; N]` | `[canonical(T); N]` |
| `ZfmtU64` | `ZfmtU64` |
| any other named type `Foo` | `Foo` |

Custom types are referenced by their short (unqualified) name.  If a custom
type's definition changes without a name change, the parent event's hash will
not automatically change — this is a known limitation.

### 3.4 Scope of the Name

Only the short (unqualified) struct or variant name is included in the hash.
The module path and crate name are excluded so that module reorganisations do
not orphan historical database entries.

### 3.5 Collision Handling

The proc-macro has no visibility into other types being compiled — it processes
one type at a time with no cross-type or cross-crate state.  Collision
detection therefore cannot happen at proc-macro expansion time.  The database
is the canonical registry of all tags ever used across all firmware versions,
and `zfmt ingest` is the correct and sufficient detection point.

When `zfmt ingest` parses a firmware ELF it reads every entry in the
`.zfmt_events` section, including multiple entries with the same 32-bit tag
that may result from a same-build collision (the linker concatenates all
subsections of the same name).  The following rules apply:

- `zfmt ingest` **must** fail with a descriptive error if two entries in the
  same ELF share the same 32-bit tag but have different 64-bit full hashes
  (a same-build collision).
- `zfmt ingest` **must** fail with a descriptive error if a new entry's 32-bit
  tag matches an existing database entry with a different 64-bit full hash
  (a cross-build wire-stream collision).
- `zfmt ingest` **must** fail with a descriptive error if a new entry's 64-bit
  full hash matches an existing database entry with different content (a
  full-hash collision, indicating data corruption or a hash algorithm defect).
- If a new entry's 64-bit full hash matches an existing database entry with
  identical content, the entry is silently skipped (idempotent re-ingest).

**Resolving a collision.**  The recommended resolution is to rename the
conflicting type or add a distinguishing field — both change the canonical
hash input and therefore the tag.  When renaming is not feasible (e.g. the
name is part of a public API), a `#[zfmt(salt = "...")]` attribute may be
added to append a `salt <value>\n` line to the canonical hash input, changing
both the full hash and the tag while keeping them mutually consistent.  The
salt mechanism is not yet implemented; this note documents the intended escape
hatch for a future release.

### 3.6 Example

```rust
#[derive(zfmt)]
#[zfmt(format = "The quick brown {clever_animal} jumped over the lazy {lazy_animal} {n} times!")]
pub struct Quick {
    pub clever_animal: String,
    pub lazy_animal: String,
    pub n: u32,
}
```

Canonical hash input:
```
struct Quick
format The quick brown {clever_animal} jumped over the lazy {lazy_animal} {n} times!
field clever_animal str
field lazy_animal str
field n u32
```

---

## 4. Bytecode

### 4.1 Opcode Encoding

Each bytecode instruction is a single byte split into two fields:

```
bits 7..3 — item type  (5 bits, 32 possible values)
bits 2..0 — operand type (3 bits, 8 possible values)

opcode = (item_type << 3) | operand_type
```

### 4.2 Item Types

| Value | Name | Description |
|-------|------|-------------|
| 0 | `end` | End of bytecode subroutine (return) |
| 1 | `u8` | Unsigned 8-bit integer |
| 2 | `u16` | Unsigned 16-bit integer |
| 3 | `u32` | Unsigned 32-bit integer |
| 4 | `u64` | Unsigned 64-bit integer |
| 5 | `i8` | Signed 8-bit integer |
| 6 | `i16` | Signed 16-bit integer |
| 7 | `i32` | Signed 32-bit integer |
| 8 | `i64` | Signed 64-bit integer |
| 9 | `utf8-byte` | One byte of a UTF-8 string; used with array operand types |
| 10 | `skip` | Advance the stream pointer without displaying anything |
| 11 | `f32` | IEEE 754 single-precision float |
| 12 | `f64` | IEEE 754 double-precision float |
| 13 | `bool` | Boolean; displayed as `true` or `false`; 1 byte in stream |
| 14 | `dispatch` | Enum discriminant dispatch (see §4.5) |
| 15 | `call` | Call a subroutine by tag (see §4.6) |
| 16 | `string-ref` | Compile-time interned string; u32 hash in stream (see §4.7) |
| 17 | `u64-pair` | 64-bit unsigned integer stored as two consecutive little-endian u32 values `[lo, hi]`; only `single` operand is valid (see §4.8) |
| 18–31 | — | Reserved |

### 4.3 Operand Types

| Value | Name | Description |
|-------|------|-------------|
| 0 | `single` | One instance of the item; no operand in bytecode |
| 1 | `fixed-array` | Fixed-length array; LEB128 element count follows in bytecode |
| 2 | `zero-term` | Zero-terminated array with fixed capacity; LEB128 max length follows in bytecode; display stops at the first zero byte |
| 3 | `var-length` | Variable-length; LEB128 element count is in the **stream**, not in the bytecode |
| 4–7 | — | Reserved |

### 4.4 LEB128 Encoding

All operands embedded in the bytecode stream are encoded as **unsigned
LEB128**.  Lengths and element counts in the event stream that use operand
type 3 are also unsigned LEB128.

### 4.5 Dispatch Instruction (`item_type=14`)

Used to handle inline enums (`repr(C, uN)` enums embedded as fields within a
struct).  This instruction does not apply to top-level enum events, which emit
variant-level tags directly (§5.3).  The operand type bits are unused for
`dispatch` and must be zero.

The instruction is followed in the bytecode by:

```
discriminant_type : LEB128   -- item type value for the discriminant (1–4 for u8–u64)
padding           : LEB128   -- bytes between discriminant and union start
count             : LEB128   -- number of entries in the dispatch table
  value_0         : LEB128   -- discriminant value for variant 0
  tag_0           : LEB128   -- 32-bit tag of the subroutine for variant 0
  value_1         : LEB128
  tag_1           : LEB128
  ...
```

**Execution:**

1. Read the discriminant from the stream (size given by `discriminant_type`)
2. Advance the stream pointer by `padding` bytes
3. Search the dispatch table for a matching discriminant value
4. Call the matching subroutine (binary-search the event table by `tag`)
5. If no match is found, behaviour is implementation-defined (recommended:
   emit a placeholder and advance by the union size)

Each variant subroutine is generated by the proc-macro to consume exactly
the union's total byte size, using explicit `skip` instructions to pad if the
variant's payload is smaller than the largest variant.

### 4.6 Call Instruction (`item_type=15`)

The operand type bits are unused for `call` and must be zero.

The instruction is followed in the bytecode by:

```
tag : LEB128   -- 32-bit tag of the target subroutine
```

**Execution:** binary-search the event table for `tag`; execute its bytecode
as a subroutine.  The maximum call depth is **4**.  Recursive calls are
forbidden.

### 4.8 U64-Pair Instruction (`item_type=17`, `operand_type=0`)

Reads 8 bytes from the event stream; interprets bytes 0–3 as a
little-endian `u32` (`lo`) and bytes 4–7 as a little-endian `u32` (`hi`);
reconstructs the 64-bit value as `(hi as u64) << 32 | lo as u64`.

Only the `single` operand type is valid; any other operand value is an error.

**Purpose:** `ZfmtU64` stores a 64-bit value as two `u32` fields, giving it
4-byte rather than 8-byte alignment.  This eliminates alignment padding in
`repr(C)` event structs on 32-bit targets where a native `u64` field would
force an extra 4 bytes of padding.  The wire byte order is identical to a
native little-endian `u64`, so host decoders reconstruct the value with a
single read.

### 4.7 String-Ref Instruction (`item_type=16`, `operand_type=0`)

Reads a `u32` from the event stream; interprets it as a string hash; looks
up the string in the string table (§8.2) and displays the result.

The `zfmt_str!("literal")` macro emits the string into the string table at
compile time and evaluates to the corresponding `u32` hash.

---

## 5. Event Tiers

### 5.1 Tier-1: Fixed-Size Zerocopy Events

A tier-1 event is a `repr(C)` struct whose fields are all fixed-size types
(integers, booleans, floats, fixed-length arrays).  Its serialized form is a
direct memory copy of the struct.  The total byte size is determined
statically from the bytecode.

The proc-macro emits explicit `skip` instructions for all `repr(C)` padding
bytes so that the host decoder does not need to know the target's alignment
rules.

Because the payload size is a compile-time constant, the firmware writes the
LEB128 length field (§6.1) as a fixed byte sequence with no runtime
computation.

### 5.2 Tier-2: Mixed Variable-Length Events

A tier-2 event contains at least one variable-length field (operand type 3).
Fields are serialized in declaration order:

- Fixed-size fields: copied directly (same as tier-1)
- Variable-length fields: `[LEB128 element count][elements...]`

There is no padding between a variable-length field and the field that follows
it; padding only applies between consecutive fixed-size fields.

Because the payload size is not known until serialization is complete, the
firmware must compute it before writing the LEB128 length field (§6.1).  The
proc-macro generates a `payload_size(&self) -> usize` method that sums the
serialized sizes of all fields; for variable-length fields this is a runtime
traversal, but tier-2 payloads are expected to be small (typically under 128
bytes) and are generally used only in debug builds.

A decoder consuming the bytecode instruction-by-instruction will consume
exactly the correct number of bytes from the payload; the length field
provides an independent boundary that allows skipping events whose tag is
absent from the database.

### 5.3 Enum Events

When `#[derive(zfmt)]` is applied to an enum, each variant is a first-class
event with its own tag derived from its canonical hash input (§3.2).  The
enum type itself has no wire tag and does not appear independently in the
stream.

Logging an enum variant emits the **variant's tag** directly:

```rust
#[derive(zfmt)]
pub enum SensorEvent {
    #[zfmt(format = "temperature={celsius}")]
    Temperature { celsius: f32 },
    #[zfmt(format = "pressure={pascals}")]
    Pressure { pascals: u32 },
}
```

`log_event!(SensorEvent::Temperature { celsius: 23.5 })` produces:

```
[SensorEvent::Temperature tag][len][celsius f32]
```

No discriminant byte appears on the wire; the tag alone identifies both the
enum type and the active variant.  A single database lookup returns the
variant name, enum name, and field layout — sufficient for a host to
re-materialize a typed value.

Each variant follows the same tier rules as a struct: Tier-1 if all fields
are fixed-size, Tier-2 if any field is variable-length.

---

## 6. Stream Format

### 6.1 Wire Encoding

The stream is a flat sequence of tagged items:

```
stream  = item*
item    = tag length payload
tag     = u32        -- little-endian, lower 32 bits of FNV-1a 64-bit hash
length  = LEB128     -- unsigned, byte count of the payload that follows
payload = <bytes described by the bytecode for this tag>
```

There are no inter-item separators or sync bytes.  The stream is
self-delimiting: a decoder with the bytecode for a given tag consumes exactly
`length` bytes as the payload.  A decoder that does not recognise a tag may
skip `length` bytes and resume at the next item.

### 6.2 Endianness

All values in the stream are **little-endian**.  This includes tags,
multi-byte integer fields, and LEB128 sequences.

### 6.3 Logical Event Structure

A logged event is represented as two consecutive items in the stream:

```
[EventHeader tag][EventHeader length][EventHeader payload][Event tag][Event length][Event payload]
```

A bare event without a header (no timestamp or severity) is also valid:

```
[Event tag][Event length][Event payload]
```

The `log_event!` macro always writes the header+event pair.

---

## 7. Well-Known Events

The following events are defined by this specification and have reserved,
spec-computed tags.  Implementations must not use these tags for
application-defined events.

### 7.1 Severity

`Severity` is a `repr(u8)` enum used as a field in `EventHeader`.

```rust
#[repr(u8)]
pub enum Severity {
    Trace = 0,
    Debug = 1,
    Info  = 2,
    Warn  = 3,
    Error = 4,
    Fatal = 5,
}
```

Display strings are `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`, `FATAL`.
Values 6–255 are reserved.

### 7.2 EventHeader

Emitted immediately before every logged event.

```rust
#[repr(C)]
#[derive(zfmt)]
#[zfmt(format = "{timestamp} {severity}")]
pub struct EventHeader {
    pub timestamp: ZfmtU64,  // ticks since boot, as two u32 halves [lo, hi]
    pub severity:  Severity,
    pub seq:       [u8; 3],  // 24-bit little-endian sequence counter
}
// sizeof = 12 bytes
```

The tick rate (ticks per second) is communicated via `StreamStart` (§7.3).
The host decoder scales `timestamp` to wall-clock time using that rate.

`seq` is a 24-bit unsigned counter (0–16,777,215) assigned by the Logger
implementation at the point of emission.  It wraps at 2²⁴.  The host decoder
checks `seq` for gaps when `StreamStart.protocol_version >= 2` (§7.3); a gap
indicates that one or more events were dropped before this header.  Loggers
that do not provide sequence numbers (e.g. IPC client loggers that forward
events to a central handler) must return 0 from `Logger::next_seq` (§12.1),
which is the default.  When all loggers return 0, `protocol_version` is left
at 1 and the host ignores the `seq` bytes.

The bytecode for `EventHeader` includes the `seq` field as
`UTF8_BYTE|FIXED_ARRAY 3` — it is consumed by the decoder but is not
referenced by the format string `"{timestamp} {severity}"`, so generic
renderers emit only the timestamp and severity.  The host EventHeader handler
reads `seq` directly from known byte offsets [9..12].

Canonical hash:
```
full_hash = 0x5a19e4cfe43ae42d
tag       = 0xe43ae42d
```

### 7.3 StreamStart

The first event emitted after firmware boot.  Declares stream metadata
required for correct decoding.

```rust
#[repr(C)]
#[derive(zfmt)]
pub struct StreamStart {
    pub protocol_version:  u16,
    pub _pad:              [u8; 2],
    pub tick_rate_hz:      ZfmtU64,  // ticks per second, as two u32 halves [lo, hi]
    pub firmware_build_id: ZfmtU64,  // FNV-1a 64-bit hash of the firmware ELF
}
// sizeof = 20 bytes
```

`firmware_build_id` allows host tooling to locate the correct ELF and extract
its linker sections.

`protocol_version` declares the stream format:

| Value | Meaning |
|-------|---------|
| 1 | Baseline: `EventHeader.seq` bytes are always zero; host ignores them |
| 2 | Sequence numbers active: host tracks `EventHeader.seq` and annotates gaps |

Implementations that use `Logger::next_seq` (§12.1) must emit `StreamStart`
with `protocol_version = 2`.  The constant `StreamStart::PROTOCOL_VERSION`
reflects the version produced by the current implementation.

**Re-emitting StreamStart** is optional and requires no library support —
the same `log_bare_event!` call used at boot is sufficient.  Consider
re-emitting periodically if your deployment includes any of:

- A host that may connect after firmware has been running (e.g. a UART where
  the client is not guaranteed to be present at power-on).
- A host that may reconnect after a disconnect (e.g. USB-CDC on device that
  stays powered while the host restarts).
- A host that reads from a ring buffer that may have wrapped, discarding the
  initial `StreamStart`.

When and how often to re-emit is an application decision; the host decoder
treats repeated `StreamStart` frames as an updated anchor for tick-rate
scaling and sequence tracking and does not treat them as errors.  Many
deployments — particularly those with a permanently connected debug link or
where the host is always present at boot — have no reason to re-emit at all.
The alternative to re-emission is to supply `--tick-rate-hz` and optionally
`--protocol-version` directly to `zfmt decode` (§15.3).

### 7.4 DroppedEvents

Emitted when the firmware's log buffer recovers after an overflow.

```rust
#[repr(C)]
#[derive(zfmt)]
pub struct DroppedEvents {
    pub count: u32,   // number of events dropped since last DroppedEvents
    pub _pad:  [u8; 4],
}
// sizeof = 8 bytes
```

### 7.5 DebugMessage

Emitted by the unstructured logging macros (§13.3).  The `message` field
carries a pre-formatted UTF-8 string produced on the firmware side.

```rust
#[derive(zfmt)]
#[zfmt(format = "{message}")]
pub struct DebugMessage<'a> {
    pub message: &'a str,   // canonical type: str; tier-2 variable-length field
}
```

The lifetime `'a` ties the event to the stack buffer holding the formatted
string.  `DebugMessage` is a Tier-2 event; its wire payload is a LEB128
length followed by the UTF-8 bytes of `message`.

---

## 8. Linker Section Layout

The proc-macro emits data into three ELF linker sections using a
`linkme`-style distributed-slice technique.  Within each section, entries are
sorted by their hash key, enabling O(log n) binary search.

### 8.1 Event Table

Section name: `.zfmt_events`

Each entry is placed in a subsection named `.zfmt_events.<hex-tag>` where
`<hex-tag>` is the 8-digit lowercase hexadecimal tag.  The linker sorts
subsections lexicographically, producing a table sorted by tag.

Entry layout:

```
tag          : u32   -- 32-bit stream tag (sort key)
_pad         : u32
full_hash    : u64   -- full 64-bit FNV-1a hash (for collision detection)
format_hash  : u32   -- key into string table for the format string
_pad         : u32
bytecode_len : u32   -- length of the bytecode that follows
bytecode     : [u8]  -- variable-length; padded to 4-byte boundary
```

### 8.2 String Table

Section name: `.zfmt_strings`

Each entry is placed in a subsection named `.zfmt_strings.<hex-hash>` where
`<hex-hash>` is the 8-digit lowercase hexadecimal FNV-1a hash of the string
content.

Entry layout:

```
hash   : u32   -- FNV-1a 32-bit hash of string content (sort key)
len    : u16   -- byte length of the string
_pad   : u16
bytes  : [u8]  -- UTF-8 string content, no null terminator; padded to 4-byte boundary
```

The string hash is the lower 32 bits of the FNV-1a 64-bit hash of the string
bytes.

### 8.3 Subroutine Calls

The event table serves as the unified subroutine table.  Entries are added
for three categories:

- **Structs**: one entry per `#[derive(zfmt)]` struct, keyed by its tag.
- **Top-level enum variants**: one entry per variant of a `#[derive(zfmt)]`
  enum, keyed by the variant's tag (§3.2).  The enum type itself has no
  entry; variants are the independently logged events.
- **Inline enum variants**: one entry per variant of a `repr(C, uN)` enum
  used as a field within a `#[derive(zfmt)]` struct, keyed by the variant's
  tag and used as a subroutine by the `dispatch` opcode.

The `call` opcode (§4.6) and `dispatch` opcode (§4.5) reference subroutines
by their 32-bit tag; the decoder binary-searches the event table to locate
them.

---

## 9. Host Database

The host database is an append-only SQLite store mapping 64-bit full hashes
to event table entries (format string + bytecode).  Entries are extracted
from the `.zfmt_events` and `.zfmt_strings` sections of each firmware ELF
at integration time.

The database allows the host decoder to display events from any firmware
version as long as the corresponding ELF has been ingested, regardless of
whether that firmware is still in use.

### 9.1 Schema

```sql
CREATE TABLE events (
    tag         TEXT NOT NULL PRIMARY KEY,  -- hex u32
    full_hash   TEXT NOT NULL UNIQUE,       -- hex u64
    format_hash TEXT NOT NULL,              -- hex u32, FK into strings
    bytecode    BLOB NOT NULL
);

CREATE TABLE strings (
    hash    TEXT NOT NULL PRIMARY KEY,      -- hex u32
    content TEXT NOT NULL
);

CREATE TABLE ingested_builds (
    build_id    TEXT NOT NULL PRIMARY KEY,  -- hex u64 from StreamStart.firmware_build_id
    ingested_at TEXT NOT NULL               -- ISO 8601
);
```

Hash values are stored as lowercase hexadecimal TEXT to avoid SQLite's
signed 64-bit integer representation truncating u64 values that exceed
`i64::MAX`.

### 9.2 Collision Policy

- If a new ELF provides an entry whose full 64-bit hash already exists in the
  database with identical content, the entry is silently skipped (idempotent).
- If a new ELF provides an entry whose full 64-bit hash matches an existing
  entry with **different** content, ingestion must fail with an error.
- If a new ELF provides an entry whose 32-bit tag matches an existing entry
  with a different full 64-bit hash, ingestion must fail with an error.

### 9.3 Version Control

The database is a first-class versioned artifact of the project.  It is
committed to the project's source repository alongside the firmware source.
The conventional default location within a project is:

```
zfmt/events.db
```

This path may be overridden via the `database` key in `.zfmt.toml`.

Because SQLite files are binary and do not diff meaningfully, `zfmt ingest`
automatically regenerates a companion plaintext export at `<database>.txt`
(e.g. `zfmt/events.db.txt`) after every successful ingest.  Both files are
committed together.  The companion export serves as the human-readable diff
target in code review.

Companion export format:

```
# zfmt event database export
# generated 2026-05-17T20:11:20Z

[event a3f2c1b0]
full_hash   = cbf29ce4a3f2c1b0feedface
format      = The quick brown {clever_animal} jumped over the lazy {lazy_animal} {n} times!
fields      = clever_animal:str lazy_animal:str n:u32
bytecode    = 4b 4b 18 00

[string 2a9f4c81]
content     = The quick brown {clever_animal} jumped over the lazy {lazy_animal} {n} times!
```

---

## 10. Format String Syntax

Format strings are UTF-8 literals that control how event fields are rendered
during firmware-side formatting (§11) and on the host decoder.  They appear
in `#[zfmt(format = "...")]` attributes on structs and enum variants and are
part of the canonical hash input (§3.2).  The same syntax is used in the
unstructured logging macros (§13).

### 10.1 Placeholders

A placeholder takes one of two forms:

```
{name}        -- render field `name` with default formatting
{name:spec}   -- render field `name` with format specifier `spec`
```

`name` is one of:

- A struct field identifier, for named struct fields
- A decimal index (`0`, `1`, …) for tuple struct or tuple variant fields (§3.2)
- An explicit binding name supplied at the macro call site (§13.3), when the
  value to log is an expression rather than a simple in-scope identifier

Positional placeholders (unnamed `{}`) are not supported; all placeholders
must be named.

### 10.2 Format Specifiers

The optional specifier after `:` is composed of the following components,
all of which are optional, in the order shown:

| Component | Syntax | Applies to | Effect |
|-----------|--------|------------|--------|
| Sign | `+` | integers, floats | Always emit a sign character |
| Alternate | `#` | `x` `X` `b` `o` | Prefix: `0x`, `0X`, `0b`, or `0o` |
| Zero-pad | `0`*N* | integers | Right-align with zero fill to *N* digits |
| Left-align | `<`*N* | all types | Left-justify with space fill to width *N* |
| Right-align | `>`*N* | all types | Right-justify with space fill to width *N* |
| Precision | `.*N*` | floats | *N* decimal places |
| Type | `x` `X` `b` `o` | integers | Radix; default is decimal |

`0`*N* and `<`*N* / `>`*N* are mutually exclusive.  `#` is only meaningful
with the `x`, `X`, `b`, and `o` types.

### 10.3 Examples

```
{addr:#010x}   -- 32-bit address, 0x prefix, zero-padded to 10 characters total
{flags:08b}    -- 8-bit flags, zero-padded binary, 8 digits
{level:>6}     -- decimal integer, right-aligned in 6-character field
{label:<12}    -- string, left-aligned in 12-character field
{temp:.2}      -- float, 2 decimal places
{count:+}      -- integer, always show sign
```

---

## 11. Firmware-Side Formatting

The `zfmt` firmware crate provides optional in-firmware event formatting with
no dependency on `core::fmt` or external formatting libraries.  When active,
the proc-macro-generated `format_into` method renders an event to any value
implementing the `Write` trait (§11.1).

### 11.1 Write Trait

```rust
pub trait Write {
    fn write_str(&mut self, s: &str) -> Result<(), Error>;

    fn write_char(&mut self, c: char) -> Result<(), Error> {
        self.write_str(c.encode_utf8(&mut [0u8; 4]))
    }
}
```

The trait deliberately excludes `write_fmt`; accepting `core::fmt::Arguments`
would re-introduce the `core::fmt` dependency.  Any destination that accepts
UTF-8 text — a fixed-size stack buffer, a UART driver, an IPC message buffer
— implements `Write`.

### 11.2 FormatSpec

`FormatSpec` captures the parsed specifier for a single placeholder.  In
proc-macro-generated code every `FormatSpec` value is a compile-time constant;
the compiler constant-folds `fmt` calls and eliminates dead branches for
unused flags.

```rust
pub struct FormatSpec {
    pub ty:        FormatType,   // Display, LowerHex, UpperHex, Binary, Octal
    pub alternate: bool,         // # flag
    pub sign:      bool,         // + flag
    pub zero_pad:  bool,         // 0N flag (right-align, zero fill)
    pub width:     u8,           // 0 = no width constraint
    pub precision: Option<u8>,   // .N for float decimal places; None = default (6)
    pub align:     Align,        // None, Left, Right
}

pub enum FormatType { Display, LowerHex, UpperHex, Binary, Octal }
pub enum Align      { None, Left, Right }
```

### 11.3 Format Trait

```rust
pub trait Format {
    fn fmt<W: Write>(&self, writer: &mut W, spec: FormatSpec)
        -> Result<(), Error>;
}
```

The `zfmt` crate provides `Format` implementations for every primitive type
listed in §3.3.  Custom types used as event fields must implement `Format`
when firmware-side formatting is required.

### 11.4 Generated `format_into` Method

The proc-macro generates a `format_into` method for every `#[derive(zfmt)]`
type that carries a `#[zfmt(format = "...")]` attribute.  The method
interleaves literal text segments with per-field `fmt` calls:

```rust
// Generated for:
// #[zfmt(format = "addr={addr:#010x} count={count}")]
fn format_into<W: Write>(&self, w: &mut W) -> Result<(), Error> {
    w.write_str("addr=")?;
    self.addr.fmt(w, FormatSpec {
        ty: FormatType::LowerHex, alternate: true,
        zero_pad: true, width: 10, ..FormatSpec::default()
    })?;
    w.write_str(" count=")?;
    self.count.fmt(w, FormatSpec::default())?;
    Ok(())
}
```

### 11.5 Alignment Implementation

**Left-align** (`<`*N*): the value is written first; a lightweight counting
wrapper around `W` tracks bytes written during the value call, then
`width − count` space characters follow.  No temporary buffer is required.

**Right-align** (`>`*N*) and **zero-pad** (`0`*N*): the display width of the
value is computed arithmetically before writing (digit count for integers,
byte length for strings), fill characters are emitted, then the value is
written.  No temporary buffer is required.

---

## 12. Logger Interface

### 12.1 Logger Trait

```rust
pub trait Logger {
    /// Returns the current timestamp in ticks.
    fn timestamp(&self) -> u64;

    /// Returns the next 24-bit sequence counter value for `EventHeader.seq`.
    ///
    /// The value is masked to 24 bits before being written.  The default
    /// implementation returns 0, keeping `protocol_version = 1` semantics.
    /// Override in the central log-handling task's Logger; IPC-client loggers
    /// that forward events to a central handler should leave the default.
    fn next_seq(&self) -> u32 { 0 }

    /// Sends `bufs` as a single logical message (scatter-gather write).
    fn send_vectored(&self, bufs: &[&[u8]]);

    /// Convenience wrapper; default calls `send_vectored(&[data])`.
    fn send(&self, data: &[u8]) {
        self.send_vectored(&[data]);
    }
}
```

`send_vectored` is the required method.  The logging macros always call
`send_vectored`, passing the wire-format segments of an event (tag bytes,
length bytes, payload bytes) as separate slices.  Implementations that support
scatter-gather IPC handle these slices natively with no intermediate copy.

Both methods take `&self` rather than `&mut self`.  IPC send operations are
inherently shared — the kernel serialises concurrent callers — and the
task-local static pattern (§12.3) means exclusive access is already guaranteed
by construction.  Implementations that must mutate internal state (e.g., a
software ring-buffer logger maintaining a write pointer) should use interior
mutability (`UnsafeCell`, atomics, or `RefCell`) within their type.

### 12.2 FlatSend and FlatAdapter

For output paths that accept only a single contiguous buffer, `FlatAdapter`
assembles the vectored slices into a fixed-size stack buffer before
forwarding:

```rust
pub trait FlatSend {
    fn timestamp(&self) -> u64;
    fn send(&self, data: &[u8]);
}

pub struct FlatAdapter<L: FlatSend, const N: usize> {
    inner: L,
}

impl<L: FlatSend, const N: usize> Logger for FlatAdapter<L, N> {
    fn timestamp(&self) -> u64 { self.inner.timestamp() }

    fn send_vectored(&self, bufs: &[&[u8]]) {
        let mut buf = [0u8; N];
        let mut pos = 0;
        for b in bufs {
            buf[pos..pos + b.len()].copy_from_slice(b);
            pos += b.len();
        }
        self.inner.send(&buf[..pos]);
    }
}
```

The const parameter `N` is the maximum total wire size of any event the task
may log.  It determines the stack frame cost of each `send_vectored` call and
is chosen per-task.

### 12.3 Task-Local Static Logger

Each task declares a static of a concrete logger type:

```rust
// Scatter-gather IPC — implements Logger directly:
static LOGGER: MyVectoredShim = MyVectoredShim::new();

// Flat IPC — wraps FlatSend in FlatAdapter:
static LOGGER: FlatAdapter<MyFlatShim, 256> = FlatAdapter::new(MyFlatShim::new());
```

In a process-isolated RTOS, each task has its own address space; a task-level
static is task-local by construction.  No cross-task sharing, no
synchronisation, and no `unsafe` global mutable state is required.

### 12.4 Static Dispatch

No dynamic dispatch (`dyn Trait`) appears anywhere in the logging call path.
The macros reference the concrete `LOGGER` static directly; the compiler
knows the exact type at every call site and generates direct calls throughout.
The `bufs: &[&[u8]]` parameter is a slice (a fat pointer carrying a length,
not a vtable pointer).

The complete call graph for any logging statement is statically resolvable,
enabling tooling to compute worst-case stack depth — a requirement for many
embedded safety analyses.

---

## 13. Logging Macros

### 13.1 Macro Family

One macro per severity level:

| Macro | Severity |
|-------|----------|
| `log_trace!` | Trace |
| `log_debug!` | Debug |
| `log_info!` | Info |
| `log_warn!` | Warn |
| `log_error!` | Error |
| `log_fatal!` | Fatal |

Each macro is overloaded by its first argument: a string literal selects the
unstructured text path (§13.3); any other expression selects the structured
event path (§13.2).

### 13.2 Structured Events

```rust
log_info!(TempReading { celsius });
log_warn!(SensorEvent::Pressure { pascals });
```

The macro:

1. Calls `LOGGER.timestamp()` to obtain the current tick count
2. Serializes `EventHeader` + the event into wire format, using `payload_size`
   to size the slices (§5.1, §5.2)
3. Calls `LOGGER.send_vectored()` with the wire-format slices

If the `output-text` or `output-both` Cargo feature is active (§13.5), the
macro also calls `event.format_into()` and sends the result as a
`DebugMessage` event (§7.5) through the same logger.

### 13.3 Unstructured Text Events

```rust
log_debug!("x={x:#010x} after {tm_ms}ms");
log_warn!("unexpected state: {state}", state = device.state());
```

Named bindings (`name = expr`) are required when the value to log is not a
simple in-scope identifier.  The format string syntax is the same as for
structured events (§10).

The macro pre-formats the message into a fixed-size stack buffer using the
firmware formatting engine (§11), then emits the result as a `DebugMessage`
event (§7.5).  The stack buffer defaults to 128 bytes; this limit is
configurable via the `debug-buffer-size` Cargo feature.

A compile-time warning is emitted when an unstructured text event appears in
`log_info!`, `log_warn!`, `log_error!`, or `log_fatal!`, encouraging use of
structured events for production-relevant log statements.  The warning can be
suppressed with `#[allow(deprecated)]` at the call site.

### 13.4 Compile-Time Severity Filtering

Events below the configured minimum level expand to nothing at compile time —
no code emitted, no stack frame, no side effects.

| Cargo feature | Minimum emitted level |
|---------------|-----------------------|
| `log-level-trace` | Trace |
| `log-level-debug` | Debug |
| `log-level-info` | Info (default) |
| `log-level-warn` | Warn |
| `log-level-error` | Error |

### 13.5 Output Modes

| Cargo feature | Behavior |
|---------------|----------|
| `output-binary` (default) | Serialize event to wire format; call `LOGGER.send_vectored()` |
| `output-text` | Call `format_into()`; send result as `DebugMessage` via `LOGGER.send_vectored()` |
| `output-both` | Perform both operations in sequence |

---

## 14. Crate Structure

The implementation is organized as a Cargo workspace:

```
zfmt/
├── zfmt-macro/    proc-macro crate: #[derive(zfmt)], zfmt_str!
├── zfmt/          firmware library: no_std; re-exports macro;
│                  Write, Format, FormatSpec, Logger, FlatSend,
│                  FlatAdapter, and the logging macros
└── zfmt-host/     host tooling: ELF parser, database, stream
                   decoder, formatter; exposes both a library
                   API and a CLI binary
```

`zfmt` and `zfmt-macro` are `no_std` compatible.  `zfmt-host` requires
`std` and carries heavier dependencies (`object` or `gimli` for ELF
parsing, `rusqlite` for the database).

### 14.1 Optional Features

| Cargo feature | Effect |
|---------------|--------|
| `no-float` | Disables `f32` and `f64` as event field types; the proc-macro emits a compile error if either type appears in a `#[derive(Zfmt)]` struct or variant; `Format` implementations for `f32`/`f64` are compiled out |
| `no-64bit` | Disables `u64` and `i64` as event field types (same compile-error behaviour); `ZfmtU64::from_u64()`, `ZfmtU64::to_u64()`, and the `From<u64>`/`From<ZfmtU64>` conversions are compiled out (they require 64-bit arithmetic); under this feature `ZfmtU64` formats as 16 lowercase hex digits using only 32-bit shifts and masks; `ZfmtU64` itself remains available and is the correct type for timestamps and other 64-bit values |

`ZfmtU64` is re-exported from the `zfmt` crate alongside `Write`, `Format`,
`Logger`, `FlatSend`, and `FlatAdapter`.

For each `#[derive(zfmt)]` type the proc-macro generates:

- A `.zfmt_events` linker section entry (§8.1)
- `payload_size(&self) -> usize` — total serialized byte count (§5.1, §5.2)
- `serialize_into(&self, buf: &mut [u8])` — writes the wire-format payload
- `format_into<W: Write>(&self, w: &mut W) -> Result<(), Error>` — renders
  the event as text using the format string (§11.4); generated only when a
  `#[zfmt(format = "...")]` attribute is present

The minimum supported Rust version is **1.77**, required for stable
`core::mem::offset_of!` used during bytecode generation.

---

## 15. CLI Reference

The `zfmt` binary is built from `zfmt-host`.  All commands accept
`--database <path>` to override the project default.

### 15.1 `zfmt ingest`

Extracts `.zfmt_events` and `.zfmt_strings` sections from an ELF and
ingests them into the database.  Idempotent: re-ingesting the same ELF
produces no change.  Regenerates the companion plaintext export on every
successful run.

```sh
zfmt ingest [--database <path>] <elf>
```

### 15.2 `zfmt check`

Validates the events and strings in an ELF against the database without
modifying it.  Detects same-build tag collisions within the ELF itself and
cross-database wire collisions or full-hash collisions against accumulated
history.  Exits non-zero and prints a descriptive error if any collision is
found; exits zero and reports counts on success.

```sh
zfmt check [--database <path>] <elf>
```

Use `zfmt check` in build and test flows where modifying the database is not
appropriate — for example, in a CI job that validates a pull-request build
without writing to the version-controlled database, or in a development
workflow where the engineer wants to confirm safety before running
`zfmt ingest`.

### 15.3 `zfmt decode`

Reads a binary event stream and prints human-readable output.  Multiple
`--database` flags are accepted; databases are searched in the order given
and the first match for a given tag is used.

```sh
zfmt decode [--database <path>]... [--tick-rate-hz <hz>] [--protocol-version <n>] <stream>
```

`--tick-rate-hz` and `--protocol-version` supply initial decoder state for
streams that do not begin with a `StreamStart` frame — for example, when a
UART client attached after boot and the initial `StreamStart` was not
captured.  A `StreamStart` frame encountered in the stream always overrides
these values.

| Flag | Default | Effect |
|------|---------|--------|
| `--tick-rate-hz <hz>` | 0 (raw ticks) | Fallback tick rate for timestamp scaling |
| `--protocol-version <n>` | 1 | 1 = ignore seq bytes; 2 = report sequence gaps |

### 15.4 `zfmt verify`

Checks that every event type referenced in an ELF is already present in the
database.  Exits non-zero if any event is missing.  Intended as a release
gate in CI.

```sh
zfmt verify [--database <path>] <elf>
```

### 15.5 `zfmt db create`

Creates a new empty database at the given path.

```sh
zfmt db create <path>
```

### 15.6 `zfmt db merge`

Copies all entries from `<src>` into `<dst>`, applying the standard
collision policy.  Useful for seeding a personal development database from
the project database.

```sh
zfmt db merge <src> <dst>
```

### 15.7 `zfmt db list`

Prints all events in the database in companion-export format.

```sh
zfmt db list [--database <path>]
```

---

## 16. Release Pipeline Integration

The recommended release pipeline runs the following steps after building
the firmware ELF:

```sh
# 1. Validate: check for collisions without modifying the database
zfmt check --database zfmt/events.db firmware.elf

# 2. Ingest events into the version-controlled database
zfmt ingest --database zfmt/events.db firmware.elf

# 3. Verify all events are present (catches ingest failures)
zfmt verify --database zfmt/events.db firmware.elf

# 4. Commit updated database and companion export
git add zfmt/events.db zfmt/events.db.txt
git commit -m "chore: ingest events for release v1.4.2"
git tag v1.4.2
```

`zfmt check` in step 1 is technically redundant with `zfmt ingest` (ingest
performs the same collision checks before writing), but running it separately
makes the pipeline failure mode more explicit: a non-zero exit from `check`
identifies a collision, while a non-zero exit from `ingest` could also
indicate a filesystem or database error.  It is also useful in CI jobs that
validate pull-request builds without touching the shared database.

---

## 17. Developer Workflow

Developers maintain a personal database seeded from the project database
and extended freely with debug-only events.  Personal databases are not
committed to the project repository.

```sh
# One-time setup: seed personal database from project database
zfmt db merge zfmt/events.db ~/zfmt-dev.db

# During development: ingest a local debug build
zfmt ingest --database ~/zfmt-dev.db target/debug/firmware.elf

# Decode using project events and personal debug events
zfmt decode \
    --database zfmt/events.db \
    --database ~/zfmt-dev.db \
    stream.bin
```
