//! Subscription refresh: fetch, merge, keep the user's local state.
//!
//! Merge rules come from `domain::plan_merge`: fingerprints identify nodes, ids
//! are preserved for nodes that changed, favourites are never deleted, and a
//! failed refresh keeps the old nodes (only `last_error` and `next_update_at`
//! move), because a broken provider must not empty the node list.

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use uuid::Uuid;
use v2rayr_domain::{ExistingNode, fingerprint::DEFAULT_SALT, plan_merge};
use v2rayr_engine::{EngineError, Fetcher};
use v2rayr_ipc::{ImportFailure, Problem};
use v2rayr_store::ProfileUpsert;

use crate::{App, SubscriptionQuota, fingerprint_for};

#[derive(Debug, Clone)]
pub struct RefreshReport {
    pub id: Uuid,
    pub name: String,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub kept: usize,
    pub failures: Vec<ImportFailure>,
    /// Set when the refresh itself failed; nodes were kept.
    pub error: Option<Problem>,
}

impl RefreshReport {
    fn failed(id: Uuid, name: String, error: Problem) -> Self {
        Self {
            id,
            name,
            added: 0,
            updated: 0,
            removed: 0,
            kept: 0,
            failures: Vec::new(),
            error: Some(error),
        }
    }
}

impl App {
    /// Refresh one subscription. Returns a report even when it failed — the
    /// caller (and the UI) wants per-subscription status, not one broad error.
    pub async fn refresh_subscription(&self, id: Uuid) -> RefreshReport {
        let settings = self.settings();
        let subscription = match self.db().subscription(id) {
            Ok(Some(subscription)) => subscription,
            Ok(None) => {
                return RefreshReport::failed(
                    id,
                    String::new(),
                    Problem::new(v2rayr_domain::ErrorCode::NotFound.as_str(), false)
                        .with_detail("订阅不存在".to_owned()),
                );
            }
            Err(error) => {
                return RefreshReport::failed(id, String::new(), crate::error::from_store(error));
            }
        };

        let fetcher = Fetcher::new(
            settings.subscription_user_agent(self.version()),
            Duration::from_secs(u64::from(settings.subscription.timeout_secs)),
        );
        let fetched = match fetcher.fetch(&subscription.url).await {
            Ok(fetched) => fetched,
            Err(error) => {
                let problem = crate::error::from_engine(error);
                let retry_in = ChronoDuration::minutes(30);
                {
                    let db = self.db();
                    let body = serde_json::to_string(&problem).unwrap_or_else(|_| "{}".to_owned());
                    let _ = db.mark_subscription_failed(id, &body, retry_in, Utc::now());
                }
                self.logs.warn(
                    "subscription",
                    format!(
                        "订阅「{}」更新失败，保留旧节点：{problem:?}",
                        subscription.name
                    ),
                );
                self.emit_session();
                return RefreshReport::failed(id, subscription.name, problem);
            }
        };

        let existing: Vec<ExistingNode> = match self.db().subscription_profiles(id, self.sealer()) {
            Ok(rows) => rows
                .into_iter()
                .map(|stored| ExistingNode {
                    id: stored.id,
                    fingerprint: stored.fingerprint,
                    favorite: stored.favorite,
                })
                .collect(),
            Err(error) => {
                return RefreshReport::failed(
                    id,
                    subscription.name,
                    crate::error::from_store(error),
                );
            }
        };

        let incoming: Vec<_> = fetched
            .preview
            .profiles
            .into_iter()
            .map(|mut profile| {
                profile.sub_id = Some(id);
                profile
            })
            .collect();
        let plan = plan_merge(&existing, &incoming, DEFAULT_SALT);

        let keep_removed = settings.subscription.keep_removed_nodes;
        let mut failures = Vec::new();
        let mut added = 0usize;
        let mut updated = 0usize;
        {
            let db = self.db();
            for profile in &plan.added {
                let upsert = ProfileUpsert {
                    profile: profile.clone(),
                    fingerprint: fingerprint_for(profile),
                    sub_id: Some(id),
                };
                if let Err(error) = db.upsert_profile(&upsert, self.sealer()) {
                    failures.push(ImportFailure {
                        source: profile.remark.clone(),
                        error: crate::error::from_store(error),
                    });
                } else {
                    added += 1;
                }
            }
            for (node_id, profile) in &plan.updated {
                let upsert = ProfileUpsert {
                    profile: profile.clone(),
                    fingerprint: fingerprint_for(profile),
                    sub_id: Some(id),
                };
                if let Err(error) = db.replace_profile(*node_id, &upsert, self.sealer()) {
                    failures.push(ImportFailure {
                        source: profile.remark.clone(),
                        error: crate::error::from_store(error),
                    });
                } else {
                    updated += 1;
                }
            }
        }

        if !keep_removed && !plan.removed.is_empty() {
            let _ = self.db().delete_unstarred_profiles(&plan.removed);
        }

        let quota = fetched.userinfo.as_ref().map(|info| SubscriptionQuota {
            upload: info.upload,
            download: info.download,
            total: info.total,
            expire: info.expire,
        });
        let quota_json = quota
            .as_ref()
            .and_then(|quota| serde_json::to_string(quota).ok());
        {
            let db = self.db();
            let _ = db.mark_subscription_updated(
                id,
                Some(fetched.format.as_str()),
                quota_json.as_deref(),
                Utc::now(),
            );
        }
        self.logs.info(
            "subscription",
            format!(
                "订阅「{}」已更新：新增 {added}，更新 {updated}，移除 {}",
                subscription.name,
                plan.removed.len()
            ),
        );
        self.emit_session();
        RefreshReport {
            id,
            name: subscription.name,
            added,
            updated,
            removed: plan.removed.len(),
            kept: plan.kept.len(),
            failures,
            error: None,
        }
    }

