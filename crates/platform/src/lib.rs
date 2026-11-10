//! OS integration.
//!
//! Three things live here, all of them untestable in the abstract and therefore
//! kept behind small traits:
//!
//! * [`SystemProxy`] — apply/restore the system proxy. On macOS this shells out
//!   to `networksetup`, which is the only supported way; the previous state is
//!   captured *before* anything changes so a failed takeover can always be
//!   rolled back, and a crash mid-takeover can be repaired on next start.
//! * [`Ports`] — probe whether a port is free and who holds it.
//! * [`Autostart`] — a launch-at-login registration.
//!
//! Everything that shells out goes through [`CommandRunner`], so the takeover
//! logic is covered by tests with a scripted runner instead of mocks of the OS.

pub mod autostart;
pub mod pac;
pub mod paths;
pub mod ports;
pub mod proxy;
pub mod single_instance;
pub mod tun;

use thiserror::Error;

pub use autostart::{Autostart, LaunchAgentAutostart};
pub use pac::{
    PacMatch, PacOverride, PacServer, render_pac, render_pac_with_overrides, render_pac_with_rules,
};
pub use paths::AppPaths;
pub use ports::PortState;
pub use ports::Ports;
pub use proxy::{
    BypassMode, CommandOutput, CommandRunner, MacOsSystemProxy, ProxyEndpoint, ServiceProxyState,
    SystemCommandRunner, SystemProxy, SystemProxyState, TakeoverSnapshot,
};
pub use single_instance::{InstanceGuard, InstanceOutcome, acquire as acquire_single_instance};

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("command failed: {program} {args:?}: {message}")]
    Command {
        program: String,
        args: Vec<String>,
        message: String,
    },
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("unsupported on this platform: {0}")]
    Unsupported(&'static str),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid state: {0}")]
    InvalidState(String),
}

impl PlatformError {
    pub fn code(&self) -> v2rayr_domain::ErrorCode {
        use v2rayr_domain::ErrorCode;
        match self {
            Self::PermissionDenied(_) => ErrorCode::PermissionDenied,
            Self::Unsupported(_) => ErrorCode::TakeoverFailed,
            Self::Command { .. } => ErrorCode::TakeoverFailed,
            Self::Io(_) => ErrorCode::Storage,
            Self::InvalidState(_) => ErrorCode::TakeoverFailed,
        }
    }

    /// Heuristic used to turn `networksetup`'s stderr into a user-facing hint.
    pub fn is_permission_failure(message: &str) -> bool {
        let lower = message.to_ascii_lowercase();
        lower.contains("not authorized")
            || lower.contains("must be run as root")
            || lower.contains("permission denied")
            || lower.contains("operation not permitted")
            || lower.contains("sudo")
    }
}
