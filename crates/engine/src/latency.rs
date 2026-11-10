//! Node latency probes.
//!
//! Three methods, in increasing cost and fidelity:
//! * TCP connect — works without a running kernel, measures reachability only;
//! * HTTP through the kernel — the honest number, requires the kernel;
//! * ICMP — not implemented (needs privileges); the UI never offers it.

use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

use crate::EngineError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LatencyTarget {
    Tcp { host: String, port: u16 },
    Http { url: String },
    ThroughKernel { proxy_tag: String, url: String },
}

pub struct LatencyProbe {
    client: reqwest::Client,
}

impl LatencyProbe {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .user_agent("v2rayR latency probe")
                .build()
                .unwrap_or_default(),
        }
    }

    pub async fn measure(&self, target: &LatencyTarget) -> Result<u32, EngineError> {
        match target {
            LatencyTarget::Tcp { host, port } => {
                tcp_latency(host, *port, Duration::from_secs(5)).await
            }
            LatencyTarget::Http { url } => self.http_latency(url).await,
            LatencyTarget::ThroughKernel { .. } => Err(EngineError::Network(
                "through-kernel latency needs a running kernel (ClashClient::delay)".to_owned(),
            )),
        }
    }

    pub async fn http_latency(&self, url: &str) -> Result<u32, EngineError> {
        let started = Instant::now();
        let response = self
            .client
            .head(url)
            .send()
            .await
            .map_err(|error| EngineError::Network(error.to_string()))?;
        let status = response.status();
        if !status.is_success() && !status.is_redirection() {
            return Err(EngineError::Network(format!("HTTP {status}")));
        }
        Ok(elapsed_ms(started))
    }
}

/// Verify HTTP through the newly started inbound. The explicit proxy and
/// disabled redirects prevent fallback to environment proxies or another URL.
pub async fn verify_inbound(port: u16, url: &str) -> Result<(), EngineError> {
    verify_route(Some(port), url).await
}

/// Probe after TUN takeover without any explicit HTTP/environment proxy.
pub async fn verify_tun_route(url: &str) -> Result<(), EngineError> {
    verify_route(None, url).await
}

async fn verify_route(port: Option<u16>, url: &str) -> Result<(), EngineError> {
    let network_error = |_: reqwest::Error| {
        EngineError::Network("经本地代理的连通性检查失败，请检查网络或更换节点".to_owned())
    };
    let mut client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5));
    if let Some(port) = port {
        let proxy =
            reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).map_err(network_error)?;
        client = client.proxy(proxy);
    }
    let client = client.build().map_err(network_error)?;
    let status = client
        .get(url)
        .send()
        .await
        .map_err(network_error)?
        .status();
    if status.is_success() {
        Ok(())
    } else {
        Err(EngineError::Network(format!(
            "代理连通性检查未通过（HTTP {status}）"
        )))
    }
}

impl Default for LatencyProbe {
    fn default() -> Self {
        Self::new()
    }
}

/// Measure a TCP handshake. Resolution time is included: a node whose hostname
/// takes two seconds to resolve is slow, and hiding that would be dishonest.
pub async fn tcp_latency(host: &str, port: u16, timeout: Duration) -> Result<u32, EngineError> {
    let started = Instant::now();
    let address = format!("{host}:{port}");
    let resolved = tokio::task::spawn_blocking(move || {
        address
            .to_socket_addrs()
            .map(|mut addrs| addrs.next())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| EngineError::Network(error.to_string()))?;
    let socket = resolved.map_err(EngineError::Network)?;
    let socket = socket.ok_or_else(|| EngineError::Network(format!("{host} did not resolve")))?;
    tokio::time::timeout(timeout, tokio::net::TcpStream::connect(socket))
        .await
        .map_err(|_| EngineError::Timeout(timeout))?
        .map_err(|error| EngineError::Network(error.to_string()))?;
    Ok(elapsed_ms(started))
}

fn elapsed_ms(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn tcp_latency_measures_a_listening_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            loop {
                if listener.accept().await.is_err() {
                    return;
                }
            }
        });
        let latency = tcp_latency("127.0.0.1", port, Duration::from_secs(2))
            .await
            .expect("latency");
        assert!(latency < 2_000);
    }

    #[tokio::test]
    async fn a_closed_port_reports_a_connection_failure() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let error = tcp_latency("127.0.0.1", port, Duration::from_secs(1))
            .await
            .expect_err("must fail");
        assert!(matches!(
            error,
            EngineError::Network(_) | EngineError::Timeout(_)
        ));
    }

    #[tokio::test]
    async fn an_unresolvable_host_is_a_network_error() {
        let error = tcp_latency("does-not-exist.invalid", 443, Duration::from_secs(1))
            .await
            .expect_err("must fail");
        assert!(matches!(error, EngineError::Network(_)));
    }

    #[tokio::test]
    async fn through_kernel_without_a_kernel_is_refused() {
        let probe = LatencyProbe::new();
        let error = probe
            .measure(&LatencyTarget::ThroughKernel {
                proxy_tag: "proxy".into(),
                url: "https://example.com".into(),
            })
            .await
            .expect_err("must fail");
        assert!(matches!(error, EngineError::Network(_)));
    }
}
