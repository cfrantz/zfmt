//! Integration tests for Phase 4: Tier-2 (variable-length field) events.

use zfmt::{Write, Zfmt};

// --- Tier-2 structs ---------------------------------------------------------

#[derive(Zfmt)]
#[zfmt(format = "{message}")]
pub struct DebugMsg<'a> {
    pub message: &'a str,
}

#[derive(Zfmt)]
#[zfmt(format = "{key}={value}")]
pub struct KvPair<'a> {
    pub key: &'a str,
    pub value: &'a str,
}

// Mixed: fixed field then str field.
#[derive(Zfmt)]
#[repr(C)]
#[zfmt(format = "code={code} msg={msg}")]
pub struct CodeMsg<'a> {
    pub code: u32,
    pub msg: &'a str,
}

// Str field then fixed field.
#[derive(Zfmt)]
#[zfmt(format = "name={name} id={id}")]
pub struct Named<'a> {
    pub name: &'a str,
    pub id: u16,
}

// --- Helpers ----------------------------------------------------------------

fn decode_leb128(buf: &[u8]) -> (u64, usize) {
    zfmt::leb128::decode(buf).expect("valid LEB128")
}

// --- Tests ------------------------------------------------------------------

#[test]
fn single_str_payload_size() {
    let m = DebugMsg { message: "hello" };
    // LEB128(5) = 1 byte + 5 bytes = 6
    assert_eq!(m.payload_size(), 6);

    let m2 = DebugMsg { message: "" };
    assert_eq!(m2.payload_size(), 1); // LEB128(0) = 1 byte

    let long: String = "x".repeat(128);
    let m3 = DebugMsg { message: &long };
    // LEB128(128) = 2 bytes + 128 = 130
    assert_eq!(m3.payload_size(), 130);
}

#[test]
fn single_str_serialize() {
    let m = DebugMsg { message: "hi" };
    let mut buf = vec![0u8; m.payload_size()];
    m.serialize_into(&mut buf);

    let (len, consumed) = decode_leb128(&buf);
    assert_eq!(len, 2);
    assert_eq!(&buf[consumed..consumed + 2], b"hi");
}

#[test]
fn two_str_fields() {
    let kv = KvPair { key: "foo", value: "bar" };
    // key: LEB128(3)=1 + 3 = 4; value: LEB128(3)=1 + 3 = 4; total = 8
    assert_eq!(kv.payload_size(), 8);

    let mut buf = vec![0u8; kv.payload_size()];
    kv.serialize_into(&mut buf);

    let (klen, kc) = decode_leb128(&buf);
    assert_eq!(klen, 3);
    assert_eq!(&buf[kc..kc + 3], b"foo");

    let rest = &buf[kc + 3..];
    let (vlen, vc) = decode_leb128(rest);
    assert_eq!(vlen, 3);
    assert_eq!(&rest[vc..vc + 3], b"bar");
}

#[test]
fn mixed_fixed_then_str() {
    let cm = CodeMsg { code: 42, msg: "err" };
    // u32(4) + LEB128(3)=1 + 3 = 8
    assert_eq!(cm.payload_size(), 8);

    let mut buf = vec![0u8; cm.payload_size()];
    cm.serialize_into(&mut buf);

    // First 4 bytes: u32 little-endian
    assert_eq!(&buf[0..4], &42u32.to_le_bytes());
    let (msglen, mc) = decode_leb128(&buf[4..]);
    assert_eq!(msglen, 3);
    assert_eq!(&buf[4 + mc..4 + mc + 3], b"err");
}

#[test]
fn tag_nonzero_and_differs() {
    assert_ne!(DebugMsg::ZFMT_TAG, 0);
    assert_ne!(KvPair::ZFMT_TAG, 0);
    assert_ne!(DebugMsg::ZFMT_TAG, KvPair::ZFMT_TAG);
}

#[test]
fn format_into_tier2() {
    struct W(String);
    impl Write for W {
        fn write_str(&mut self, s: &str) -> Result<(), zfmt::Error> {
            self.0.push_str(s); Ok(())
        }
    }
    let cm = CodeMsg { code: 7, msg: "oops" };
    let mut w = W(String::new());
    cm.format_into(&mut w).unwrap();
    assert_eq!(w.0, "code=7 msg=oops");
}
