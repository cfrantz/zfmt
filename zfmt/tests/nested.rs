//! Integration tests for nested Zfmt struct support (CALL opcode, §4.5).

use zfmt::{FormatInto, Write, Zfmt};

struct StrBuf(String);
impl Write for StrBuf {
    fn write_str(&mut self, s: &str) -> Result<(), zfmt::Error> {
        self.0.push_str(s);
        Ok(())
    }
}
fn buf() -> StrBuf { StrBuf(String::new()) }

// Inner struct used as a field in outer structs.
#[derive(Zfmt)]
#[repr(C)]
#[zfmt(format = "celsius={c} sensor={s}")]
pub struct TempReading {
    pub c: i16,
    pub s: u8,
    pub _pad: u8,
}

// Outer struct: nested field immediately after a fixed field.
#[derive(Zfmt)]
#[repr(C)]
#[zfmt(format = "seq={seq} temp={reading}")]
pub struct SensorPacket {
    pub seq: u32,
    pub reading: TempReading,
}

// Outer struct: nested field with alignment gap + trailing fixed field.
#[derive(Zfmt)]
#[repr(C)]
#[zfmt(format = "id={id} temp={reading} flags={flags}")]
pub struct Report {
    pub id: u16,
    pub _pad: [u8; 2],
    pub reading: TempReading,
    pub flags: u32,
}

// --- Tag tests

#[test]
fn inner_tag_nonzero() {
    assert_ne!(TempReading::ZFMT_TAG, 0);
}

#[test]
fn outer_tags_nonzero() {
    assert_ne!(SensorPacket::ZFMT_TAG, 0);
    assert_ne!(Report::ZFMT_TAG, 0);
}

#[test]
fn nested_tags_distinct() {
    assert_ne!(TempReading::ZFMT_TAG, SensorPacket::ZFMT_TAG);
    assert_ne!(TempReading::ZFMT_TAG, Report::ZFMT_TAG);
    assert_ne!(SensorPacket::ZFMT_TAG, Report::ZFMT_TAG);
}

#[test]
fn full_hash_lower32_is_tag() {
    assert_eq!(TempReading::ZFMT_FULL_HASH as u32, TempReading::ZFMT_TAG);
    assert_eq!(SensorPacket::ZFMT_FULL_HASH as u32, SensorPacket::ZFMT_TAG);
    assert_eq!(Report::ZFMT_FULL_HASH as u32, Report::ZFMT_TAG);
}

// --- payload_size

#[test]
fn inner_payload_size() {
    let t = TempReading { c: 0, s: 0, _pad: 0 };
    assert_eq!(t.payload_size(), core::mem::size_of::<TempReading>());
}

#[test]
fn outer_payload_size() {
    let p = SensorPacket { seq: 0, reading: TempReading { c: 0, s: 0, _pad: 0 } };
    assert_eq!(p.payload_size(), core::mem::size_of::<SensorPacket>());
}

// --- serialize_into

#[test]
fn inner_serialize() {
    let t = TempReading { c: -10i16, s: 3, _pad: 0xAB };
    let mut buf = vec![0u8; t.payload_size()];
    t.serialize_into(&mut buf);
    assert_eq!(&buf[0..2], &(-10i16).to_le_bytes());
    assert_eq!(buf[2], 3);
    assert_eq!(buf[3], 0xAB);
}

#[test]
fn outer_serialize() {
    let p = SensorPacket {
        seq: 0x12345678,
        reading: TempReading { c: 215, s: 7, _pad: 0 },
    };
    let mut buf = vec![0u8; p.payload_size()];
    p.serialize_into(&mut buf);
    // seq at offset 0
    assert_eq!(&buf[0..4], &0x12345678u32.to_le_bytes());
    // reading.c at offset 4
    assert_eq!(&buf[4..6], &215i16.to_le_bytes());
    // reading.s at offset 6
    assert_eq!(buf[6], 7);
}

#[test]
fn report_serialize() {
    let r = Report {
        id: 0xBEEF,
        _pad: [0; 2],
        reading: TempReading { c: 100, s: 1, _pad: 0 },
        flags: 0xCAFE,
    };
    let mut buf = vec![0u8; r.payload_size()];
    r.serialize_into(&mut buf);
    assert_eq!(&buf[0..2], &0xBEEFu16.to_le_bytes());
    // reading.c at offset 4
    assert_eq!(&buf[4..6], &100i16.to_le_bytes());
    // flags at offset 8
    assert_eq!(&buf[8..12], &0xCAFEu32.to_le_bytes());
}

// --- format_into

#[test]
fn inner_format_into() {
    let t = TempReading { c: 215, s: 3, _pad: 0 };
    let mut w = buf();
    t.format_into(&mut w).unwrap();
    assert_eq!(w.0, "celsius=215 sensor=3");
}

#[test]
fn outer_format_into_renders_nested() {
    let p = SensorPacket {
        seq: 42,
        reading: TempReading { c: 215, s: 3, _pad: 0 },
    };
    let mut w = buf();
    p.format_into(&mut w).unwrap();
    assert_eq!(w.0, "seq=42 temp=celsius=215 sensor=3");
}

#[test]
fn report_format_into() {
    let r = Report {
        id: 7,
        _pad: [0; 2],
        reading: TempReading { c: 100, s: 1, _pad: 0 },
        flags: 0xAB,
    };
    let mut w = buf();
    r.format_into(&mut w).unwrap();
    assert_eq!(w.0, "id=7 temp=celsius=100 sensor=1 flags=171");
}
