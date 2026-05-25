#![cfg_attr(not(test), no_std)]

mod format;
pub mod leb128;
mod logger;
pub mod output;
mod write;
pub mod events;
mod macros;

pub use format::{Align, Format, FormatInto, FormatSpec, FormatType, ZfmtStr, ZfmtU64};
pub use logger::{FlatAdapter, FlatSend, Logger};
pub use write::{Error, FixedBuf, Write};

pub use zfmt_macro::Zfmt;
pub use zfmt_macro::zfmt_str;

/// Internal proc-macro for the unstructured logging arms of `log_debug!` etc.
/// Not intended for direct use.
#[doc(hidden)]
pub use zfmt_macro::__zfmt_log_text;

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

/// Called by unstructured logging arms of `log_info!`, `log_warn!`,
/// `log_error!`, and `log_fatal!` to emit a compile-time deprecation warning.
///
/// Suppress with `#[allow(deprecated)]` at the call site.
#[deprecated = "prefer structured events for log_info!/log_warn!/log_error!/log_fatal!; \
                use log_debug! for unstructured text; suppress with #[allow(deprecated)]"]
#[doc(hidden)]
#[inline(always)]
pub fn __zfmt_unstructured_above_debug() {}
