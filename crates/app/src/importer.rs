//! Turning pasted text, files and subscription payloads into stored nodes.
//!
//! Import never trusts the paste: every link is parsed into a typed profile,
//! validated, fingerprinted, and duplicates collapse onto the existing row. A
//! partially broken paste reports per-line failures instead of failing whole.

use uuid::Uuid;
use v2rayr_domain::{Profile, parse_share_links};
use v2rayr_ipc::{ImportFailure, ImportReport, Problem, SubscriptionView};
use v2rayr_store::ProfileUpsert;

use crate::{App, fingerprint_for, url_host};

#[derive(Debug, Clone, Default)]
pub struct ImportRequest {
    pub text: String,
    /// When the text belongs to a subscription, its nodes are stamped with it.
    pub subscription_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct ImportSummary {
    pub report: ImportReport,
    /// Subscriptions discovered in the payload; the caller decides whether to
    /// refresh them right away.
    pub subscription_ids: Vec<Uuid>,
}

impl App {
    pub fn import_text(&self, request: &ImportRequest) -> Result<ImportSummary, Problem> {
        let outcome = parse_share_links(&request.text);
        let preview = outcome.dedupe(v2rayr_domain::fingerprint::DEFAULT_SALT);
        Ok(self.import_preview(preview, request.subscription_id, Some("links".to_owned())))
    }

    /// Store an already-parsed preview (used by subscription refresh too).
    pub(crate) fn import_preview(
        &self,
        preview: v2rayr_domain::ImportPreview,
        subscription_id: Option<Uuid>,
        detected: Option<String>,
    ) -> ImportSummary {
        let mut added_ids: Vec<Uuid> = Vec::new();
        let mut duplicates = preview.duplicates;
        let mut failures: Vec<ImportFailure> = preview
            .failures
            .into_iter()
            .map(|failure| ImportFailure {
                source: failure.source,
                error: Problem::from(failure.error),
            })
            .collect();

        {
            let db = self.db();
            for profile in preview.profiles {
                let fingerprint = fingerprint_for(&profile);
                match db.fingerprint_exists(&fingerprint) {
                    Ok(true) => duplicates += 1,
                    Ok(false) => {
                        let mut profile = profile;
                        profile.sub_id = subscription_id;
                        let upsert = ProfileUpsert {
                            profile: profile.clone(),
                            fingerprint,
                            sub_id: subscription_id,
                        };
                        match db.upsert_profile(&upsert, self.sealer()) {
                            Ok(stored) => added_ids.push(stored.id),
                            Err(error) => failures.push(ImportFailure {
                                source: redact(&profile),
                                error: crate::error::from_store(error),
                            }),
                        }
                    }
                    Err(error) => failures.push(ImportFailure {
                        source: redact(&profile),
                        error: crate::error::from_store(error),
                    }),
                }
            }
        }
        let mut subscription_ids = Vec::new();
        for url in &preview.subscription_urls {
            match self.add_subscription(url, None, true) {
                Ok(view) => subscription_ids.push(view.id),
                Err(error) => failures.push(ImportFailure {
                    source: redacted_url(url),
                    error,
                }),
            }
        }
        let all_nodes = self.nodes();
        let added: Vec<_> = all_nodes
            .into_iter()
            .filter(|node| added_ids.contains(&node.id))
            .collect();
        let report = ImportReport {
            added,
            duplicates,
            failures,
            subscription_urls: preview.subscription_urls,
            detected_format: detected,
        };
        self.logs.info(
            "import",
            format!(
                "导入完成：新增 {}，重复 {}，失败 {}",
                report.added.len(),
                report.duplicates,
                report.failures.len()
            ),
        );
        self.touch_session();
        ImportSummary {
            report,
            subscription_ids,
        }
    }

    /// Add a subscription row (no network access yet).
    pub fn add_subscription(
        &self,
        url: &str,
        name: Option<&str>,
        auto_update: bool,
    ) -> Result<SubscriptionView, Problem> {
        v2rayr_engine::fetcher::validate_subscription_url(url)
            .map_err(crate::error::from_engine)?;
        let settings = self.settings();
        let db = self.db();
        let upsert = v2rayr_store::SubscriptionUpsert {
            name: name
                .map(str::to_owned)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| url_host(url)),
            url: url.to_owned(),
            enabled: true,
            auto_update,
            interval_hours: settings.subscription.interval_hours,
        };
        let stored = db
            .add_subscription(&upsert)
            .map_err(crate::error::from_store)?;
        drop(db);
        self.logs
            .info("subscription", format!("已添加订阅 {}", stored.name));
        self.subscriptions()
            .into_iter()
            .find(|view| view.id == stored.id)
            .ok_or_else(|| crate::error::internal("订阅写入后读取失败"))
    }

