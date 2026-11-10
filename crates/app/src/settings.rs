//! `settings.json`: schema, migration, atomic writes, validation.
//!
//! Invariants from the design document:
//! * the file is written atomically (temp + fsync + rename), so a crash never
//!   leaves a half-written config;
//! * a file from a *newer* app is never overwritten silently — it is copied
//!   aside and defaults are used, because guessing at unknown fields is how
//!   settings get destroyed;
//! * a corrupt file is preserved next to the original before defaults replace it;
//! * every group is `deny_unknown_fields`, so a typo in a patch is an error the
//!   UI can show instead of a silent no-op.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use v2rayr_domain::CoreKind;
use v2rayr_ipc::{
    AdvancedSettings, AppearanceSettings, InboundSettings, LogLevel, Problem, RoutingPolicy,
    RoutingSettings, SettingsGroupId, SettingsPatch, SettingsView, StartupSettings,
    SubscriptionSettings,
};

pub const CURRENT_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub schema: u32,
    #[serde(default)]
    pub startup: StartupSettings,
    #[serde(default)]
    pub inbound: InboundSettings,
    #[serde(default)]
    pub routing: RoutingSettings,
    #[serde(default)]
    pub subscription: SubscriptionSettings,
    #[serde(default)]
    pub appearance: AppearanceSettings,
    #[serde(default)]
    pub advanced: AdvancedSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: CURRENT_SCHEMA,
            startup: StartupSettings::default(),
            inbound: InboundSettings::default(),
            routing: RoutingSettings::default(),
            subscription: SubscriptionSettings::default(),
            appearance: AppearanceSettings::default(),
            advanced: AdvancedSettings::default(),
        }
    }
}

/// Outcome of loading: warnings are surfaced in `AppSnapshot.warnings` so the UI
/// can say "your settings file was corrupt, defaults were used".
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSettings {
    pub settings: Settings,
    pub warnings: Vec<String>,
}

