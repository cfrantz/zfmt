//! Well-known zfmt events (§7).
//!
//! These are implemented manually rather than via #[derive(Zfmt)] because the
//! derive macro generates `::zfmt::` absolute paths, which cannot resolve from
//! within the `zfmt` crate itself.

use crate::{Format, FormatInto, FormatSpec, FormatType, Align, Write, Error, leb128, ZfmtEvent, ZfmtU64};

// ---------------------------------------------------------------------------
// §7.1  Severity

/// Log severity level.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Severity {
    Trace = 0,
    Debug = 1,
    Info  = 2,
    Warn  = 3,
    Error = 4,
    Fatal = 5,
}

impl Format for Severity {
    fn fmt<W: Write>(&self, writer: &mut W, _spec: FormatSpec) -> Result<(), Error> {
        writer.write_str(match self {
            Severity::Trace => "TRACE",
            Severity::Debug => "DEBUG",
            Severity::Info  => "INFO",
            Severity::Warn  => "WARN",
            Severity::Error => "ERROR",
            Severity::Fatal => "FATAL",
        })
    }
}

// ---------------------------------------------------------------------------
// Linker entry helpers
//
// Entry layout (matches the derive macro's output, §8.1):
//   tag(4) | _pad(4) | full_hash(8) | format_hash(4) | _pad(4) | bc_len(4) | bytecode[padded to 4]
//
// Total header = 28 bytes.

const fn u32_le(v: u32) -> [u8; 4] { v.to_le_bytes() }
const fn u64_le(v: u64) -> [u8; 8] { v.to_le_bytes() }

// ---------------------------------------------------------------------------
// §7.2  EventHeader
//
// Non-padding fields used in hash: timestamp:ZfmtU64, severity:u8, seq:[u8;3]
// Hash input:
//   "struct EventHeader\nformat {timestamp} {severity}\nfield timestamp ZfmtU64\nfield severity u8\nfield seq [u8; 3]\n"
// full_hash = 0x5a19e4cfe43ae42d, tag = 0xe43ae42d
// format_hash = fnv1a_64("{timestamp} {severity}") as u32 = 0x112d69b2
//
// Layout: ZfmtU64(8) + u8(1) + [u8;3](3) = 12 bytes, 4-byte aligned.
//
// Bytecode: U64_PAIR/single(0x88) U8/single(0x08) UTF8_BYTE|FIXED_ARRAY(0x49) LEB128(3)(0x03) END(0x00) → 5 raw bytes
// Padded:   [0x88, 0x08, 0x49, 0x03, 0x00, 0x00, 0x00, 0x00]  bc_len=5
//
// `seq` is a 24-bit little-endian sequence counter assigned by the Logger.
// The host decoder reads it at known byte offsets [9..12] when protocol_version >= 2
// (§7.3); the bytecode treats it as a [u8;3] fixed array consumed but not
// referenced by the format string, so generic decoders ignore it.

/// Precedes every log record in the stream.
#[repr(C)]
pub struct EventHeader {
    pub timestamp: ZfmtU64,
    /// Raw severity discriminant (Severity as u8).
    pub severity: u8,
    /// 24-bit sequence counter, little-endian. Zero when sequencing is disabled.
    pub seq: [u8; 3],
}

impl EventHeader {
    pub const ZFMT_TAG: u32       = 0xe43ae42d;
    pub const ZFMT_FULL_HASH: u64 = 0x5a19e4cfe43ae42d;

    pub fn new(timestamp: ZfmtU64, severity: Severity, seq: u32) -> Self {
        let s = seq & 0x00FF_FFFF;
        Self { timestamp, severity: severity as u8, seq: [s as u8, (s >> 8) as u8, (s >> 16) as u8] }
    }

    /// Reconstruct the 24-bit sequence counter as a u32.
    pub fn seq_value(&self) -> u32 {
        u32::from_le_bytes([self.seq[0], self.seq[1], self.seq[2], 0])
    }

    pub fn zfmt_tag(&self) -> u32 { Self::ZFMT_TAG }

    pub fn payload_size(&self) -> usize { core::mem::size_of::<Self>() }

    pub fn serialize_into(&self, buf: &mut [u8]) {
        let n = core::mem::size_of::<Self>();
        let bytes = unsafe { core::slice::from_raw_parts(self as *const Self as *const u8, n) };
        buf[..n].copy_from_slice(bytes);
    }

