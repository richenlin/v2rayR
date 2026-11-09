//! Batch latency tests.
//!
//! TCP connect is the default: it is cheap, needs no privileges, and answers the
//! question the node list asks ("is this endpoint reachable?"). A failed node is
//! reported, not skipped, so the UI can show what happened instead of a spinner
//! that never stops. Concurrency is capped (8 by default) so a 10k-node list
//! cannot melt the network stack or the provider's rate limiter.

use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;
use v2rayr_domain::ErrorCode;
use v2rayr_ipc::{LatencyMethod, LatencyResult, Problem};

use crate::{App, latency_view};

const PER_NODE_TIMEOUT: Duration = Duration::from_secs(5);

impl App {
    /// Test the given nodes (or every node when empty), capped by the configured
    /// concurrency.
    pub async fn test_latency(&self, ids: &[Uuid]) -> Result<Vec<LatencyResult>, Problem> {
        let settings = self.settings();
        let targets: Vec<(Uuid, String, u16)> = {
            let db = self.db();
            let rows = if ids.is_empty() {
                db.list_profiles(self.sealer(), None)
                    .map_err(crate::error::from_store)?
            } else {
                let mut rows = Vec::with_capacity(ids.len());
                for id in ids {
                    if let Some(stored) = db
                        .profile(*id, self.sealer())
                        .map_err(crate::error::from_store)?
                    {
                        rows.push(stored);
                    }
                }
                rows
            };
            rows.into_iter()
                .map(|stored| {
                    (
                        stored.id,
                        stored.profile.address.clone(),
                        stored.profile.port,
                    )
                })
                .collect()
        };

        let limit = usize::from(settings.subscription.concurrency.clamp(1, 16));
        let semaphore = Arc::new(tokio::sync::Semaphore::new(limit));
        let mut tasks = Vec::with_capacity(targets.len());
        for (id, address, port) in targets {
            let semaphore = Arc::clone(&semaphore);
            tasks.push(tokio::spawn(async move {
                let _permit = semaphore.acquire().await.expect("semaphore");
                let outcome = v2rayr_engine::tcp_latency(&address, port, PER_NODE_TIMEOUT).await;
                (id, outcome)
            }));
        }

        let mut results = Vec::with_capacity(tasks.len());
        for task in tasks {
            let (id, outcome) = match task.await {
                Ok(value) => value,
                Err(error) => {
                    results.push(LatencyResult {
                        id: Uuid::nil(),
                        latency: None,
                        error: Some(crate::error::internal(format!("测速任务异常: {error}"))),
                    });
                    continue;
                }
            };
            match outcome {
                Ok(ms) => {
                    self.db()
                        .record_latency(id, ms, "tcp")
                        .map_err(crate::error::from_store)?;
                    results.push(LatencyResult {
                        id,
                        latency: Some(v2rayr_ipc::LatencyView {
                            ms,
                            method: LatencyMethod::Tcp,
                            tested_at: chrono::Utc::now(),
                        }),
                        error: None,
                    });
                }
                Err(error) => {
                    results.push(LatencyResult {
                        id,
                        latency: None,
                        error: Some(crate::error::from_engine(error)),
                    });
                }
            }
        }
        self.logs.info(
            "latency",
            format!(
                "测速完成：{} 个节点，{} 个成功",
                results.len(),
                results
                    .iter()
                    .filter(|result| result.latency.is_some())
                    .count()
            ),
        );
        Ok(results)
    }

    /// Last stored latency for one node, already projected for the wire.
    pub fn latency_of(&self, id: Uuid) -> Option<v2rayr_ipc::LatencyView> {
        self.node(id)
            .and_then(|stored| stored.latency)
            .as_ref()
            .map(latency_view)
    }

    /// A per-node timeout problem, exposed so the UI can explain a bare failure.
    pub fn timeout_problem() -> Problem {
        Problem::new(ErrorCode::NetworkError.as_str(), true)
            .with_detail(format!("{} 秒内未连通", PER_NODE_TIMEOUT.as_secs()))
    }
}

#[cfg(test)]
mod tests {
    use crate::importer::ImportRequest;
    use crate::test_support::temp_app;

    #[tokio::test]
    async fn a_closed_port_is_reported_as_a_failure() {
        let app = temp_app();
        // Reserve a port, then close it so the connect is refused quickly.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        app.import_text(&ImportRequest {
            text: format!("vless://11111111-1111-1111-1111-111111111111@127.0.0.1:{port}?encryption=none#closed\n"),
            subscription_id: None,
        })
        .expect("import");
        let results = app.test_latency(&[]).await.expect("test");
        assert_eq!(results.len(), 1);
        assert!(results[0].latency.is_none());
        assert!(results[0].error.is_some());
    }

    #[tokio::test]
    async fn a_live_port_reports_milliseconds_and_persists_them() {
        let app = temp_app();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        app.import_text(&ImportRequest {
            text: format!("vless://11111111-1111-1111-1111-111111111111@127.0.0.1:{port}?encryption=none#live\n"),
            subscription_id: None,
        })
        .expect("import");
        let results = app.test_latency(&[]).await.expect("test");
        assert_eq!(results.len(), 1);
        assert!(results[0].latency.is_some(), "{:?}", results[0].error);
        let node = app.nodes().into_iter().next().expect("node");
        assert!(node.latency.is_some());
        drop(listener);
    }
}
