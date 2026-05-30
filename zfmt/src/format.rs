//! Format trait, FormatSpec, and Format implementations for primitive types.

use crate::write::{Error, Write};

// ---------------------------------------------------------------------------
// FormatSpec types

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FormatType {
    #[default]
    Display,
    LowerHex,
    UpperHex,
    Binary,
    Octal,
    /// FourCC character display (§10.2): bytes in little-endian order,
    /// printable ASCII (0x20–0x7E) as characters, others as `\xNN`.
    Char,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    None,
    Left,
    Right,
}

/// Describes how a single placeholder field should be rendered.
///
/// Every field in proc-macro-generated `format_into` code is paired with a
/// compile-time-constant `FormatSpec`; the compiler constant-folds the
/// `Format::fmt` call and eliminates dead branches for unused flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FormatSpec {
    pub ty:        FormatType,
    pub alternate: bool,        // # flag: 0x / 0b / 0o prefix
    pub sign:      bool,        // + flag: always emit sign
    pub zero_pad:  bool,        // 0N: right-align with zero fill
    pub width:     u8,          // 0 = no minimum width
    pub precision: Option<u8>,  // .N decimal places for floats; None = 6
    pub align:     Align,
}

// ---------------------------------------------------------------------------
// Format trait

/// Formats `self` into `writer` according to `spec`.
///
/// Implement this for any type that may appear as an event field when
/// firmware-side formatting is required.  The `zfmt` crate provides
/// implementations for all primitive types listed in the spec (§3.3).
pub trait Format {
    fn fmt<W: Write>(&self, writer: &mut W, spec: FormatSpec) -> Result<(), Error>;
}

// ---------------------------------------------------------------------------
// FormatInto trait

