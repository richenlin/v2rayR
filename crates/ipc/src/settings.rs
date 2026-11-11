//! User settings, grouped the way the settings page is laid out.
//!
//! The page saves one group at a time, so the wire format is "one group of
//! values" rather than a giant flat object. Groups are `deny_unknown_fields` on
//! the receiving side (in `v2rayr-app`), which turns a UI/backend drift into a
//! loud error instead of a silently ignored setting.

use serde::{Deserialize, Serialize};

use crate::events::LogLevel;
use crate::snapshot::RoutingPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    System,
    ZhCn,
    EnUs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoreOverride {
    /// Pick a kernel per node using the capability matrix.
    Auto,
    SingBox,
    Xray,
    Mihomo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartupSettings {
    pub launch_at_login: bool,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub show_tray_icon: bool,
    pub single_instance: bool,
    pub connect_on_launch: bool,
}

impl Default for StartupSettings {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            start_minimized: false,
            close_to_tray: true,
            show_tray_icon: true,
            single_instance: true,
            connect_on_launch: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InboundSettings {
    /// `0` means "pick the first free port starting at 10808". Anything else is
    /// a fixed port the user asked for, and a conflict is surfaced instead of
    /// being silently replaced.
    pub mixed_port: u16,
    #[serde(default)]
    pub dokodemo_listen_port: u16,
    #[serde(default)]
    pub dokodemo_address: String,
    #[serde(default)]
    pub dokodemo_target_port: u16,
    pub allow_lan: bool,
    #[serde(default)]
    pub lan_bind: String,
    pub bypass_local: bool,
    /// Extra bypass rules, one domain or IP per line.
    pub bypass_extra: String,
    pub tun_enabled: bool,
}

impl Default for InboundSettings {
    fn default() -> Self {
        Self {
            mixed_port: 0,
            dokodemo_listen_port: 0,
            dokodemo_address: String::new(),
            dokodemo_target_port: 0,
            allow_lan: false,
            lan_bind: String::new(),
            bypass_local: true,
            bypass_extra: String::new(),
            tun_enabled: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingSettings {
    pub policy: RoutingPolicy,
    pub rule_set_enabled: bool,
    pub block_ads: bool,
    pub dns_strategy: String,
    /// Comma separated DNS servers; empty means "use the system resolver".
    pub dns_servers: String,
    pub fake_ip: bool,
}

impl Default for RoutingSettings {
    fn default() -> Self {
        Self {
            policy: RoutingPolicy::Rule,
            rule_set_enabled: true,
            block_ads: false,
            dns_strategy: "prefer_ipv4".to_owned(),
            dns_servers: String::new(),
            fake_ip: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionSettings {
    pub interval_hours: u32,
    pub update_on_start: bool,
    pub concurrency: u8,
    pub timeout_secs: u32,
    pub user_agent: String,
    /// Keep nodes that disappeared upstream even when they are not favourited.
    pub keep_removed_nodes: bool,
}

impl Default for SubscriptionSettings {
    fn default() -> Self {
        Self {
            interval_hours: 12,
            update_on_start: false,
            concurrency: 4,
            timeout_secs: 10,
            user_agent: concat!("v2rayR/", env!("CARGO_PKG_VERSION")).to_owned(),
            keep_removed_nodes: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppearanceSettings {
    pub theme: Theme,
    pub language: Language,
    pub compact: bool,
    pub reduce_motion: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            language: Language::System,
            compact: false,
            reduce_motion: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvancedSettings {
    pub core_override: CoreOverride,
    pub log_level: LogLevel,
    pub tun_mtu: u16,
    pub sniffing: bool,
    pub tcp_fast_open: bool,
    pub mux_enabled: bool,
    pub mux_concurrency: u16,
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        Self {
            core_override: CoreOverride::Auto,
            // Info makes the kernels log every connection (three lines per
            // request for Xray); a resident proxy should not write that by default.
            log_level: LogLevel::Warn,
            tun_mtu: 1500,
            sniffing: true,
            tcp_fast_open: true,
            mux_enabled: false,
            mux_concurrency: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SettingsGroupId {
    Startup,
    Inbound,
    Routing,
    Subscription,
    Appearance,
    Advanced,
}

impl SettingsGroupId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Inbound => "inbound",
            Self::Routing => "routing",
            Self::Subscription => "subscription",
            Self::Appearance => "appearance",
            Self::Advanced => "advanced",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    /// Monotonic; the UI uses it to discard stale patch responses.
    pub revision: u64,
    pub startup: StartupSettings,
    pub inbound: InboundSettings,
    pub routing: RoutingSettings,
    pub subscription: SubscriptionSettings,
    pub appearance: AppearanceSettings,
    pub advanced: AdvancedSettings,
}

/// A partial update for exactly one group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub group: SettingsGroupId,
    /// The full new value of the group, validated by deserialising into its
    /// typed struct on the receiving side.
    pub values: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_design_document() {
        let settings = SettingsView::default();
        assert_eq!(settings.inbound.mixed_port, 0, "auto-port by default");
        assert!(settings.inbound.bypass_local);
        assert!(!settings.inbound.allow_lan);
        assert!(!settings.inbound.tun_enabled);
        assert!(!settings.routing.fake_ip);
        assert_eq!(settings.subscription.interval_hours, 12);
        assert!(
            settings.subscription.concurrency <= 8,
            "plan caps concurrency at 8"
        );
        assert_eq!(settings.appearance.theme, Theme::System);
        assert_eq!(settings.appearance.language, Language::System);
        assert_eq!(settings.advanced.core_override, CoreOverride::Auto);
    }

    #[test]
    fn wire_format_is_camel_case_kebab_enums() {
        let json = serde_json::to_value(SettingsView::default()).expect("serialize");
        assert!(json["inbound"]["mixedPort"].is_number());
        assert_eq!(json["appearance"]["theme"], "system");
        assert_eq!(json["advanced"]["coreOverride"], "auto");
        assert_eq!(json["routing"]["policy"], "rule");
    }

    #[test]
    fn unknown_field_in_a_group_is_rejected() {
        let error = serde_json::from_value::<InboundSettings>(serde_json::json!({
            "mixedPort": 10808,
            "allowLan": false,
            "bypassLocal": true,
            "bypassExtra": "",
            "tunEnabled": false,
            "surprise": 1
        }))
        .expect_err("must reject");
        assert!(error.to_string().contains("surprise"));
    }
}
