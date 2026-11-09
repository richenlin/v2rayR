//! List/detail views for nodes, subscriptions, kernels and imports.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use v2rayr_domain::CoreKind;

use crate::error::Problem;
use crate::snapshot::{RoutingPolicy, TakeoverMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatencyMethod {
    Tcp,
    Http,
    ThroughKernel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyView {
    pub ms: u32,
    pub method: LatencyMethod,
    pub tested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeView {
    pub id: Uuid,
    pub remark: String,
    /// Protocol id (`vless`, `hysteria2`, ...); the UI localises the label.
    pub protocol: String,
    pub transport: String,
    pub address: String,
    pub port: u16,
    pub tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_name: Option<String>,
    pub tags: Vec<String>,
    pub favorite: bool,
    /// Kernel that will be used for this node.
    pub core: CoreKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<LatencyView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaView {
    pub upload_bytes: u64,
    pub download_bytes: u64,
    pub total_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expire_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionView {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    /// Host only, for logs and support bundles.
    pub url_host: String,
    pub enabled: bool,
    pub auto_update: bool,
    pub interval_hours: u32,
    pub node_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_update_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<Problem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<QuotaView>,
    /// Detected payload format (`links`, `clash`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreStatus {
    pub kind: CoreKind,
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Protocol ids this kernel can serve, used by the node list badge.
    pub protocols: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<Problem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub app_version: String,
    pub api_version: u32,
    pub settings_schema: u32,
    pub db_schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFailure {
    /// Redacted: `scheme://…` or a short prefix.
    pub source: String,
    pub error: Problem,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub added: Vec<NodeView>,
    pub duplicates: usize,
    pub failures: Vec<ImportFailure>,
    /// `http(s)://` lines found in the payload; the UI offers to add them as
    /// subscriptions instead of pretending they are nodes.
    pub subscription_urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyResult {
    pub id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<LatencyView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Problem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    /// `None` means "use the last selected node".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<TakeoverMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<RoutingPolicy>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_view_carries_no_secret() {
        let node = NodeView {
            id: Uuid::nil(),
            remark: "n".into(),
            protocol: "trojan".into(),
            transport: "raw".into(),
            address: "example.com".into(),
            port: 443,
            tls: true,
            subscription_id: None,
            subscription_name: None,
            tags: vec![],
            favorite: false,
            core: CoreKind::SingBox,
            latency: None,
            last_used_at: None,
        };
        let json = serde_json::to_string(&node).expect("serialize");
        assert!(
            !json.contains("password"),
            "no protocol payload on the wire"
        );
        assert_eq!(
            serde_json::to_value(&node).expect("value")["address"],
            "example.com"
        );
    }
}

/// One custom routing rule, as shown in the settings page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleView {
    pub id: Uuid,
    /// `domain` | `domain-suffix` | `ip-cidr` | `process` | `geoip` | `geosite`.
    pub kind: String,
    pub value: String,
    /// `proxy` | `direct` | `block`.
    pub action: String,
    pub enabled: bool,
    pub position: i32,
}