/// Renders the event as human-readable text (§13.5).
///
/// The derive macro generates this impl when the event carries a
/// `#[zfmt(format = "...")]` attribute.  Events without a format string
/// receive a no-op default that writes nothing.
///
/// This trait enables the `output-text` / `output-both` features to call
/// `format_into()` without requiring the type to have an inherent method of
/// the same name.
pub trait FormatInto {
    fn format_into<W: Write>(&self, w: &mut W) -> Result<(), Error> {
        let _ = w;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Private helpers

/// Write `n` copies of `ch` to `w`.
fn write_fill<W: Write>(w: &mut W, ch: char, n: usize) -> Result<(), Error> {
    let mut buf = [0u8; 4];
    let s = ch.encode_utf8(&mut buf);
    for _ in 0..n {
        w.write_str(s)?;
    }
    Ok(())
}

/// Fill `buf` from the right with the digits of `n` in `base`.
/// Returns the index of the first digit (digits are `buf[start..]`).
/// `buf` must be at least 64 bytes (sufficient for a u64 in binary).
fn write_digits(buf: &mut [u8; 64], mut n: u64, base: u64, upper: bool) -> usize {
    let mut pos = buf.len();
    if n == 0 {
        pos -= 1;
        buf[pos] = b'0';
    } else {
        while n > 0 {
            pos -= 1;
            let d = (n % base) as u8;
            buf[pos] = if d < 10 {
                b'0' + d
            } else if upper {
                b'A' + (d - 10)
            } else {
                b'a' + (d - 10)
            };
            n /= base;
        }
    }
    pos
}

/// Format an unsigned 64-bit value according to `spec`.
/// `negative` controls the sign prefix for decimal display.
fn fmt_uint<W: Write>(
    w: &mut W,
    value: u64,
    spec: FormatSpec,
    negative: bool,
) -> Result<(), Error> {
    let (base, upper) = match spec.ty {
        FormatType::Display  => (10u64, false),
        FormatType::LowerHex => (16u64, false),
        FormatType::UpperHex => (16u64, true),
        FormatType::Binary   => (2u64,  false),
        FormatType::Octal    => (8u64,  false),
        // Char is intercepted before fmt_uint is called; fall back to decimal.
        FormatType::Char     => (10u64, false),
    };

    let mut buf = [0u8; 64];
    let start = write_digits(&mut buf, value, base, upper);
    // SAFETY: write_digits only writes ASCII bytes.
    let digits = unsafe { core::str::from_utf8_unchecked(&buf[start..]) };

    let sign = if negative {
        "-"
    } else if spec.sign {
        "+"
    } else {
        ""
    };

    // Alternate prefix only applies to non-decimal types.
    let prefix = if spec.alternate {
        match spec.ty {
            FormatType::LowerHex => "0x",
            FormatType::UpperHex => "0X",
            FormatType::Binary   => "0b",
            FormatType::Octal    => "0o",
            FormatType::Display | FormatType::Char => "",
        }
    } else {
        ""
    };

    let content_len = sign.len() + prefix.len() + digits.len();
    let width = spec.width as usize;

    // Zero-pad: sign + prefix + <zeros> + digits (only when align is None).
    if spec.zero_pad && spec.align == Align::None && width > content_len {
        let pad = width - content_len;
        w.write_str(sign)?;
        w.write_str(prefix)?;
        write_fill(w, '0', pad)?;
        return w.write_str(digits);
    }

    let pad = width.saturating_sub(content_len);
    match spec.align {
        Align::Right => {
            write_fill(w, ' ', pad)?;
            w.write_str(sign)?;
            w.write_str(prefix)?;
            w.write_str(digits)
        }
        Align::Left => {
            w.write_str(sign)?;
            w.write_str(prefix)?;
            w.write_str(digits)?;
            write_fill(w, ' ', pad)
        }
        Align::None => {
            w.write_str(sign)?;
            w.write_str(prefix)?;
            w.write_str(digits)
        }
    }
}

/// Format a string value with optional alignment padding.
fn fmt_str_value<W: Write>(w: &mut W, s: &str, spec: FormatSpec) -> Result<(), Error> {
    let pad = (spec.width as usize).saturating_sub(s.len());
    match spec.align {
        Align::Right => {
            write_fill(w, ' ', pad)?;
            w.write_str(s)
        }
        Align::Left => {
            w.write_str(s)?;
            write_fill(w, ' ', pad)
        }
        Align::None => w.write_str(s),
    }
}

/// FourCC character display: each byte is printed as a printable ASCII
/// character (0x20–0x7E) or as `\xNN` for non-printable/non-ASCII bytes.
/// Bytes are consumed in the order supplied, which for integers should be
/// little-endian (LSB-first) so the display matches memory and hexdump order.
fn fmt_fourcc<W: Write>(w: &mut W, bytes: &[u8]) -> Result<(), Error> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &b in bytes {
        if b >= 0x20 && b <= 0x7e {
            w.write_char(b as char)?;
        } else {
            let escape = [b'\\', b'x', HEX[(b >> 4) as usize], HEX[(b & 0xf) as usize]];
            // SAFETY: escape contains only ASCII bytes.
            w.write_str(unsafe { core::str::from_utf8_unchecked(&escape) })?;
        }
    }
    Ok(())
}

/// 10^n as u64, saturating at u64::MAX for large n.
fn pow10(n: usize) -> u64 {
    let mut v = 1u64;
    for _ in 0..n {
        v = v.saturating_mul(10);
    }
    v
}

#[cfg(not(feature = "no-float"))]
fn fmt_float<W: Write>(w: &mut W, value: f64, spec: FormatSpec) -> Result<(), Error> {
    if value.is_nan() {
        return fmt_str_value(w, "NaN", spec);
    }

    let negative = value.is_sign_negative();
    let abs = if negative { -value } else { value };

    if abs.is_infinite() {
        return fmt_str_value(w, if negative { "-inf" } else { "inf" }, spec);
    }

    let precision = spec.precision.map(|p| p as usize).unwrap_or(6);

    let int_part = abs as u64;
    let frac = abs - int_part as f64;
    let scale = pow10(precision);
    let mut frac_int = (frac * scale as f64 + 0.5) as u64;
    let mut int_final = int_part;

    // Propagate carry from rounding.
    if frac_int >= scale {
        int_final += 1;
        frac_int = 0;
    }

    // Build the rendered string in a stack buffer.
    // Worst case: sign(1) + 20 int digits + dot(1) + 20 frac digits = 42 bytes.
    let mut out = [0u8; 48];
    let mut pos = 0usize;

    if negative {
        out[pos] = b'-';
        pos += 1;
    } else if spec.sign {
        out[pos] = b'+';
        pos += 1;
    }

    let mut int_buf = [0u8; 64];
    let int_start = write_digits(&mut int_buf, int_final, 10, false);
    let int_digits = &int_buf[int_start..];
    out[pos..pos + int_digits.len()].copy_from_slice(int_digits);
    pos += int_digits.len();

    if precision > 0 {
        out[pos] = b'.';
        pos += 1;

        // Write fractional digits with leading zeros to reach `precision` digits.
        let mut frac_buf = [0u8; 20];
        let mut f = frac_int;
        for i in (0..precision.min(20)).rev() {
            frac_buf[i] = b'0' + (f % 10) as u8;
            f /= 10;
        }
        let frac_len = precision.min(20);
        out[pos..pos + frac_len].copy_from_slice(&frac_buf[..frac_len]);
        pos += frac_len;
    }

    // SAFETY: out[..pos] contains only ASCII bytes.
    let s = unsafe { core::str::from_utf8_unchecked(&out[..pos]) };
    fmt_str_value(w, s, spec)
}

// ---------------------------------------------------------------------------
// Format implementations for primitive types

macro_rules! impl_fmt_uint {
    ($($t:ty),*) => {$(
        impl Format for $t {
            fn fmt<W: Write>(&self, w: &mut W, spec: FormatSpec) -> Result<(), Error> {
                if spec.ty == FormatType::Char {
                    return fmt_fourcc(w, &self.to_le_bytes());
                }
                fmt_uint(w, *self as u64, spec, false)
            }
        }
    )*};
}

macro_rules! impl_fmt_sint {
    ($(($signed:ty, $unsigned:ty)),*) => {$(
        impl Format for $signed {
            fn fmt<W: Write>(&self, w: &mut W, spec: FormatSpec) -> Result<(), Error> {
                if spec.ty == FormatType::Char {
                    return fmt_fourcc(w, &(*self as $unsigned).to_le_bytes());
                }
                match spec.ty {
                    // Decimal: show sign and absolute value.
                    FormatType::Display => {
                        let negative = *self < 0;
                        fmt_uint(w, (*self as i64).unsigned_abs(), spec, negative)
                    }
                    // Non-decimal: show the 2's-complement bit pattern as unsigned.
                    _ => fmt_uint(w, *self as $unsigned as u64, spec, false),
                }
            }
        }
    )*};
}

#[cfg(not(feature = "no-64bit"))]
impl_fmt_uint!(u8, u16, u32, u64);
#[cfg(feature = "no-64bit")]
impl_fmt_uint!(u8, u16, u32);

#[cfg(not(feature = "no-64bit"))]
impl_fmt_sint!((i8, u8), (i16, u16), (i32, u32), (i64, u64));
#[cfg(feature = "no-64bit")]
impl_fmt_sint!((i8, u8), (i16, u16), (i32, u32));

#[cfg(not(feature = "no-float"))]
impl Format for f32 {
    fn fmt<W: Write>(&self, w: &mut W, spec: FormatSpec) -> Result<(), Error> {
        fmt_float(w, *self as f64, spec)
    }
}

#[cfg(not(feature = "no-float"))]
impl Format for f64 {
    fn fmt<W: Write>(&self, w: &mut W, spec: FormatSpec) -> Result<(), Error> {
        fmt_float(w, *self, spec)
    }
}

impl Format for bool {
    fn fmt<W: Write>(&self, w: &mut W, spec: FormatSpec) -> Result<(), Error> {
        fmt_str_value(w, if *self { "true" } else { "false" }, spec)
    }
}

impl Format for char {
    fn fmt<W: Write>(&self, w: &mut W, spec: FormatSpec) -> Result<(), Error> {
        let mut buf = [0u8; 4];
        let s = self.encode_utf8(&mut buf);
        fmt_str_value(w, s, spec)
    }
}

impl Format for str {
    fn fmt<W: Write>(&self, w: &mut W, spec: FormatSpec) -> Result<(), Error> {
        fmt_str_value(w, self, spec)
    }
}

impl<'a> Format for &'a str {
    fn fmt<W: Write>(&self, w: &mut W, spec: FormatSpec) -> Result<(), Error> {
        fmt_str_value(w, self, spec)
    }
}

// ---------------------------------------------------------------------------
// ZfmtStr

/// A compile-time interned string handle (§4.7).
///
/// Created by `zfmt_str!("literal")`, which interns the string into the
/// `.zfmt_strings` linker section and evaluates to the corresponding `u32`
/// FNV-1a hash.  On the wire the hash is transmitted as a `u32`; the host
/// decoder resolves it back to the original string via the string table.
///
/// The firmware cannot resolve string hashes, so `Format` renders the hash
/// in hexadecimal as a fallback when `output-text` mode is active.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ZfmtStr(pub u32);

impl ZfmtStr {
    pub const fn new(hash: u32) -> Self {
        Self(hash)
    }
}

impl From<u32> for ZfmtStr {
    fn from(h: u32) -> Self {
        Self(h)
    }
}

impl Format for ZfmtStr {
    fn fmt<W: Write>(&self, w: &mut W, spec: FormatSpec) -> Result<(), Error> {
        let hex_spec = FormatSpec {
            ty: FormatType::LowerHex,
            alternate: true,
            zero_pad: true,
            width: 10, // "0x" + 8 hex digits
            ..spec
        };
        fmt_uint(w, self.0 as u64, hex_spec, false)
    }
}

// ---------------------------------------------------------------------------
// ZfmtU64

/// A 64-bit unsigned value stored as two 32-bit halves for 4-byte alignment.
///
/// Using `u64` directly in embedded structs forces 8-byte alignment, wasting
/// 4 bytes of padding in event headers.  `ZfmtU64` is `repr(C)` with two
/// `u32` fields so the overall alignment is 4.  On the wire the two halves
/// are transmitted as `[lo, hi]` and the decoder reconstructs the `u64`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ZfmtU64 {
    pub lo: u32,
    pub hi: u32,
}

