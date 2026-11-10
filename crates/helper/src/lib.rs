//! Privileged helper.
//!
//! The design document keeps every operation that touches system state behind a
//! separate, small binary: the UI never runs as root, and a manifest patch cannot
//! turn the main app into a root shell. This crate holds the protocol, the
//! authorization check and the system operations; `src/main.rs` is the CLI that
//! the app (or a launchd job) executes.
//!
//! Linux TUN is a separate stdin lease authorized by the administrator's
//! exact sudo policy; it never accepts a caller token file or config path.
//! Legacy token commands are unprivileged on Unix.

pub mod authorize;
pub mod ops;
pub mod protocol;

pub use authorize::{Token, TokenError};
pub use ops::{HelperError, execute};
pub use protocol::{HelperRequest, HelperResponse, ProxySpec};
