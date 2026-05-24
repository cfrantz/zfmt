//! Logging macros (§13).

/// Send a structured event to a logger.
///
/// Usage: `log_event!(logger, severity, event_expr)`
/// where `event_expr` evaluates to a type that implements the zfmt event interface.
#[macro_export]
macro_rules! log_event {
    ($logger:expr, $severity:expr, $event:expr) => {{
        let ts = $crate::Logger::timestamp(&$logger);
        let hdr = $crate::events::EventHeader::new(ts, $severity);
        let event = $event;
        let hdr_payload_len = hdr.payload_size();
        let evt_payload_len = event.payload_size();
        let hdr_leb_len = $crate::leb128::encoded_len(hdr_payload_len as u64);
        let evt_leb_len = $crate::leb128::encoded_len(evt_payload_len as u64);
        let total = 4 + hdr_leb_len + hdr_payload_len + 4 + evt_leb_len + evt_payload_len;
        const BUF: usize = 256;
        let mut buf = [0u8; BUF];
        if total <= BUF {
            let mut pos = 0usize;
            buf[pos..pos+4].copy_from_slice(&hdr.zfmt_tag().to_le_bytes()); pos += 4;
            pos += $crate::leb128::encode(hdr_payload_len as u64, &mut buf[pos..]);
            hdr.serialize_into(&mut buf[pos..pos+hdr_payload_len]); pos += hdr_payload_len;
            buf[pos..pos+4].copy_from_slice(&event.zfmt_tag().to_le_bytes()); pos += 4;
            pos += $crate::leb128::encode(evt_payload_len as u64, &mut buf[pos..]);
            event.serialize_into(&mut buf[pos..pos+evt_payload_len]);
            $crate::Logger::send(&mut $logger, &buf[..total]);
        }
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