impl ZfmtU64 {
    pub const fn new(lo: u32, hi: u32) -> Self {
        Self { lo, hi }
    }

    #[cfg(not(feature = "no-64bit"))]
    pub const fn from_u64(v: u64) -> Self {
        Self { lo: v as u32, hi: (v >> 32) as u32 }
    }

    #[cfg(not(feature = "no-64bit"))]
    pub const fn to_u64(self) -> u64 {
        (self.hi as u64) << 32 | (self.lo as u64)
    }
}

#[cfg(not(feature = "no-64bit"))]
impl From<u64> for ZfmtU64 {
    fn from(v: u64) -> Self {
        Self::from_u64(v)
    }
}

#[cfg(not(feature = "no-64bit"))]
impl From<ZfmtU64> for u64 {
    fn from(v: ZfmtU64) -> u64 {
        v.to_u64()
    }
}

#[cfg(not(feature = "no-64bit"))]
impl Format for ZfmtU64 {
    fn fmt<W: Write>(&self, w: &mut W, spec: FormatSpec) -> Result<(), Error> {
        fmt_uint(w, self.to_u64(), spec, false)
    }
}

/// Render a u32 as exactly 8 lowercase hex digits — no u64 arithmetic.
#[cfg(feature = "no-64bit")]
fn write_u32_hex8<W: Write>(w: &mut W, v: u32) -> Result<(), Error> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let buf = [
        DIGITS[((v >> 28) & 0xF) as usize],
        DIGITS[((v >> 24) & 0xF) as usize],
        DIGITS[((v >> 20) & 0xF) as usize],
        DIGITS[((v >> 16) & 0xF) as usize],
        DIGITS[((v >> 12) & 0xF) as usize],
        DIGITS[((v >>  8) & 0xF) as usize],
        DIGITS[((v >>  4) & 0xF) as usize],
        DIGITS[( v        & 0xF) as usize],
    ];
    // SAFETY: all bytes are valid ASCII hex digits.
    w.write_str(unsafe { core::str::from_utf8_unchecked(&buf) })
}

