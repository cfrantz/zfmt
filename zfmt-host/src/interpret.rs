//! Bytecode interpreter (§4) — walks bytecode and payload, producing decoded values.
//!
//! # Positional matching
//!
//! Format string placeholders are matched positionally to decoded values: the
//! N-th placeholder (left-to-right in the format string) is paired with the
//! N-th non-skip value produced by the bytecode.  This is correct whenever the
//! format string references fields in the same order they are declared in the
//! struct — the natural convention and the one enforced by the derive macro for
//! generated `format_into` calls.

use anyhow::{bail, Context, Result};

use crate::db::Db;

// ---------------------------------------------------------------------------
// Item / operand type constants (§4.2, §4.3)

mod item {
    pub const END:        u8 = 0;
    pub const U8:         u8 = 1;
    pub const U16:        u8 = 2;
    pub const U32:        u8 = 3;
    pub const U64:        u8 = 4;
    pub const I8:         u8 = 5;
    pub const I16:        u8 = 6;
    pub const I32:        u8 = 7;
    pub const I64:        u8 = 8;
    pub const UTF8_BYTE:  u8 = 9;
    pub const SKIP:       u8 = 10;
    pub const F32:        u8 = 11;
    pub const F64:        u8 = 12;
    pub const BOOL:       u8 = 13;
    pub const DISPATCH:   u8 = 14;
    pub const CALL:       u8 = 15;
    pub const STRING_REF: u8 = 16;
    pub const U64_PAIR:   u8 = 17;
}

mod operand {
    pub const SINGLE:      u8 = 0;
    pub const FIXED_ARRAY: u8 = 1;
    pub const ZERO_TERM:   u8 = 2;
    pub const VAR_LENGTH:  u8 = 3;
}

const MAX_DEPTH: u8 = 4;

// ---------------------------------------------------------------------------
// Value — decoded field value

#[derive(Debug, Clone)]
pub enum Value {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Str(String),
    Array(Vec<Value>),
}

impl Value {
    /// Default display (decimal for integers, 6-place for floats, etc.).
    pub fn display_default(&self) -> String {
        match self {
            Value::U8(v)  => v.to_string(),
            Value::U16(v) => v.to_string(),
            Value::U32(v) => v.to_string(),
            Value::U64(v) => v.to_string(),
            Value::I8(v)  => v.to_string(),
            Value::I16(v) => v.to_string(),
            Value::I32(v) => v.to_string(),
            Value::I64(v) => v.to_string(),
            Value::F32(v) => format_float(*v as f64, None),
            Value::F64(v) => format_float(*v, None),
            Value::Bool(v) => if *v { "true" } else { "false" }.to_owned(),
            Value::Str(s)  => s.clone(),
            Value::Array(items) => {
                let parts: Vec<_> = items.iter().map(|v| v.display_default()).collect();
                format!("[{}]", parts.join(", "))
            }
        }
    }

