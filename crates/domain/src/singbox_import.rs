//! Import nodes from a sing-box style JSON configuration.
//!
//! Providers that hand out "sing-box format" subscriptions ship a full config or
//! a single outbound; either way the shape is the same as the one our emitter
//! produces, so this module reads it back into [`Profile`]s. Outbounds that do
//! not describe a remote server (`direct`, `block`, `selector`, `urltest`) are
//! skipped quietly; remote types we do not support are reported in `skipped`.

use serde_json::Value;
use uuid::Uuid;

use crate::error::{DomainError, ErrorCode};
use crate::model::{
    AnyTls, Congestion, Hysteria2Obfs, ObfsKind, Profile, Protocol, RealityOptions, Secret,
    SsMethod, TlsOptions, Transport, Tuic, VmessSecurity, WireGuard,
};

#[derive(Debug, Clone, Default)]
pub struct SingboxImport {
    pub profiles: Vec<Profile>,
    pub skipped: Vec<String>,
}

const LOCAL_TYPES: &[&str] = &["direct", "block", "selector", "urltest", "dns"];

/// Read `outbounds` (or a single outbound object) into profiles.
pub fn parse_singbox_config(root: &Value) -> Result<SingboxImport, DomainError> {
    let outbounds = if let Some(items) = root.get("outbounds").and_then(Value::as_array) {
        items.as_slice()
    } else if root.get("type").is_some() {
        std::slice::from_ref(root)
    } else {
        return Err(DomainError::new(ErrorCode::UnsupportedFormat)
            .with_detail("JSON 里没有 outbounds 数组".to_owned()));
    };

    let mut out = SingboxImport::default();
    for outbound in outbounds {
        let kind = outbound.get("type").and_then(Value::as_str).unwrap_or("");
        if LOCAL_TYPES.contains(&kind) {
            continue;
        }
        let tag = outbound
            .get("tag")
            .and_then(Value::as_str)
            .unwrap_or("未命名节点");
        match outbound_to_profile(outbound) {
            Ok(profile) => out.profiles.push(profile),
            Err(reason) => out.skipped.push(format!("{tag}: {reason}")),
        }
    }
    Ok(out)
}