impl Settings {
    pub fn load(path: &Path) -> LoadedSettings {
        let mut warnings = Vec::new();
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let settings = Settings::default();
                if let Err(error) = settings.save(path) {
                    warnings.push(format!("无法写入默认设置: {error}"));
                }
                return LoadedSettings { settings, warnings };
            }
            Err(error) => {
                warnings.push(format!("无法读取设置文件，已使用默认值: {error}"));
                return LoadedSettings {
                    settings: Settings::default(),
                    warnings,
                };
            }
        };

        let value: Value = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(error) => {
                let backup = backup_path(path);
                if std::fs::copy(path, &backup).is_ok() {
                    warnings.push(format!(
                        "设置文件损坏（{error}），已备份到 {} 并使用默认值",
                        backup.display()
                    ));
                } else {
                    warnings.push(format!("设置文件损坏（{error}），已使用默认值"));
                }
                return LoadedSettings {
                    settings: Settings::default(),
                    warnings,
                };
            }
        };

        // A newer app wrote this file: keep it and its meaning, do not downgrade.
        let schema = value.get("schema").and_then(Value::as_u64).unwrap_or(0) as u32;
        if schema > CURRENT_SCHEMA {
            let backup = backup_path(path);
            let _ = std::fs::copy(path, &backup);
            warnings.push(format!(
                "设置来自更新版本的 v2rayR（schema {schema}），已备份到 {}",
                backup.display()
            ));
            return LoadedSettings {
                settings: Settings::default(),
                warnings,
            };
        }

        let migrated = migrate(value, schema).and_then(|value| {
            serde_json::from_value::<Settings>(value).map_err(|error| error.to_string())
        });
        match migrated {
            Ok(settings) => {
                let mut settings = settings;
                settings.schema = CURRENT_SCHEMA;
                LoadedSettings { settings, warnings }
            }
            Err(error) => {
                warnings.push(format!("设置字段无效（{error}），已使用默认值"));
                LoadedSettings {
                    settings: Settings::default(),
                    warnings,
                }
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let body = serde_json::to_vec_pretty(self)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        write_atomic(path, &body)
    }

    pub fn view(&self, revision: u64) -> SettingsView {
        SettingsView {
            revision,
            startup: self.startup.clone(),
            inbound: self.inbound.clone(),
            routing: self.routing.clone(),
            subscription: self.subscription.clone(),
            appearance: self.appearance.clone(),
            advanced: self.advanced.clone(),
        }
    }

    /// Apply one group patch, validating before it is accepted. The caller only
    /// persists after this returns `Ok`.
    pub fn apply(&mut self, patch: &SettingsPatch) -> Result<(), Problem> {
        let values = serde_json::to_value(&patch.values)
            .map_err(|error| Problem::internal(error.to_string()))?;
        let mut next = self.clone();
        match patch.group {
            SettingsGroupId::Startup => {
                next.startup = serde_json::from_value(values)
                    .map_err(|error| invalid("startup", error.to_string()))?;
            }
            SettingsGroupId::Inbound => {
                next.inbound = serde_json::from_value(values)
                    .map_err(|error| invalid("inbound", error.to_string()))?;
            }
            SettingsGroupId::Routing => {
                next.routing = serde_json::from_value(values)
                    .map_err(|error| invalid("routing", error.to_string()))?;
            }
            SettingsGroupId::Subscription => {
                next.subscription = serde_json::from_value(values)
                    .map_err(|error| invalid("subscription", error.to_string()))?;
            }
            SettingsGroupId::Appearance => {
                next.appearance = serde_json::from_value(values)
                    .map_err(|error| invalid("appearance", error.to_string()))?;
            }
            SettingsGroupId::Advanced => {
                next.advanced = serde_json::from_value(values)
                    .map_err(|error| invalid("advanced", error.to_string()))?;
            }
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), Problem> {
        if self.inbound.allow_lan
            && !self
                .inbound
                .lan_bind
                .trim()
                .parse::<std::net::Ipv4Addr>()
                .is_ok_and(|address| address.is_private())
        {
            return Err(invalid(
                "inbound.lanBind",
                "请填写本机可信网络接口的私有 IPv4 地址".into(),
            ));
        }
        let port = self.inbound.mixed_port;
        if self.inbound.dokodemo_listen_port != 0 {
            let host = self.inbound.dokodemo_address.trim();
            let valid_host = host.parse::<std::net::IpAddr>().is_ok()
                || (!host.is_empty()
                    && host.len() <= 253
                    && host.split('.').all(|label| {
                        !label.is_empty()
                            && label.len() <= 63
                            && label
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    }));
            if self.inbound.dokodemo_listen_port < 1024
                || self.inbound.dokodemo_target_port == 0
                || !valid_host
            {
                return Err(invalid(
                    "inbound.dokodemoListenPort",
                    "请填写 1024–65535 的本地端口，以及有效的目标域名或 IP 和目标端口".into(),
                ));
            }
        }
        if port != 0 && port < 1024 {
            return Err(invalid(
                "inbound.mixedPort",
                "端口需为 0（自动）或 1024–65535".to_owned(),
            ));
        }
        if !(1..=168).contains(&self.subscription.interval_hours) {
            return Err(invalid(
                "subscription.intervalHours",
                "更新间隔需在 1–168 小时之间".to_owned(),
            ));
        }
        if !(1..=16).contains(&self.subscription.concurrency) {
            return Err(invalid(
                "subscription.concurrency",
                "并发数需在 1–16 之间".to_owned(),
            ));
        }
        if !(1..=120).contains(&self.subscription.timeout_secs) {
            return Err(invalid(
                "subscription.timeoutSecs",
                "超时需在 1–120 秒之间".to_owned(),
            ));
        }
        if !(1280..=9000).contains(&self.advanced.tun_mtu) {
            return Err(invalid(
                "advanced.tunMtu",
                "MTU 需在 1280–9000 之间".to_owned(),
            ));
        }
        if !(1..=256).contains(&self.advanced.mux_concurrency) {
            return Err(invalid(
                "advanced.muxConcurrency",
                "多路复用并发需在 1–256 之间".to_owned(),
            ));
        }
        const DNS_STRATEGIES: [&str; 4] = ["prefer_ipv4", "prefer_ipv6", "ipv4_only", "ipv6_only"];
        if !DNS_STRATEGIES.contains(&self.routing.dns_strategy.as_str()) {
            return Err(invalid("routing.dnsStrategy", "未知的 DNS 策略".to_owned()));
        }
        if self.routing.dns_servers.lines().count() > 8 {
            return Err(invalid(
                "routing.dnsServers",
                "最多 8 个 DNS 服务器".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn dns_servers(&self) -> Vec<String> {
        split_lines(&self.routing.dns_servers)
    }

    pub fn bypass_extra(&self) -> Vec<String> {
        split_lines(&self.inbound.bypass_extra)
    }

    pub fn subscription_user_agent(&self, version: &str) -> String {
        self.subscription.user_agent.replace("<ver>", version)
    }

    /// The inbound port to aim for: explicit, or the documented 10808 default.
    pub fn desired_inbound_port(&self) -> u16 {
        if self.inbound.mixed_port == 0 {
            10808
        } else {
            self.inbound.mixed_port
        }
    }

    pub fn core_override(&self) -> Option<CoreKind> {
        match self.advanced.core_override {
            v2rayr_ipc::CoreOverride::Auto => None,
            v2rayr_ipc::CoreOverride::SingBox => Some(CoreKind::SingBox),
            v2rayr_ipc::CoreOverride::Xray => Some(CoreKind::Xray),
            v2rayr_ipc::CoreOverride::Mihomo => Some(CoreKind::Mihomo),
        }
    }

    pub fn policy(&self) -> RoutingPolicy {
        self.routing.policy
    }

    pub fn log_level(&self) -> LogLevel {
        self.advanced.log_level
    }

    pub fn default_value() -> Value {
        json!(Settings::default())
    }
}

fn invalid(field: &str, detail: String) -> Problem {
    Problem::new(v2rayr_domain::ErrorCode::InvalidValue.as_str(), false)
        .with_detail(format!("{field}: {detail}"))
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn backup_path(path: &Path) -> PathBuf {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings.json".to_owned());
    path.with_file_name(format!("{name}.{stamp}.bak"))
}

/// Migrations are idempotent: running `migrate` on an already-migrated value must
/// not change it.
pub fn migrate(mut value: Value, from: u32) -> Result<Value, String> {
    if !value.is_object() {
        return Err("settings root is not an object".to_owned());
    }
    // schema 0 -> 1: the pre-release key was `port`, the released one is
    // `mixedPort`, and 0 now means "automatic".
    if from < 1
        && let Some(inbound) = value.get_mut("inbound").and_then(Value::as_object_mut)
    {
        if let Some(port) = inbound.remove("port") {
            inbound.entry("mixedPort").or_insert(port);
            inbound.entry("allowLan").or_insert(Value::Bool(false));
        }
    }
    value["schema"] = json!(CURRENT_SCHEMA);
    Ok(value)
}

pub fn write_atomic(path: &Path, body: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    // NamedTempFile creates an exclusive private file instead of truncating a
    // predictable path. Kernel configs can contain passwords and private keys.
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(body)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|failure| failure.error)?;
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("settings.json")
    }

    #[test]
    fn defaults_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = temp_path(&dir);
        let settings = Settings::default();
        settings.save(&path).expect("save");
        let loaded = Settings::load(&path);
        assert_eq!(loaded.settings, settings);
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.settings.desired_inbound_port(), 10808);
    }

    #[test]
    fn a_missing_file_is_created_with_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = temp_path(&dir);
        let loaded = Settings::load(&path);
        assert!(path.is_file());
        assert_eq!(loaded.settings, Settings::default());
    }

    #[test]
    fn a_corrupt_file_is_backed_up_not_lost() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = temp_path(&dir);
        std::fs::write(&path, "{ this is not json").expect("write");
        let loaded = Settings::load(&path);
        assert_eq!(loaded.settings, Settings::default());
        assert!(
            loaded
                .warnings
                .iter()
                .any(|warning| warning.contains("损坏"))
        );
        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".bak"))
            .collect();
        assert_eq!(backups.len(), 1);
    }

    #[test]
    fn a_newer_schema_is_not_downgraded_in_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = temp_path(&dir);
        std::fs::write(&path, r#"{"schema":99,"inbound":{"mixedPort":1234}}"#).expect("write");
        let loaded = Settings::load(&path);
        assert_eq!(loaded.settings, Settings::default());
        assert!(
            loaded
                .warnings
                .iter()
                .any(|warning| warning.contains("更新版本"))
        );
        assert!(
            dir.path()
                .read_dir()
                .expect("read")
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains("settings.json."))
        );
    }

    #[test]
    fn migration_renames_the_legacy_port_key_and_is_idempotent() {
        let legacy = json!({"inbound": {"port": 10888}});
        let migrated = migrate(legacy, 0).expect("migrate");
        assert_eq!(migrated["inbound"]["mixedPort"], 10888);
        assert_eq!(migrated["schema"], CURRENT_SCHEMA);
        let again = migrate(migrated.clone(), CURRENT_SCHEMA).expect("migrate");
        assert_eq!(again, migrated);
    }

    #[test]
    fn patches_are_type_checked_and_validated() {
        let mut settings = Settings::default();
        settings
            .apply(&SettingsPatch {
                group: SettingsGroupId::Inbound,
                values: json!({"mixedPort": 10888, "allowLan": false, "bypassLocal": true, "bypassExtra": "", "tunEnabled": false}),
            })
            .expect("patch");
        assert_eq!(settings.desired_inbound_port(), 10888);

        let error = settings
            .apply(&SettingsPatch {
                group: SettingsGroupId::Inbound,
                values: json!({"mixedPort": 80}),
            })
            .expect_err("privileged port");
        assert_eq!(error.key, "error.invalid_value");

        let error = settings
            .apply(&SettingsPatch {
                group: SettingsGroupId::Inbound,
                values: json!({"mixedPort": 10888, "typo": true}),
            })
            .expect_err("unknown field");
        assert_eq!(error.key, "error.invalid_value");
    }

    #[test]
    fn validation_rejects_out_of_range_values() {
        let mut settings = Settings::default();
        settings.subscription.interval_hours = 0;
        assert!(settings.validate().is_err());
        settings.subscription.interval_hours = 12;
        settings.routing.dns_strategy = "whatever".into();
        assert!(settings.validate().is_err());
        settings.routing.dns_strategy = "prefer_ipv4".into();
        settings.advanced.tun_mtu = 100;
        assert!(settings.validate().is_err());
        settings.advanced.tun_mtu = 1500;
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn multiline_lists_ignore_comments_and_blanks() {
        let mut settings = Settings::default();
        settings.routing.dns_servers = "1.1.1.1\n# comment\n\n8.8.8.8\n".into();
        assert_eq!(settings.dns_servers(), vec!["1.1.1.1", "8.8.8.8"]);
    }

    #[test]
    fn the_user_agent_template_is_substituted() {
        let mut settings = Settings::default();
        settings.subscription.user_agent = "v2rayR/<ver>".to_owned();
        assert_eq!(settings.subscription_user_agent("1.0.0"), "v2rayR/1.0.0");
        // A concrete agent is passed through unchanged.
        settings.subscription.user_agent = "v2rayR/0.9.0".to_owned();
        assert_eq!(settings.subscription_user_agent("1.0.0"), "v2rayR/0.9.0");
    }
}