/// Under `no-64bit`, ZfmtU64 is displayed as 16 hex digits (hi then lo).
/// This avoids all 64-bit arithmetic — only 32-bit shifts and masks are used.
#[cfg(feature = "no-64bit")]
impl Format for ZfmtU64 {
    fn fmt<W: Write>(&self, w: &mut W, _spec: FormatSpec) -> Result<(), Error> {
        write_u32_hex8(w, self.hi)?;
        write_u32_hex8(w, self.lo)
    }
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;

    struct StrWriter(String);

    impl Write for StrWriter {
        fn write_str(&mut self, s: &str) -> Result<(), Error> {
            self.0.push_str(s);
            Ok(())
        }
    }

    fn render<T: Format>(value: T, spec: FormatSpec) -> String {
        let mut w = StrWriter(String::new());
        value.fmt(&mut w, spec).unwrap();
        w.0
    }

    fn spec() -> FormatSpec { FormatSpec::default() }

    // --- unsigned integers ---

    #[test]
    fn uint_decimal() {
        assert_eq!(render(0u32,  spec()), "0");
        assert_eq!(render(42u32, spec()), "42");
        #[cfg(not(feature = "no-64bit"))]
        assert_eq!(render(u64::MAX, spec()), "18446744073709551615");
    }

    #[test]
    fn uint_hex() {
        let s = FormatSpec { ty: FormatType::LowerHex, ..spec() };
        assert_eq!(render(0u32,   s), "0");
        assert_eq!(render(255u8,  s), "ff");
        assert_eq!(render(256u16, s), "100");

        let u = FormatSpec { ty: FormatType::UpperHex, ..spec() };
        assert_eq!(render(255u8,  u), "FF");
    }

