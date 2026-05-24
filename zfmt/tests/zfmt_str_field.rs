//! Integration tests for ZfmtStr fields in Tier-1 structs (§4.7).

use zfmt::{Format, FormatInto, FormatSpec, ZfmtStr, Zfmt};

// ---------------------------------------------------------------------------
// Test structs

/// A Tier-1 struct with a ZfmtStr field (string-ref opcode in bytecode).
#[derive(Zfmt)]
#[repr(C)]
#[zfmt(format = "label={label} count={count}")]
pub struct Labeled {
    pub label: ZfmtStr,
    pub count: u32,
}

/// ZfmtStr-only struct for minimal coverage.
#[derive(Zfmt)]
#[repr(C)]
pub struct StringOnly {
    pub name: ZfmtStr,
}

// ---------------------------------------------------------------------------
// ZfmtStr type tests

#[test]
fn zfmt_str_new_roundtrip() {
    let s = ZfmtStr::new(0xDEAD_BEEF);
    assert_eq!(s.0, 0xDEAD_BEEF);
}

#[test]
fn zfmt_str_from_u32() {
    let s: ZfmtStr = ZfmtStr::from(0x1234_5678);
    assert_eq!(s.0, 0x1234_5678);
}

#[test]
fn zfmt_str_format_renders_hex() {
    use std::string::String;
    struct Buf(String);
    impl zfmt::Write for Buf {
        fn write_str(&mut self, s: &str) -> Result<(), zfmt::Error> {
            self.0.push_str(s);
            Ok(())
        }
    }
    let s = ZfmtStr::new(0xABCD_EF01);
    let mut w = Buf(String::new());
    s.fmt(&mut w, FormatSpec::default()).unwrap();
    assert_eq!(w.0, "0xabcdef01");
}

#[test]
fn zfmt_str_equality() {
    let a = ZfmtStr::new(42);
    let b = ZfmtStr::new(42);
    let c = ZfmtStr::new(99);
    assert_eq!(a, b);
    assert_ne!(a, c);
}

// ---------------------------------------------------------------------------
// Tier-1 struct with ZfmtStr field

#[test]
fn labeled_tag_nonzero() {
    assert_ne!(Labeled::ZFMT_TAG, 0);
}

#[test]
fn labeled_full_hash_lower32_is_tag() {
    assert_eq!(Labeled::ZFMT_FULL_HASH as u32, Labeled::ZFMT_TAG);
}

#[test]
fn labeled_payload_size() {
    let ev = Labeled { label: ZfmtStr::new(0), count: 0 };
    // ZfmtStr = 4 bytes (repr transparent over u32), count = 4 bytes → 8 total
    assert_eq!(ev.payload_size(), 8);
    assert_eq!(core::mem::size_of::<Labeled>(), 8);
}

#[test]
fn labeled_serialize_roundtrip() {
    let hash: u32 = 0xCAFE_BABE;
    let ev = Labeled { label: ZfmtStr::new(hash), count: 77 };
    let mut buf = [0u8; 8];
    ev.serialize_into(&mut buf);
    // label (u32 LE) at bytes 0..4
    assert_eq!(&buf[0..4], &hash.to_le_bytes());
    // count (u32 LE) at bytes 4..8
    assert_eq!(&buf[4..8], &77u32.to_le_bytes());
}

#[test]
fn string_only_payload_size() {
    let ev = StringOnly { name: ZfmtStr::new(0) };
    assert_eq!(ev.payload_size(), 4);
}

#[test]
fn string_only_serialize() {
    let ev = StringOnly { name: ZfmtStr::new(0x1234_5678) };
    let mut buf = [0u8; 4];
    ev.serialize_into(&mut buf);
    assert_eq!(&buf[..], &0x1234_5678u32.to_le_bytes());
}

// ---------------------------------------------------------------------------
// format_into with ZfmtStr field

#[test]
fn labeled_format_into() {
    use std::string::String;
    struct Buf(String);
    impl zfmt::Write for Buf {
        fn write_str(&mut self, s: &str) -> Result<(), zfmt::Error> {
            self.0.push_str(s);
            Ok(())
        }
    }
    let ev = Labeled { label: ZfmtStr::new(0xABCD_1234), count: 5 };
    let mut w = Buf(String::new());
    ev.format_into(&mut w).unwrap();
    assert_eq!(w.0, "label=0xabcd1234 count=5");
}

// ---------------------------------------------------------------------------
// zfmt_str! + ZfmtStr struct field round-trip

#[test]
fn zfmt_str_macro_matches_struct_field() {
    let hash = zfmt::zfmt_str!("my event label");
    let ev = StringOnly { name: ZfmtStr::new(hash) };
    let mut buf = [0u8; 4];
    ev.serialize_into(&mut buf);
    assert_eq!(&buf[..], &hash.to_le_bytes());
}
