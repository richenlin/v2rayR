//! Profile → share link.
//!
//! Round-tripping matters for two reasons: the node list lets users copy a link
//! to a phone, and the tests use export → import to prove that parsing does not
//! silently lose connection parameters.

use serde_json::{Value, json};

use super::uri::pct_encode;
use crate::base64;
use crate::model::{Profile, Protocol, Transport};

/// Returns `None` for protocols without a stable URI scheme (chains, raw custom
/// JSON), where inventing a link would be worse than saying "not supported".
pub fn export_share_link(profile: &Profile) -> Option<String> {
    let remark = profile.remark.as_str();
    let link = match &profile.protocol {
        Protocol::Vless {
            id,
            flow,
            encryption: _,
        } => {
            let mut params = common_query(profile);
            if let Some(flow) = flow {
                params.push(("flow".into(), flow.as_str().to_owned()));
            }
            if let Some(security) = &profile.tls {
                params.push(("security".into(), security_kind(profile)));
                if let Some(sni) = &security.sni {
                    params.push(("sni".into(), sni.clone()));
                }
                if let Some(fingerprint) = &security.fingerprint {
                    params.push(("fp".into(), fingerprint.clone()));
                }
                if security.insecure {
                    params.push(("allowInsecure".into(), "1".into()));
                }
                if let Some(reality) = &security.reality {
                    params.push(("pbk".into(), reality.public_key.clone()));
                    if !reality.short_id.is_empty() {
                        params.push(("sid".into(), reality.short_id.clone()));
                    }
                }
            }
            transport_query(profile, &mut params);
            format!(
                "vless://{}@{}:{}?{}#{}",
                id,
                host_for_uri(&profile.address),
                profile.port,
                encode_query(&params),
                pct_encode(remark)
            )
        }
        Protocol::Trojan { password } => {
            let mut params = common_query(profile);
            if let Some(security) = &profile.tls {
                if let Some(sni) = &security.sni {
                    params.push(("sni".into(), sni.clone()));
                }
                if security.insecure {
                    params.push(("allowInsecure".into(), "1".into()));
                }
            } else {
                params.push(("security".into(), "none".into()));
            }
            transport_query(profile, &mut params);
            format!(
                "trojan://{}@{}:{}?{}#{}",
                pct_encode(password.expose()),
                host_for_uri(&profile.address),
                profile.port,
                encode_query(&params),
                pct_encode(remark)
            )
        }
        Protocol::Shadowsocks { method, password } => {
            let userinfo =
                base64::encode(format!("{}:{}", method.as_str(), password.expose()).as_bytes());
            format!(
                "ss://{}@{}:{}#{}",
                userinfo,
                host_for_uri(&profile.address),
                profile.port,
                pct_encode(remark)
            )
        }
        Protocol::Hysteria2 {
            password,
            obfs,
            ports,
            ..
        } => {
            let mut params: Vec<(String, String)> = Vec::new();
            if let Some(obfs) = obfs {
                params.push(("obfs".into(), "salamander".into()));
                params.push(("obfs-password".into(), obfs.password.expose().to_owned()));
            }
            if let Some(ports) = ports {
                params.push(("mport".into(), ports.clone()));
            }
            if let Some(sni) = profile.tls.as_ref().and_then(|tls| tls.sni.clone()) {
                params.push(("sni".into(), sni));
            }
            format!(
                "hysteria2://{}@{}:{}?{}#{}",
                pct_encode(password.expose()),
                host_for_uri(&profile.address),
                profile.port,
                encode_query(&params),
                pct_encode(remark)
            )
        }
        Protocol::Tuic(tuic) => {
            let mut params: Vec<(String, String)> = Vec::new();
            if let Some(congestion) = tuic.congestion {
                params.push(("congestion_control".into(), congestion.as_str().to_owned()));
            }
            if let Some(mode) = &tuic.udp_relay_mode {
                params.push(("udp_relay_mode".into(), mode.clone()));
            }
            if let Some(sni) = profile.tls.as_ref().and_then(|tls| tls.sni.clone()) {
                params.push(("sni".into(), sni));
            }
            format!(
                "tuic://{}:{}@{}:{}?{}#{}",
                tuic.uuid,
                pct_encode(tuic.password.expose()),
                host_for_uri(&profile.address),
                profile.port,
                encode_query(&params),
                pct_encode(remark)
            )
        }
        Protocol::AnyTls(anytls) => format!(
            "anytls://{}@{}:{}?sni={}#{}",
            pct_encode(anytls.password.expose()),
            host_for_uri(&profile.address),
            profile.port,
            pct_encode(
                profile
                    .tls
                    .as_ref()
                    .and_then(|tls| tls.sni.as_deref())
                    .unwrap_or(&profile.address)
            ),
            pct_encode(remark)
        ),
        Protocol::Vmess {
            id,
            alter_id,
            security,
        } => {
            let mut object = json!({
                "v": "2",
                "ps": remark,
                "add": profile.address,
                "port": profile.port.to_string(),
                "id": id,
                "aid": alter_id.to_string(),
                "scy": security.as_str(),
                "net": profile.transport.kind(),
                "type": "none",
                "host": "",
                "path": "",
                "tls": if profile.tls.is_some() { "tls" } else { "" },
                "sni": profile.tls.as_ref().and_then(|tls| tls.sni.clone()).unwrap_or_default(),
            });
            apply_transport_json(profile, &mut object);
            let encoded = base64::encode(serde_json::to_string(&object).ok()?.as_bytes());
            format!("vmess://{encoded}")
        }
        _ => return None,
    };
    Some(link)
}

