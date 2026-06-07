//! `Write` trait — a minimal UTF-8 text sink with no `core::fmt` dependency.

/// Returned when a `Write` operation cannot accept more bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error;

/// Sink for UTF-8 text produced by the zfmt formatting engine.
///
/// Implement this for any destination that accepts UTF-8 text: a fixed-size
/// stack buffer, a UART driver, an IPC message buffer, etc.
///
/// `write_fmt` is deliberately absent; accepting `core::fmt::Arguments` would
/// reintroduce a `core::fmt` dependency.
pub trait Write {
    fn write_str(&mut self, s: &str) -> Result<(), Error>;

    fn write_char(&mut self, c: char) -> Result<(), Error> {
        self.write_str(c.encode_utf8(&mut [0u8; 4]))
    }
}

// ---------------------------------------------------------------------------
// A fixed-capacity stack buffer that implements Write — used in the logging
// macros to pre-format DebugMessage payloads.

pub struct FixedBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> Default for FixedBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> FixedBuf<N> {
    pub const fn new() -> Self {
        Self {
            buf: [0u8; N],
            len: 0,
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn as_str(&self) -> &str {
        // SAFETY: we only write valid UTF-8 through Write::write_str.
        unsafe { core::str::from_utf8_unchecked(&self.buf[..self.len]) }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> Write for FixedBuf<N> {
    fn write_str(&mut self, s: &str) -> Result<(), Error> {
        let bytes = s.as_bytes();
        let remaining = N - self.len;
        if bytes.len() > remaining {
            return Err(Error);
        }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_buf_basic() {
        let mut buf = FixedBuf::<16>::new();
        buf.write_str("hello").unwrap();
        buf.write_char(' ').unwrap();
        buf.write_str("world").unwrap();
        assert_eq!(buf.as_str(), "hello world");
    }

    #[test]
    fn fixed_buf_overflow() {
        let mut buf = FixedBuf::<4>::new();
        buf.write_str("abc").unwrap();
        assert_eq!(buf.write_str("de"), Err(Error));
        assert_eq!(buf.as_str(), "abc");
    }
}
