//! Unsigned LEB128 encode/decode for the event stream (§4.4).

/// Encode `value` into `buf`, returning the number of bytes written.
/// `buf` must be at least 10 bytes long.
pub fn encode(mut value: u64, buf: &mut [u8]) -> usize {
    let mut i = 0;
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            buf[i] = byte;
            i += 1;
            break;
        } else {
            buf[i] = byte | 0x80;
            i += 1;
        }
    }
    i
}

/// Decode a LEB128 value from `buf`, returning `(value, bytes_consumed)`.
/// Returns `None` if `buf` is empty or truncated.
pub fn decode(buf: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    for (i, &byte) in buf.iter().enumerate() {
        let low = (byte & 0x7f) as u64;
        value |= low << shift;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
        shift += 7;
        if shift >= 64 {
            return None; // overflow
        }
    }
    None // truncated
}

/// Return the number of bytes needed to LEB128-encode `value`.
pub fn encoded_len(mut value: u64) -> usize {
    let mut n = 1usize;
    while value >= 0x80 {
        value >>= 7;
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(v: u64) {
        let mut buf = [0u8; 10];
        let n = encode(v, &mut buf);
        let (decoded, consumed) = decode(&buf[..n]).unwrap();
        assert_eq!(decoded, v, "roundtrip failed for {}", v);
        assert_eq!(consumed, n);
        assert_eq!(n, encoded_len(v));
    }

    #[test]
    fn roundtrip_values() {
        for v in [0u64, 1, 127, 128, 255, 300, 16383, 16384, u32::MAX as u64, u64::MAX] {
            roundtrip(v);
        }
    }

    #[test]
    fn single_byte_range() {
        let mut buf = [0u8; 1];
        assert_eq!(encode(0, &mut buf), 1);
        assert_eq!(buf[0], 0x00);
        assert_eq!(encode(127, &mut buf), 1);
        assert_eq!(buf[0], 0x7f);
    }

    #[test]
    fn two_byte_128() {
        let mut buf = [0u8; 2];
        let n = encode(128, &mut buf);
        assert_eq!(n, 2);
        assert_eq!(&buf[..2], &[0x80, 0x01]);
    }

    #[test]
    fn decode_truncated_returns_none() {
        assert_eq!(decode(&[0x80]), None);
    }

    #[test]
    fn encoded_len_values() {
        assert_eq!(encoded_len(0), 1);
        assert_eq!(encoded_len(127), 1);
        assert_eq!(encoded_len(128), 2);
        assert_eq!(encoded_len(16383), 2);
        assert_eq!(encoded_len(16384), 3);
    }
}