fn security_kind(profile: &Profile) -> String {
    match profile.tls.as_ref() {
        Some(tls) if tls.reality.is_some() => "reality".into(),
        Some(_) => "tls".into(),
        None => "none".into(),
    }
}

/// Copy TLS-independent parameters (transport, host header, path) into `params`.
fn transport_query(profile: &Profile, params: &mut Vec<(String, String)>) -> Option<()> {
    match &profile.transport {
        Transport::Raw => {
            params.push(("type".into(), "tcp".into()));
        }
        Transport::Ws { path, host, .. } => {
            params.push(("type".into(), "ws".into()));
            params.push(("path".into(), path.clone()));
            if let Some(host) = host {
                params.push(("host".into(), host.clone()));
            }
        }
        Transport::Grpc { service_name, .. } => {
            params.push(("type".into(), "grpc".into()));
            params.push(("serviceName".into(), service_name.clone()));
        }
        Transport::HttpUpgrade { path, host } => {
            params.push(("type".into(), "httpupgrade".into()));
            params.push(("path".into(), path.clone()));
            if let Some(host) = host {
                params.push(("host".into(), host.clone()));
            }
        }
        Transport::Http { path, hosts } => {
            params.push(("type".into(), "http".into()));
            params.push(("path".into(), path.clone()));
            params.push(("host".into(), hosts.join(",")));
        }
        Transport::Xhttp {
            path, host, mode, ..
        } => {
            params.push(("type".into(), "xhttp".into()));
            params.push(("path".into(), path.clone()));
            if let Some(host) = host {
                params.push(("host".into(), host.clone()));
            }
            params.push(("mode".into(), mode.clone()));
        }
        Transport::Mkcp { .. } => {
            params.push(("type".into(), "kcp".into()));
        }
        Transport::Quic { .. } => {
            params.push(("type".into(), "quic".into()));
        }
    }
    Some(())
}

fn common_query(profile: &Profile) -> Vec<(String, String)> {
    let mut params = Vec::new();
    if profile.tls.is_none() {
        params.push(("security".into(), "none".into()));
    }
    if let Some(alpn) = profile
        .tls
        .as_ref()
        .map(|tls| tls.alpn.clone())
        .filter(|alpn| !alpn.is_empty())
    {
        params.push(("alpn".into(), alpn.join(",")));
    }
    params
}

