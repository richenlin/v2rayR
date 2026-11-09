//! `vmess://` links.
//!
//! Two mutually exclusive encodings exist in the wild:
//!
//! * the classic base64-wrapped JSON object (`vmess://eyJ2IjoiMiIs...`), and
//! * the "standard URI" form `vmess://<uuid>@host:port?type=ws&security=tls`.
//!
//! They are told apart by looking for an `@` before the fragment; base64 never
//! contains one.

use serde::Deserialize;
use uuid::Uuid;

use super::{Uri, invalid, missing, remark_of, tls_from, transport_from};
use crate::error::{DomainError, ErrorCode};
use crate::model::{
    Profile, Protocol, Secret, TlsOptions, Transport, VmessSecurity, de_u64_from_any,
};

pub fn parse(link: &str) -> Result<Profile, DomainError> {
    let body = link
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(|| DomainError::new(ErrorCode::InvalidUrl).with_detail("missing scheme"))?;
    let before_fragment = body.split('#').next().unwrap_or(body);

    if before_fragment.contains('@') {
        parse_standard_uri(link)
    } else {
        parse_json(body)
    }
}

fn parse_json(body: &str) -> Result<Profile, DomainError> {
    let decoded = crate::base64::decode_str(body)
        .ok_or_else(|| DomainError::new(ErrorCode::InvalidBase64).with_detail("vmess"))?;
    let payload: VmessJson = serde_json::from_str(decoded.trim()).map_err(|error| {
        DomainError::new(ErrorCode::InvalidEntry).with_detail(format!("vmess json: {error}"))
    })?;
    payload.into_profile()
}

fn parse_standard_uri(link: &str) -> Result<Profile, DomainError> {
    let uri = Uri::split(link)?;
    let id = uri
        .userinfo
        .as_deref()
        .ok_or_else(|| missing("id"))?
        .parse::<Uuid>()
        .map_err(|_| invalid("id", "not a uuid"))?;

    let net = uri.param(&["type", "net"]).unwrap_or_default();
    let header_type = uri.param(&["headerType"]).unwrap_or_default();
    let transport = transport_from(&uri, net, header_type)?;
    let tls = tls_from(&uri, false);

    let mut profile = Profile::new(
        remark_of(&uri, "vmess"),
        uri.host.clone(),
        uri.port.ok_or_else(|| missing("port"))?,
        Protocol::Vmess {
            id,
            alter_id: uri
                .param(&["aid", "alterId"])
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(0),
            security: VmessSecurity::parse(uri.param(&["scy", "encryption"]).unwrap_or("auto")),
        },
    );
    profile.transport = transport;
    profile.tls = tls;
    profile.validate()?;
    Ok(profile)
}

#[derive(Debug, Deserialize)]
struct VmessJson {
    #[serde(default)]
    ps: String,
    #[serde(default)]
    add: String,
    #[serde(default, deserialize_with = "de_u64_from_any")]
    port: u64,
    #[serde(default)]
    id: String,
    #[serde(default, deserialize_with = "opt_u64_from_any")]
    aid: Option<u64>,
    #[serde(default)]
    scy: Option<String>,
    #[serde(default)]
    net: Option<String>,
    #[serde(default, rename = "type")]
    header: Option<String>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    tls: Option<String>,
    #[serde(default)]
    sni: Option<String>,
    #[serde(default)]
    alpn: Option<String>,
    #[serde(default)]
    fp: Option<String>,
}

fn opt_u64_from_any<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Any {
        Null,
        Num(u64),
        Str(String),
    }
    match Any::deserialize(deserializer)? {
        Any::Null => Ok(None),
        Any::Num(value) => Ok(Some(value)),
        Any::Str(value) if value.trim().is_empty() => Ok(None),
        Any::Str(value) => value
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| serde::de::Error::custom("aid")),
    }
}

impl VmessJson {
    fn into_profile(self) -> Result<Profile, DomainError> {
        if self.add.trim().is_empty() {
            return Err(missing("add"));
        }
        let port = u16::try_from(self.port)
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| invalid("port", &self.port.to_string()))?;
        let id = self
            .id
            .parse::<Uuid>()
            .map_err(|_| invalid("id", "not a uuid"))?;

        let security = VmessSecurity::parse(self.scy.as_deref().unwrap_or("auto"));
        let header = self.header.as_deref().unwrap_or("none");
        let transport = build_transport(
            self.net.as_deref().unwrap_or("tcp"),
            header,
            self.path.as_deref(),
            self.host.as_deref(),
        )?;