    #[test]
    fn uint_binary() {
        let s = FormatSpec { ty: FormatType::Binary, ..spec() };
        assert_eq!(render(0u8,  s), "0");
        assert_eq!(render(5u8,  s), "101");
        assert_eq!(render(255u8, s), "11111111");
    }

    #[test]
    fn uint_octal() {
        let s = FormatSpec { ty: FormatType::Octal, ..spec() };
        assert_eq!(render(8u32,  s), "10");
        assert_eq!(render(64u32, s), "100");
    }

    #[test]
    fn uint_alternate() {
        let h = FormatSpec { ty: FormatType::LowerHex, alternate: true, ..spec() };
        assert_eq!(render(255u8, h), "0xff");

        let uh = FormatSpec { ty: FormatType::UpperHex, alternate: true, ..spec() };
        assert_eq!(render(255u8, uh), "0XFF");

        let b = FormatSpec { ty: FormatType::Binary, alternate: true, ..spec() };
        assert_eq!(render(5u8,  b), "0b101");

        let o = FormatSpec { ty: FormatType::Octal, alternate: true, ..spec() };
        assert_eq!(render(8u32, o), "0o10");
    }

    #[test]
    fn uint_zero_pad() {
        let s = FormatSpec {
            ty: FormatType::LowerHex, zero_pad: true, width: 8, ..spec()
        };
        assert_eq!(render(0xabu32,      s), "000000ab");
        assert_eq!(render(0xdeadbeefu32, s), "deadbeef");
    }

    #[test]
    fn uint_zero_pad_with_alternate() {
        // 0x + 8 digits = 10 total; width 10 → no extra zeros needed
        let s = FormatSpec {
            ty: FormatType::LowerHex,
            alternate: true,
            zero_pad: true,
            width: 10,
            ..spec()
        };
        assert_eq!(render(0xabu32, s), "0x000000ab");
    }