    /// Display the value according to a format specifier.
    pub fn display_spec(&self, spec: &Spec) -> String {
        // FourCC character display: intercept before delegating to fmt_uint/fmt_int.
        if spec.fmt_type == FmtType::Char {
            return match self {
                Value::U8(v)  => fmt_fourcc_host(&v.to_le_bytes()),
                Value::U16(v) => fmt_fourcc_host(&v.to_le_bytes()),
                Value::U32(v) => fmt_fourcc_host(&v.to_le_bytes()),
                Value::U64(v) => fmt_fourcc_host(&v.to_le_bytes()),
                Value::I8(v)  => fmt_fourcc_host(&v.to_le_bytes()),
                Value::I16(v) => fmt_fourcc_host(&v.to_le_bytes()),
                Value::I32(v) => fmt_fourcc_host(&v.to_le_bytes()),
                Value::I64(v) => fmt_fourcc_host(&v.to_le_bytes()),
                _ => self.display_default(),
            };
        }
        match self {
            Value::U8(v)  => fmt_uint(*v as u64, false, spec),
            Value::U16(v) => fmt_uint(*v as u64, false, spec),
            Value::U32(v) => fmt_uint(*v as u64, false, spec),
            Value::U64(v) => fmt_uint(*v, false, spec),
            Value::I8(v)  => fmt_int(*v as i64, *v as u64, spec),
            Value::I16(v) => fmt_int(*v as i64, *v as u16 as u64, spec),
            Value::I32(v) => fmt_int(*v as i64, *v as u32 as u64, spec),
            Value::I64(v) => fmt_int(*v, *v as u64, spec),
            Value::F32(v) => format_float(*v as f64, spec.precision),
            Value::F64(v) => format_float(*v, spec.precision),
            Value::Bool(v) => if *v { "true" } else { "false" }.to_owned(),
            Value::Str(s)  => apply_width(s, spec),
            Value::Array(items) => {
                let parts: Vec<_> = items.iter().map(|v| v.display_default()).collect();
                let s = format!("[{}]", parts.join(", "));
                apply_width(&s, spec)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Format specifier

#[derive(Debug, Clone, Default)]
pub struct Spec {
    pub align:     Align,
    pub sign:      bool,
    pub alternate: bool,
    pub zero_pad:  bool,
    pub width:     usize,
    pub precision: Option<u8>,
    pub fmt_type:  FmtType,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Align { #[default] None, Left, Right }

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum FmtType { #[default] Display, LowerHex, UpperHex, Binary, Octal, Char }

/// Parse a format spec string (the part after `:` in `{name:spec}`).
pub fn parse_spec(s: &str) -> Result<Spec> {
    let mut spec = Spec::default();
    let mut it = s.chars().peekable();

    if let Some(&c) = it.peek() {
        if c == '<' || c == '>' {
            spec.align = if c == '<' { Align::Left } else { Align::Right };
            it.next();
        }
    }
    if it.peek() == Some(&'+') { spec.sign = true; it.next(); }
    if it.peek() == Some(&'#') { spec.alternate = true; it.next(); }
    if it.peek() == Some(&'0') { spec.zero_pad = true; it.next(); }

    let mut w = String::new();
    while matches!(it.peek(), Some(&c) if c.is_ascii_digit()) { w.push(it.next().unwrap()); }
    if !w.is_empty() {
        spec.width = w.parse::<usize>().unwrap_or(0);
    }

    if it.peek() == Some(&'.') {
        it.next();
        let mut p = String::new();
        while matches!(it.peek(), Some(&c) if c.is_ascii_digit()) { p.push(it.next().unwrap()); }
        if p.is_empty() { bail!("`.` in format spec must be followed by a digit"); }
        spec.precision = Some(p.parse::<u8>().unwrap_or(6));
    }

    match it.next() {
        None => {}
        Some('x') => spec.fmt_type = FmtType::LowerHex,
        Some('X') => spec.fmt_type = FmtType::UpperHex,
        Some('b') => spec.fmt_type = FmtType::Binary,
        Some('o') => spec.fmt_type = FmtType::Octal,
        Some('c') => spec.fmt_type = FmtType::Char,
        Some(c)   => bail!("unknown format type `{c}`"),
    }

    Ok(spec)
}

// ---------------------------------------------------------------------------
// Rendering helpers

fn apply_width(s: &str, spec: &Spec) -> String {
    if spec.width == 0 || s.len() >= spec.width {
        return s.to_owned();
    }
    let pad = spec.width - s.len();
    match spec.align {
        Align::Left => format!("{s}{}", " ".repeat(pad)),
        Align::Right | Align::None => format!("{}{s}", " ".repeat(pad)),
    }
}

fn fmt_fourcc_host(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        if b >= 0x20 && b <= 0x7e {
            out.push(b as char);
        } else {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            out.push('\\');
            out.push('x');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0xf) as usize] as char);
        }
    }
    out
}

fn fmt_uint(v: u64, negative: bool, spec: &Spec) -> String {
    let (radix, upper) = match spec.fmt_type {
        FmtType::Display  => (10u64, false),
        FmtType::LowerHex => (16u64, false),
        FmtType::UpperHex => (16u64, true),
        FmtType::Binary   => (2u64,  false),
        FmtType::Octal    => (8u64,  false),
        // Char is intercepted in display_spec before fmt_uint is called.
        FmtType::Char     => (10u64, false),
    };
    let digits = to_digits(v, radix, upper);
    let sign = if negative { "-" } else if spec.sign { "+" } else { "" };
    let prefix = if spec.alternate {
        match spec.fmt_type {
            FmtType::LowerHex => "0x",
            FmtType::UpperHex => "0X",
            FmtType::Binary   => "0b",
            FmtType::Octal    => "0o",
            FmtType::Display | FmtType::Char => "",
        }
    } else { "" };

    let content_len = sign.len() + prefix.len() + digits.len();
    if spec.zero_pad && spec.align == Align::None && spec.width > content_len {
        let z = spec.width - content_len;
        format!("{sign}{prefix}{}{digits}", "0".repeat(z))
    } else {
        let s = format!("{sign}{prefix}{digits}");
        apply_width(&s, spec)
    }
}

fn fmt_int(v: i64, bits: u64, spec: &Spec) -> String {
    match spec.fmt_type {
        FmtType::Display => {
            let neg = v < 0;
            fmt_uint(v.unsigned_abs(), neg, spec)
        }
        _ => fmt_uint(bits, false, spec),
    }
}

fn to_digits(mut v: u64, radix: u64, upper: bool) -> String {
    if v == 0 { return "0".to_owned(); }
    let mut buf = Vec::new();
    while v > 0 {
        let d = (v % radix) as u8;
        buf.push(if d < 10 {
            b'0' + d
        } else if upper {
            b'A' + d - 10
        } else {
            b'a' + d - 10
        });
        v /= radix;
    }
    buf.reverse();
    String::from_utf8(buf).unwrap()
}

fn format_float(v: f64, precision: Option<u8>) -> String {
    if v.is_nan()      { return "NaN".to_owned(); }
    if v.is_infinite() { return if v > 0.0 { "inf" } else { "-inf" }.to_owned(); }
    let prec = precision.map(|p| p as usize).unwrap_or(6);
    let neg = v < 0.0;
    let abs = if neg { -v } else { v };
    let scale = 10u64.saturating_pow(prec as u32) as f64;
    let int_part = abs as u64;
    let frac_raw = ((abs - int_part as f64) * scale + 0.5) as u64;
    let (int_final, frac_final) = if frac_raw >= scale as u64 {
        (int_part + 1, 0u64)
    } else {
        (int_part, frac_raw)
    };
    let sign = if neg { "-" } else { "" };
    if prec == 0 {
        format!("{sign}{int_final}")
    } else {
        format!("{sign}{int_final}.{frac_final:0>prec$}")
    }
}

// ---------------------------------------------------------------------------
// Bytecode interpreter

/// Interpret `bc` against `payload`, returning the decoded values (non-skip fields).
/// `db` is used for CALL, DISPATCH, and STRING_REF instructions.
pub fn interpret(bc: &[u8], payload: &[u8], db: &Db) -> Result<Vec<Value>> {
    let mut bc_pos = 0usize;
    let mut p_pos  = 0usize;
    run_bc(bc, &mut bc_pos, payload, &mut p_pos, db, MAX_DEPTH)
}

fn run_bc(
    bc:      &[u8],
    bc_pos:  &mut usize,
    payload: &[u8],
    p_pos:   &mut usize,
    db:      &Db,
    depth:   u8,
) -> Result<Vec<Value>> {
    let mut values: Vec<Value> = Vec::new();

    loop {
        if *bc_pos >= bc.len() {
            bail!("bytecode overrun: missing END instruction");
        }
        let op       = bc[*bc_pos]; *bc_pos += 1;
        let item_ty  = op >> 3;
        let operand  = op & 0x07;

        match item_ty {
            item::END => break,

            item::SKIP => {
                // FIXED_ARRAY operand: LEB128 byte count follows in bytecode.
                if operand != operand::FIXED_ARRAY {
                    bail!("SKIP must use FIXED_ARRAY operand (got {operand})");
                }
                let (n, n_len) = leb128_bc(bc, *bc_pos)?;
                *bc_pos += n_len;
                *p_pos  += n as usize;
            }

            item::U8 | item::I8 | item::BOOL => {
                values.extend(read_items(1, item_ty, operand, bc, bc_pos, payload, p_pos)?);
            }
            item::U16 | item::I16 => {
                values.extend(read_items(2, item_ty, operand, bc, bc_pos, payload, p_pos)?);
            }
            item::U32 | item::I32 | item::F32 => {
                values.extend(read_items(4, item_ty, operand, bc, bc_pos, payload, p_pos)?);
            }
            item::U64 | item::I64 | item::F64 => {
                values.extend(read_items(8, item_ty, operand, bc, bc_pos, payload, p_pos)?);
            }

            item::UTF8_BYTE => {
                let s = read_utf8(operand, bc, bc_pos, payload, p_pos)?;
                values.push(Value::Str(s));
            }

            item::STRING_REF => {
                // Reads u32 hash from payload, looks up in strings table.
                let hash = read_u32_le(payload, *p_pos)?;
                *p_pos += 4;
                let content = db.lookup_string(hash)
                    .context("STRING_REF lookup")?
                    .unwrap_or_else(|| format!("<unknown string {:08x}>", hash));
                values.push(Value::Str(content));
            }

            item::CALL => {
                if depth == 0 { bail!("CALL: max call depth exceeded"); }
                let (tag, tag_len) = leb128_bc(bc, *bc_pos)?;
                *bc_pos += tag_len;
                let callee_bc = lookup_bytecode(db, tag as u32)?;
                let sub_values = call_bc(&callee_bc, payload, p_pos, db, depth - 1)?;
                values.extend(sub_values);
            }

            item::DISPATCH => {
                if depth == 0 { bail!("DISPATCH: max call depth exceeded"); }
                let sub_values = exec_dispatch(bc, bc_pos, payload, p_pos, db, depth - 1)?;
                values.extend(sub_values);
            }

            item::U64_PAIR => {
                if operand != operand::SINGLE {
                    bail!("U64_PAIR must use SINGLE operand (got {operand})");
                }
                need(payload, *p_pos, 8)?;
                let lo = u32::from_le_bytes(payload[*p_pos..*p_pos + 4].try_into().unwrap()) as u64;
                let hi = u32::from_le_bytes(payload[*p_pos + 4..*p_pos + 8].try_into().unwrap()) as u64;
                *p_pos += 8;
                values.push(Value::U64((hi << 32) | lo));
            }

            other => bail!("unsupported item type {other} in bytecode"),
        }
    }

    Ok(values)
}

// ---------------------------------------------------------------------------
// CALL helper

fn call_bc(
    callee_bc: &[u8],
    payload:   &[u8],
    p_pos:     &mut usize,
    db:        &Db,
    depth:     u8,
) -> Result<Vec<Value>> {
    let mut bc_pos = 0usize;
    run_bc(callee_bc, &mut bc_pos, payload, p_pos, db, depth)
}

fn lookup_bytecode(db: &Db, tag: u32) -> Result<Vec<u8>> {
    let events = db.all_events().context("lookup_bytecode: query db")?;
    events
        .into_iter()
        .find(|e| e.tag == tag)
        .map(|e| e.bytecode)
        .ok_or_else(|| anyhow::anyhow!("no subroutine for tag {:08x}", tag))
}

// ---------------------------------------------------------------------------
// DISPATCH helper

fn exec_dispatch(
    bc:      &[u8],
    bc_pos:  &mut usize,
    payload: &[u8],
    p_pos:   &mut usize,
    db:      &Db,
    depth:   u8,
) -> Result<Vec<Value>> {
    // Read dispatch table from bytecode.
    let (discrim_type, dl) = leb128_bc(bc, *bc_pos)?; *bc_pos += dl;
    let (padding,      pl) = leb128_bc(bc, *bc_pos)?; *bc_pos += pl;
    let (count,        cl) = leb128_bc(bc, *bc_pos)?; *bc_pos += cl;

    let mut table: Vec<(u64, u64)> = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let (val, vl) = leb128_bc(bc, *bc_pos)?; *bc_pos += vl;
        let (tag, tl) = leb128_bc(bc, *bc_pos)?; *bc_pos += tl;
        table.push((val, tag));
    }

    // Read discriminant from payload.
    let discrim_size = match discrim_type as u8 {
        item::U8  => 1usize,
        item::U16 => 2,
        item::U32 => 4,
        item::U64 => 8,
        other => bail!("DISPATCH: invalid discriminant type {other}"),
    };
    let discrim = read_uint_le(payload, *p_pos, discrim_size)?;
    *p_pos += discrim_size;
    *p_pos += padding as usize;

    // Find matching variant.
    match table.iter().find(|(v, _)| *v == discrim) {
        Some((_, tag)) => {
            let callee_bc = lookup_bytecode(db, *tag as u32)?;
            call_bc(&callee_bc, payload, p_pos, db, depth)
        }
        None => {
            // Unknown discriminant: emit a placeholder, no bytes consumed.
            Ok(vec![Value::Str(format!("<unknown discriminant {discrim}>"))])
        }
    }
}

// ---------------------------------------------------------------------------
// Scalar readers

fn read_items(
    size:    usize,
    item_ty: u8,
    operand: u8,
    bc:      &[u8],
    bc_pos:  &mut usize,
    payload: &[u8],
    p_pos:   &mut usize,
) -> Result<Vec<Value>> {
    let count = match operand {
        operand::SINGLE => 1usize,
        operand::FIXED_ARRAY => {
            let (n, nl) = leb128_bc(bc, *bc_pos)?; *bc_pos += nl;
            n as usize
        }
        operand::ZERO_TERM => {
            let (max, ml) = leb128_bc(bc, *bc_pos)?; *bc_pos += ml;
            // Find null byte in payload, up to max.
            let max = max as usize;
            let end = (0..max.min(payload.len() - *p_pos))
                .find(|&i| payload[*p_pos + i] == 0)
                .unwrap_or(max.min(payload.len().saturating_sub(*p_pos)));
            let n = end; // number of elements before null
            // We'll handle reading below by converting count→single scalar per element.
            // Return items then skip up to max (including null if present).
            let mut vals = Vec::with_capacity(n);
            for _ in 0..n {
                vals.push(decode_scalar(item_ty, payload, *p_pos, size)?);
                *p_pos += size;
            }
            // Advance past null and remaining padding up to max.
            *p_pos += (max - n).min(payload.len().saturating_sub(*p_pos));
            return Ok(if vals.len() == 1 { vals } else { vec![Value::Array(vals)] });
        }
        operand::VAR_LENGTH => {
            let (n, nl) = leb128_payload(payload, *p_pos)?; *p_pos += nl;
            n as usize
        }
        other => bail!("unknown operand type {other}"),
    };

    let mut vals = Vec::with_capacity(count);
    for _ in 0..count {
        vals.push(decode_scalar(item_ty, payload, *p_pos, size)?);
        *p_pos += size;
    }
    Ok(if vals.len() == 1 { vals } else { vec![Value::Array(vals)] })
}

fn decode_scalar(item_ty: u8, payload: &[u8], pos: usize, size: usize) -> Result<Value> {
    need(payload, pos, size)?;
    let b = &payload[pos..pos + size];
    Ok(match item_ty {
        item::U8   => Value::U8(b[0]),
        item::U16  => Value::U16(u16::from_le_bytes(b.try_into().unwrap())),
        item::U32  => Value::U32(u32::from_le_bytes(b.try_into().unwrap())),
        item::U64  => Value::U64(u64::from_le_bytes(b.try_into().unwrap())),
        item::I8   => Value::I8(b[0] as i8),
        item::I16  => Value::I16(i16::from_le_bytes(b.try_into().unwrap())),
        item::I32  => Value::I32(i32::from_le_bytes(b.try_into().unwrap())),
        item::I64  => Value::I64(i64::from_le_bytes(b.try_into().unwrap())),
        item::F32  => Value::F32(f32::from_le_bytes(b.try_into().unwrap())),
        item::F64  => Value::F64(f64::from_le_bytes(b.try_into().unwrap())),
        item::BOOL => Value::Bool(b[0] != 0),
        other => bail!("decode_scalar: unexpected item_ty {other}"),
    })
}

// ---------------------------------------------------------------------------
// UTF8_BYTE reader

fn read_utf8(
    operand: u8,
    bc:      &[u8],
    bc_pos:  &mut usize,
    payload: &[u8],
    p_pos:   &mut usize,
) -> Result<String> {
    let (byte_count, read_leb_from_payload) = match operand {
        operand::FIXED_ARRAY => {
            let (n, nl) = leb128_bc(bc, *bc_pos)?; *bc_pos += nl;
            (n as usize, false)
        }
        operand::ZERO_TERM => {
            let (max, ml) = leb128_bc(bc, *bc_pos)?; *bc_pos += ml;
            let max = max as usize;
            let end = (0..max.min(payload.len().saturating_sub(*p_pos)))
                .find(|&i| payload[*p_pos + i] == 0)
                .unwrap_or(max.min(payload.len().saturating_sub(*p_pos)));
            need(payload, *p_pos, max)?;
            let s = String::from_utf8_lossy(&payload[*p_pos..*p_pos + end]).into_owned();
            *p_pos += max;
            return Ok(s);
        }
        operand::VAR_LENGTH => {
            (0, true) // actual length in payload
        }
        operand::SINGLE => (1, false),
        other => bail!("UTF8_BYTE: unsupported operand type {other}"),
    };

    let byte_count = if read_leb_from_payload {
        let (n, nl) = leb128_payload(payload, *p_pos)?; *p_pos += nl;
        n as usize
    } else {
        byte_count
    };

    need(payload, *p_pos, byte_count)?;
    let bytes = &payload[*p_pos..*p_pos + byte_count];
    *p_pos += byte_count;
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

// ---------------------------------------------------------------------------
// LEB128 readers

fn leb128_bc(bc: &[u8], pos: usize) -> Result<(u64, usize)> {
    leb128(&bc[pos..]).context("LEB128 in bytecode")
}

fn leb128_payload(payload: &[u8], pos: usize) -> Result<(u64, usize)> {
    leb128(&payload[pos..]).context("LEB128 in payload")
}

fn leb128(buf: &[u8]) -> Result<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    for (i, &byte) in buf.iter().enumerate() {
        value |= ((byte & 0x7f) as u64) << shift;
        shift += 7;
        if byte & 0x80 == 0 { return Ok((value, i + 1)); }
        if shift >= 64 { bail!("LEB128 overflow"); }
    }
    bail!("truncated LEB128")
}

fn read_uint_le(payload: &[u8], pos: usize, size: usize) -> Result<u64> {
    need(payload, pos, size)?;
    let b = &payload[pos..pos + size];
    Ok(match size {
        1 => b[0] as u64,
        2 => u16::from_le_bytes(b.try_into().unwrap()) as u64,
        4 => u32::from_le_bytes(b.try_into().unwrap()) as u64,
        8 => u64::from_le_bytes(b.try_into().unwrap()),
        n => bail!("unsupported read_uint_le size {n}"),
    })
}

fn read_u32_le(payload: &[u8], pos: usize) -> Result<u32> {
    need(payload, pos, 4)?;
    Ok(u32::from_le_bytes(payload[pos..pos + 4].try_into().unwrap()))
}

fn need(payload: &[u8], pos: usize, n: usize) -> Result<()> {
    if pos + n > payload.len() {
        bail!(
            "payload underrun: need {} bytes at offset {} but payload is {} bytes",
            n, pos, payload.len()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Format string renderer

/// Render `format_str` by substituting the N-th placeholder with `values[N]`.
///
/// Values are matched positionally: the first `{name}` placeholder in left-to-right
/// order receives `values[0]`, the second receives `values[1]`, and so on.
/// If the same name appears twice, the second occurrence receives the next value.
///
/// Returns the rendered string, or an error if the format string is malformed
/// or there are more placeholders than values.
pub fn render(format_str: &str, values: &[Value]) -> Result<String> {
    let mut out = String::new();
    let mut val_idx = 0usize;
    let mut chars = format_str.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '{' if chars.peek() == Some(&'{') => { chars.next(); out.push('{'); }
            '{' => {
                let mut inner = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' { closed = true; break; }
                    inner.push(c);
                }
                if !closed { bail!("unclosed '{{' in format string"); }

                let (_name, spec_str) = match inner.find(':') {
                    Some(pos) => (&inner[..pos], Some(&inner[pos + 1..])),
                    None      => (inner.as_str(), None),
                };
                let spec = spec_str.map(parse_spec).transpose()
                    .context("parse format spec")?
                    .unwrap_or_default();

                if val_idx >= values.len() {
                    bail!("format string has more placeholders than decoded values \
                           (placeholder {val_idx} but only {} values)", values.len());
                }
                out.push_str(&values[val_idx].display_spec(&spec));
                val_idx += 1;
            }
            '}' if chars.peek() == Some(&'}') => { chars.next(); out.push('}'); }
            '}' => bail!("unexpected '}}' in format string"),
            c => out.push(c),
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db { Db::memory().unwrap() }

    // --- Value display ---

    #[test]
    fn display_integers() {
        assert_eq!(Value::U32(42).display_default(), "42");
        assert_eq!(Value::I32(-7).display_default(), "-7");
        assert_eq!(Value::Bool(true).display_default(), "true");
    }

    #[test]
    fn display_hex() {
        let spec = parse_spec("08x").unwrap();
        assert_eq!(Value::U32(0xab).display_spec(&spec), "000000ab");
    }

    #[test]
    fn display_hex_alternate() {
        let spec = parse_spec("#x").unwrap();
        assert_eq!(Value::U32(255).display_spec(&spec), "0xff");
    }

    #[test]
    fn display_float_default() {
        let v = format_float(3.14159, None);
        assert!(v.starts_with("3.141590"), "got {v}");
    }

    #[test]
    fn display_float_precision() {
        assert_eq!(format_float(3.14159, Some(2)), "3.14");
        assert_eq!(format_float(0.0, Some(3)), "0.000");
        assert_eq!(format_float(-1.5, Some(1)), "-1.5");
    }

    #[test]
    fn display_right_align() {
        let spec = parse_spec(">6").unwrap();
        assert_eq!(Value::U32(42).display_spec(&spec), "    42");
    }

    #[test]
    fn display_left_align() {
        let spec = parse_spec("<6").unwrap();
        assert_eq!(Value::U32(42).display_spec(&spec), "42    ");
    }

    // --- Interpreter ---

    #[test]
    fn interpret_single_u64() {
        // Bytecode: U64/single END
        let bc = &[0x20, 0x00];
        let payload = 12345u64.to_le_bytes();
        let vals = interpret(bc, &payload, &db()).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(vals[0], Value::U64(12345)));
    }

    #[test]
    fn interpret_skip() {
        // Bytecode: U8/single, SKIP/fixed-array 3, U8/single, END
        // 0x08 = U8/single, 0x51 = SKIP/fixed-array, 0x03 = 3, 0x08 = U8/single, 0x00 = END
        let bc = &[0x08, 0x51, 0x03, 0x08, 0x00];
        let payload = &[0xaau8, 0x00, 0x00, 0x00, 0xbb];
        let vals = interpret(bc, payload, &db()).unwrap();
        assert_eq!(vals.len(), 2);
        assert!(matches!(vals[0], Value::U8(0xaa)));
        assert!(matches!(vals[1], Value::U8(0xbb)));
    }

    #[test]
    fn interpret_utf8_var_length() {
        // Bytecode: UTF8_BYTE/var-length (0x4b), END (0x00)
        let bc = &[0x4b, 0x00];
        // Payload: LEB128(5) + "hello"
        let mut payload = vec![5u8];
        payload.extend_from_slice(b"hello");
        let vals = interpret(bc, &payload, &db()).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(&vals[0], Value::Str(s) if s == "hello"));
    }

    #[test]
    fn interpret_utf8_fixed_array() {
        // UTF8_BYTE/fixed-array (0x49), LEB128(5), END
        let bc = &[0x49, 0x05, 0x00];
        let payload = b"world";
        let vals = interpret(bc, payload, &db()).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(&vals[0], Value::Str(s) if s == "world"));
    }

    #[test]
    fn interpret_u32_fixed_array() {
        // U32/fixed-array (0x19), LEB128(2), END
        let bc = &[0x19, 0x02, 0x00];
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&2u32.to_le_bytes());
        let vals = interpret(bc, &payload, &db()).unwrap();
        assert_eq!(vals.len(), 1); // array is one Value::Array
        assert!(matches!(&vals[0], Value::Array(a) if a.len() == 2));
    }