    pub fn format_into<W: Write>(&self, writer: &mut W) -> Result<(), Error> {
        let spec = FormatSpec { ty: FormatType::Display, align: Align::None,
            alternate: false, sign: false, zero_pad: false, width: 0, precision: None };
        self.timestamp.fmt(writer, spec)?;
        writer.write_char(' ')?;
        writer.write_str(match self.severity {
            0 => "TRACE", 1 => "DEBUG", 2 => "INFO",
            3 => "WARN",  4 => "ERROR", 5 => "FATAL",
            _ => "?",
        })
        // `seq` is transport metadata; not referenced by the format string.
    }

    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() == 12 {
            let tlo = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            let thi = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
            Some(Self { timestamp: ZfmtU64::new(tlo, thi),
                severity: buf[8],
                seq: [buf[9], buf[10], buf[11]],
            })
        } else {
            None
        }
    }
}

impl ZfmtEvent for EventHeader {
    fn zfmt_tag(&self) -> u32 { Self::ZFMT_TAG }
    fn payload_size(&self) -> usize { core::mem::size_of::<Self>() }
    fn with_payload_bytes<F: FnOnce(&[u8])>(&self, f: F) {
        // SAFETY: repr(C), explicit _pad field — no uninitialized bytes.
        let bytes = unsafe {
            core::slice::from_raw_parts(self as *const Self as *const u8, core::mem::size_of::<Self>())
        };
        f(bytes);
    }
}

impl FormatInto for EventHeader {} // no format string — uses default no-op

#[used]
#[cfg_attr(    target_os = "none",  link_section = ".zfmt_events.e43ae42d")]
#[cfg_attr(not(target_os = "none"), link_section = ".zfmt_events.e43ae42d")]
static _ZFMT_EVENT_HEADER: [u8; 36] = {
    let t  = u32_le(EventHeader::ZFMT_TAG);
    let fh = u64_le(EventHeader::ZFMT_FULL_HASH);
    let fm = u32_le(0x112d69b2u32); // format_hash
    [
        t[0], t[1], t[2], t[3],            // tag
        0, 0, 0, 0,                         // _pad
        fh[0], fh[1], fh[2], fh[3], fh[4], fh[5], fh[6], fh[7], // full_hash
        fm[0], fm[1], fm[2], fm[3],         // format_hash
        0, 0, 0, 0,                         // _pad
        5, 0, 0, 0,                         // bc_len = 5
        0x88, 0x08, 0x49, 0x03, 0x00,       // bytecode: U64_PAIR U8 UTF8_BYTE|FIXED_ARRAY/3 END
        0, 0, 0,                            // padding to 4-byte boundary
    ]
};

// String section for EventHeader format string "{timestamp} {severity}" (22 bytes).
// hash=0x112d69b2, len=22; entry padded to 32 bytes.
#[used]
#[cfg_attr(    target_os = "none",  link_section = ".zfmt_strings.112d69b2")]
#[cfg_attr(not(target_os = "none"), link_section = ".zfmt_strings.112d69b2")]
static _ZFMT_STR_EVENT_HEADER_FMT: [u8; 32] = [
    0xb2, 0x69, 0x2d, 0x11,     // hash = 0x112d69b2
    0x16, 0x00,                  // len = 22
    0x00, 0x00,                  // _pad
    // "{timestamp} {severity}"
    b'{', b't', b'i', b'm', b'e', b's', b't', b'a',
    b'm', b'p', b'}', b' ',  b'{', b's', b'e', b'v',
    b'e', b'r', b'i', b't', b'y', b'}',
    0x00, 0x00,                  // padding to 32 bytes
];

// ---------------------------------------------------------------------------
// §7.3  StreamStart
//
// Non-padding fields: protocol_version:u16, tick_rate_hz:ZfmtU64, firmware_build_id:ZfmtU64
// Hash input:
//   "struct StreamStart\nfield protocol_version u16\nfield tick_rate_hz ZfmtU64\nfield firmware_build_id ZfmtU64\n"
// full_hash = 0x1aaef8910ef1ba00, tag = 0x0ef1ba00, format_hash = 0
//
// Layout: u16(2) + _pad0(2) + ZfmtU64(8) + ZfmtU64(8) = 20 bytes, 4-byte aligned.
//
// Bytecode: U16/single(0x10) SKIP/fa 2(0x51,0x02) U64_PAIR/single(0x88) U64_PAIR/single(0x88) END(0x00) → 6 bytes
// Padded:   [0x10, 0x51, 0x02, 0x88, 0x88, 0x00, 0x00, 0x00]  bc_len=6

/// First event in every stream.
#[repr(C)]
pub struct StreamStart {
    pub protocol_version: u16,
    pub _pad0: [u8; 2],
    pub tick_rate_hz: ZfmtU64,
    pub firmware_build_id: ZfmtU64,
}

