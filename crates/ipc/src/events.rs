//! Push-style events emitted by the shell and consumed by the UI stores.

use serde::{Deserialize, Serialize};

/// Event channel names. Tauri listens on these exact strings.
pub const EVENT_SESSION: &str = "session://changed";
pub const EVENT_TRAFFIC: &str = "traffic://tick";
pub const EVENT_LOG: &str = "log://line";
pub const EVENT_SUBSCRIPTION: &str = "subscription://changed";
pub const EVENT_CORES: &str = "core://status";

/// A namespace for the constants above (handy in `commands`-style imports).
pub struct EventName;

impl EventName {
    pub const SESSION: &'static str = EVENT_SESSION;
    pub const TRAFFIC: &'static str = EVENT_TRAFFIC;
    pub const LOG: &'static str = EVENT_LOG;
    pub const SUBSCRIPTION: &'static str = EVENT_SUBSCRIPTION;
    pub const CORES: &'static str = EVENT_CORES;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub seq: u64,
    /// Unix milliseconds; the UI formats with the user's locale.
    pub ts: i64,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
}

/// One point of the traffic chart. `ts` is unix milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficPoint {
    pub ts: i64,
    pub down_bps: u64,
    pub up_bps: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficTick {
    pub down_bps: u64,
    pub up_bps: u64,
    pub total_down_bytes: u64,
    pub total_up_bytes: u64,
    /// Live connection count reported by the kernel API, when available.
    pub connections: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<TrafficPoint>,
}
