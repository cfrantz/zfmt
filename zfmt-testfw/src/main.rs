//! Test firmware binary — exercises the derive macro and produces a known event stream.
//!
//! Usage:
//!   zfmt-testfw <stream_output_path>
//!
//! The binary itself is also used as the ELF for `zfmt ingest` (it contains
//! `.zfmt_events.*` and `.zfmt_strings.*` sections).

use zfmt::ZfmtStr;

// ---------------------------------------------------------------------------
// Event definitions — all use the real #[derive(Zfmt)] macro

/// A periodic heartbeat event emitted once per second.
#[derive(zfmt::Zfmt)]
#[zfmt(format = "heartbeat ts={timestamp} up={uptime_ms}ms")]
pub struct Heartbeat {
    pub timestamp: u64,
    pub uptime_ms: u32,
}

/// Temperature reading from a sensor.
#[derive(zfmt::Zfmt)]
#[zfmt(format = "temp celsius_x10={celsius_x10} sensor={sensor_id}")]
pub struct TempReading {
    pub celsius_x10: i16,
    pub sensor_id: u8,
}

/// An alert condition — top-level enum event.
#[derive(zfmt::Zfmt)]
pub enum Alert {
    #[zfmt(format = "alert critical code={code}")]
    Critical { code: u32 },
    #[zfmt(format = "alert warning")]
    Warning,
}

/// An event whose `label` field is a compile-time interned string (§4.7).
#[derive(zfmt::Zfmt)]
#[repr(C)]
#[zfmt(format = "named label={label} seq={seq}")]
pub struct NamedEvent {
    pub label: ZfmtStr,
    pub seq:   u32,
}

// ---------------------------------------------------------------------------
// Minimal logger (std-side: assembles bytes into a Vec)

struct VecCollect(Vec<u8>);

impl zfmt::FlatSend for VecCollect {
    fn timestamp(&self) -> u64 { 0 }
    fn send(&mut self, data: &[u8]) { self.0.extend_from_slice(data); }
}

// ---------------------------------------------------------------------------
// Known test values — published so CLI tests can assert against them

pub const HEARTBEAT_TIMESTAMP: u64 = 1_000;
pub const HEARTBEAT_UPTIME_MS: u32 = 5_000;
pub const TEMP_CELSIUS_X10: i16    = 215;  // 21.5 °C
pub const TEMP_SENSOR_ID:    u8    = 3;
pub const ALERT_CODE:        u32   = 42;
pub const NAMED_SEQ:         u32   = 7;

/// Generate the binary event stream for the canonical test scenario.
#[allow(deprecated)]
pub fn generate_test_stream() -> Vec<u8> {
    let mut logger = zfmt::FlatAdapter::<VecCollect, 256>::new(VecCollect(Vec::new()));

    zfmt::log_info!(logger, Heartbeat {
        timestamp:  HEARTBEAT_TIMESTAMP,
        uptime_ms:  HEARTBEAT_UPTIME_MS,
    });
    zfmt::log_warn!(logger, TempReading {
        celsius_x10: TEMP_CELSIUS_X10,
        sensor_id:   TEMP_SENSOR_ID,
    });
    zfmt::log_error!(logger, Alert::Critical { code: ALERT_CODE });
    zfmt::log_info!(logger, Alert::Warning);

    let label = zfmt::zfmt_str!("firmware node");
    zfmt::log_info!(logger, NamedEvent { label: ZfmtStr::new(label), seq: NAMED_SEQ });

    zfmt::log_fatal!(logger, "debug note seq={seq}", seq = NAMED_SEQ);

    logger.inner().0.clone()
}

// ---------------------------------------------------------------------------

fn main() {
    use std::io::Write;
    let stream = generate_test_stream();
    let path = std::env::args().nth(1);
    if let Some(p) = path {
        std::fs::write(p, &stream).expect("write stream file");
    } else {
        std::io::stdout().write_all(&stream).expect("write stdout");
    }
}