    #[test]
    fn interpret_bool() {
        let bc = &[0x68, 0x68, 0x00]; // BOOL/single, BOOL/single, END
        let payload = &[1u8, 0u8];
        let vals = interpret(bc, payload, &db()).unwrap();
        assert!(matches!(vals[0], Value::Bool(true)));
        assert!(matches!(vals[1], Value::Bool(false)));
    }

    #[test]
    fn interpret_event_header_payload() {
        // EventHeader bytecode: U64_PAIR/single(0x88) U8/single(0x08) SKIP/fa 3(0x51,0x03) END(0x00)
        let bc = &[0x88u8, 0x08, 0x51, 0x03, 0x00];
        let mut payload = vec![0u8; 12];
        payload[..4].copy_from_slice(&42u32.to_le_bytes());  // lo
        payload[4..8].copy_from_slice(&0u32.to_le_bytes());  // hi
        payload[8] = 2; // Info severity
        let vals = interpret(bc, &payload, &db()).unwrap();
        assert_eq!(vals.len(), 2);
        assert!(matches!(vals[0], Value::U64(42)));
        assert!(matches!(vals[1], Value::U8(2)));
    }

    #[test]
    fn interpret_u64_pair() {
        // U64_PAIR/single = (17<<3)|0 = 0x88, END
        let bc = &[0x88u8, 0x00];
        let mut payload = vec![0u8; 8];
        payload[..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes()); // lo
        payload[4..8].copy_from_slice(&0x00000001u32.to_le_bytes()); // hi
        let vals = interpret(bc, &payload, &db()).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(vals[0], Value::U64(0x00000001_DEADBEEFu64)));
    }

    #[test]
    fn interpret_debug_message_payload() {
        // DebugMessage bytecode: UTF8_BYTE/var-length (0x4b) END (0x00)
        let bc = &[0x4b, 0x00];
        let mut payload = vec![11u8]; // LEB128(11)
        payload.extend_from_slice(b"hello world");
        let vals = interpret(bc, &payload, &db()).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(&vals[0], Value::Str(s) if s == "hello world"));
    }

    // --- Format string renderer ---

    #[test]
    fn render_basic() {
        let vals = vec![Value::U64(100), Value::U8(2)];
        let s = render("{timestamp} {severity}", &vals).unwrap();
        assert_eq!(s, "100 2");
    }

    #[test]
    fn render_literal_only() {
        let s = render("hello world", &[]).unwrap();
        assert_eq!(s, "hello world");
    }

    #[test]
    fn render_with_spec() {
        let vals = vec![Value::U32(0xdeadbeef)];
        let s = render("{addr:#010x}", &vals).unwrap();
        assert_eq!(s, "0xdeadbeef");
    }

    #[test]
    fn render_escaped_braces() {
        let s = render("{{{}}}",  &[Value::U32(42)]).unwrap();
        assert_eq!(s, "{42}");
    }

    #[test]
    fn render_string_value() {
        let vals = vec![Value::Str("hi".to_owned())];
        let s = render("{message}", &vals).unwrap();
        assert_eq!(s, "hi");
    }

    #[test]
    fn render_too_few_values() {
        assert!(render("{a} {b}", &[Value::U32(1)]).is_err());
    }

    // --- STRING_REF instruction ---

    #[test]
    fn interpret_string_ref_known() {
        use crate::elf::StringEntry;
        // Bytecode: STRING_REF/single END
        // STRING_REF opcode = (16 << 3) | 0 = 0x80
        let bc = &[0x80u8, 0x00];
        let hash: u32 = 0xDEAD_BEEF;
        let payload = hash.to_le_bytes();
        let mut db = Db::memory().unwrap();
        db.ingest(&[], &[StringEntry { hash, content: "my label".to_owned() }], 0).unwrap();
        let vals = interpret(bc, &payload, &db).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(&vals[0], Value::Str(s) if s == "my label"));
    }

    #[test]
    fn interpret_string_ref_unknown() {
        // STRING_REF with hash not in the database → fallback placeholder.
        let bc = &[0x80u8, 0x00];
        let hash: u32 = 0x1234_5678;
        let payload = hash.to_le_bytes();
        let vals = interpret(bc, &payload, &db()).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(&vals[0], Value::Str(s) if s.contains("12345678")));
    }

    // --- DISPATCH instruction ---

    #[test]
    fn interpret_dispatch_matching_variant() {
        use crate::elf::EventEntry;
        // Inline enum: repr(u8), two variants
        //   Low = 0 → subroutine tag 0x1111 (payload: U8/single + END = [0x08, 0x00])
        //   High = 1 → subroutine tag 0x2222 (payload: U32/single + END = [0x18, 0x00])
        //
        // DISPATCH bytecode:
        //   opcode: (14<<3)|0 = 0x70
        //   discrim_type LEB128: U8=1 → 0x01
        //   padding LEB128: 0 → 0x00
        //   count LEB128: 2 → 0x02
        //   (val=0 LEB128, tag=0x1111 LEB128): 0x00, 0x91,0x22 (0x1111=4369)
        //   (val=1 LEB128, tag=0x2222 LEB128): 0x01, 0xa2,0x44 (0x2222=8738)
        //   END: 0x00
        //
        // LEB128(0x1111=4369): 4369 = 0x1111; low7=0x11|0x80=0x91, next=0x22
        // LEB128(0x2222=8738): 8738 = 0x2222; low7=0x22|0x80=0xa2, next=0x44
        let low_tag:  u32 = 0x1111;
        let high_tag: u32 = 0x2222;

        let mut db = Db::memory().unwrap();
        db.ingest(&[
            EventEntry { tag: low_tag,  full_hash: low_tag as u64,  format_hash: 0,
                         bytecode: vec![0x08, 0x00] }, // U8/single + END
            EventEntry { tag: high_tag, full_hash: high_tag as u64, format_hash: 0,
                         bytecode: vec![0x18, 0x00] }, // U32/single + END
        ], &[], 0).unwrap();

        // Build dispatch bytecode with the computed LEB128 values.
        let leb = |n: u32| -> Vec<u8> {
            let mut v = Vec::new();
            let mut x = n as u64;
            loop {
                let b = (x & 0x7f) as u8;
                x >>= 7;
                if x == 0 { v.push(b); break; } else { v.push(b | 0x80); }
            }
            v
        };
        let mut bc = vec![0x70u8]; // DISPATCH/single
        bc.push(0x01); // discrim_type = U8
        bc.push(0x00); // padding = 0
        bc.push(0x02); // count = 2
        bc.push(0x00); bc.extend_from_slice(&leb(low_tag));  // val=0, tag=low_tag
        bc.push(0x01); bc.extend_from_slice(&leb(high_tag)); // val=1, tag=high_tag
        bc.push(0x00); // END

        // Dispatch on discriminant = 0 (Low variant) → U8/single → value 42
        let payload_low = [0u8, 42u8]; // discriminant=0, then Low's payload (u8=42)
        let vals = interpret(&bc, &payload_low, &db).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(vals[0], Value::U8(42)));

        // Dispatch on discriminant = 1 (High variant) → U32/single → value 0xCAFE
        let mut payload_high = vec![1u8];
        payload_high.extend_from_slice(&0x0000_CAFEu32.to_le_bytes());
        let vals = interpret(&bc, &payload_high, &db).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(vals[0], Value::U32(0xCAFE)));
    }

    #[test]
    fn interpret_dispatch_unknown_discriminant() {
        // Unknown discriminant → placeholder string, no bytes consumed for variant body.
        let mut bc = vec![0x70u8]; // DISPATCH/single
        bc.push(0x01); // discrim_type = U8
        bc.push(0x00); // padding = 0
        bc.push(0x01); // count = 1
        bc.push(0x00); bc.extend(b"\x00"); // val=0, tag=0
        bc.push(0x00); // END

        let payload = [0x99u8]; // discriminant = 0x99 — no entry in table
        let vals = interpret(&bc, &payload, &db()).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(&vals[0], Value::Str(s) if s.contains("153"))); // 0x99 = 153
    }

    // --- CALL subroutine ---

    #[test]
    fn interpret_call() {
        use crate::elf::EventEntry;
        // Callee subroutine: U32/single END
        let callee_bc = vec![0x18u8, 0x00];
        let callee_tag: u32 = 0xabcd;
        let mut db = Db::memory().unwrap();
        db.ingest(
            &[EventEntry {
                tag: callee_tag,
                full_hash: callee_tag as u64,
                format_hash: 0,
                bytecode: callee_bc,
            }],
            &[],
            0,
        ).unwrap();

        // Caller bytecode: CALL tag=0xabcd (LEB128), END
        // CALL opcode = (15<<3)|0 = 0x78
        // LEB128(0xabcd) = 0xCD 0xD5 0x02 (let me check: 0xabcd = 43981)
        // 43981 in LEB128: 43981 & 0x7f = 0x4d, 43981 >> 7 = 343, 343 & 0x7f = 0x57, 343 >> 7 = 2
        // So: 0xcd, 0xd7, 0x02
        // Wait: 43981 = 0b1010_1011_1100_1101
        // Low 7 bits: 0b100_1101 = 0x4d, set continuation: 0xcd
        // Next 7: 0b101_0111 = 0x57, set continuation: 0xd7
        // Next: 2, no continuation: 0x02
        let callee_tag_leb = {
            let mut v = Vec::new();
            let mut n = callee_tag as u64;
            loop {
                let b = (n & 0x7f) as u8;
                n >>= 7;
                if n == 0 { v.push(b); break; } else { v.push(b | 0x80); }
            }
            v
        };

        let mut caller_bc = vec![0x78u8]; // CALL opcode
        caller_bc.extend_from_slice(&callee_tag_leb);
        caller_bc.push(0x00); // END

        let payload = 777u32.to_le_bytes();
        let vals = interpret(&caller_bc, &payload, &db).unwrap();
        assert_eq!(vals.len(), 1);
        assert!(matches!(vals[0], Value::U32(777)));
    }

    #[test]
    fn display_fourcc_all_printable() {
        // "RIFF" stored LE: 0x46464952 → bytes [0x52, 0x49, 0x46, 0x46]
        let spec = parse_spec("c").unwrap();
        assert_eq!(Value::U32(0x46464952).display_spec(&spec), "RIFF");
    }

    #[test]
    fn display_fourcc_space_printable() {
        // "AVI " stored LE: 0x20495641 → [0x41, 0x56, 0x49, 0x20]
        let spec = parse_spec("c").unwrap();
        assert_eq!(Value::U32(0x20495641).display_spec(&spec), "AVI ");
    }

    #[test]
    fn display_fourcc_non_printable_escape() {
        // Low byte 0x00, rest 'F', 'I', 'F': 0x46494600 → [0x00, 0x46, 0x49, 0x46]
        let spec = parse_spec("c").unwrap();
        assert_eq!(Value::U32(0x46494600).display_spec(&spec), r"\x00FIF");
    }

    #[test]
    fn display_fourcc_high_byte_escape() {
        // High byte 0xFF: 0xFF464952 → [0x52, 0x49, 0x46, 0xFF]
        let spec = parse_spec("c").unwrap();
        assert_eq!(Value::U32(0xFF464952).display_spec(&spec), r"RIF\xff");
    }

    #[test]
    fn display_fourcc_u8() {
        let spec = parse_spec("c").unwrap();
        assert_eq!(Value::U8(b'R').display_spec(&spec), "R");
        assert_eq!(Value::U8(0x00).display_spec(&spec), r"\x00");
    }
}
