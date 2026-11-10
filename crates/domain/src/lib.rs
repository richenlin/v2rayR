//! Domain model for v2rayR: pure data and parsing, no IO.
//!
//! The layout follows the design document (§5.1): protocol specific fields live
//! in an enum, while everything shared between protocols (address, port,
//! transport, TLS) sits on `Profile` itself. Config emitters can therefore match
//! on the enum instead of probing dozens of optional string columns.

pub mod base64;
pub mod clash;
pub mod error;
pub mod filter;
pub mod fingerprint;
pub mod merge;
pub mod model;
pub mod sharelink;
pub mod singbox_import;
pub mod subscription;

pub use error::{DomainError, ErrorCode};
pub use filter::NodeFilter;
pub use merge::{ExistingNode, MergePlan, plan_merge};
pub use model::{
    AnyTls, Congestion, CoreKind, Hysteria2Obfs, MkcpHeader, MuxOptions, MuxProtocol, Naive,
    ObfsKind, Profile, Protocol, QuicSecurity, RealityOptions, Secret, SsMethod, TlsOptions,
    Transport, Tuic, VlessFlow, VmessSecurity, WireGuard,
};
pub use sharelink::{
    ImportOutcome, ImportPreview, LinkFailure, export_share_link, parse_share_link,
    parse_share_links,
};
pub use subscription::{
    Payload, SniffedSubscription, SubscriptionFormat, SubscriptionUserInfo, sniff_subscription,
};

/// Crate level version of the *payload* format stored in SQLite. Bumping this
/// requires a migration in `v2rayr-store`; it is intentionally independent from
/// the application version and from the config-emitter version.
pub const PROFILE_PAYLOAD_VERSION: u32 = 1;

/// A locally selected executable. Display labels never participate in matching.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleApplication {
    pub path: String,
    pub sha256: String,
}
