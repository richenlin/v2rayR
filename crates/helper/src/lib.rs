//! Privileged helper.
//!
//! The design document keeps every operation that touches system state behind a
//! separate, small binary: the UI never runs as root, and a manifest patch cannot
//! turn the main app into a root shell. This crate holds the protocol, the
//! authorization check and the system operations; `src/main.rs` is the CLI that
//! the app (or a launchd job) executes.
//!
//! Threat model, stated plainly: the helper runs as root only for the two
//! operations that need it (TUN device setup, writing system-wide proxy state).
//! Requests carry a token stored in the app's runtime directory with mode 0600
//! and compared in constant time. That stops other local users; it does not stop
//! an attacker who already controls the invoking user account — at that point the
//! attacker can edit the proxy settings through `networksetup` anyway.

pub mod authorize;
pub mod ops;
pub mod protocol;

pub use authorize::{Token, TokenError};
pub use ops::{HelperError, execute};
pub use protocol::{HelperRequest, HelperResponse, ProxySpec};
