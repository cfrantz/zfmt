//! Integration tests for Phase 3: format_into generation.

use zfmt::{Write, Zfmt};

struct StrWriter(String);
impl Write for StrWriter {
    fn write_str(&mut self, s: &str) -> Result<(), zfmt::Error> {
        self.0.push_str(s);
        Ok(())
    }
}
fn buf() -> StrWriter { StrWriter(String::new()) }

// ---- Structs ---------------------------------------------------------------

#[derive(Zfmt)]
#[repr(C)]
#[zfmt(format = "count={count} value={value}")]
pub struct Annotated {
    pub count: u32,
    pub value: u32,
}

#[derive(Zfmt)]
#[repr(C)]
#[zfmt(format = "ts={timestamp} flags={flags:08x}")]
pub struct Sensor {
    pub timestamp: u64,
    pub flags: u32,
    pub _pad: [u8; 4],
}

// A struct with no format string — should not have format_into.
#[derive(Zfmt)]
#[repr(C)]
pub struct NoFormat {
    pub x: u32,
}

// ---- Tests -----------------------------------------------------------------

#[test]
fn basic_format_into() {
    let a = Annotated { count: 42, value: 7 };
    let mut w = buf();
    a.format_into(&mut w).unwrap();
    assert_eq!(w.0, "count=42 value=7");
}

#[test]
fn hex_specifier() {
    let s = Sensor { timestamp: 1000, flags: 0xDEAD, _pad: [0; 4] };
    let mut w = buf();
    s.format_into(&mut w).unwrap();
    assert_eq!(w.0, "ts=1000 flags=0000dead");
}

#[test]
fn literal_only_before_and_after() {
    #[derive(Zfmt)]
    #[repr(C)]
    #[zfmt(format = "hello {x} world")]
    struct Wrap { pub x: u8 }

    let mut w = buf();
    Wrap { x: 5 }.format_into(&mut w).unwrap();
    assert_eq!(w.0, "hello 5 world");
}

#[test]
fn alignment_specifier() {
    #[derive(Zfmt)]
    #[repr(C)]
    #[zfmt(format = "[{v:>8}]")]
    struct R { pub v: u32 }

    let mut w = buf();
    R { v: 42 }.format_into(&mut w).unwrap();
    assert_eq!(w.0, "[      42]");
}