fn outbound_to_profile(outbound: &Value) -> Result<Profile, String> {
    let kind = outbound
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "缺少 type".to_owned())?;
    let server = outbound
        .get("server")
        .and_then(Value::as_str)
        .ok_or_else(|| "缺少 server".to_owned())?;
    let port = outbound
        .get("server_port")
        .and_then(Value::as_u64)
        .ok_or_else(|| "缺少 server_port".to_owned())? as u16;
    if port == 0 {
        return Err("server_port 不合法".to_owned());
    }
    let remark = outbound
        .get("tag")
        .and_then(Value::as_str)
        .unwrap_or(server)
        .to_owned();

    let protocol = match kind {
        "vless" => Protocol::Vless {
            id: uuid_field(outbound, "uuid")?,
            flow: outbound
                .get("flow")
                .and_then(Value::as_str)
                .and_then(crate::model::VlessFlow::parse),
            encryption: outbound
                .get("encryption")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        "vmess" => Protocol::Vmess {
            id: uuid_field(outbound, "uuid")?,
            alter_id: outbound
                .get("alter_id")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u16,
            security: outbound
                .get("security")
                .and_then(Value::as_str)
                .map(VmessSecurity::parse)
                .unwrap_or(VmessSecurity::Auto),
        },
        "trojan" => Protocol::Trojan {
            password: Secret::new(str_field(outbound, "password").unwrap_or_default()),
        },
        "shadowsocks" => Protocol::Shadowsocks {
            method: SsMethod::new(str_field(outbound, "method").ok_or("缺少 method")?),
            password: Secret::new(str_field(outbound, "password").unwrap_or_default()),
        },
        "hysteria2" => Protocol::Hysteria2 {
            password: Secret::new(str_field(outbound, "password").unwrap_or_default()),
            obfs: outbound.get("obfs").and_then(|obfs| {
                obfs.get("type").and_then(Value::as_str)?;
                Some(Hysteria2Obfs {
                    // sing-box only ships salamander.
                    kind: ObfsKind::Salamander,
                    password: Secret::new(
                        obfs.get("password")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    ),
                })
            }),
            up_mbps: outbound
                .get("up_mbps")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            down_mbps: outbound
                .get("down_mbps")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            ports: outbound
                .get("server_ports")
                .and_then(Value::as_array)
                .map(|ports| {
                    ports
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .filter(|ports| !ports.is_empty()),
        },
        "tuic" => Protocol::Tuic(Tuic {
            uuid: uuid_field(outbound, "uuid")?,
            password: Secret::new(str_field(outbound, "password").unwrap_or_default()),
            congestion: outbound
                .get("congestion_control")
                .and_then(Value::as_str)
                .and_then(Congestion::parse),
            udp_relay_mode: outbound
                .get("udp_relay_mode")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "anytls" => Protocol::AnyTls(AnyTls {
            password: Secret::new(str_field(outbound, "password").unwrap_or_default()),
        }),
        "socks" => Protocol::Socks {
            username: str_field(outbound, "username").map(str::to_owned),
            password: str_field(outbound, "password").map(Secret::new),
        },
        "http" => Protocol::Http {
            username: str_field(outbound, "username").map(str::to_owned),
            password: str_field(outbound, "password").map(Secret::new),
        },
        "wireguard" => Protocol::WireGuard(WireGuard {
            private_key: Secret::new(str_field(outbound, "private_key").unwrap_or_default()),
            public_key: str_field(outbound, "peer_public_key")
                .unwrap_or_default()
                .to_owned(),
            preshared_key: str_field(outbound, "pre_shared_key").map(Secret::new),
            mtu: outbound.get("mtu").and_then(Value::as_u64).unwrap_or(1420) as u16,
            local_address: outbound
                .get("local_address")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        }),
        other => return Err(format!("暂不支持 {other}")),
    };

    let transport = transport_of(outbound)?;
    let tls = tls_of(outbound);
    let mut profile = Profile::new(remark, server.to_owned(), port, protocol);
    profile.transport = transport;
    profile.tls = tls;
    profile.mux = None;
    profile
        .validate()
        .map_err(|error| error.code().as_str().to_owned())?;
    Ok(profile)
}

fn uuid_field(value: &Value, key: &str) -> Result<Uuid, String> {
    let text = str_field(value, key).ok_or_else(|| format!("缺少 {key}"))?;
    Uuid::parse_str(text).map_err(|_| format!("{key} 格式不正确"))
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn tls_of(outbound: &Value) -> Option<TlsOptions> {
    let tls = outbound.get("tls")?;
    if !tls.get("enabled").and_then(Value::as_bool).unwrap_or(true) {
        return None;
    }
    let reality = tls.get("reality").and_then(|reality| {
        let public_key = reality.get("public_key").and_then(Value::as_str)?;
        if public_key.is_empty() {
            return None;
        }
        Some(RealityOptions {
            public_key: public_key.to_owned(),
            short_id: reality
                .get("short_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            spider_x: reality
                .get("spider_x")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    });
    Some(TlsOptions {
        sni: tls
            .get("server_name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        insecure: tls
            .get("insecure")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        alpn: tls
            .get("alpn")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        fingerprint: tls
            .get("utls")
            .and_then(|utls| utls.get("fingerprint"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        reality,
        ech: tls
            .get("ech")
            .and_then(|ech| ech.get("enabled"))
            .and_then(Value::as_bool)
            .filter(|enabled| *enabled)
            .map(|_| "true".to_owned()),
    })
}

fn transport_of(outbound: &Value) -> Result<Transport, String> {
    let Some(transport) = outbound.get("transport") else {
        return Ok(Transport::Raw);
    };
    let kind = transport
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("tcp");
    Ok(match kind {
        "tcp" | "raw" => Transport::Raw,
        "ws" => Transport::Ws {
            path: transport
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("/")
                .to_owned(),
            host: transport
                .get("headers")
                .and_then(|headers| headers.get("Host"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            early_data: transport
                .get("max_early_data")
                .and_then(Value::as_u64)
                .map(|value| value as u32),
        },
        "grpc" => Transport::Grpc {
            service_name: transport
                .get("service_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            multi_mode: false,
        },
        "http" => Transport::Http {
            path: transport
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("/")
                .to_owned(),
            hosts: transport
                .get("host")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        },
        "httpupgrade" => Transport::HttpUpgrade {
            path: transport
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("/")
                .to_owned(),
            host: transport
                .get("host")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        "quic" => Transport::Quic {
            security: crate::model::QuicSecurity::None,
            key: Secret::new(""),
            header: String::new(),
        },
        other => return Err(format!("暂不支持 transport={other}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Protocol, Transport};

    #[test]
    fn reads_a_full_singbox_config() {
        let json: Value = serde_json::from_str(
            r#"{
              "outbounds": [
                {"type": "direct", "tag": "direct"},
                {"type": "vless", "tag": "Tokyo", "server": "a.example.com", "server_port": 443,
                 "uuid": "8a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d", "flow": "xtls-rprx-vision",
                 "tls": {"enabled": true, "server_name": "a.example.com",
                         "utls": {"enabled": true, "fingerprint": "chrome"},
                         "reality": {"enabled": true, "public_key": "PUBKEY", "short_id": "abcd"}},
                 "transport": {"type": "ws", "path": "/ws", "headers": {"Host": "cdn.example.com"}}},
                {"type": "hysteria2", "tag": "Hy2", "server": "b.example.com", "server_port": 443,
                 "password": "pw", "obfs": {"type": "salamander", "password": "obfs"},
                 "tls": {"enabled": true}},
                {"type": "tuic", "tag": "Tuic", "server": "c.example.com", "server_port": 443,
                 "uuid": "9a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d", "password": "pw",
                 "congestion_control": "bbr", "tls": {"enabled": true, "alpn": ["h3"]}},
                {"type": "custom-unknown", "tag": "Nope", "server": "d.example.com", "server_port": 1}
              ]
            }"#,
        )
        .expect("json");

        let import = parse_singbox_config(&json).expect("import");
        assert_eq!(import.profiles.len(), 3, "skipped: {:?}", import.skipped);
        assert_eq!(import.skipped.len(), 1);

        let vless = &import.profiles[0];
        assert!(matches!(
            &vless.protocol,
            Protocol::Vless { flow: Some(_), .. }
        ));
        assert_eq!(
            vless.transport,
            Transport::Ws {
                path: "/ws".to_owned(),
                host: Some("cdn.example.com".to_owned()),
                early_data: None,
            }
        );
        let tls = vless.tls.as_ref().expect("tls");
        assert_eq!(tls.sni.as_deref(), Some("a.example.com"));
        assert_eq!(tls.fingerprint.as_deref(), Some("chrome"));
        assert!(tls.reality.is_some());

        let hy2 = &import.profiles[1];
        assert!(matches!(
            &hy2.protocol,
            Protocol::Hysteria2 { obfs: Some(_), .. }
        ));

        let tuic = &import.profiles[2];
        assert_eq!(
            tuic.tls.as_ref().map(|tls| tls.alpn.clone()),
            Some(vec!["h3".to_owned()])
        );
        assert!(matches!(&tuic.protocol, Protocol::Tuic(t) if t.congestion.is_some()));
    }

    #[test]
    fn accepts_a_single_outbound() {
        let json: Value = serde_json::from_str(
            r#"{"type": "trojan", "tag": "Solo", "server": "t.example.com", "server_port": 443,
                "password": "pw", "tls": {"enabled": true, "server_name": "t.example.com"}}"#,
        )
        .expect("json");
        let import = parse_singbox_config(&json).expect("import");
        assert_eq!(import.profiles.len(), 1);
        assert_eq!(import.profiles[0].remark, "Solo");
        assert!(matches!(
            import.profiles[0].protocol,
            Protocol::Trojan { .. }
        ));
    }

    #[test]
    fn rejects_documents_without_outbounds() {
        let json: Value = serde_json::from_str(r#"{"log": {"level": "info"}}"#).expect("json");
        let error = parse_singbox_config(&json).expect_err("no outbounds");
        assert_eq!(error.code(), ErrorCode::UnsupportedFormat);
    }
}
