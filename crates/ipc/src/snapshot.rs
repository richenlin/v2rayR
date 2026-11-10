//! Connection state and the snapshot the UI renders on boot.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Problem;
use crate::events::{TrafficPoint, TrafficTick};
use crate::settings::SettingsView;
use crate::views::{CoreStatus, NodeView, SubscriptionView, VersionInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectionPhase {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    /// A connection was attempted and failed. Distinct from `Disconnected`
    /// because the UI must show the failure, not a neutral empty state.
    Failed,
}

/// How system traffic reaches the proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TakeoverMode {
    /// macOS/Windows system proxy pointing at the mixed inbound.
    SystemProxy,
    /// Automatic proxy configuration (PAC) served over loopback.
    Pac,
    /// Virtual interface; requires the privileged helper.
    Tun,
    /// Do nothing to the OS, just run the inbound for other apps to use.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoutingPolicy {
    Rule,
    Global,
    Direct,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanEndpoint {
    pub address: std::net::Ipv4Addr,
    pub socks_port: u16,
    pub http_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionState {
    pub phase: ConnectionPhase,
    pub mode: TakeoverMode,
    pub policy: RoutingPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_remark: Option<String>,
    /// Kernel actually running, resolved from the capability matrix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core: Option<v2rayr_domain::CoreKind>,
    /// Real inbound port (may differ from the configured one when the kernel
    /// shifts it, e.g. TUN's clash_api port).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbound_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pac_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// Set when the system proxy / TUN was applied. A failed takeover with a
    /// healthy kernel is still *not* "connected" for the user.
    pub takeover_applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan: Option<LanEndpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub takeover_error: Option<Problem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Problem>,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            phase: ConnectionPhase::Disconnected,
            mode: TakeoverMode::SystemProxy,
            policy: RoutingPolicy::Rule,
            profile_id: None,
            profile_remark: None,
            core: None,
            inbound_port: None,
            api_port: None,
            pac_url: None,
            started_at: None,
            takeover_applied: false,
            lan: None,
            takeover_error: None,
            error: None,
        }
    }
}

impl ConnectionState {
    /// The single source of truth for the headline the UI shows. Takeover
    /// failures downgrade a technically running session to `Failed`, because
    /// "已连接" while nothing is routed would be a lie.
    pub fn headline(&self) -> ConnectionPhase {
        if self.phase == ConnectionPhase::Connected
            && self.mode != TakeoverMode::Manual
            && !self.takeover_applied
        {
            return ConnectionPhase::Failed;
        }
        self.phase
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.phase,
            ConnectionPhase::Connecting | ConnectionPhase::Disconnecting
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficView {
    pub down_bps: u64,
    pub up_bps: u64,
    pub total_down_bytes: u64,
    pub total_up_bytes: u64,
    pub connections: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<TrafficPoint>,
}

impl From<TrafficTick> for TrafficView {
    fn from(tick: TrafficTick) -> Self {
        Self {
            down_bps: tick.down_bps,
            up_bps: tick.up_bps,
            total_down_bytes: tick.total_down_bytes,
            total_up_bytes: tick.total_up_bytes,
            connections: tick.connections,
            samples: tick.samples,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub version: VersionInfo,
    pub connection: ConnectionState,
    #[serde(default)]
    pub tun_available: bool,
    pub settings: SettingsView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_node: Option<NodeView>,
    pub nodes: Vec<NodeView>,
    pub subscriptions: Vec<SubscriptionView>,
    pub traffic: TrafficView,
    pub cores: Vec<CoreStatus>,
    /// Non-fatal conditions worth a banner: missing kernel, occupied port,
    /// autostart disabled by the OS, ...
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Problem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takeover_failure_downgrades_the_headline() {
        let state = ConnectionState {
            phase: ConnectionPhase::Connected,
            takeover_applied: false,
            ..ConnectionState::default()
        };
        assert_eq!(state.headline(), ConnectionPhase::Failed);
    }

    #[test]
    fn healthy_session_keeps_connected_headline() {
        let state = ConnectionState {
            phase: ConnectionPhase::Connected,
            takeover_applied: true,
            ..ConnectionState::default()
        };
        assert_eq!(state.headline(), ConnectionPhase::Connected);
    }

    #[test]
    fn default_is_disconnected_with_system_proxy() {
        let state = ConnectionState::default();
        assert_eq!(state.phase, ConnectionPhase::Disconnected);
        assert_eq!(state.mode, TakeoverMode::SystemProxy);
        assert_eq!(state.policy, RoutingPolicy::Rule);
        assert!(!state.is_busy());
    }
}
