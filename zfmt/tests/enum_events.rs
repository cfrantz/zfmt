//! Integration tests for Phase 5: enum events and inline enums.

use zfmt::{FormatInto, Write, Zfmt};

struct StrBuf(String);
impl Write for StrBuf {
    fn write_str(&mut self, s: &str) -> Result<(), zfmt::Error> {
        self.0.push_str(s);
        Ok(())
    }
}
fn w() -> StrBuf { StrBuf(String::new()) }

fn decode_leb128(buf: &[u8]) -> (u64, usize) {
    zfmt::leb128::decode(buf).expect("valid LEB128")
}

// ---- Top-level enums -------------------------------------------------------

#[derive(Zfmt)]
pub enum Sensor {
    #[zfmt(format = "temperature={celsius}")]
    Temperature { celsius: f32 },

    #[zfmt(format = "pressure={pascals}")]
    Pressure { pascals: u32 },

    /// Variant with no format string — format_into is a no-op
    Reset,
}

#[derive(Zfmt)]
pub enum Level {
    #[zfmt(format = "low={v}")]
    Low { v: u8 },
    #[zfmt(format = "mid={v}")]
    Mid { v: u16 },
    #[zfmt(format = "high={v}")]
    High { v: u64 },
}

// Top-level enum with Tier-2 (str) variant
#[derive(Zfmt)]
pub enum Log<'a> {
    #[zfmt(format = "msg={text}")]
    Text { text: &'a str },
    #[zfmt(format = "code={code}")]
    Code { code: u32 },
}

// Tuple variants
#[derive(Zfmt)]
pub enum Point {
    #[zfmt(format = "2d x={0} y={1}")]
    D2(f32, f32),
    #[zfmt(format = "3d x={0} y={1} z={2}")]
    D3(f32, f32, f32),
}

// ---- Inline enum -----------------------------------------------------------

#[derive(Zfmt)]
#[repr(u8)]
pub enum Priority {
    Low = 0,
    High = 1,
}

#[derive(Zfmt)]
#[repr(u8)]
pub enum Status {
    Ok = 0,
    Warn = 1,
    Error = 2,
}

// ---- Tests -----------------------------------------------------------------

// --- Top-level enum tags

#[test]
fn toplevel_tags_nonzero() {
    assert_ne!(Sensor::TEMPERATURE_ZFMT_TAG, 0);
    assert_ne!(Sensor::PRESSURE_ZFMT_TAG, 0);
    assert_ne!(Sensor::RESET_ZFMT_TAG, 0);
}

#[test]
fn toplevel_tags_distinct() {
    assert_ne!(Sensor::TEMPERATURE_ZFMT_TAG, Sensor::PRESSURE_ZFMT_TAG);
    assert_ne!(Sensor::TEMPERATURE_ZFMT_TAG, Sensor::RESET_ZFMT_TAG);
    assert_ne!(Sensor::PRESSURE_ZFMT_TAG,    Sensor::RESET_ZFMT_TAG);
}

#[test]
fn toplevel_full_hash_lower32_is_tag() {
    assert_eq!(Sensor::TEMPERATURE_ZFMT_TAG, Sensor::TEMPERATURE_ZFMT_FULL_HASH as u32);
    assert_eq!(Sensor::PRESSURE_ZFMT_TAG,    Sensor::PRESSURE_ZFMT_FULL_HASH as u32);
}

#[test]
fn zfmt_tag_matches_const() {
    assert_eq!(Sensor::Temperature { celsius: 0.0 }.zfmt_tag(), Sensor::TEMPERATURE_ZFMT_TAG);
    assert_eq!(Sensor::Pressure { pascals: 0 }.zfmt_tag(),      Sensor::PRESSURE_ZFMT_TAG);
    assert_eq!(Sensor::Reset.zfmt_tag(),                         Sensor::RESET_ZFMT_TAG);
}

// --- Payload size

#[test]
fn payload_size_temperature() {
    // f32 = 4 bytes
    assert_eq!(Sensor::Temperature { celsius: 1.0 }.payload_size(), 4);
}

#[test]
fn payload_size_pressure() {
    // u32 = 4 bytes
    assert_eq!(Sensor::Pressure { pascals: 1 }.payload_size(), 4);
}

