//! The sing-box / mihomo control API (Clash-compatible).
//!
//! sing-box always exposes `clash_api`, which is how the connection page gets
//! live speeds and how "test this node through the running kernel" works. Xray
//! has no such API, so the caller falls back to a local TCP probe.

use std::time::Duration;

use serde_json::Value;
use v2rayr_domain::CoreKind;

use crate::EngineError;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClashSample {
    pub down_bps: u64,
    pub up_bps: u64,
    pub connections: u64,
}

pub struct ClashClient {
    base: String,
    secret: String,
    http: reqwest::Client,
}

impl ClashClient {
    pub fn new(port: u16, secret: impl Into<String>) -> Self {
        Self {
            base: format!("http://127.0.0.1:{port}"),
            secret: secret.into(),
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .expect("valid loopback HTTP client"),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let builder = self.http.request(method, format!("{}{path}", self.base));
        if self.secret.is_empty() {
            builder
        } else {
            builder.bearer_auth(&self.secret)
        }
    }

    /// One `/traffic` frame. The endpoint is a stream, so we read a single line
    /// and drop the connection: at one sample per second that is cheaper than
    /// keeping a streaming body alive across the whole connection.
    pub async fn sample(&self) -> Result<ClashSample, EngineError> {
        let response = self
            .request(reqwest::Method::GET, "/traffic")
            .send()
            .await
            .map_err(|error| EngineError::ControlApi(error.to_string()))?;
        if !response.status().is_success() {
            return Err(EngineError::ControlApi(format!(
                "traffic endpoint returned {}",
                response.status()
            )));
        }
        let mut response = response;
        let mut buffer = String::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| EngineError::ControlApi(error.to_string()))?
        {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            if let Some(line) = buffer.lines().find(|line| !line.trim().is_empty()) {
                let value: Value = serde_json::from_str(line.trim())
                    .map_err(|error| EngineError::ControlApi(error.to_string()))?;
                let down = value.get("down").and_then(Value::as_u64).unwrap_or(0);
                let up = value.get("up").and_then(Value::as_u64).unwrap_or(0);
                let connections = self.connections().await.unwrap_or(0);
                return Ok(ClashSample {
                    down_bps: down,
                    up_bps: up,
                    connections,
                });
            }
        }
        Err(EngineError::ControlApi("traffic stream closed".to_owned()))
    }

    pub async fn connections(&self) -> Result<u64, EngineError> {
        let response = self
            .request(reqwest::Method::GET, "/connections")
            .send()
            .await
            .map_err(|error| EngineError::ControlApi(error.to_string()))?;
        let value: Value = response
            .json()
            .await
            .map_err(|error| EngineError::ControlApi(error.to_string()))?;
        Ok(value
            .get("connections")
            .and_then(Value::as_array)
            .map(|items| items.len() as u64)
            .unwrap_or(0))
    }

    /// Latency of one outbound as measured *through* the kernel.
    pub async fn delay(
        &self,
        proxy_tag: &str,
        test_url: &str,
        timeout_ms: u32,
    ) -> Result<u32, EngineError> {
        let path = format!(
            "/proxies/{}/delay?url={}&timeout={timeout_ms}",
            urlencode(proxy_tag),
            urlencode(test_url)
        );
        let response = self
            .request(reqwest::Method::GET, &path)
            .timeout(Duration::from_millis(timeout_ms as u64 + 1_500))
            .send()
            .await
            .map_err(|error| EngineError::ControlApi(error.to_string()))?;
        let status = response.status();
        let value: Value = response
            .json()
            .await
            .map_err(|error| EngineError::ControlApi(error.to_string()))?;
        if !status.is_success() {
            return Err(EngineError::Network(
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("delay test failed")
                    .to_owned(),
            ));
        }
        value
            .get("delay")
            .and_then(Value::as_u64)
            .and_then(|delay| u32::try_from(delay).ok())
            .ok_or_else(|| EngineError::ControlApi("delay response has no delay".to_owned()))
    }

    pub async fn version(&self) -> Result<String, EngineError> {
        let response = self
            .request(reqwest::Method::GET, "/version")
            .send()
            .await
            .map_err(|error| EngineError::ControlApi(error.to_string()))?;
        let value: Value = response
            .json()
            .await
            .map_err(|error| EngineError::ControlApi(error.to_string()))?;
        Ok(value
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned())
    }

    /// Which kernels expose this API at all.
    pub fn available_for(core: CoreKind) -> bool {
        matches!(core, CoreKind::SingBox | CoreKind::Mihomo)
    }
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Minimal fake control API: answers one request per connection.
    async fn spawn_fake(script: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = [0u8; 2048];
                let _ = socket.read(&mut buffer).await;
                let request = String::from_utf8_lossy(&buffer).to_string();
                let body = if request.starts_with("GET /traffic") {
                    "{\"down\":2048,\"up\":512}\n"
                } else if request.starts_with("GET /connections") {
                    script
                } else if request.starts_with("GET /version") {
                    "{\"version\":\"1.11.3\",\"meta\":true}"
                } else if request.contains("/delay") {
                    "{\"delay\":42}"
                } else {
                    "{}"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        port
    }

    #[tokio::test]
    async fn sample_reads_rates_and_connection_count() {
        let port = spawn_fake(r#"{"connections":[{"id":"1"},{"id":"2"}]}"#).await;
        let client = ClashClient::new(port, "secret");
        let sample = client.sample().await.expect("sample");
        assert_eq!(sample.down_bps, 2048);
        assert_eq!(sample.up_bps, 512);
        assert_eq!(sample.connections, 2);
    }

    #[tokio::test]
    async fn delay_is_parsed() {
        let port = spawn_fake("{}").await;
        let client = ClashClient::new(port, "");
        assert_eq!(
            client
                .delay("proxy", "https://example.com", 5000)
                .await
                .expect("delay"),
            42
        );
    }

    #[tokio::test]
    async fn version_is_parsed() {
        let port = spawn_fake("{}").await;
        let client = ClashClient::new(port, "");
        assert_eq!(client.version().await.expect("version"), "1.11.3");
    }

    #[tokio::test]
    async fn a_closed_port_is_a_control_api_error() {
        // Bind and immediately drop to get a port nobody listens on.
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let client = ClashClient::new(port, "");
        let error = client.sample().await.expect_err("must fail");
        assert!(matches!(error, EngineError::ControlApi(_)));
    }

    #[test]
    fn only_singbox_and_mihomo_expose_the_api() {
        assert!(ClashClient::available_for(CoreKind::SingBox));
        assert!(ClashClient::available_for(CoreKind::Mihomo));
        assert!(!ClashClient::available_for(CoreKind::Xray));
    }

    #[test]
    fn urlencode_escapes_proxy_tags() {
        assert_eq!(urlencode("a/b c"), "a%2Fb%20c");
        assert_eq!(urlencode("plain-tag_1.0"), "plain-tag_1.0");
    }
}
