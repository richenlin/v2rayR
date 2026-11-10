//! Wire types shared by the Tauri shell, the CLI and the UI.
//!
//! Rules that keep this crate honest:
//!
//! * every field is `camelCase` on the wire — the UI is TypeScript;
//! * nothing here contains a secret: `NodeView` carries the protocol *name*, not
//!   the protocol payload;
//! * every error is a [`Problem`] with a stable `code`, an i18n `key` and an
//!   optional, already-redacted `detail`.
//!
//! `ui/src/lib/contract.ts` mirrors these types; `crates/ipc/tests/contract.rs`
//! pins the JSON shape so the two cannot drift silently.

mod error;
mod events;
mod settings;
mod snapshot;
mod views;

pub use error::Problem;
pub use events::{EventName, LogEntry, LogLevel, TrafficPoint, TrafficTick};
pub use settings::{
    AdvancedSettings, AppearanceSettings, CoreOverride, InboundSettings, RoutingSettings,
    SettingsGroupId, SettingsPatch, SettingsView, StartupSettings, SubscriptionSettings,
};
pub use snapshot::{
    AppSnapshot, ConnectionPhase, ConnectionState, LanEndpoint, RoutingPolicy, TakeoverMode,
    TrafficView,
};
pub use views::{
    CoreStatus, ImportFailure, ImportReport, LatencyMethod, LatencyResult, LatencyView, NodeView,
    QuotaView, RuleView, SessionRequest, SubscriptionView, VersionInfo,
};

/// Bumped whenever a field is removed or its meaning changes. Additive changes
/// do not bump it; the UI must tolerate unknown fields.
pub const API_VERSION: u32 = 1;

/// Canonical command names. Keeping them in one place makes it impossible for
/// the shell and the UI to disagree about a string.
pub mod commands {
    pub const SNAPSHOT: &str = "app_snapshot";
    pub const VERSION: &str = "app_version";

    pub const SESSION_CONNECT: &str = "session_connect";
    pub const SESSION_DISCONNECT: &str = "session_disconnect";
    pub const SESSION_SWITCH: &str = "session_switch";
    pub const SESSION_SET_POLICY: &str = "session_set_policy";
    pub const SESSION_SET_MODE: &str = "session_set_mode";
    pub const SESSION_RETRY: &str = "session_retry";

    pub const NODES_LIST: &str = "nodes_list";
    pub const NODE_UPSERT: &str = "node_upsert";
    pub const NODE_REMOVE: &str = "node_remove";
    pub const NODE_SET_FAVORITE: &str = "node_set_favorite";
    pub const NODE_SET_TAGS: &str = "node_set_tags";
    pub const NODE_SHARE_LINK: &str = "node_share_link";
    pub const NODES_IMPORT_TEXT: &str = "nodes_import_text";
    pub const NODES_TEST_LATENCY: &str = "nodes_test_latency";

    pub const SUBSCRIPTIONS_LIST: &str = "subscriptions_list";
    pub const SUBSCRIPTION_ADD: &str = "subscription_add";
    pub const SUBSCRIPTION_UPDATE: &str = "subscription_update";
    pub const SUBSCRIPTION_REMOVE: &str = "subscription_remove";
    pub const SUBSCRIPTION_SET_ENABLED: &str = "subscription_set_enabled";

    pub const SETTINGS_GET: &str = "settings_get";
    pub const SETTINGS_UPDATE: &str = "settings_update";
    pub const SETTINGS_RESET: &str = "settings_reset";

    pub const CORES_STATUS: &str = "cores_status";
    pub const CORES_INSTALL: &str = "cores_install";

    pub const RULES_LIST: &str = "rules_list";
    pub const RULE_ADD: &str = "rule_add";
    pub const RULE_SET_ENABLED: &str = "rule_set_enabled";
    pub const RULE_REMOVE: &str = "rule_remove";

    pub const LOGS_TAIL: &str = "logs_tail";
    pub const PATHS: &str = "app_paths";
}
