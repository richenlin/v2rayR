//! Subscription payload detection.
//!
//! Providers serve four different formats from the same `http(s)://` URL, often
//! without a meaningful `Content-Type`. The sniffing order below is deliberate:
//! JSON first (cheap and unambiguous), then Clash YAML (`proxies:` cannot occur
//! inside base64 because `:` is not in the alphabet), then plain links, then a
//! base64-wrapped link list as the last resort.

use serde_json::Value;

use crate::error::{DomainError, ErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionFormat {
    /// One share link per line (or space separated).
    Links,
    /// Base64 of a link list.
    Base64Links,
    /// Clash / mihomo YAML with a `proxies:` block.
    Clash,
    /// sing-box JSON (`outbounds`).
    SingBox,
    /// SIP008 JSON (`servers`).
    Sip008,
    /// Recognised as a subscription but not as anything we can import yet.
    Unknown,
}

impl SubscriptionFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Links => "links",
            Self::Base64Links => "base64_links",
            Self::Clash => "clash",
            Self::SingBox => "sing-box",
            Self::Sip008 => "sip008",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Payload {
    Links(Vec<String>),
    Clash { yaml: String },
    SingBox(Value),
    Sip008(Value),
    Unknown { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SniffedSubscription {
    pub format: SubscriptionFormat,
    pub payload: Payload,
}

impl SniffedSubscription {
    /// Number of nodes the payload claims to carry, for the UI preview.
    pub fn node_count(&self) -> usize {
        match &self.payload {
            Payload::Links(links) => links.len(),
            Payload::SingBox(value) => value
                .get("outbounds")
                .and_then(Value::as_array)
                .map(|outbounds| outbounds.len())
                .unwrap_or(0),
            Payload::Sip008(value) => value
                .get("servers")
                .and_then(Value::as_array)
                .map(|servers| servers.len())
                .unwrap_or(0),
            Payload::Clash { .. } | Payload::Unknown { .. } => 0,
        }
    }
}

/// Detects the format of a subscription response body.
pub fn sniff_subscription(body: &str) -> SniffedSubscription {
    let body = body.trim().trim_start_matches('\u{feff}');

    if body.is_empty() {
        return SniffedSubscription {
            format: SubscriptionFormat::Unknown,
            payload: Payload::Unknown {
                reason: ErrorCode::EmptyInput.to_string(),
            },
        };
    }

    if body.starts_with('{') || body.starts_with('[') {
        if let Ok(value) = serde_json::from_str::<Value>(body) {
            return from_json(value);
        }
    }

    if looks_like_clash(body) {
        return SniffedSubscription {
            format: SubscriptionFormat::Clash,
            payload: Payload::Clash {
                yaml: body.to_owned(),
            },
        };
    }

    if body.contains("://") {
        let links = collect_links(body);
        if !links.is_empty() {
            return SniffedSubscription {
                format: SubscriptionFormat::Links,
                payload: Payload::Links(links),
            };
        }
    }

    if let Some(decoded) = crate::base64::decode_str(body) {
        let decoded = decoded.trim();
        if decoded.contains("://") {
            let links = collect_links(decoded);
            if !links.is_empty() {
                return SniffedSubscription {
                    format: SubscriptionFormat::Base64Links,
                    payload: Payload::Links(links),
                };
            }
        }
        if decoded.starts_with('{') || decoded.starts_with('[') {
            if let Ok(value) = serde_json::from_str::<Value>(decoded) {
                return from_json(value);
            }
        }
    }

    SniffedSubscription {
        format: SubscriptionFormat::Unknown,
        payload: Payload::Unknown {
            reason: DomainError::new(ErrorCode::UnsupportedFormat).to_string(),
        },
    }
}

fn from_json(value: Value) -> SniffedSubscription {
    let is_singbox = value.get("outbounds").map(Value::is_array).unwrap_or(false);
    let is_sip008 = value.get("servers").map(Value::is_array).unwrap_or(false);
    match (is_singbox, is_sip008) {
        (true, _) => SniffedSubscription {
            format: SubscriptionFormat::SingBox,
            payload: Payload::SingBox(value),
        },
        (_, true) => SniffedSubscription {
            format: SubscriptionFormat::Sip008,
            payload: Payload::Sip008(value),
        },
        _ => SniffedSubscription {
            format: SubscriptionFormat::Unknown,
            payload: Payload::Unknown {
                reason: "json without outbounds or servers".to_owned(),
            },
        },
    }
}

fn looks_like_clash(body: &str) -> bool {
    body.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("proxies:") || trimmed.starts_with("proxy-providers:")
    })
}

fn collect_links(body: &str) -> Vec<String> {
    body.split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|token| token.contains("://") && !token.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// `subscription-userinfo: upload=123; download=456; total=789; expire=1700000000`
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubscriptionUserInfo {
    pub upload: u64,
    pub download: u64,
    pub total: u64,
    pub expire: Option<i64>,
}

impl SubscriptionUserInfo {
    pub fn parse(header: &str) -> Option<Self> {
        let mut info = Self::default();
        let mut seen = false;
        for part in header.split(';') {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "upload" => {
                    info.upload = value.parse().unwrap_or(0);
                    seen = true;
                }
                "download" => {
                    info.download = value.parse().unwrap_or(0);
                    seen = true;
                }
                "total" => {
                    info.total = value.parse().unwrap_or(0);
                    seen = true;
                }
                "expire" => {
                    info.expire = value.parse().ok().filter(|expire| *expire > 0);
                    seen = true;
                }
                _ => {}
            }
        }
        seen.then_some(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_plain_links() {
        let sniffed = sniff_subscription("vless://a@b:443#x\ntrojan://p@c:443#y\n");
        assert_eq!(sniffed.format, SubscriptionFormat::Links);
        assert_eq!(sniffed.node_count(), 2);
    }

    #[test]
    fn detects_base64_links() {
        let payload = crate::base64::encode(b"vless://a@b:443#x");
        let sniffed = sniff_subscription(&payload);
        assert_eq!(sniffed.format, SubscriptionFormat::Base64Links);
        assert_eq!(sniffed.node_count(), 1);
    }

    #[test]
    fn detects_clash_before_links() {
        let yaml = "port: 7890\nproxies:\n  - name: a\n    type: vmess\n    server: b\n";
        let sniffed = sniff_subscription(yaml);
        assert_eq!(sniffed.format, SubscriptionFormat::Clash);
    }

    #[test]
    fn detects_singbox_json() {
        let json = serde_json::json!({"outbounds": [{"type": "direct", "tag": "direct"}]});
        let sniffed = sniff_subscription(&json.to_string());
        assert_eq!(sniffed.format, SubscriptionFormat::SingBox);
        assert_eq!(sniffed.node_count(), 1);
    }

    #[test]
    fn detects_sip008_json() {
        let json = serde_json::json!({"version": 1, "servers": [{"id": "x"}]});
        let sniffed = sniff_subscription(&json.to_string());
        assert_eq!(sniffed.format, SubscriptionFormat::Sip008);
    }

    #[test]
    fn unknown_payload_is_reported_not_guessed() {
        let sniffed = sniff_subscription("hello world");
        assert_eq!(sniffed.format, SubscriptionFormat::Unknown);
        assert!(matches!(sniffed.payload, Payload::Unknown { .. }));
    }

    #[test]
    fn parses_userinfo_header() {
        let info = SubscriptionUserInfo::parse("upload=1; download=2; total=3; expire=1700000000")
            .expect("parse");
        assert_eq!(info.upload, 1);
        assert_eq!(info.expire, Some(1_700_000_000));
        assert!(SubscriptionUserInfo::parse("garbage").is_none());
    }
}
