//! Integration tests for Phase 2: Tier-1 struct derive.

use zfmt::Zfmt;

// A simple Tier-1 struct with only fixed-size primitive fields.
#[derive(Zfmt)]
#[repr(C)]
pub struct Counter {
    pub count: u32,
    pub value: i16,
    pub _pad: [u8; 2],
}

// Struct matching the §3.6 worked example (Tier-2 — will be expanded in Phase 4,
// but we can still test that the derive compiles for it as a stub).

// A Tier-1 struct with mixed primitives and a fixed array.
#[derive(Zfmt)]
#[repr(C)]
pub struct Sensor {
    pub timestamp: u64,
    pub readings: [i16; 4],
    pub flags: u8,
    pub _pad: [u8; 7],
}

// A struct with a format string.
#[derive(Zfmt)]
#[repr(C)]
#[zfmt(format = "count={count} value={value}")]
pub struct Annotated {
    pub count: u32,
    pub value: u32,
}

#[test]
fn payload_size_equals_size_of() {
    let c = Counter { count: 1, value: -1, _pad: [0; 2] };
    assert_eq!(c.payload_size(), core::mem::size_of::<Counter>());

    let s = Sensor {
        timestamp: 42,
        readings: [1, 2, 3, 4],
        flags: 0,
        _pad: [0; 7],
    };
    assert_eq!(s.payload_size(), core::mem::size_of::<Sensor>());
}

#[test]
fn serialize_roundtrip() {
    let c = Counter { count: 0xDEAD_BEEF, value: -1, _pad: [0xAB, 0xCD] };
    let mut buf = [0u8; 8];
    c.serialize_into(&mut buf);

    // count: little-endian u32
    assert_eq!(&buf[0..4], &0xDEAD_BEEFu32.to_le_bytes());
    // value: little-endian i16 (-1 = 0xFFFF)
    assert_eq!(&buf[4..6], &(-1i16).to_le_bytes());
    // padding bytes
    assert_eq!(buf[6], 0xAB);
    assert_eq!(buf[7], 0xCD);
}

#[test]
fn tag_is_stable() {
    // The tag must be the same const every time; verify it is nonzero.
    assert_ne!(Counter::ZFMT_TAG, 0);
    assert_ne!(Sensor::ZFMT_TAG, 0);
    assert_ne!(Annotated::ZFMT_TAG, 0);

    // Tags must differ across distinct structs.
    assert_ne!(Counter::ZFMT_TAG, Sensor::ZFMT_TAG);
    assert_ne!(Counter::ZFMT_TAG, Annotated::ZFMT_TAG);
}

#[test]
fn full_hash_lower32_matches_tag() {
    assert_eq!(Counter::ZFMT_TAG, Counter::ZFMT_FULL_HASH as u32);
    assert_eq!(Sensor::ZFMT_TAG, Sensor::ZFMT_FULL_HASH as u32);
}

#[test]
fn sensor_serialize_roundtrip() {
    let s = Sensor {
        timestamp: 0x0102_0304_0506_0708u64,
        readings: [10, -20, 30, -40],
        flags: 0xFF,
        _pad: [0; 7],
    };
    let mut buf = [0u8; core::mem::size_of::<Sensor>()];
    s.serialize_into(&mut buf);

    assert_eq!(&buf[0..8], &0x0102_0304_0506_0708u64.to_le_bytes());
    assert_eq!(&buf[8..10], &10i16.to_le_bytes());
    assert_eq!(&buf[10..12], &(-20i16).to_le_bytes());
    assert_eq!(&buf[12..14], &30i16.to_le_bytes());
    assert_eq!(&buf[14..16], &(-40i16).to_le_bytes());
    assert_eq!(buf[16], 0xFF);
}
