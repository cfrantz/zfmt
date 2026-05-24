#![cfg_attr(not(test), no_std)]

mod format;
pub mod leb128;
mod logger;
mod write;
pub mod events;
mod macros;

pub use format::{Align, Format, FormatSpec, FormatType};
pub use logger::{FlatAdapter, FlatSend, Logger};
pub use write::{Error, FixedBuf, Write};

pub use zfmt_macro::Zfmt;

/// Implemented by every event type, enabling generic logging via `log_event!`.
///
/// The derive macro generates this impl automatically.  For the well-known
/// built-in events (EventHeader, DebugMessage) it is written by hand in
/// `events.rs`.
pub trait ZfmtEvent {
    fn zfmt_tag(&self) -> u32;
    fn payload_size(&self) -> usize;
    /// Call `f` with a byte slice covering the serialized event payload.
    ///
    /// For `repr(C)` structs with no interior padding (i.e., `size_of ==
    /// sum-of-field-sizes`) this is a zero-copy slice of the struct itself.
    /// For variable-length (Tier-2) events or structs with padding, the
    /// payload is first serialized into a stack buffer before calling `f`.
    fn with_payload_bytes<F: FnOnce(&[u8])>(&self, f: F);
}
