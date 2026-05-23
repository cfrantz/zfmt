#![cfg_attr(not(test), no_std)]

mod format;
mod logger;
mod write;

pub use format::{Align, Format, FormatSpec, FormatType};
pub use logger::{FlatAdapter, FlatSend, Logger};
pub use write::{Error, Write};
