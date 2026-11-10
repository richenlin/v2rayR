//! Portable preferences and custom routing rules. The format cannot contain
//! database files, credentials, runtime state or downloaded executable assets.
use std::{
    io::{Read, Write},
    path::Path,
    sync::atomic::Ordering,
};

use serde::{Deserialize, Serialize};
use v2rayr_domain::RuleApplication;
use v2rayr_ipc::{AppearanceSettings, ConnectionPhase, Problem, RoutingPolicy};
use v2rayr_store::{RuleAction, RuleKind};

use crate::{App, error, rules::{normalize_applications, normalize_rule_value}};

const VERSION: u32 = 1;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_RULES: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableBackup {
    version: u32,
    appearance: AppearanceSettings,
    routing_policy: RoutingPolicy,
    rule_set_enabled: bool,
    block_ads: bool,
    subscription_interval_hours: u32,
    rules: Vec<PortableRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PortableRule {
    kind: String,
    value: String,
    action: String,
    enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    applications: Vec<RuleApplication>,
}

impl PortableBackup {
    pub fn decode(bytes: &[u8]) -> Result<Self, Problem> {
        if bytes.len() > MAX_BYTES {
            return Err(Problem::config("备份超过 4 MiB"));
        }
        let backup: Self =
            serde_json::from_slice(bytes).map_err(|_| Problem::config("无效的 v2rayR 便携备份"))?;
        if backup.version != VERSION
            || backup.rules.len() > MAX_RULES
            || !(1..=168).contains(&backup.subscription_interval_hours)
        {
            return Err(Problem::config("备份版本、规则数量或订阅间隔不受支持"));
        }
        for rule in &backup.rules {
            rule.validate()?;
        }
        Ok(backup)
    }

    pub fn read(path: &Path) -> Result<Self, Problem> {
        let file = std::fs::File::open(path).map_err(storage)?;
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(storage)?;
        Self::decode(&bytes)
    }

    fn encode(&self) -> Result<Vec<u8>, Problem> {
        let bytes = serde_json::to_vec_pretty(self).map_err(error::internal)?;
        Self::decode(&bytes)?;
        Ok(bytes)
    }

    pub fn write_new(&self, path: &Path) -> Result<(), Problem> {
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(storage)?;
        temp.write_all(&self.encode()?).map_err(storage)?;
        temp.as_file().sync_all().map_err(storage)?;
        temp.persist_noclobber(path)
            .map_err(|failure| storage(failure.error))?;
        Ok(())
    }

    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "version": self.version, "rules": self.rules.len(),
            "preferences": ["appearance", "routingPolicy", "ruleSetEnabled", "blockAds", "subscriptionIntervalHours"],
        })
    }

    /// GET a portable file using a transient HTTPS WebDAV connection.
    pub async fn webdav_get(
        url: &str,
        auth: Option<(&str, &str)>,
    ) -> Result<(Self, Option<String>), Problem> {
        let mut response = request(reqwest::Method::GET, url, auth)?
            .send()
            .await
            .map_err(network)?;
        let status = response.status();
        if !status.is_success() {
            return Err(status_error(status));
        }
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return Err(Problem::config("远程备份超过 4 MiB"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(Problem::config("远程备份超过 4 MiB"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((Self::decode(&bytes)?, etag))
    }

    /// No last-writer-wins: default is create-only. Replacing requires the
    /// exact strong ETag the user obtained when fetching the existing file.
    pub async fn webdav_put(
        &self,
        url: &str,
        auth: Option<(&str, &str)>,
        etag: Option<&str>,
    ) -> Result<(), Problem> {
        let mut request = request(reqwest::Method::PUT, url, auth)?;
        request = if let Some(etag) = etag {
            if !etag.starts_with('"')
                || !etag.ends_with('"')
                || etag.len() < 2
                || etag[1..etag.len() - 1].contains('"')
                || etag.chars().any(char::is_control)
            {
                return Err(Problem::config("需要服务端返回的单个强 ETag"));
            }
            request.header(reqwest::header::IF_MATCH, etag)
        } else {
            request.header(reqwest::header::IF_NONE_MATCH, "*")
        };
        let status = request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(self.encode()?)
            .send()
            .await
            .map_err(network)?
            .status();
        if status.is_success() {
            Ok(())
        } else {
            Err(status_error(status))
        }
    }
}

impl PortableRule {
    fn validate(&self) -> Result<(RuleKind, String, RuleAction, bool, Vec<RuleApplication>), Problem> {
        let kind =
            RuleKind::parse(&self.kind).ok_or_else(|| Problem::config("备份包含未知规则类型"))?;
        let action = RuleAction::parse(&self.action)
            .ok_or_else(|| Problem::config("备份包含未知规则动作"))?;
        Ok((
            kind,
            normalize_rule_value(&self.value)?,
            action,
            self.enabled,
            normalize_applications(&self.applications)?,
        ))
    }
}

impl App {
    pub fn export_portable_backup(&self) -> Result<PortableBackup, Problem> {
        let settings = self.settings();
        let rules = self
            .rules()?
            .into_iter()
            .map(|rule| PortableRule {
                kind: rule.kind,
                value: rule.value,
                action: rule.action,
                enabled: rule.enabled,
                applications: rule.applications,
            })
            .collect();
        Ok(PortableBackup {
            version: VERSION,
            appearance: settings.appearance,
            routing_policy: settings.routing.policy,
            rule_set_enabled: settings.routing.rule_set_enabled,
            block_ads: settings.routing.block_ads,
            subscription_interval_hours: settings.subscription.interval_hours,
            rules,
        })
    }

    pub async fn import_portable_backup(&self, backup: PortableBackup) -> Result<usize, Problem> {
        let _guard = self.op_lock.lock().await;
        if self.snapshot()?.connection.phase != ConnectionPhase::Disconnected {
            return Err(Problem::config("请先断开连接并恢复网络，再应用备份"));
        }
        let previous = self.settings();
        let mut next = previous.clone();
        next.appearance = backup.appearance;
        next.routing.policy = backup.routing_policy;
        next.routing.rule_set_enabled = backup.rule_set_enabled;
        next.routing.block_ads = backup.block_ads;
        next.subscription.interval_hours = backup.subscription_interval_hours;
        next.validate()?;
        let rules = backup
            .rules
            .iter()
            .map(PortableRule::validate)
            .collect::<Result<Vec<_>, _>>()?;
        let mut wrote_settings = false;
        let result = self.db().merge_backup_rules(&rules, || {
            next.save(&self.paths().settings())?;
            wrote_settings = true;
            Ok(())
        });
        let count = match result {
            Ok(count) => count,
            Err(err) => {
                if wrote_settings {
                    previous.save(&self.paths().settings()).map_err(storage)?;
                }
                return Err(error::from_store(err));
            }
        };
        self.set_idle_policy(next.policy());
        *self.settings.write().expect("settings") = next;
        self.settings_revision.fetch_add(1, Ordering::Relaxed);
        self.emit_session();
        Ok(count)
    }
}

fn request(
    method: reqwest::Method,
    url: &str,
    auth: Option<(&str, &str)>,
) -> Result<reqwest::RequestBuilder, Problem> {
    let url = reqwest::Url::parse(url).map_err(|_| Problem::config("WebDAV 地址无效"))?;
    if url.scheme() != "https"
        || url.host().is_none()
        || url.path().ends_with('/')
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err(Problem::config(
            "请输入不带凭据、查询参数或片段的 HTTPS WebDAV 文件地址",
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(network)?;
    let request = client.request(method, url);
    Ok(if let Some((user, password)) = auth {
        request.basic_auth(user, Some(password))
    } else {
        request
    })
}

fn network(_: reqwest::Error) -> Problem {
    Problem::network("WebDAV 请求失败，请检查地址、证书或网络")
}
fn storage(error: std::io::Error) -> Problem {
    Problem::storage(error.to_string())
}
fn status_error(status: reqwest::StatusCode) -> Problem {
    if status == reqwest::StatusCode::PRECONDITION_FAILED || status == reqwest::StatusCode::CONFLICT
    {
        Problem::new("webdav_conflict", false)
            .with_detail("远程备份已存在或已变更，请另存为新文件，或重新读取 ETag 后再覆盖")
    } else {
        Problem::network(format!("WebDAV 返回 HTTP {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn export_omits_secrets_and_restore_merges_without_overwriting_rules() {
        let source = crate::test_support::temp_app();
        let target = crate::test_support::temp_app();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("backup.json");
        source
            .add_subscription("https://example.com/sub?token=secret-token", None, false)
            .unwrap();
        source.add_rule("domain", "example.com", "block").unwrap();
        let rule = source.add_rule("domain", "example.org", "direct").unwrap()[1].clone();
        source.set_rule_enabled(rule.id, false).unwrap();
        target.add_rule("domain", "example.com", "block").unwrap();
        let backup = source.export_portable_backup().unwrap();
        backup.write_new(&path).unwrap();
        assert!(backup.write_new(&path).is_err());
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("secret-token")
        );
        assert_eq!(
            target
                .import_portable_backup(PortableBackup::read(&path).unwrap())
                .await
                .unwrap(),
            1
        );
        let rules = target.rules().unwrap();
        assert_eq!(rules.len(), 2);
        assert!(rules[0].enabled);
        assert!(!rules[1].enabled);
        assert_eq!(target.import_portable_backup(backup).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn scoped_backup_rules_do_not_merge_with_wildcard_rules() {
        let source = crate::test_support::temp_app();
        let target = crate::test_support::temp_app();
        let application = RuleApplication {
            path: "/Applications/Example.app/Contents/MacOS/Example".to_owned(),
            sha256: "a".repeat(64),
        };
        source.add_rule_for_applications(
            "domain", "example.com", "block", std::slice::from_ref(&application),
        ).unwrap();
        target.add_rule("domain", "example.com", "block").unwrap();
        let backup = source.export_portable_backup().unwrap();
        let decoded = PortableBackup::decode(&backup.encode().unwrap()).unwrap();
        assert_eq!(target.import_portable_backup(decoded).await.unwrap(), 1);
        assert_eq!(target.rules().unwrap()[1].applications, [application]);
        assert_eq!(target.import_portable_backup(backup).await.unwrap(), 0);
    }

    #[test]
    fn remote_input_rejects_embedded_credentials_and_unsafe_schemes() {
        for url in [
            "http://example.com/backup.json",
            "https://alice:secret@example.com/backup.json",
            "https://example.com/backup.json?token=secret",
        ] {
            assert!(request(reqwest::Method::GET, url, None).is_err());
        }
    }
}
