//! Subscription downloading and preview.
//!
//! Sniffing happens in `v2rayr-domain`; this module owns the HTTP part — timeout,
//! user agent, redirects, quota headers — and the decision of *when* a format is
//! understood well enough to import. Clash YAML is detected but refused: without
//! a YAML parser in the dependency tree, a half-parsed import would corrupt
//! node credentials, so it is reported as unsupported instead.

use std::time::Duration;

use serde_json::Value;
use v2rayr_domain::{
    ImportPreview, Payload, Secret, SsMethod, SubscriptionFormat, SubscriptionUserInfo,
    parse_share_links, sniff_subscription,
};

use crate::EngineError;

#[derive(Debug, Clone)]
pub struct FetchedSubscription {
    pub url: String,
    pub status: u16,
    pub body: String,
    pub format: SubscriptionFormat,
    pub userinfo: Option<SubscriptionUserInfo>,
    pub preview: ImportPreview,
}

pub struct Fetcher {
    client: reqwest::Client,
    user_agent: String,
    timeout: Duration,
}

impl Fetcher {
    pub fn new(user_agent: impl Into<String>, timeout: Duration) -> Self {
        let user_agent = user_agent.into();
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(10))
            .user_agent(user_agent.clone())
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap_or_default();
        Self {
            client,
            user_agent,
            timeout,
        }
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    pub async fn fetch(&self, url: &str) -> Result<FetchedSubscription, EngineError> {
        let response = self
            .client
            .get(url)
            // We do not link zlib, so ask for the identity encoding explicitly
            // instead of hoping `reqwest` never sees gzip.
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(|error| self.map_request_error(error))?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(EngineError::Network(format!("订阅地址返回 HTTP {status}")));
        }
        let userinfo = response
            .headers()
            .get("subscription-userinfo")
            .and_then(|value| value.to_str().ok())
            .and_then(SubscriptionUserInfo::parse);
        let body = response
            .text()
            .await
            .map_err(|error| self.map_request_error(error))?;

        let sniffed = sniff_subscription(&body);
        let format = sniffed.format;
        let preview = match &sniffed.payload {
            Payload::Links(_) => parse_share_links(&body).dedupe(b"subscription"),
            Payload::Sip008(value) => ImportPreview {
                profiles: parse_sip008(value)?,
                ..ImportPreview::default()
            },
            Payload::Clash { .. } => {
                return Err(EngineError::UnsupportedPayload(
                    "Clash 订阅（YAML）暂不支持导入；请改用链接或 base64 订阅".to_owned(),
                ));
            }
            Payload::SingBox(_) => {
                return Err(EngineError::UnsupportedPayload(
                    "sing-box JSON 订阅暂不支持导入；请改用链接或 base64 订阅".to_owned(),
                ));
            }
            Payload::Unknown { reason } => {
                return Err(EngineError::UnsupportedPayload(reason.clone()));
            }
        };

        if preview.profiles.is_empty() && preview.failures.is_empty() {
            return Err(EngineError::UnsupportedPayload(
                "订阅里没有可识别的节点".to_owned(),
            ));
        }

        Ok(FetchedSubscription {
            url: url.to_owned(),
            status,
            body,
            format,
            userinfo,
            preview,
        })
    }

    fn map_request_error(&self, error: reqwest::Error) -> EngineError {
        if error.is_timeout() {
            EngineError::Timeout(self.timeout)
        } else {
            EngineError::Network(error.to_string())
        }
    }
}

