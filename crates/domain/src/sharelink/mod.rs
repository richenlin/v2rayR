//! Share-link parsing.
//!
//! One entry point, [`parse_share_link`], plus [`parse_share_links`] for the
//! multi-line / base64 blobs that subscription endpoints return. Protocol
//! parsers stay in their own modules so a new scheme is one file and one match
//! arm.

mod anytls;
pub mod export;
mod hysteria2;
mod naive;
mod socks;
mod ss;
mod trojan;
mod tuic;
mod uri;
mod vless;
mod vmess;
mod wireguard;

pub use export::export_share_link;
pub use uri::Uri;

use crate::error::{DomainError, ErrorCode};
use crate::fingerprint;
use crate::model::{Profile, TlsOptions, Transport};

/// One link that could not be imported, kept so the UI can show *why* instead
/// of silently dropping rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFailure {
    /// The offending line, truncated to a safe length and stripped of anything
    /// after `://` (credentials never reach the UI).
    pub source: String,
    pub error: DomainError,
}

impl LinkFailure {
    pub fn new(source: &str, error: DomainError) -> Self {
        Self {
            source: redact(source),
            error,
        }
    }
}

fn redact(line: &str) -> String {
    let line = line.trim();
    let truncated: String = line.chars().take(48).collect();
    match truncated.split_once("://") {
        Some((scheme, _)) => format!("{scheme}://…"),
        None => truncated,
    }
}

#[derive(Debug, Clone, Default)]
pub struct ImportOutcome {
    pub profiles: Vec<Profile>,
    pub failures: Vec<LinkFailure>,
    /// `http(s)://` lines are subscriptions, not profiles. The caller decides
    /// whether to add them.
    pub subscription_urls: Vec<String>,
}

impl ImportOutcome {
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty() && self.subscription_urls.is_empty()
    }

    /// Drops exact duplicates (by connection fingerprint) while keeping the
    /// first occurrence, which is the one the user pasted deliberately.
    pub fn dedupe(self, salt: &[u8]) -> ImportPreview {
        let mut seen = std::collections::HashSet::new();
        let mut profiles = Vec::with_capacity(self.profiles.len());
        let mut duplicates = 0usize;
        for profile in self.profiles {
            let digest = fingerprint::fingerprint(&profile, salt);
            if seen.insert(digest) {
                profiles.push(profile);
            } else {
                duplicates += 1;
            }
        }
        ImportPreview {
            profiles,
            duplicates,
            failures: self.failures,
            subscription_urls: self.subscription_urls,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ImportPreview {
    pub profiles: Vec<Profile>,
    pub duplicates: usize,
    pub failures: Vec<LinkFailure>,
    pub subscription_urls: Vec<String>,
}

/// Parses a single share link.
pub fn parse_share_link(input: &str) -> Result<Profile, DomainError> {
    let link = input.trim().trim_start_matches('\u{feff}');
    if link.is_empty() {
        return Err(DomainError::new(ErrorCode::EmptyInput));
    }
    let lower = link.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Err(DomainError::new(ErrorCode::NotAProfileLink).with_detail("http"));
    }
    let scheme = lower
        .split_once("://")
        .map(|(scheme, _)| scheme)
        .ok_or_else(|| DomainError::new(ErrorCode::InvalidUrl).with_detail("missing scheme"))?;

    match scheme {
        "vmess" => vmess::parse(link),
        "vless" => vless::parse(link),
        "trojan" | "trojan-go" => trojan::parse(link),
        "ss" | "shadowsocks" => ss::parse(link),
        "socks" | "socks5" | "socks4" => socks::parse(link),
        "http-proxy" | "https-proxy" => {
            let uri = Uri::split(link)?;
            if !uri.path.is_empty() && uri.path != "/" {
                return Err(invalid("path", "HTTP 代理地址不能包含路径"));
            }
            let (username, password) = uri.user_pass();
            let tls = uri.scheme == "https-proxy";
            let mut profile = Profile::new(
                remark_of(&uri, "http"),
                uri.host.clone(),
                uri.port_or(if tls { 443 } else { 80 }),
                crate::model::Protocol::Http {
                    username,
                    password: password.map(crate::model::Secret::new),
                },
            );
            if tls {
                profile.tls = Some(tls_from(&uri, true).unwrap_or_default());
            }
            profile.validate()?;
            Ok(profile)
        }
        "hysteria2" | "hy2" => hysteria2::parse(link),
        "tuic" => tuic::parse(link),
        "wireguard" | "wg" => wireguard::parse(link),
        "anytls" => anytls::parse(link),
        other if other == "naive" || other.starts_with("naive+") => naive::parse(link),
        other => Err(DomainError::new(ErrorCode::UnsupportedScheme).with_detail(other)),
    }
}

/// Parses a newline separated list of links, automatically unwrapping a
/// base64-encoded blob (the payload format most subscription providers use).
pub fn parse_share_links(text: &str) -> ImportOutcome {
    let mut outcome = ImportOutcome::default();
    let text = text.trim().trim_start_matches('\u{feff}');

    // A whole-blob base64 payload, possibly without newlines.
    let expanded = if !text.contains("://") {
        crate::base64::decode_str(text)
            .filter(|decoded| decoded.contains("://"))
            .unwrap_or_else(|| text.to_owned())
    } else {
        text.to_owned()
    };

    for line in expanded.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        if line.starts_with("http://") || line.starts_with("https://") {
            if !outcome.subscription_urls.iter().any(|url| url == line) {
                outcome.subscription_urls.push(line.to_owned());
            }
            continue;
        }
        match parse_share_link(line) {
            Ok(profile) => outcome.profiles.push(profile),
            Err(error) => outcome.failures.push(LinkFailure::new(line, error)),
        }
    }

    outcome
}