impl StreamStart {
    pub const ZFMT_TAG: u32       = 0x0ef1ba00;
    pub const ZFMT_FULL_HASH: u64 = 0x1aaef8910ef1ba00;
    pub const PROTOCOL_VERSION: u16 = 2;

    pub fn zfmt_tag(&self) -> u32 { Self::ZFMT_TAG }
    pub fn payload_size(&self) -> usize { core::mem::size_of::<Self>() }

    pub fn serialize_into(&self, buf: &mut [u8]) {
        let n = core::mem::size_of::<Self>();
        let bytes = unsafe { core::slice::from_raw_parts(self as *const Self as *const u8, n) };
        buf[..n].copy_from_slice(bytes);
    }
}

impl ZfmtEvent for StreamStart {
    fn zfmt_tag(&self) -> u32 { Self::ZFMT_TAG }
    fn payload_size(&self) -> usize { core::mem::size_of::<Self>() }
    fn with_payload_bytes<F: FnOnce(&[u8])>(&self, f: F) {
        // SAFETY: repr(C), explicit _pad0 field — no uninitialized bytes.
        let bytes = unsafe {
            core::slice::from_raw_parts(self as *const Self as *const u8, core::mem::size_of::<Self>())
        };
        f(bytes);
    }
}

impl FormatInto for StreamStart {} // no format string — uses default no-op

#[used]
#[cfg_attr(    target_os = "none",  link_section = ".zfmt_events.0ef1ba00")]
#[cfg_attr(not(target_os = "none"), link_section = ".zfmt_events.0ef1ba00")]
static _ZFMT_STREAM_START: [u8; 36] = {
    let t  = u32_le(StreamStart::ZFMT_TAG);
    let fh = u64_le(StreamStart::ZFMT_FULL_HASH);
    [
        t[0], t[1], t[2], t[3],
        0, 0, 0, 0,
        fh[0], fh[1], fh[2], fh[3], fh[4], fh[5], fh[6], fh[7],
        0, 0, 0, 0,         // format_hash = 0
        0, 0, 0, 0,         // _pad
        6, 0, 0, 0,         // bc_len = 6
        0x10, 0x51, 0x02, 0x88, 0x88, 0x00, // bytecode: U16 SKIP/2 U64_PAIR U64_PAIR END
        0, 0,               // padding to 4-byte boundary
    ]
};

// ---------------------------------------------------------------------------
// §7.4  DroppedEvents
//
// Non-padding fields: count:u32
// Hash input:
//   "struct DroppedEvents\nfield count u32\n"
// full_hash = 0xcb0b57d1e0ee1b4e, tag = 0xe0ee1b4e, format_hash = 0
//
// Bytecode: U32/single(0x18) SKIP/fa 4(0x51,0x04) END(0x00) + 1 pad → 4 bytes
// bc_len = 4 (including END; skips the struct pad)

/// Emitted when the logger drops events (ring buffer overflow).
#[repr(C)]
pub struct DroppedEvents {
    pub count: u32,
    pub _pad: [u8; 4],
}

impl DroppedEvents {
    pub const ZFMT_TAG: u32       = 0xe0ee1b4e;
    pub const ZFMT_FULL_HASH: u64 = 0xcb0b57d1e0ee1b4e;

    pub fn zfmt_tag(&self) -> u32 { Self::ZFMT_TAG }
    pub fn payload_size(&self) -> usize { core::mem::size_of::<Self>() }

    pub fn serialize_into(&self, buf: &mut [u8]) {
        let n = core::mem::size_of::<Self>();
        let bytes = unsafe { core::slice::from_raw_parts(self as *const Self as *const u8, n) };
        buf[..n].copy_from_slice(bytes);
    }
}

impl ZfmtEvent for DroppedEvents {
    fn zfmt_tag(&self) -> u32 { Self::ZFMT_TAG }
    fn payload_size(&self) -> usize { core::mem::size_of::<Self>() }
    fn with_payload_bytes<F: FnOnce(&[u8])>(&self, f: F) {
        // SAFETY: repr(C), explicit _pad field — no uninitialized bytes.
        let bytes = unsafe {
            core::slice::from_raw_parts(self as *const Self as *const u8, core::mem::size_of::<Self>())
        };
        f(bytes);
    }
}

impl FormatInto for DroppedEvents {} // no format string — uses default no-op