    #[test]
    fn uint_right_align() {
        let s = FormatSpec { align: Align::Right, width: 6, ..spec() };
        assert_eq!(render(42u32,  s), "    42");
        assert_eq!(render(999999u32, s), "999999"); // exact width
        assert_eq!(render(9999999u32, s), "9999999"); // wider than field
    }

    #[test]
    fn uint_left_align() {
        let s = FormatSpec { align: Align::Left, width: 6, ..spec() };
        assert_eq!(render(42u32, s), "42    ");
    }

    #[test]
    fn uint_sign() {
        let s = FormatSpec { sign: true, ..spec() };
        assert_eq!(render(42u32, s), "+42");
        assert_eq!(render(0u32,  s), "+0");
    }

    // --- signed integers ---

    #[test]
    fn sint_decimal() {
        assert_eq!(render(0i32,   spec()), "0");
        assert_eq!(render(42i32,  spec()), "42");
        assert_eq!(render(-42i32, spec()), "-42");
        #[cfg(not(feature = "no-64bit"))]
        assert_eq!(render(i64::MIN, spec()), "-9223372036854775808");
        #[cfg(not(feature = "no-64bit"))]
        assert_eq!(render(i64::MAX, spec()), "9223372036854775807");
    }

    #[test]
    fn sint_hex_twos_complement() {
        let s = FormatSpec { ty: FormatType::LowerHex, ..spec() };
        assert_eq!(render(-1i8,  s), "ff");
        assert_eq!(render(-1i16, s), "ffff");
        assert_eq!(render(-1i32, s), "ffffffff");
        #[cfg(not(feature = "no-64bit"))]
        assert_eq!(render(-1i64, s), "ffffffffffffffff");
    }

    #[test]
    fn sint_sign_flag() {
        let s = FormatSpec { sign: true, ..spec() };
        assert_eq!(render(42i32,  s), "+42");
        assert_eq!(render(-42i32, s), "-42");
    }

    // --- floats ---

    #[cfg(not(feature = "no-float"))]
    #[test]
    fn float_default_precision() {
        // Default 6 decimal places
        let s = render(3.14159f64, spec());
        assert!(s.starts_with("3.14159"), "got {s}");
    }

    #[cfg(not(feature = "no-float"))]
    #[test]
    fn float_precision() {
        let s = FormatSpec { precision: Some(2), ..spec() };
        assert_eq!(render(3.14159f64, s), "3.14");
        assert_eq!(render(3.145f64,   s), "3.15"); // rounding
        assert_eq!(render(0.0f64,     s), "0.00");
    }

    #[cfg(not(feature = "no-float"))]
    #[test]
    fn float_zero_precision() {
        let s = FormatSpec { precision: Some(0), ..spec() };
        assert_eq!(render(3.7f64,  s), "4");
        assert_eq!(render(3.0f64,  s), "3");
    }

    #[cfg(not(feature = "no-float"))]
    #[test]
    fn float_negative() {
        let s = FormatSpec { precision: Some(2), ..spec() };
        assert_eq!(render(-1.5f64, s), "-1.50");
    }

    #[cfg(not(feature = "no-float"))]
    #[test]
    fn float_special() {
        assert_eq!(render(f64::NAN,           spec()), "NaN");
        assert_eq!(render(f64::INFINITY,      spec()), "inf");
        assert_eq!(render(f64::NEG_INFINITY,  spec()), "-inf");
    }

    #[cfg(not(feature = "no-float"))]
    #[test]
    fn float_sign_flag() {
        let s = FormatSpec { sign: true, precision: Some(1), ..spec() };
        assert_eq!(render(1.0f64,  s), "+1.0");
        assert_eq!(render(-1.0f64, s), "-1.0");
    }