// ---------------------------------------------------------------------------
// Helpers shared by the per-protocol parsers
// ---------------------------------------------------------------------------

/// Maps `type=` / `net=` plus `headerType=` onto the transport enum.
pub(crate) fn transport_from(
    uri: &Uri,
    net: &str,
    header_type: &str,
) -> Result<Transport, DomainError> {
    let path = uri
        .param(&["path", "wspath"])
        .map(str::to_owned)
        .unwrap_or_else(|| "/".to_owned());
    let host = uri
        .param(&["host", "obfsParam"])
        .map(str::to_owned)
        .filter(|host| !host.is_empty());

    let transport = match net.trim().to_ascii_lowercase().as_str() {
        "" | "tcp" | "raw" | "none" => {
            if header_type.eq_ignore_ascii_case("http") {
                Transport::Http {
                    path,
                    hosts: host.into_iter().collect(),
                }
            } else {
                Transport::Raw
            }
        }
        "ws" | "websocket" => Transport::Ws {
            path,
            host,
            early_data: uri
                .param(&["ed", "eh", "maxEarlyData"])
                .and_then(|value| value.parse::<u32>().ok()),
        },
        "grpc" | "gun" => {
            let service_name = uri
                .param(&["serviceName", "service", "path"])
                .map(|value| value.trim_start_matches('/').to_owned())
                .unwrap_or_default();
            Transport::Grpc {
                service_name,
                // `mode=multi` selects gRPC multi-stream mode; anything else
                // (including `gun`) is treated as plain gRPC.
                multi_mode: uri
                    .param(&["mode"])
                    .is_some_and(|mode| mode.eq_ignore_ascii_case("multi")),
            }
        }
        "http" | "h2" => Transport::Http {
            path,
            hosts: host
                .map(|host| {
                    host.split(',')
                        .map(|part| part.trim().to_owned())
                        .filter(|part| !part.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        },
        "httpupgrade" => Transport::HttpUpgrade { path, host },
        "xhttp" | "splithttp" => Transport::Xhttp {
            path,
            host,
            mode: uri
                .param(&["mode"])
                .map(|mode| mode.to_ascii_lowercase())
                .unwrap_or_else(|| "auto".to_owned()),
            extra: serde_json::Value::Null,
        },
        "kcp" | "mkcp" => Transport::Mkcp {
            header: crate::model::MkcpHeader::parse(header_type),
            seed: uri.param(&["seed"]).map(str::to_owned),
        },
        "quic" => Transport::Quic {
            security: crate::model::QuicSecurity::parse(
                uri.param(&["quicSecurity", "security"]).unwrap_or("none"),
            ),
            key: crate::model::Secret::new(uri.param(&["key"]).unwrap_or_default()),
            header: uri.param(&["headerType"]).unwrap_or_default().to_owned(),
        },
        other => {
            return Err(DomainError::new(ErrorCode::UnsupportedTransport).with_detail(other));
        }
    };
    Ok(transport)
}

/// Builds [`TlsOptions`] from `security=` and friends.
///
/// `default_tls` is true for protocols where TLS is the norm (Trojan, Naive,
/// Hysteria2, TUIC): a link without `security=` still speaks TLS.
pub(crate) fn tls_from(uri: &Uri, default_tls: bool) -> Option<TlsOptions> {
    let security = uri
        .param(&["security", "tls", "encryption"])
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    let enabled = match security.as_str() {
        "none" | "false" | "0" => false,
        "tls" | "xtls" | "reality" => true,
        "" => default_tls,
        // Unknown security values are treated as TLS so that a typo degrades to
        // a normal TLS connection instead of a plaintext one.
        _ => true,
    };
    if !enabled {
        return None;
    }

    let sni = uri
        .param(&["sni", "peer", "serverName", "host"])
        .map(str::to_owned)
        .filter(|value| !value.is_empty());
    let alpn = uri
        .param(&["alpn"])
        .map(|value| {
            value
                .split(',')
                .map(|part| part.trim().to_owned())
                .filter(|part| !part.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let reality_key = uri
        .param(&["pbk", "publicKey", "public-key"])
        .map(str::to_owned)
        .filter(|value| !value.is_empty());

    let reality = reality_key.map(|public_key| crate::model::RealityOptions {
        public_key,
        short_id: uri
            .param(&["sid", "shortId", "short-id"])
            .unwrap_or_default()
            .to_owned(),
        spider_x: uri.param(&["spx", "spiderX"]).map(str::to_owned),
    });

    Some(TlsOptions {
        sni,
        insecure: uri.flag(&["allowInsecure", "insecure", "skip-cert-verify"]),
        alpn,
        fingerprint: uri
            .param(&["fp", "fingerprint", "client-fingerprint"])
            .map(str::to_owned)
            .filter(|value| !value.is_empty()),
        reality,
        ech: None,
    })
}

pub(crate) fn remark_of(uri: &Uri, protocol: &str) -> String {
    uri.remark()
        .unwrap_or_else(|| crate::model::derive_remark(protocol, &uri.host, uri.port_or(0)))
}

fn missing(field: &str) -> DomainError {
    DomainError::new(ErrorCode::MissingField).with_detail(field)
}

fn invalid(field: &str, value: &str) -> DomainError {
    DomainError::new(ErrorCode::InvalidValue).with_detail(format!("{field}={value}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn dispatches_every_supported_scheme() {
        let links = [
            "vless://11111111-1111-1111-1111-111111111111@a.com:443?security=tls#vless",
            "trojan://pw@a.com:443#trojan",
            "ss://YWVzLTI1Ni1nY206cHc=@a.com:8388#ss",
            "socks://dXNlcjpwdw==@a.com:1080#socks",
            "hysteria2://pw@a.com:443#hy2",
            "tuic://11111111-1111-1111-1111-111111111111:pw@a.com:443#tuic",
            "anytls://pw@a.com:443#anytls",
            "naive+https://user:pw@a.com:443#naive",
            "wireguard://cHJpdmF0ZQ==@a.com:51820?publickey=cHVibGlj&address=10.0.0.2/32#wg",
        ];
        for link in links {
            let profile = parse_share_link(link).unwrap_or_else(|error| panic!("{link}: {error}"));
            profile
                .validate()
                .unwrap_or_else(|error| panic!("{link}: {error}"));
        }
    }

    #[test]
    fn http_links_are_reported_as_subscriptions() {
        let error = parse_share_link("https://example.com/sub").expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::NotAProfileLink);
    }

    #[test]
    fn explicit_http_proxy_link_round_trips_without_becoming_a_subscription() {
        let input = "https-proxy://alice:p%40ss@[2001:db8::1]:8443?sni=proxy.example#HTTP";
        let imported = parse_share_links(input);
        assert!(imported.failures.is_empty());
        assert!(imported.subscription_urls.is_empty());
        let profile = &imported.profiles[0];
        assert!(matches!(
            &profile.protocol,
            crate::model::Protocol::Http { .. }
        ));
        let exported = export_share_link(profile).unwrap();
        let round_trip = parse_share_link(&exported).unwrap();
        assert_eq!(round_trip.protocol, profile.protocol);
        assert_eq!(round_trip.tls, profile.tls);
        assert_eq!(
            (round_trip.address, round_trip.port),
            (profile.address.clone(), profile.port)
        );
    }

    #[test]
    fn unknown_scheme_is_reported() {
        let error = parse_share_link("ssr://whatever").expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::UnsupportedScheme);
        assert_eq!(error.detail(), Some("ssr"));
    }

    #[test]
    fn failures_are_recorded_and_redacted() {
        let outcome = parse_share_links(
            "vless://11111111-1111-1111-1111-111111111111@a.com:443#ok\nssr://secret-password\nhttps://sub.example/x\n",
        );
        assert_eq!(outcome.profiles.len(), 1);
        assert_eq!(outcome.failures.len(), 1);
        assert!(!outcome.failures[0].source.contains("secret"));
        assert_eq!(
            outcome.subscription_urls,
            vec!["https://sub.example/x".to_owned()]
        );
    }

    #[test]
    fn base64_wrapped_subscription_is_unwrapped() {
        let payload =
            "vless://11111111-1111-1111-1111-111111111111@a.com:443#a\ntrojan://pw@b.com:443#b\n";
        let encoded = crate::base64::encode(payload.as_bytes());
        let outcome = parse_share_links(&encoded);
        assert_eq!(outcome.profiles.len(), 2);
        assert!(outcome.failures.is_empty());
    }

    #[test]
    fn dedupe_keeps_first_and_counts_rest() {
        let outcome = parse_share_links("trojan://pw@a.com:443#one\ntrojan://pw@a.com:443#two\n");
        let preview = outcome.dedupe(b"salt");
        assert_eq!(preview.profiles.len(), 1);
        assert_eq!(preview.duplicates, 1);
        // First occurrence wins: it is the one the user pasted on purpose.
        assert_eq!(preview.profiles[0].remark, "one");
    }

    #[test]
    fn vless_flow_and_reality_survive() {
        let profile = parse_share_link(
            "vless://11111111-1111-1111-1111-111111111111@a.com:443?type=ws&path=%2Fws&security=reality&pbk=PUBKEY&sid=abcd&sni=www.example.com&fp=chrome&flow=xtls-rprx-vision#r",
        )
        .expect("parse");
        assert_eq!(profile.protocol_kind(), "vless");
        let tls = profile.tls.expect("tls");
        assert_eq!(tls.reality.expect("reality").public_key, "PUBKEY");
        assert_eq!(tls.fingerprint.as_deref(), Some("chrome"));
        match profile.protocol {
            crate::model::Protocol::Vless { flow, id, .. } => {
                assert_eq!(flow, Some(crate::model::VlessFlow::XtlsRprxVision));
                assert_eq!(id, Uuid::from_u128(0x11111111_1111_1111_1111_111111111111));
            }
            other => panic!("unexpected protocol: {other:?}"),
        }
        match profile.transport {
            Transport::Ws { path, .. } => assert_eq!(path, "/ws"),
            other => panic!("unexpected transport: {other:?}"),
        }
    }

    #[test]
    fn vmess_json_security_defaults_to_auto() {
        let json = serde_json::json!({
            "v": "2",
            "ps": "node",
            "add": "a.com",
            "port": "443",
            "id": "11111111-1111-1111-1111-111111111111",
            "aid": "0",
            "net": "ws",
            "path": "/v",
            "host": "cdn.example.com",
            "tls": "tls",
            "sni": "cdn.example.com"
        });
        let link = format!(
            "vmess://{}",
            crate::base64::encode(json.to_string().as_bytes())
        );
        let profile = parse_share_link(&link).expect("parse");
        match profile.protocol {
            crate::model::Protocol::Vmess { security, .. } => {
                assert_eq!(security, crate::model::VmessSecurity::Auto)
            }
            other => panic!("unexpected protocol: {other:?}"),
        }
        assert_eq!(profile.remark, "node");
    }
}
