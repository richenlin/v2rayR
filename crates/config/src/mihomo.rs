//! mihomo (Clash Meta) emitter.
//!
//! Optional kernel: it is the only one that can speak Snell and the Clash
//! provider ecosystem, but everything it can do here sing-box can do too. The
//! config is YAML, produced by our own writer because `serde_yaml` is not a
//! dependency.

use serde_json::{Map, Value, json};
#[cfg(test)]
use v2rayr_domain::Profile;
use v2rayr_domain::{CoreKind, Protocol, TlsOptions, Transport};

use crate::capability::choose_core;
use crate::plan::{RoutingMode, SessionPlan};
use crate::yaml::to_yaml;
use crate::{ConfigError, EmittedConfig, Format};

pub fn emit(plan: &SessionPlan) -> Result<EmittedConfig, ConfigError> {
    if choose_core(&plan.profile, Some(CoreKind::Mihomo)).is_err() {
        return Err(ConfigError::Unsupported {
            core: "mihomo",
            reason: format!(
                "{} over {}",
                plan.profile.protocol.kind(),
                plan.profile.transport.kind()
            ),
        });
    }

    let mut root = Map::new();
    root.insert("mixed-port".into(), json!(plan.inbound.port));
    if plan.inbound.lan.is_some() {
        return Err(ConfigError::InvalidPlan(
            "mihomo 尚不支持独立认证 LAN 入站".into(),
        ));
    }
    root.insert("allow-lan".into(), json!(false));
    root.insert("bind-address".into(), json!(plan.inbound.listen));
    root.insert(
        "mode".into(),
        json!(match plan.policy {
            RoutingMode::Rule => "rule",
            RoutingMode::Global => "global",
            RoutingMode::Direct => "direct",
        }),
    );
    root.insert("log-level".into(), json!(mihomo_log_level(&plan.log_level)));
    root.insert("ipv6".into(), json!(true));
    root.insert(
        "external-controller".into(),
        json!(format!("127.0.0.1:{}", plan.api.port)),
    );
    root.insert("secret".into(), json!(plan.api.secret));
    root.insert("dns".into(), dns(plan));
    root.insert("proxies".into(), Value::Array(vec![proxy(plan)]));
    root.insert(
        "proxy-groups".into(),
        json!([{
            "name": "PROXY",
            "type": "select",
            "proxies": ["proxy", "DIRECT"],
        }]),
    );
    root.insert("rules".into(), Value::Array(rules(plan)));

    let body = to_yaml(&Value::Object(root));
    let mut emitted = EmittedConfig::new(CoreKind::Mihomo, Format::Yaml, body);
    if plan.tun.is_some() {
        emitted = emitted.with_note("mihomo 的 TUN 需要额外权限，本版本暂不支持，已忽略");
    }
    Ok(emitted)
}

fn mihomo_log_level(level: &str) -> &str {
    match level {
        "trace" => "debug",
        "debug" | "info" | "warning" | "error" | "silent" => level,
        "warn" => "warning",
        _ => "info",
    }
}

fn dns(plan: &SessionPlan) -> Value {
    let nameserver: Vec<Value> = if plan.dns.servers.is_empty() {
        vec![json!("1.1.1.1"), json!("https://dns.google/dns-query")]
    } else {
        plan.dns
            .servers
            .iter()
            .map(|server| json!(server))
            .collect()
    };
    json!({
        "enable": true,
        "ipv6": false,
        "enhanced-mode": if plan.dns.fake_ip { "fake-ip" } else { "redir-host" },
        "nameserver": nameserver,
    })
}