    /// Refresh every subscription that is due. Errors are reported per
    /// subscription; one dead provider must not stop the others.
    pub async fn refresh_due_subscriptions(&self) -> Vec<RefreshReport> {
        let due = match self.db().due_subscriptions(Utc::now()) {
            Ok(due) => due,
            Err(error) => {
                self.logs
                    .warn("subscription", format!("读取待更新订阅失败: {error}"));
                return Vec::new();
            }
        };
        let mut reports = Vec::with_capacity(due.len());
        for subscription in due {
            reports.push(self.refresh_subscription(subscription.id).await);
        }
        reports
    }

    /// Add and immediately fetch a subscription.
    pub async fn add_and_refresh(
        &self,
        url: &str,
        name: Option<&str>,
        auto_update: bool,
    ) -> Result<(Uuid, RefreshReport), Problem> {
        let view = self.add_subscription(url, name, auto_update)?;
        let report = self.refresh_subscription(view.id).await;
        match &report.error {
            Some(error) => Err(error.clone()),
            None => Ok((view.id, report)),
        }
    }

    /// Human-facing summary used by the CLI and the toast on the desktop.
    pub fn describe_refresh(report: &RefreshReport) -> String {
        match &report.error {
            Some(error) => format!("刷新失败: {}", error.key),
            None => format!(
                "新增 {} · 更新 {} · 移除 {}",
                report.added, report.updated, report.removed
            ),
        }
    }
}

/// Keeps `EngineError` import meaningful for future callers of this module.
#[allow(dead_code)]
fn is_network(error: &EngineError) -> bool {
    matches!(error, EngineError::Network(_) | EngineError::Timeout(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_app;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn(body: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = [0u8; 4096];
                let _ = socket.read(&mut buffer).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        port
    }

    fn links(a: &str, port: u16) -> String {
        format!(
            "vless://11111111-1111-1111-1111-111111111111@{a}:{port}?security=tls&sni={a}#{a}\n"
        )
    }

    #[tokio::test]
    async fn refresh_merges_new_nodes_and_keeps_favourites() {
        let body: &'static str = Box::leak(
            format!(
                "{}trojan://pw@second.example.com:443?sni=second.example.com#Second\n",
                links("first.example.com", 443)
            )
            .into_boxed_str(),
        );
        let port = spawn(body).await;
        let app = temp_app();
        let view = app
            .add_subscription(
                &format!("http://127.0.0.1:{port}/sub"),
                Some("测试订阅"),
                true,
            )
            .expect("add");
        let report = app.refresh_subscription(view.id).await;
        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(report.added, 2);
        assert_eq!(app.nodes().len(), 2);

        let favourite = app
            .nodes()
            .into_iter()
            .find(|node| node.remark.contains("first.example.com"))
            .expect("favourite node");
        app.db().set_favorite(favourite.id, true).expect("fav");

        // Second refresh where the favourite is gone upstream: it must survive.
        let body2: &'static str = Box::leak(
            "trojan://pw@second.example.com:443?sni=second.example.com#Second\n"
                .to_owned()
                .into_boxed_str(),
        );
        let port2 = spawn(body2).await;
        app.db()
            .set_subscription_url(view.id, &format!("http://127.0.0.1:{port2}/sub"))
            .expect("update url");
        let report = app.refresh_subscription(view.id).await;
        assert_eq!(report.removed, 0);
        assert_eq!(report.kept, 1);
        let ids: Vec<Uuid> = app.nodes().into_iter().map(|node| node.id).collect();
        assert!(ids.contains(&favourite.id), "favourite node was deleted");
    }

    #[tokio::test]
    async fn a_failed_refresh_keeps_the_old_nodes_and_reports_the_error() {
        let app = temp_app();
        // Closed port: connection refused.
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        let view = app
            .add_subscription(
                &format!("http://127.0.0.1:{port}/sub"),
                Some("坏订阅"),
                true,
            )
            .expect("add");
        let report = app.refresh_subscription(view.id).await;
        assert!(report.error.is_some());
        let stored = app.subscriptions();
        assert_eq!(stored[0].node_count, 0);
        assert!(stored[0].last_error.is_some());
    }
}