#[used]
#[cfg_attr(    target_os = "none",  link_section = ".zfmt_events.e0ee1b4e")]
#[cfg_attr(not(target_os = "none"), link_section = ".zfmt_events.e0ee1b4e")]
static _ZFMT_DROPPED_EVENTS: [u8; 32] = {
    let t  = u32_le(DroppedEvents::ZFMT_TAG);
    let fh = u64_le(DroppedEvents::ZFMT_FULL_HASH);
    [
        t[0], t[1], t[2], t[3],
        0, 0, 0, 0,
        fh[0], fh[1], fh[2], fh[3], fh[4], fh[5], fh[6], fh[7],
        0, 0, 0, 0,         // format_hash = 0
        0, 0, 0, 0,         // _pad
        4, 0, 0, 0,         // bc_len = 4
        0x18, 0x51, 0x04, 0x00, // bytecode (4 bytes)
    ]
};

// ---------------------------------------------------------------------------
// §7.5  DebugMessage (Tier-2)
//
// Hash input:
//   "struct DebugMessage\nformat {message}\nfield message str\n"
// full_hash = 0xcef2c6c3a1a6a340, tag = 0xa1a6a340, format_hash = 0x524fb994
//
// Bytecode: UTF8_BYTE/var-length(0x4b) END(0x00) → 2 bytes padded to 4

/// Unstructured text message.
pub struct DebugMessage<'a> {
    pub message: &'a str,
}

impl<'a> DebugMessage<'a> {
    pub const ZFMT_TAG: u32       = 0xa1a6a340;
    pub const ZFMT_FULL_HASH: u64 = 0xcef2c6c3a1a6a340;

    pub fn zfmt_tag(&self) -> u32 { Self::ZFMT_TAG }

    pub fn payload_size(&self) -> usize {
        leb128::encoded_len(self.message.len() as u32) + self.message.len()
    }

    pub fn serialize_into(&self, buf: &mut [u8]) {
        let mut pos = 0usize;
        let mut leb = [0u8; 5];
        let ln = leb128::encode(self.message.len() as u32, &mut leb);
        buf[pos..pos + ln].copy_from_slice(&leb[..ln]);
        pos += ln;
        let sb = self.message.as_bytes();
        buf[pos..pos + sb.len()].copy_from_slice(sb);
    }

}

impl<'a> FormatInto for DebugMessage<'a> {
    fn format_into<W: Write>(&self, writer: &mut W) -> Result<(), Error> {
        writer.write_str(self.message)
    }
}

impl<'a> ZfmtEvent for DebugMessage<'a> {
    fn zfmt_tag(&self) -> u32 { Self::ZFMT_TAG }
    fn payload_size(&self) -> usize { self.payload_size() }
    fn with_payload_bytes<F: FnOnce(&[u8])>(&self, f: F) {
        const MAX_MSG: usize = 256;
        let sz = self.payload_size();
        let mut buf = [0u8; MAX_MSG];
        self.serialize_into(&mut buf);
        f(&buf[..sz]);
    }
}

// String section for DebugMessage format string "{message}" (9 bytes).
// hash=0x524fb994, len=9; entry padded to 20 bytes.
#[used]
#[cfg_attr(    target_os = "none",  link_section = ".zfmt_strings.524fb994")]
#[cfg_attr(not(target_os = "none"), link_section = ".zfmt_strings.524fb994")]
static _ZFMT_STR_DEBUG_MESSAGE_FMT: [u8; 20] = [
    0x94, 0xb9, 0x4f, 0x52,     // hash = 0x524fb994
    0x09, 0x00,                  // len = 9
    0x00, 0x00,                  // _pad
    // "{message}"
    b'{', b'm', b'e', b's', b's', b'a', b'g', b'e', b'}',
    0x00, 0x00, 0x00,            // padding to 20 bytes
];

#[used]
#[cfg_attr(    target_os = "none",  link_section = ".zfmt_events.a1a6a340")]
#[cfg_attr(not(target_os = "none"), link_section = ".zfmt_events.a1a6a340")]
static _ZFMT_DEBUG_MESSAGE: [u8; 32] = {
    let t  = u32_le(DebugMessage::ZFMT_TAG);
    let fh = u64_le(DebugMessage::ZFMT_FULL_HASH);
    let fm = u32_le(0x524fb994u32);
    [
        t[0], t[1], t[2], t[3],
        0, 0, 0, 0,
        fh[0], fh[1], fh[2], fh[3], fh[4], fh[5], fh[6], fh[7],
        fm[0], fm[1], fm[2], fm[3],
        0, 0, 0, 0,         // _pad
        2, 0, 0, 0,         // bc_len = 2
        0x4b, 0x00, 0, 0,   // bytecode (2 bytes) + 2 pad
    ]
};