fn proxy(plan: &SessionPlan) -> Value {
    let profile = &plan.profile;
    let mut node = Map::new();
    node.insert("name".into(), json!("proxy"));
    node.insert("server".into(), json!(profile.address));
    node.insert("port".into(), json!(profile.port));

    match &profile.protocol {
        Protocol::Vless {
            id,
            flow,
            encryption,
        } => {
            node.insert("type".into(), json!("vless"));
            node.insert("uuid".into(), json!(id.to_string()));
            if let Some(flow) = flow {
                node.insert("flow".into(), json!(flow.as_str()));
            }
            if let Some(encryption) = encryption {
                node.insert("encryption".into(), json!(encryption));
            }
        }
        Protocol::Vmess {
            id,
            alter_id,
            security,
        } => {
            node.insert("type".into(), json!("vmess"));
            node.insert("uuid".into(), json!(id.to_string()));
            node.insert("alterId".into(), json!(alter_id));
            node.insert("cipher".into(), json!(vmess_cipher(*security)));
        }
        Protocol::Trojan { password } => {
            node.insert("type".into(), json!("trojan"));
            node.insert("password".into(), json!(password.expose()));
        }
        Protocol::Shadowsocks { method, password } => {
            node.insert("type".into(), json!("ss"));
            node.insert("cipher".into(), json!(method.as_str()));
            node.insert("password".into(), json!(password.expose()));
        }
        Protocol::Hysteria2 {
            password,
            obfs,
            up_mbps,
            down_mbps,
            ports,
        } => {
            node.insert("type".into(), json!("hysteria2"));
            node.insert("password".into(), json!(password.expose()));
            if let Some(obfs) = obfs {
                node.insert("obfs".into(), json!(obfs.kind.as_str()));
                node.insert("obfs-password".into(), json!(obfs.password.expose()));
            }
            if let Some(up) = up_mbps {
                node.insert("up".into(), json!(format!("{up} Mbps")));
            }
            if let Some(down) = down_mbps {
                node.insert("down".into(), json!(format!("{down} Mbps")));
            }
            if let Some(ports) = ports {
                node.insert("ports".into(), json!(ports));
            }
        }
        Protocol::Tuic(tuic) => {
            node.insert("type".into(), json!("tuic"));
            node.insert("uuid".into(), json!(tuic.uuid.to_string()));
            node.insert("password".into(), json!(tuic.password.expose()));
            node.insert(
                "congestion-controller".into(),
                json!(
                    tuic.congestion
                        .map(|congestion| congestion.as_str())
                        .unwrap_or("cubic")
                ),
            );
            node.insert("udp-relay-mode".into(), json!(tuic.udp_relay_mode));
        }
        _ => {}
    }

    if let Some(tls_options) = profile.tls.as_ref() {
        node.insert("tls".into(), json!(true));
        for (key, value) in tls(tls_options, &profile.address) {
            node.insert(key, value);
        }
    }
    transport(&profile.transport, &mut node);
    if plan.tcp_fast_open {
        node.insert("tfo".into(), json!(true));
    }
    if let Some(mux) = plan.mux.as_ref().filter(|mux| mux.enabled) {
        node.insert(
            "smux".into(),
            json!({
                "enabled": true,
                "protocol": "smux",
                "max-connections": mux.concurrency,
            }),
        );
    }
    Value::Object(node)
}

fn vmess_cipher(security: v2rayr_domain::VmessSecurity) -> &'static str {
    use v2rayr_domain::VmessSecurity::*;
    match security {
        Auto => "auto",
        None => "none",
        Aes128Gcm => "aes-128-gcm",
        Chacha20Poly1305 => "chacha20-poly1305",
        Zero => "zero",
    }
}

fn tls(options: &TlsOptions, address: &str) -> Vec<(String, Value)> {
    let mut fields = Vec::new();
    fields.push(("servername".to_owned(), json!(options.server_name(address))));
    if options.insecure {
        fields.push(("skip-cert-verify".to_owned(), json!(true)));
    }
    if !options.alpn.is_empty() {
        fields.push(("alpn".to_owned(), json!(options.alpn)));
    }
    if let Some(fingerprint) = &options.fingerprint {
        fields.push(("client-fingerprint".to_owned(), json!(fingerprint)));
    }
    if let Some(reality) = &options.reality {
        fields.push((
            "reality-opts".to_owned(),
            json!({
                "public-key": reality.public_key,
                "short-id": reality.short_id,
            }),
        ));
    }
    fields
}

fn transport(transport: &Transport, node: &mut Map<String, Value>) {
    match transport {
        Transport::Raw => {}
        Transport::Ws { path, host, .. } => {
            node.insert("network".into(), json!("ws"));
            let mut opts = Map::new();
            opts.insert("path".into(), json!(path));
            if let Some(host) = host {
                opts.insert("headers".into(), json!({"Host": host}));
            }
            node.insert("ws-opts".into(), Value::Object(opts));
        }
        Transport::Grpc {
            service_name,
            multi_mode: _,
        } => {
            node.insert("network".into(), json!("grpc"));
            node.insert(
                "grpc-opts".into(),
                json!({"grpc-service-name": service_name}),
            );
        }
        Transport::HttpUpgrade { path, host } => {
            node.insert("network".into(), json!("httpupgrade"));
            let mut opts = Map::new();
            opts.insert("path".into(), json!(path));
            if let Some(host) = host {
                opts.insert("host".into(), json!(host));
            }
            node.insert("httpupgrade-opts".into(), Value::Object(opts));
        }
        Transport::Http { path, hosts } => {
            node.insert("network".into(), json!("http"));
            node.insert(
                "http-opts".into(),
                json!({"path": [path], "headers": {"Host": hosts}}),
            );
        }
        Transport::Xhttp { .. } | Transport::Mkcp { .. } | Transport::Quic { .. } => {
            // Rejected by the capability matrix.
        }
    }
}

