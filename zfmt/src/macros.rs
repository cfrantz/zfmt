//! Logging macros (§13).

/// Send a structured event to a logger.
///
/// Usage: `log_event!(logger, severity, event_expr)`
/// where `event_expr` evaluates to a type that implements `ZfmtEvent`.
///
/// Sends exactly two slices via `send_vectored`:
///   slice[0] — framing: hdr_tag + hdr_leb + hdr_payload + evt_tag + evt_leb (≤ 30 bytes)
///   slice[1] — event payload (zero-copy for Tier-1; serialized for Tier-2)
#[macro_export]
macro_rules! log_event {
    ($logger:expr, $severity:expr, $event:expr) => {{
        // Bind logger once to avoid evaluating the expression twice.
        let ref mut _logger = $logger;
        let ts = $crate::Logger::timestamp(&*_logger);
        let hdr = $crate::events::EventHeader::new(ts, $severity);
        let event = $event;

        let hdr_payload_len = $crate::ZfmtEvent::payload_size(&hdr) as u32;
        let evt_payload_len = $crate::ZfmtEvent::payload_size(&event) as u32;

        // framing: hdr_tag(4) + hdr_leb(≤5) + hdr_payload(16) + evt_tag(4) + evt_leb(≤5) = ≤34
        let mut framing = [0u8; 34];
        let mut n = 0usize;

        framing[n..n + 4].copy_from_slice(&$crate::ZfmtEvent::zfmt_tag(&hdr).to_le_bytes());
        n += 4;
        n += $crate::leb128::encode(hdr_payload_len, &mut framing[n..]);
        $crate::ZfmtEvent::with_payload_bytes(&hdr, |hdr_bytes| {
            framing[n..n + hdr_bytes.len()].copy_from_slice(hdr_bytes);
            n += hdr_bytes.len();
        });
        framing[n..n + 4].copy_from_slice(&$crate::ZfmtEvent::zfmt_tag(&event).to_le_bytes());
        n += 4;
        n += $crate::leb128::encode(evt_payload_len, &mut framing[n..]);

        let framing_len = n;
        $crate::ZfmtEvent::with_payload_bytes(&event, |evt_bytes| {
            $crate::Logger::send_vectored(_logger, &[&framing[..framing_len], evt_bytes]);
        });
    }};
}

/// Log a structured event at TRACE severity.
#[cfg(feature = "log-level-trace")]
#[macro_export]
macro_rules! log_trace {
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Trace, $event)
    };
}
#[cfg(not(feature = "log-level-trace"))]
#[macro_export]
macro_rules! log_trace {
    ($logger:expr, $event:expr) => { let _ = &$logger; let _ = &$event; };
}

/// Log a structured event at DEBUG severity.
#[cfg(feature = "log-level-debug")]
#[macro_export]
macro_rules! log_debug {
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Debug, $event)
    };
}
#[cfg(not(feature = "log-level-debug"))]
#[macro_export]
macro_rules! log_debug {
    ($logger:expr, $event:expr) => { let _ = &$logger; let _ = &$event; };
}

/// Log a structured event at INFO severity.
#[cfg(feature = "log-level-info")]
#[macro_export]
macro_rules! log_info {
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Info, $event)
    };
}
#[cfg(not(feature = "log-level-info"))]
#[macro_export]
macro_rules! log_info {
    ($logger:expr, $event:expr) => { let _ = &$logger; let _ = &$event; };
}

/// Log a structured event at WARN severity.
#[cfg(feature = "log-level-warn")]
#[macro_export]
macro_rules! log_warn {
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Warn, $event)
    };
}
#[cfg(not(feature = "log-level-warn"))]
#[macro_export]
macro_rules! log_warn {
    ($logger:expr, $event:expr) => { let _ = &$logger; let _ = &$event; };
}

/// Log a structured event at ERROR severity.
#[cfg(feature = "log-level-error")]
#[macro_export]
macro_rules! log_error {
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Error, $event)
    };
}
#[cfg(not(feature = "log-level-error"))]
#[macro_export]
macro_rules! log_error {
    ($logger:expr, $event:expr) => { let _ = &$logger; let _ = &$event; };
}

/// Log a structured event at FATAL severity.
#[macro_export]
macro_rules! log_fatal {
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Fatal, $event)
    };
}