        let tls = match self
            .tls
            .as_deref()
            .unwrap_or("none")
            .to_ascii_lowercase()
            .as_str()
        {
            "tls" | "reality" | "xtls" => Some(TlsOptions {
                sni: self.sni.or_else(|| self.host.clone()),
                alpn: self
                    .alpn
                    .map(|alpn| alpn.split(',').map(|part| part.trim().to_owned()).collect())
                    .unwrap_or_default(),
                fingerprint: self.fp.filter(|fp| !fp.is_empty()),
                ..TlsOptions::default()
            }),
            _ => None,
        };

        let remark = if self.ps.trim().is_empty() {
            crate::model::derive_remark("vmess", &self.add, port)
        } else {
            self.ps
        };

        let mut profile = Profile::new(
            remark,
            self.add,
            port,
            Protocol::Vmess {
                id,
                alter_id: self.aid.unwrap_or(0).min(u16::MAX as u64) as u16,
                security,
            },
        );
        profile.transport = transport;
        profile.tls = tls;
        // Keep the original cipher name so an export round-trips.
        if let Some(scy) = self.scy.filter(|scy| !scy.is_empty()) {
            profile.extra = serde_json::json!({ "scy": scy });
        }
        profile.validate()?;
        Ok(profile)
    }
}

fn build_transport(
    net: &str,
    header: &str,
    path: Option<&str>,
    host: Option<&str>,
) -> Result<Transport, DomainError> {
    let path = path
        .filter(|path| !path.is_empty())
        .unwrap_or("/")
        .to_owned();
    let host = host.map(str::to_owned).filter(|host| !host.is_empty());
    let transport = match net.to_ascii_lowercase().as_str() {
        "tcp" | "raw" | "" => {
            if header.eq_ignore_ascii_case("http") {
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
            early_data: None,
        },
        "grpc" => Transport::Grpc {
            service_name: path.trim_start_matches('/').to_owned(),
            multi_mode: false,
        },
        "h2" | "http" => Transport::Http {
            path,
            hosts: host.into_iter().collect(),
        },
        "httpupgrade" => Transport::HttpUpgrade { path, host },
        "xhttp" | "splithttp" => Transport::Xhttp {
            path,
            host,
            mode: "auto".to_owned(),
            extra: serde_json::Value::Null,
        },
        "kcp" | "mkcp" => Transport::Mkcp {
            header: crate::model::MkcpHeader::parse(header),
            seed: None,
        },
        "quic" => Transport::Quic {
            security: crate::model::QuicSecurity::None,
            key: Secret::default(),
            header: header.to_owned(),
        },
        other => return Err(DomainError::new(ErrorCode::UnsupportedTransport).with_detail(other)),
    };
    Ok(transport)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(json: serde_json::Value) -> String {
        format!(
            "vmess://{}",
            crate::base64::encode(json.to_string().as_bytes())
        )
    }

    #[test]
    fn parses_classic_json_payload() {
        let link = encode(serde_json::json!({
            "v": "2",
            "ps": "Tokyo 01",
            "add": "jp.example.com",
            "port": 443,
            "id": "11111111-1111-1111-1111-111111111111",
            "aid": 0,
            "scy": "chacha20-poly1305",
            "net": "grpc",
            "path": "gsvc",
            "tls": "tls",
            "sni": "jp.example.com"
        }));
        let profile = parse(&link).expect("parse");
        assert_eq!(profile.remark, "Tokyo 01");
        match profile.transport {
            Transport::Grpc { service_name, .. } => assert_eq!(service_name, "gsvc"),
            other => panic!("unexpected transport: {other:?}"),
        }
        assert_eq!(profile.extra["scy"], "chacha20-poly1305");
    }

    #[test]
    fn parses_standard_uri_payload() {
        let profile = parse(
            "vmess://11111111-1111-1111-1111-111111111111@a.com:443?type=ws&path=%2Fv&security=tls&sni=a.com#URI",
        )
        .expect("parse");
        assert_eq!(profile.remark, "URI");
        assert!(profile.tls_expect_sni() == "a.com");
    }

    #[test]
    fn base64_junk_is_rejected() {
        let error = parse("vmess://!!!notbase64!!!").expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::InvalidBase64);
    }

    #[test]
    fn zero_port_is_rejected() {
        let link = encode(serde_json::json!({
            "add": "a.com", "port": 0, "id": "11111111-1111-1111-1111-111111111111"
        }));
        let error = parse(&link).expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::InvalidValue);
    }

    impl Profile {
        fn tls_expect_sni(&self) -> String {
            self.tls
                .as_ref()
                .and_then(|tls| tls.sni.clone())
                .unwrap_or_default()
        }
    }
}
