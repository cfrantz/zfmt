//! Logging macros (§13).

/// Send a bare event (no `EventHeader`) to a logger (§6.3).
///
/// Used for protocol-level events like `StreamStart` that are emitted
/// without a timestamp/severity wrapper.
#[macro_export]
macro_rules! log_bare_event {
    ($logger:expr, $event:expr) => {{
        let ref mut _logger = $logger;
        let _event = $event;
        $crate::output::send_bare_event(_logger, &_event);
    }};
}

/// Send a structured event to a logger.
///
/// Usage: `log_event!(logger, severity, event_expr)`
/// where `event_expr` evaluates to a type implementing `ZfmtEvent + FormatInto`.
///
/// Delegates to `::zfmt::output::send_event` which applies the active output
/// mode (`output-binary`, `output-text`, or `output-both`) according to the
/// feature flags set on the `zfmt` crate.
#[macro_export]
macro_rules! log_event {
    ($logger:expr, $severity:expr, $event:expr) => {{
        let ref mut _logger = $logger;
        let _ts = $crate::Logger::timestamp(&*_logger);
        let _hdr = $crate::events::EventHeader::new(_ts, $severity);
        let _event = $event;
        $crate::output::send_event(_logger, &_hdr, &_event);
    }};
}

/// Log a structured event at TRACE severity.
#[cfg(feature = "log-level-trace")]
#[macro_export]
macro_rules! log_trace {
    ($logger:expr, $fmt:literal $(, $name:ident = $val:expr)* $(,)?) => {
        $crate::__zfmt_log_text!($logger, $crate::events::Severity::Trace, $fmt $(, $name = $val)*)
    };
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Trace, $event)
    };
}
#[cfg(not(feature = "log-level-trace"))]
#[macro_export]
macro_rules! log_trace {
    ($logger:expr, $fmt:literal $(, $name:ident = $val:expr)* $(,)?) => {
        let _ = &$logger; $(let _ = &$val;)*
    };
    ($logger:expr, $event:expr) => {
        let _ = &$logger; let _ = &$event;
    };
}

/// Log a structured event at DEBUG severity.
#[cfg(feature = "log-level-debug")]
#[macro_export]
macro_rules! log_debug {
    ($logger:expr, $fmt:literal $(, $name:ident = $val:expr)* $(,)?) => {
        $crate::__zfmt_log_text!($logger, $crate::events::Severity::Debug, $fmt $(, $name = $val)*)
    };
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Debug, $event)
    };
}
#[cfg(not(feature = "log-level-debug"))]
#[macro_export]
macro_rules! log_debug {
    ($logger:expr, $fmt:literal $(, $name:ident = $val:expr)* $(,)?) => {
        let _ = &$logger; $(let _ = &$val;)*
    };
    ($logger:expr, $event:expr) => {
        let _ = &$logger; let _ = &$event;
    };
}

/// Log a structured event at INFO severity.
///
/// When passed a string literal (unstructured text), a deprecation warning is
/// emitted at the call site.  Suppress with `#[allow(deprecated)]`.
#[cfg(feature = "log-level-info")]
#[macro_export]
macro_rules! log_info {
    ($logger:expr, $fmt:literal $(, $name:ident = $val:expr)* $(,)?) => {{
        $crate::__zfmt_unstructured_above_debug();
        $crate::__zfmt_log_text!($logger, $crate::events::Severity::Info, $fmt $(, $name = $val)*)
    }};
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Info, $event)
    };
}
#[cfg(not(feature = "log-level-info"))]
#[macro_export]
macro_rules! log_info {
    ($logger:expr, $fmt:literal $(, $name:ident = $val:expr)* $(,)?) => {
        let _ = &$logger; $(let _ = &$val;)*
    };
    ($logger:expr, $event:expr) => {
        let _ = &$logger; let _ = &$event;
    };
}

/// Log a structured event at WARN severity.
///
/// When passed a string literal (unstructured text), a deprecation warning is
/// emitted at the call site.  Suppress with `#[allow(deprecated)]`.
#[cfg(feature = "log-level-warn")]
#[macro_export]
macro_rules! log_warn {
    ($logger:expr, $fmt:literal $(, $name:ident = $val:expr)* $(,)?) => {{
        $crate::__zfmt_unstructured_above_debug();
        $crate::__zfmt_log_text!($logger, $crate::events::Severity::Warn, $fmt $(, $name = $val)*)
    }};
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Warn, $event)
    };
}
#[cfg(not(feature = "log-level-warn"))]
#[macro_export]
macro_rules! log_warn {
    ($logger:expr, $fmt:literal $(, $name:ident = $val:expr)* $(,)?) => {
        let _ = &$logger; $(let _ = &$val;)*
    };
    ($logger:expr, $event:expr) => {
        let _ = &$logger; let _ = &$event;
    };
}

/// Log a structured event at ERROR severity.
///
/// When passed a string literal (unstructured text), a deprecation warning is
/// emitted at the call site.  Suppress with `#[allow(deprecated)]`.
#[cfg(feature = "log-level-error")]
#[macro_export]
macro_rules! log_error {
    ($logger:expr, $fmt:literal $(, $name:ident = $val:expr)* $(,)?) => {{
        $crate::__zfmt_unstructured_above_debug();
        $crate::__zfmt_log_text!($logger, $crate::events::Severity::Error, $fmt $(, $name = $val)*)
    }};
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Error, $event)
    };
}
#[cfg(not(feature = "log-level-error"))]
#[macro_export]
macro_rules! log_error {
    ($logger:expr, $fmt:literal $(, $name:ident = $val:expr)* $(,)?) => {
        let _ = &$logger; $(let _ = &$val;)*
    };
    ($logger:expr, $event:expr) => {
        let _ = &$logger; let _ = &$event;
    };
}

/// Log a structured event at FATAL severity.
///
/// When passed a string literal (unstructured text), a deprecation warning is
/// emitted at the call site.  Suppress with `#[allow(deprecated)]`.
#[macro_export]
macro_rules! log_fatal {
    ($logger:expr, $fmt:literal $(, $name:ident = $val:expr)* $(,)?) => {{
        $crate::__zfmt_unstructured_above_debug();
        $crate::__zfmt_log_text!($logger, $crate::events::Severity::Fatal, $fmt $(, $name = $val)*)
    }};
    ($logger:expr, $event:expr) => {
        $crate::log_event!($logger, $crate::events::Severity::Fatal, $event)
    };
}