fn encode_query(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", pct_encode(key), pct_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn host_for_uri(address: &str) -> String {
    if address.contains(':') && !address.starts_with('[') {
        format!("[{address}]")
    } else {
        address.to_owned()
    }
}

fn apply_transport_json(profile: &Profile, object: &mut Value) -> Option<Value> {
    let map = object.as_object_mut()?;
    match &profile.transport {
        Transport::Raw => {}
        Transport::Ws { path, host, .. } => {
            map.insert("path".into(), json!(path));
            map.insert("host".into(), json!(host.clone().unwrap_or_default()));
        }
        Transport::Grpc { service_name, .. } => {
            map.insert("path".into(), json!(service_name));
        }
        Transport::HttpUpgrade { path, host } => {
            map.insert("net".into(), json!("httpupgrade"));
            map.insert("path".into(), json!(path));
            map.insert("host".into(), json!(host));
        }
        _ => {}
    }
    Some(object.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Protocol, Secret, SsMethod};
    use crate::sharelink::parse_share_link;

    fn vless() -> Profile {
        let mut profile = parse_share_link(
            "vless://11111111-1111-1111-1111-111111111111@a.example.com:443?security=tls&sni=a.example.com&type=ws&path=%2Fws#Tokyo",
        )
        .expect("parse");
        profile.remark = "Tokyo".into();
        profile
    }

    #[test]
    fn a_vless_link_round_trips() {
        let profile = vless();
        let link = export_share_link(&profile).expect("export");
        let parsed = parse_share_link(&link).expect("re-parse");
        assert_eq!(parsed.address, profile.address);
        assert_eq!(parsed.port, profile.port);
        assert_eq!(parsed.remark, "Tokyo");
        assert_eq!(parsed.transport.kind(), "ws");
    }

    #[test]
    fn a_trojan_link_round_trips() {
        let mut profile = Profile::new(
            "Seoul",
            "b.example.com",
            443,
            Protocol::Trojan {
                password: Secret::new("pw"),
            },
        );
        profile.tls = Some(crate::model::TlsOptions::default());
        let link = export_share_link(&profile).expect("export");
        assert!(link.starts_with("trojan://pw@b.example.com:443"));
        let parsed = parse_share_link(&link).expect("re-parse");
        assert_eq!(parsed.port, 443);
    }

    #[test]
    fn a_shadowsocks_link_round_trips() {
        let profile = Profile::new(
            "SS",
            "c.example.com",
            8388,
            Protocol::Shadowsocks {
                method: SsMethod::new("aes-256-gcm"),
                password: Secret::new("secret"),
            },
        );
        let link = export_share_link(&profile).expect("export");
        let parsed = parse_share_link(&link).expect("re-parse");
        assert_eq!(parsed.port, 8388);
        assert_eq!(parsed.remark, "SS");
    }

    #[test]
    fn a_vmess_link_round_trips_through_base64_json() {
        let mut profile = Profile::new(
            "VM",
            "d.example.com",
            443,
            Protocol::Vmess {
                id: uuid::Uuid::parse_str("11111111-1111-1111-1111-111111111111").expect("uuid"),
                alter_id: 0,
                security: crate::model::VmessSecurity::Auto,
            },
        );
        profile.tls = Some(crate::model::TlsOptions::default());
        let link = export_share_link(&profile).expect("export");
        assert!(link.starts_with("vmess://"));
        let parsed = parse_share_link(&link).expect("re-parse");
        assert_eq!(parsed.address, "d.example.com");
        assert_eq!(parsed.remark, "VM");
    }

    #[test]
    fn chains_are_not_exportable() {
        let profile = Profile::new("chain", "e.example.com", 443, Protocol::Chain(Vec::new()));
        assert!(export_share_link(&profile).is_none());
    }
}