#[test]
fn payload_size_reset() {
    // unit variant — no fields
    assert_eq!(Sensor::Reset.payload_size(), 0);
}

#[test]
fn payload_size_tier2_variant() {
    // Text { text: "hello" } → LEB128(5)=1 + 5 = 6
    assert_eq!(Log::Text { text: "hello" }.payload_size(), 6);
    assert_eq!(Log::Code { code: 7 }.payload_size(), 4);
}

// --- Serialize

#[test]
fn serialize_temperature() {
    let v = Sensor::Temperature { celsius: 1.5 };
    let mut buf = vec![0u8; v.payload_size()];
    v.serialize_into(&mut buf);
    assert_eq!(&buf, &1.5f32.to_le_bytes());
}

#[test]
fn serialize_pressure() {
    let v = Sensor::Pressure { pascals: 0xDEAD_BEEF };
    let mut buf = vec![0u8; v.payload_size()];
    v.serialize_into(&mut buf);
    assert_eq!(&buf, &0xDEAD_BEEFu32.to_le_bytes());
}

#[test]
fn serialize_reset_empty() {
    let v = Sensor::Reset;
    let mut buf = vec![0u8; 0];
    v.serialize_into(&mut buf); // must not panic
}

#[test]
fn serialize_tier2_text() {
    let v = Log::Text { text: "hi" };
    let mut buf = vec![0u8; v.payload_size()];
    v.serialize_into(&mut buf);
    let (len, n) = decode_leb128(&buf);
    assert_eq!(len, 2);
    assert_eq!(&buf[n..n + 2], b"hi");
}

#[test]
fn serialize_tuple_variant() {
    let v = Point::D2(1.0, 2.0);
    let mut buf = vec![0u8; v.payload_size()];
    v.serialize_into(&mut buf);
    assert_eq!(&buf[0..4], &1.0f32.to_le_bytes());
    assert_eq!(&buf[4..8], &2.0f32.to_le_bytes());
}

// --- Format into

#[test]
fn format_temperature() {
    let mut w = w();
    Sensor::Temperature { celsius: 23.5 }.format_into(&mut w).unwrap();
    assert_eq!(w.0, "temperature=23.500000");
}

#[test]
fn format_pressure() {
    let mut w = w();
    Sensor::Pressure { pascals: 101325 }.format_into(&mut w).unwrap();
    assert_eq!(w.0, "pressure=101325");
}

#[test]
fn format_reset_empty() {
    let mut w = w();
    Sensor::Reset.format_into(&mut w).unwrap();
    assert_eq!(w.0, ""); // no format string → no output
}

#[test]
fn format_tier2_text() {
    let mut w = w();
    Log::Text { text: "hello world" }.format_into(&mut w).unwrap();
    assert_eq!(w.0, "msg=hello world");
}

#[test]
fn format_tuple_variant() {
    let mut w = w();
    Point::D2(3.0, 4.0).format_into(&mut w).unwrap();
    assert_eq!(w.0, "2d x=3.000000 y=4.000000");
}

// --- Inline enum

#[test]
fn inline_enum_discriminant_item_type() {
    // Both are repr(C, u8) → item type 1 (U8)
    assert_eq!(Priority::ZFMT_DISCRIMINANT_ITEM_TYPE, 1u8); // item::U8
    assert_eq!(Status::ZFMT_DISCRIMINANT_ITEM_TYPE, 1u8);
}

#[test]
fn inline_enum_variant_tags_nonzero() {
    assert_ne!(Priority::LOW_ZFMT_TAG, 0);
    assert_ne!(Priority::HIGH_ZFMT_TAG, 0);
    assert_ne!(Priority::LOW_ZFMT_TAG, Priority::HIGH_ZFMT_TAG);
}

#[test]
fn inline_enum_tags_differ_from_toplevel() {
    // Inline enum variant tags must not clash with top-level variant tags.
    assert_ne!(Priority::LOW_ZFMT_TAG, Sensor::TEMPERATURE_ZFMT_TAG);
}

#[test]
fn inline_enum_full_hash_lower32_is_tag() {
    assert_eq!(Priority::LOW_ZFMT_TAG, Priority::LOW_ZFMT_FULL_HASH as u32);
}
