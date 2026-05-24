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