    pub fn remove_subscription(&self, id: Uuid) -> Result<(), Problem> {
        let db = self.db();
        let removed = db
            .delete_subscription(id)
            .map_err(crate::error::from_store)?;
        drop(db);
        if !removed {
            return Err(
                Problem::new(v2rayr_domain::ErrorCode::NotFound.as_str(), false)
                    .with_detail("订阅不存在".to_owned()),
            );
        }
        self.emit_session();
        Ok(())
    }

    pub fn set_subscription_enabled(&self, id: Uuid, enabled: bool) -> Result<(), Problem> {
        self.db()
            .set_subscription_enabled(id, enabled)
            .map_err(crate::error::from_store)?;
        self.emit_session();
        Ok(())
    }

    pub fn set_subscription_auto_update(&self, id: Uuid, auto: bool) -> Result<(), Problem> {
        self.db()
            .set_subscription_auto_update(id, auto)
            .map_err(crate::error::from_store)?;
        self.emit_session();
        Ok(())
    }

    /// Delete every node of a subscription that the user has not favourited.
    pub fn prune_subscription_nodes(&self, id: Uuid) -> Result<usize, Problem> {
        let db = self.db();
        let ids: Vec<Uuid> = db
            .subscription_profiles(id, self.sealer())
            .map_err(crate::error::from_store)?
            .into_iter()
            .filter(|stored| !stored.favorite)
            .map(|stored| stored.id)
            .collect();
        let removed = db
            .delete_unstarred_profiles(&ids)
            .map_err(crate::error::from_store)?;
        drop(db);
        self.emit_session();
        Ok(removed)
    }
}

fn redact(profile: &Profile) -> String {
    redacted_url(&format!(
        "{}://{}:{}",
        profile.protocol.kind(),
        profile.address,
        profile.port
    ))
}

fn redacted_url(url: &str) -> String {
    format!("{}://…", url.split("://").next().unwrap_or("link"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_app;

    const LINKS: &str = "vless://11111111-1111-1111-1111-111111111111@a.example.com:443?security=tls&sni=a.example.com#Tokyo\n\
                         trojan://pw@b.example.com:443?sni=b.example.com#Seoul\n\
                         not-a-link\n";

    #[test]
    fn import_reports_added_duplicates_and_failures() {
        let app = temp_app();
        let first = app
            .import_text(&ImportRequest {
                text: LINKS.into(),
                subscription_id: None,
            })
            .expect("import");
        assert_eq!(first.report.added.len(), 2);
        assert_eq!(first.report.failures.len(), 1);

        let second = app
            .import_text(&ImportRequest {
                text: LINKS.into(),
                subscription_id: None,
            })
            .expect("import again");
        assert_eq!(second.report.added.len(), 0);
        assert_eq!(second.report.duplicates, 2);
        assert_eq!(app.nodes().len(), 2);
    }

    #[test]
    fn importing_http_lines_registers_subscriptions() {
        let app = temp_app();
        let summary = app
            .import_text(&ImportRequest {
                text: "https://sub.example.com/link/abc\n".into(),
                subscription_id: None,
            })
            .expect("import");
        assert_eq!(summary.report.subscription_urls.len(), 1);
        assert_eq!(summary.subscription_ids.len(), 1);
        let subscriptions = app.subscriptions();
        assert_eq!(subscriptions.len(), 1);
        assert_eq!(subscriptions[0].url_host, "sub.example.com");
    }

    #[test]
    fn a_favourite_node_survives_a_prune() {
        let app = temp_app();
        let summary = app
            .import_text(&ImportRequest {
                text: LINKS.into(),
                subscription_id: None,
            })
            .expect("import");
        let node = app.nodes().into_iter().next().expect("node");
        app.db().set_favorite(node.id, true).expect("favourite");
        let removed = app.prune_subscription_nodes(Uuid::nil()).expect("prune");
        // Only nodes of that subscription are considered, so nothing of the
        // global list is touched.
        assert_eq!(removed, 0);
        assert_eq!(app.nodes().len(), summary.report.added.len());
    }
}