fn rules(plan: &SessionPlan) -> Vec<Value> {
    let mut rules = Vec::new();
    if plan.policy != RoutingMode::Direct {
        rules.push(json!("IP-CIDR,10.0.0.0/8,DIRECT,no-resolve"));
        rules.push(json!("IP-CIDR,172.16.0.0/12,DIRECT,no-resolve"));
        rules.push(json!("IP-CIDR,192.168.0.0/16,DIRECT,no-resolve"));
        rules.push(json!("IP-CIDR,127.0.0.0/8,DIRECT,no-resolve"));
    }
    if plan.policy == RoutingMode::Rule {
        for rule in &plan.rules {
            let target = match rule.action {
                crate::plan::RuleAction::Proxy => "PROXY",
                crate::plan::RuleAction::Direct => "DIRECT",
                crate::plan::RuleAction::Block => "REJECT",
            };
            let line = match rule.kind {
                crate::plan::RuleKind::Domain => format!("DOMAIN,{},{target}", rule.value),
                crate::plan::RuleKind::DomainSuffix => {
                    format!("DOMAIN-SUFFIX,{},{target}", rule.value)
                }
                crate::plan::RuleKind::IpCidr => {
                    format!("IP-CIDR,{},{target},no-resolve", rule.value)
                }
                crate::plan::RuleKind::Process => format!("PROCESS-NAME,{},{target}", rule.value),
                crate::plan::RuleKind::GeoIp => format!("GEOIP,{},{target}", rule.value),
                crate::plan::RuleKind::GeoSite => format!("GEOSITE,{},{target}", rule.value),
            };
            rules.push(json!(line));
        }
        if plan.block_ads {
            rules.push(json!("GEOSITE,category-ads-all,REJECT"));
        }
    }
    rules.push(json!(match plan.policy {
        RoutingMode::Direct => "MATCH,DIRECT",
        _ => "MATCH,PROXY",
    }));
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{ApiPlan, InboundPlan};
    use v2rayr_domain::Secret;

    fn plan(profile: Profile) -> SessionPlan {
        SessionPlan {
            profile,
            core_override: None,
            inbound: InboundPlan {
                listen: "127.0.0.1".into(),
                port: 10808,
                http_port: None,
                dokodemo: None,
                lan: None,
            },
            api: ApiPlan {
                port: 10812,
                secret: "s".into(),
            },
            tun: None,
            policy: RoutingMode::Rule,
            dns: Default::default(),
            rules: Vec::new(),
            rule_sets: Vec::new(),
            sniffing: true,
            tcp_fast_open: false,
            mux: None,
            log_level: "info".into(),
            block_ads: false,
            data_dir: "/tmp/v2rayr".into(),
        }
    }

    fn trojan_ws() -> Profile {
        Profile::new(
            "node",
            "example.com",
            443,
            Protocol::Trojan {
                password: Secret::new("pw"),
            },
        )
        .with_transport(Transport::Ws {
            path: "/ws".into(),
            host: Some("cdn.example.com".into()),
            early_data: None,
        })
        .with_tls(TlsOptions {
            sni: Some("cdn.example.com".into()),
            ..Default::default()
        })
    }

    #[test]
    fn emits_yaml_with_the_expected_top_level_keys() {
        let config = emit(&plan(trojan_ws())).expect("emit");
        assert_eq!(config.format, Format::Yaml);
        for key in [
            "mixed-port: 10808",
            "mode: rule",
            "proxies:",
            "proxy-groups:",
            "rules:",
            "external-controller: 127.0.0.1:10812",
        ] {
            assert!(config.body.contains(key), "missing {key}");
        }
    }

    #[test]
    fn ws_options_and_tls_are_nested_under_the_proxy() {
        let config = emit(&plan(trojan_ws())).expect("emit");
        assert!(config.body.contains("network: ws"));
        assert!(config.body.contains("path: /ws"));
        assert!(config.body.contains("Host: cdn.example.com"));
        assert!(config.body.contains("servername: cdn.example.com"));
    }

    #[test]
    fn direct_policy_matches_direct() {
        let mut plan = plan(trojan_ws());
        plan.policy = RoutingMode::Direct;
        let config = emit(&plan).expect("emit");
        assert!(config.body.contains("mode: direct"));
        assert!(config.body.trim_end().ends_with("MATCH,DIRECT"));
    }

    #[test]
    fn unsupported_protocol_is_rejected() {
        let node = Profile::new(
            "x",
            "a.com",
            443,
            Protocol::AnyTls(v2rayr_domain::AnyTls {
                password: Secret::new("p"),
            }),
        );
        assert!(emit(&plan(node)).is_err());
    }
}