    #[cfg(not(feature = "no-float"))]
    #[test]
    fn float_alignment() {
        let r = FormatSpec { align: Align::Right, width: 8, precision: Some(2), ..spec() };
        assert_eq!(render(3.14f64, r), "    3.14");

        let l = FormatSpec { align: Align::Left, width: 8, precision: Some(2), ..spec() };
        assert_eq!(render(3.14f64, l), "3.14    ");
    }

    // --- bool ---

    #[test]
    fn bool_display() {
        assert_eq!(render(true,  spec()), "true");
        assert_eq!(render(false, spec()), "false");
    }

    #[test]
    fn bool_alignment() {
        let r = FormatSpec { align: Align::Right, width: 6, ..spec() };
        assert_eq!(render(true, r),  "  true");
        let l = FormatSpec { align: Align::Left, width: 6, ..spec() };
        assert_eq!(render(false, l), "false ");
    }

    // --- char ---

    #[test]
    fn char_display() {
        assert_eq!(render('A', spec()), "A");
        assert_eq!(render('€', spec()), "€");
    }

    // --- &str ---

    #[test]
    fn str_display() {
        assert_eq!(render("hello", spec()), "hello");
    }

    #[test]
    fn str_right_align() {
        let s = FormatSpec { align: Align::Right, width: 8, ..spec() };
        assert_eq!(render("hi", s), "      hi");
    }

    #[test]
    fn str_left_align() {
        let s = FormatSpec { align: Align::Left, width: 8, ..spec() };
        assert_eq!(render("hi", s), "hi      ");
    }

    #[test]
    fn str_exact_width() {
        let s = FormatSpec { align: Align::Right, width: 5, ..spec() };
        assert_eq!(render("hello", s), "hello");
    }

    #[test]
    fn str_wider_than_field() {
        let s = FormatSpec { align: Align::Right, width: 3, ..spec() };
        assert_eq!(render("hello", s), "hello"); // no truncation
    }

    // --- FourCC / Char ---

    #[test]
    fn fourcc_all_printable() {
        // "RIFF" stored LE: 0x46464952 → bytes [0x52, 0x49, 0x46, 0x46]
        let s = FormatSpec { ty: FormatType::Char, ..spec() };
        assert_eq!(render(0x46464952u32, s), "RIFF");
    }

    #[test]
    fn fourcc_all_printable_u8() {
        let s = FormatSpec { ty: FormatType::Char, ..spec() };
        assert_eq!(render(b'R', s), "R");
    }

    #[test]
    fn fourcc_non_printable_escape() {
        // Low byte 0x00 → \x00, remaining bytes 'R', 'I', 'F' printable.
        // LE bytes of 0x46494600: [0x00, 0x46, 0x49, 0x46]
        let s = FormatSpec { ty: FormatType::Char, ..spec() };
        assert_eq!(render(0x46494600u32, s), r"\x00FIF");
    }

    #[test]
    fn fourcc_high_byte_escape() {
        // High byte 0xFF → \xff.  LE bytes of 0xFF464952: [0x52, 0x49, 0x46, 0xFF]
        let s = FormatSpec { ty: FormatType::Char, ..spec() };
        assert_eq!(render(0xFF464952u32, s), r"RIF\xff");
    }

    #[test]
    fn fourcc_all_non_printable() {
        let s = FormatSpec { ty: FormatType::Char, ..spec() };
        assert_eq!(render(0x00000000u32, s), r"\x00\x00\x00\x00");
    }

    #[test]
    fn fourcc_space_is_printable() {
        // Space (0x20) is printable.  "AVI " LE: 0x20495641 → [0x41, 0x56, 0x49, 0x20]
        let s = FormatSpec { ty: FormatType::Char, ..spec() };
        assert_eq!(render(0x20495641u32, s), "AVI ");
    }

    #[test]
    fn fourcc_del_is_escaped() {
        // DEL (0x7F) is not printable.  LE bytes of 0x7F000000: [0x00, 0x00, 0x00, 0x7F]
        let s = FormatSpec { ty: FormatType::Char, ..spec() };
        assert_eq!(render(0x7F000000u32, s), r"\x00\x00\x00\x7f");
    }
}
