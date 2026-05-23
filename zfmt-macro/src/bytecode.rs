//! Bytecode encoding helpers (§4).
//!
//! The output of this module is a Vec<u8> that becomes a compile-time byte
//! array literal in the generated code.

/// Item type values (§4.2, bits 7..3).
#[allow(dead_code)]
pub mod item {
    pub const END: u8 = 0;
    pub const U8: u8 = 1;
    pub const U16: u8 = 2;
    pub const U32: u8 = 3;
    pub const U64: u8 = 4;
    pub const I8: u8 = 5;
    pub const I16: u8 = 6;
    pub const I32: u8 = 7;
    pub const I64: u8 = 8;
    pub const UTF8_BYTE: u8 = 9;
    pub const SKIP: u8 = 10;
    pub const F32: u8 = 11;
    pub const F64: u8 = 12;
    pub const BOOL: u8 = 13;
    pub const DISPATCH: u8 = 14;
    pub const CALL: u8 = 15;
    pub const STRING_REF: u8 = 16;
}

/// Operand type values (§4.3, bits 2..0).
#[allow(dead_code)]
pub mod operand {
    pub const SINGLE: u8 = 0;
    pub const FIXED_ARRAY: u8 = 1;
    pub const ZERO_TERM: u8 = 2;
    pub const VAR_LENGTH: u8 = 3;
}

pub fn opcode(item_type: u8, operand_type: u8) -> u8 {
    (item_type << 3) | (operand_type & 0x07)
}

/// Encode an unsigned integer as LEB128, appending bytes to `out`.
pub fn push_uleb128(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        } else {
            out.push(byte | 0x80);
        }
    }
}

/// Encode a single "skip N bytes" instruction sequence.
pub fn push_skip(out: &mut Vec<u8>, n: usize) {
    if n == 0 {
        return;
    }
    // skip with fixed-array operand: item=SKIP, operand=FIXED_ARRAY, then LEB128 count
    out.push(opcode(item::SKIP, operand::FIXED_ARRAY));
    push_uleb128(out, n as u64);
}

/// Map a canonical type name to its item type byte.
/// Returns `None` for types handled specially (arrays, str, custom).
pub fn item_type_for(canonical: &str) -> Option<u8> {
    match canonical {
        "u8" => Some(item::U8),
        "u16" => Some(item::U16),
        "u32" => Some(item::U32),
        "u64" => Some(item::U64),
        "i8" => Some(item::I8),
        "i16" => Some(item::I16),
        "i32" => Some(item::I32),
        "i64" => Some(item::I64),
        "f32" => Some(item::F32),
        "f64" => Some(item::F64),
        "bool" => Some(item::BOOL),
        _ => None,
    }
}

/// Byte size of a primitive canonical type (None for variable-size or unknown).
pub fn size_of_canonical(canonical: &str) -> Option<usize> {
    match canonical {
        "u8" | "i8" | "bool" => Some(1),
        "u16" | "i16" => Some(2),
        "u32" | "i32" | "f32" => Some(4),
        "u64" | "i64" | "f64" => Some(8),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_encoding() {
        // u8 single = (1 << 3) | 0 = 0x08
        assert_eq!(opcode(item::U8, operand::SINGLE), 0x08);
        // skip fixed-array = (10 << 3) | 1 = 0x51
        assert_eq!(opcode(item::SKIP, operand::FIXED_ARRAY), 0x51);
    }

    #[test]
    fn uleb128_single_byte() {
        let mut out = Vec::new();
        push_uleb128(&mut out, 42);
        assert_eq!(out, &[42]);
    }

    #[test]
    fn uleb128_multibyte() {
        let mut out = Vec::new();
        push_uleb128(&mut out, 128);
        assert_eq!(out, &[0x80, 0x01]);
    }

    #[test]
    fn uleb128_300() {
        let mut out = Vec::new();
        push_uleb128(&mut out, 300);
        // 300 = 0x12C; low 7 bits = 0x2C | 0x80, next = 0x02
        assert_eq!(out, &[0xAC, 0x02]);
    }

    #[test]
    fn skip_zero_emits_nothing() {
        let mut out = Vec::new();
        push_skip(&mut out, 0);
        assert!(out.is_empty());
    }

    #[test]
    fn skip_emits_opcode_and_length() {
        let mut out = Vec::new();
        push_skip(&mut out, 3);
        // opcode(SKIP, FIXED_ARRAY) = 0x51, then LEB128(3) = 0x03
        assert_eq!(out, &[0x51, 0x03]);
    }
}
