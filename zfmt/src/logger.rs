//! Logger trait and adapters for firmware-side event transport.

use crate::format::ZfmtU64;

/// Primary logger interface — implemented by the console task's static LOGGER.
///
/// Static dispatch throughout: no `dyn Logger`, enabling static stack analysis.
pub trait Logger {
    fn timestamp(&self) -> ZfmtU64;

    /// 24-bit sequence counter for gap detection on the host (§7.2).
    ///
    /// The value is masked to 24 bits when written into `EventHeader.seq`.
    /// The default returns 0, disabling sequence tracking.  Override this
    /// in the main log-handling task's Logger implementation; IPC-client
    /// loggers should leave the default.
    fn next_seq(&self) -> u32 { 0 }

    /// Send a gather-write list of byte slices as a single logical message.
    fn send_vectored(&self, bufs: &[&[u8]]);

    /// Convenience wrapper; defaults to a single-slice vectored call.
    fn send(&self, data: &[u8]) {
        self.send_vectored(&[data]);
    }
}

/// Implemented by tasks whose IPC layer only supports flat (contiguous) sends.
pub trait FlatSend {
    fn timestamp(&self) -> ZfmtU64;
    fn send(&self, data: &[u8]);
}

/// Wraps a `FlatSend` implementation and presents a `Logger` interface by
/// assembling scattered slices into a stack-local buffer of `N` bytes before
/// forwarding.
pub struct FlatAdapter<L: FlatSend, const N: usize> {
    inner: L,
}

impl<L: FlatSend, const N: usize> FlatAdapter<L, N> {
    pub const fn new(inner: L) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &L {
        &self.inner
    }

}

impl<L: FlatSend, const N: usize> Logger for FlatAdapter<L, N> {
    fn timestamp(&self) -> ZfmtU64 {
        self.inner.timestamp()
    }

    fn send_vectored(&self, bufs: &[&[u8]]) {
        let mut buf = [0u8; N];
        let mut pos = 0usize;
        for slice in bufs {
            let end = pos + slice.len();
            if end > N {
                // Truncate silently — the receiver will detect the short payload
                // via the LEB128 length field and discard the event.
                let available = N - pos;
                buf[pos..N].copy_from_slice(&slice[..available]);
                pos = N;
                break;
            }
            buf[pos..end].copy_from_slice(slice);
            pos = end;
        }
        self.inner.send(&buf[..pos]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;

    struct MockFlat {
        received: RefCell<Vec<u8>>,
        ts: ZfmtU64,
    }

    impl MockFlat {
        fn new(ts: ZfmtU64) -> Self {
            Self { received: RefCell::new(Vec::new()), ts }
        }

        fn received(&self) -> Vec<u8> {
            self.received.borrow().clone()
        }
    }

    impl FlatSend for MockFlat {
        fn timestamp(&self) -> ZfmtU64 {
            self.ts
        }

        fn send(&self, data: &[u8]) {
            *self.received.borrow_mut() = data.to_vec();
        }
    }

    #[test]
    fn flat_adapter_single_slice() {
        let adapter = FlatAdapter::<_, 128>::new(MockFlat::new(ZfmtU64::new(42, 0)));
        adapter.send(b"hello");
        assert_eq!(adapter.inner().received(), b"hello");
    }

    #[test]
    fn flat_adapter_vectored_assembles() {
        let adapter = FlatAdapter::<_, 128>::new(MockFlat::new(ZfmtU64::new(0, 0)));
        adapter.send_vectored(&[b"foo", b"bar", b"baz"]);
        assert_eq!(adapter.inner().received(), b"foobarbaz");
    }

    #[test]
    fn flat_adapter_timestamp_forwarded() {
        let adapter = FlatAdapter::<_, 64>::new(MockFlat::new(ZfmtU64::new(999, 0)));
        assert_eq!(adapter.timestamp(), ZfmtU64::new(999, 0));
    }

    #[test]
    fn flat_adapter_overflow_truncates() {
        let adapter = FlatAdapter::<_, 8>::new(MockFlat::new(ZfmtU64::new(0, 0)));
        // 6 + 6 = 12 bytes, buffer is 8 — should truncate to 8
        adapter.send_vectored(&[b"abcdef", b"ghijkl"]);
        assert_eq!(adapter.inner().received(), b"abcdefgh");
    }

    #[test]
    fn logger_send_default_impl() {
        let adapter = FlatAdapter::<_, 64>::new(MockFlat::new(ZfmtU64::new(0, 0)));
        adapter.send(b"direct");
        assert_eq!(adapter.inner().received(), b"direct");
    }
}
