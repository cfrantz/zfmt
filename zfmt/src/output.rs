//! Output mode dispatch for `log_event!` (§13.5).
//!
//! This module centralises the `output-binary`, `output-text`, and
//! `output-both` feature logic so that the cfg checks are evaluated in the
//! `zfmt` crate's compilation context rather than in the calling crate's
//! context (which would be the case if the logic lived inside a `macro_rules!`
//! body).

use crate::{
    events::EventHeader,
    format::FormatInto,
    leb128,
    logger::Logger,
    ZfmtEvent,
};

#[cfg(any(feature = "output-text", feature = "output-both"))]
use crate::events::DebugMessage;

// ---------------------------------------------------------------------------
// Internal helper: binary send
//
// Packs [EventHeader frame | event frame] into a ≤34-byte stack buffer and
// calls `send_vectored` with two slices: [framing, event-payload].

fn binary_send<L, E>(logger: &L, hdr: &EventHeader, event: &E)
where
    L: Logger + ?Sized,
    E: ZfmtEvent,
{
    let hpl = ZfmtEvent::payload_size(hdr) as u32;
    let epl = ZfmtEvent::payload_size(event) as u32;
    let mut frm = [0u8; 34];
    let mut n = 0usize;
    frm[n..n + 4].copy_from_slice(&ZfmtEvent::zfmt_tag(hdr).to_le_bytes());
    n += 4;
    n += leb128::encode(hpl, &mut frm[n..]);
    ZfmtEvent::with_payload_bytes(hdr, |hb| {
        frm[n..n + hb.len()].copy_from_slice(hb);
        n += hb.len();
    });
    frm[n..n + 4].copy_from_slice(&ZfmtEvent::zfmt_tag(event).to_le_bytes());
    n += 4;
    n += leb128::encode(epl, &mut frm[n..]);
    let fl = n;
    ZfmtEvent::with_payload_bytes(event, |eb| {
        logger.send_vectored(&[&frm[..fl], eb]);
    });
}

// ---------------------------------------------------------------------------
// Internal helper: text send (output-text / output-both only)
//
// Calls `event.format_into()` into a 128-byte stack buffer, wraps the result
// in a `DebugMessage`, and binary-sends it.  Requires `E: FormatInto`.

#[cfg(any(feature = "output-text", feature = "output-both"))]
fn text_send<L, E>(logger: &L, hdr: &EventHeader, event: &E)
where
    L: Logger + ?Sized,
    E: FormatInto,
{
    let mut tbuf = crate::write::FixedBuf::<128>::new();
    let _ = event.format_into(&mut tbuf);
    let tmsg = DebugMessage { message: tbuf.as_str() };
    binary_send(logger, hdr, &tmsg);
}

// ---------------------------------------------------------------------------
// Public entry point: bare event (no EventHeader prefix, §6.3)

/// Send a bare event — just `[tag][LEB128(len)][payload]` — without an
/// `EventHeader`.  Use this for protocol-level events like `StreamStart`.
#[inline]
pub fn send_bare_event<L, E>(logger: &L, event: &E)
where
    L: Logger + ?Sized,
    E: ZfmtEvent,
{
    let epl = ZfmtEvent::payload_size(event) as u32;
    let mut frm = [0u8; 9]; // tag(4) + LEB128(up to 5)
    let mut n = 0usize;
    frm[n..n + 4].copy_from_slice(&ZfmtEvent::zfmt_tag(event).to_le_bytes());
    n += 4;
    n += leb128::encode(epl, &mut frm[n..]);
    let fl = n;
    ZfmtEvent::with_payload_bytes(event, |eb| {
        logger.send_vectored(&[&frm[..fl], eb]);
    });
}

// ---------------------------------------------------------------------------
// Public entry point called by `log_event!`

/// Send an event through `logger` according to the active output mode.
///
/// - Default / `output-binary`: binary wire format via `send_vectored`.
/// - `output-text`: render as text via `format_into()`, send as `DebugMessage`.
/// - `output-both`: perform both in sequence.
///
/// The `cfg` feature checks here are evaluated in `zfmt`'s compilation
/// context (controlled by the `zfmt = { features = [...] }` declaration in
/// the consumer's `Cargo.toml`), which is the intended design.
#[inline]
pub fn send_event<L, E>(logger: &L, hdr: &EventHeader, event: &E)
where
    L: Logger + ?Sized,
    E: ZfmtEvent + FormatInto,
{
    #[cfg(not(feature = "output-text"))]
    binary_send(logger, hdr, event);

    #[cfg(any(feature = "output-text", feature = "output-both"))]
    text_send(logger, hdr, event);
}