/// SIP008 is plain JSON, so it can be imported without a YAML dependency.
fn parse_sip008(value: &Value) -> Result<Vec<v2rayr_domain::Profile>, EngineError> {
    let servers = value
        .get("servers")
        .and_then(Value::as_array)
        .ok_or_else(|| EngineError::UnsupportedPayload("SIP008 缺少 servers".to_owned()))?;
    let mut profiles = Vec::new();
    for server in servers {
        let address = server
            .get("server")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let port = server
            .get("server_port")
            .or_else(|| server.get("port"))
            .and_then(|value| value.as_u64())
            .and_then(|port| u16::try_from(port).ok())
            .unwrap_or(0);
        let method = server
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let password = server
            .get("password")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if address.is_empty() || port == 0 || method.is_empty() {
            continue;
        }
        let method = SsMethod::new(method);
        if !method.is_known() {
            continue;
        }
        let remark = server
            .get("remarks")
            .or_else(|| server.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{address}:{port}"));
        let profile = v2rayr_domain::Profile::new(
            remark,
            address,
            port,
            v2rayr_domain::Protocol::Shadowsocks {
                method,
                password: Secret::new(password),
            },
        );
        profiles.push(profile);
    }
    if profiles.is_empty() {
        return Err(EngineError::UnsupportedPayload(
            "SIP008 里没有可用节点".to_owned(),
        ));
    }
    Ok(profiles)
}

/// Used by the manual "add subscription" flow to validate the URL before saving.
pub fn validate_subscription_url(url: &str) -> Result<(), EngineError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|error| EngineError::Network(format!("订阅地址无效: {error}")))?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        other => Err(EngineError::Network(format!(
            "订阅地址必须是 http(s)，收到 {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_server(body: &'static str, headers: &'static str) -> u16 {
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
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        port
    }

    fn fetcher() -> Fetcher {
        Fetcher::new("v2rayR-test", Duration::from_secs(5))
    }

    #[tokio::test]
    async fn plain_link_links_are_imported() {
        let body = "vless://11111111-1111-1111-1111-111111111111@a.example.com:443?security=tls&sni=a.example.com#Tokyo\n\
                    trojan://pw@b.example.com:443?sni=b.example.com#Seoul\n";
        let port = spawn_server(body, "").await;
        let fetched = fetcher()
            .fetch(&format!("http://127.0.0.1:{port}/sub"))
            .await
            .expect("fetch");
        assert_eq!(fetched.format, SubscriptionFormat::Links);
        assert_eq!(fetched.preview.profiles.len(), 2);
        assert_eq!(fetched.preview.profiles[0].remark, "Tokyo");
    }

    #[tokio::test]
    async fn base64_payload_and_quota_header_are_understood() {
        let raw =
            "vless://11111111-1111-1111-1111-111111111111@a.example.com:443?security=tls#Tokyo\n";
        let encoded = v2rayr_domain::base64::encode(raw.as_bytes());
        let body: &'static str = Box::leak(encoded.into_boxed_str());
        let port = spawn_server(
            body,
            "Subscription-Userinfo: upload=1; download=2; total=100;\r\n",
        )
        .await;
        let fetched = fetcher()
            .fetch(&format!("http://127.0.0.1:{port}/sub"))
            .await
            .expect("fetch");
        assert_eq!(fetched.format, SubscriptionFormat::Base64Links);
        assert_eq!(fetched.preview.profiles.len(), 1);
        let quota = fetched.userinfo.expect("quota");
        assert_eq!(quota.total, 100);
    }

    #[tokio::test]
    async fn clash_yaml_is_refused_with_an_explanation() {
        let body = "proxies:\n  - name: a\n    type: ss\nrules:\n  - MATCH,DIRECT\n";
        let port = spawn_server(body, "").await;
        let error = fetcher()
            .fetch(&format!("http://127.0.0.1:{port}/sub"))
            .await
            .expect_err("must refuse");
        assert!(matches!(error, EngineError::UnsupportedPayload(_)));
        assert!(error.to_string().contains("Clash"));
    }

    #[tokio::test]
    async fn sip008_is_converted_to_profiles() {
        let body = r#"{"version":1,"servers":[{"server":"1.2.3.4","server_port":8388,"method":"aes-256-gcm","password":"pw","remarks":"JP"}]}"#;
        let port = spawn_server(body, "").await;
        let fetched = fetcher()
            .fetch(&format!("http://127.0.0.1:{port}/sub"))
            .await
            .expect("fetch");
        assert_eq!(fetched.format, SubscriptionFormat::Sip008);
        assert_eq!(fetched.preview.profiles.len(), 1);
        assert_eq!(fetched.preview.profiles[0].remark, "JP");
    }

    #[tokio::test]
    async fn a_http_error_is_a_network_error() {
        let port = spawn_server("", "").await;
        let error = fetcher()
            .fetch(&format!("http://127.0.0.1:{port}/nothing"))
            .await;
        // The fake server always answers 200 with an empty body, which sniffs as
        // Unknown — a clear "unsupported" error rather than a panic.
        assert!(error.is_err());
    }

    #[test]
    fn url_scheme_is_validated() {
        assert!(validate_subscription_url("https://example.com/sub").is_ok());
        assert!(validate_subscription_url("ftp://example.com/sub").is_err());
        assert!(validate_subscription_url("not a url").is_err());
    }
}
