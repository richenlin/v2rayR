//! Kernel supervision and network probes.
//!
//! Nothing here decides *whether* to connect; it only knows how to start a
//! kernel, tell whether it came up, read its counters, stop it again, and fail in
//! a way the caller can explain to the user.

pub mod check;
pub mod clash;
pub mod cores;
pub mod fetcher;
pub mod latency;
pub mod logs;
pub mod process;

use v2rayr_domain::{CoreKind, ErrorCode};

pub use check::{CheckOutcome, check_config};
pub use clash::{ClashClient, ClashSample};
pub use cores::{CoreBinaries, CoreProbe};
pub use fetcher::{FetchedSubscription, Fetcher};
pub use latency::{LatencyProbe, LatencyTarget, tcp_latency};
pub use process::{KernelProcess, LaunchSpec};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("no {core} binary found; install it or set a core path in settings")]
    MissingCore { core: &'static str },
    #[error("failed to start {core}: {message}")]
    Spawn { core: &'static str, message: String },
    #[error("{core} exited during startup: {message}")]
    StartupFailed { core: &'static str, message: String },
    #[error("{core} config check failed: {message}")]
    ConfigRejected { core: &'static str, message: String },
    #[error("control API unavailable: {0}")]
    ControlApi(String),
    #[error("network: {0}")]
    Network(String),
    #[error("the subscription returned an unsupported payload: {0}")]
    UnsupportedPayload(String),
    #[error("timeout after {0:?}")]
    Timeout(std::time::Duration),
}

impl EngineError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Io(_) => ErrorCode::Internal,
            Self::MissingCore { .. } => ErrorCode::CoreMissing,
            Self::Spawn { .. } | Self::StartupFailed { .. } => ErrorCode::CoreFailed,
            Self::ConfigRejected { .. } => ErrorCode::ConfigInvalid,
            Self::ControlApi(_) => ErrorCode::CoreFailed,
            Self::Network(_) => ErrorCode::NetworkError,
            Self::UnsupportedPayload(_) => ErrorCode::UnsupportedFormat,
            Self::Timeout(_) => ErrorCode::NetworkError,
        }
    }

    pub fn core_name(core: CoreKind) -> &'static str {
        core.as_str()
    }
}
