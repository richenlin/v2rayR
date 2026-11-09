//! sing-box emitter (1.11+ schema).
//!
//! Only what we actually configure is emitted; anything the user did not ask for
//! (`fakeip`, extra inbounds) stays absent so the kernel uses its own defaults.

use serde_json::{Map, Value, json};
use v2rayr_domain::{
    CoreKind, Hysteria2Obfs, Profile, Protocol, QuicSecurity, TlsOptions, Transport, VmessSecurity,
};

use crate::capability::choose_core;
use crate::plan::{RoutingMode, RuleAction, RuleSetKind, SessionPlan};
use crate::{ConfigError, EmittedConfig, Format};

pub const PROXY_TAG: &str = "proxy";

pub fn emit(plan: &SessionPlan) -> Result<EmittedConfig, ConfigError> {
    if choose_core(&plan.profile, Some(CoreKind::SingBox)).is_err() {
        return Err(ConfigError::Unsupported {
            core: "sing-box",
            reason: describe_unsupported(&plan.profile),
        });
    }

    let mut root = Map::new();
    root.insert(
        "log".into(),
        json!({"level": log_level(&plan.log_level), "timestamp": true}),
    );
    root.insert("dns".into(), dns(plan));

    let mut inbounds = vec![mixed_inbound(plan)];
    if let Some(tun) = &plan.tun {
        inbounds.push(tun_inbound(plan, tun));
    }
    root.insert("inbounds".into(), Value::Array(inbounds));
    root.insert("outbounds".into(), outbounds(plan));
    root.insert("route".into(), route(plan));
    root.insert(
        "experimental".into(),
        json!({
            "clash_api": {
                "external_controller": format!("127.0.0.1:{}", plan.api.port),
                "secret": plan.api.secret,
            },
            "cache_file": {"enabled": true, "path": format!("{}/cache.db", plan.data_dir)},
        }),
    );

    let body = serde_json::to_string_pretty(&Value::Object(root))?;
    let mut emitted = EmittedConfig::new(CoreKind::SingBox, Format::Json, body);
    if plan.tun.is_none() && plan.inbound.allow_lan {
        emitted = emitted.with_note("局域网访问已启用，请确认已设置访问凭据");
    }
    if plan.dns.fake_ip {
        emitted = emitted.with_note("FakeIP 需要 TUN，本版本未启用，已忽略该设置");
    }
    if plan.rule_sets.is_empty() && plan.policy == RoutingMode::Rule {
        emitted = emitted.with_note("未找到规则集文件，规则模式只使用内置私有网段与自定义规则");
    }
    Ok(emitted)
}

fn describe_unsupported(profile: &Profile) -> String {
    format!(
        "{} over {} is not supported",
        profile.protocol.kind(),
        profile.transport.kind()
    )
}

fn log_level(level: &str) -> &str {
    match level {
        "trace" | "debug" | "info" | "warn" | "error" => level,
        _ => "info",
    }
}

fn mixed_inbound(plan: &SessionPlan) -> Value {
    let mut inbound = Map::new();
    inbound.insert("type".into(), json!("mixed"));
    inbound.insert("tag".into(), json!("mixed-in"));
    inbound.insert("listen".into(), json!(plan.inbound.listen));
    inbound.insert("listen_port".into(), json!(plan.inbound.port));
    if plan.sniffing {
        inbound.insert("sniff".into(), json!(true));
        inbound.insert("sniff_override_destination".into(), json!(false));
    }
    if let (Some(username), Some(password)) = (
        plan.inbound.username.as_ref(),
        plan.inbound.password.as_ref(),
    ) {
        inbound.insert(
            "users".into(),
            json!([{"username": username, "password": password}]),
        );
    }
    Value::Object(inbound)
}

fn tun_inbound(plan: &SessionPlan, tun: &crate::plan::TunPlan) -> Value {
    let interface = if tun.interface.is_empty() {
        "utun9".to_owned()
    } else {
        tun.interface.clone()
    };
    let mut inbound = Map::new();
    inbound.insert("type".into(), json!("tun"));
    inbound.insert("tag".into(), json!("tun-in"));
    inbound.insert("interface_name".into(), json!(interface));
    inbound.insert(
        "address".into(),
        json!(["172.19.0.1/30", "fdfe:dcba:9876::1/126"]),
    );
    inbound.insert("mtu".into(), json!(tun.mtu));
    inbound.insert("auto_route".into(), json!(true));
    inbound.insert("strict_route".into(), json!(true));
    inbound.insert("stack".into(), json!("mixed"));
    if plan.sniffing {
        inbound.insert("sniff".into(), json!(true));
    }
    Value::Object(inbound)
}

fn dns(plan: &SessionPlan) -> Value {
    let mut servers: Vec<Value> = Vec::new();
    servers.push(json!({"tag": "dns-local", "address": "local", "detour": "direct"}));

    let mut remote_tags = Vec::new();
    let configured: Vec<&String> = plan.dns.servers.iter().collect();
    if configured.is_empty() {
        servers.push(json!({
            "tag": "dns-remote",
            "address": "https://1.1.1.1/dns-query",
            "detour": PROXY_TAG,
        }));
        remote_tags.push("dns-remote".to_owned());
    } else {
        for (index, address) in configured.iter().enumerate() {
            let tag = format!("dns-remote-{index}");
            servers.push(json!({
                "tag": tag,
                "address": address,
                "detour": PROXY_TAG,
            }));
            remote_tags.push(tag);
        }
    }

    let mut dns = Map::new();
    dns.insert("servers".into(), Value::Array(servers));
    dns.insert(
        "final".into(),
        json!(
            remote_tags
                .first()
                .cloned()
                .unwrap_or_else(|| "dns-local".to_owned())
        ),
    );
    dns.insert("strategy".into(), json!(dns_strategy(&plan.dns.strategy)));
    if plan.policy == RoutingMode::Direct {
        // In direct mode traffic must not depend on a proxy that is not there.
        dns.insert("final".into(), json!("dns-local"));
    }
    Value::Object(dns)
}

fn dns_strategy(value: &str) -> &str {
    match value {
        "prefer_ipv4" | "prefer_ipv6" | "ipv4_only" | "ipv6_only" => value,
        _ => "prefer_ipv4",
    }
}

fn outbounds(plan: &SessionPlan) -> Value {
    let mut outbounds = vec![proxy_outbound(plan)];
    outbounds.push(json!({"type": "direct", "tag": "direct"}));
    outbounds.push(json!({"type": "block", "tag": "block"}));
    Value::Array(outbounds)
}

fn proxy_outbound(plan: &SessionPlan) -> Value {
    let profile = &plan.profile;
    let mut outbound = Map::new();
    outbound.insert("tag".into(), json!(PROXY_TAG));
    outbound.insert("server".into(), json!(profile.address));
    outbound.insert("server_port".into(), json!(profile.port));

    match &profile.protocol {
        Protocol::Vmess {
            id,
            alter_id,
            security,
        } => {
            outbound.insert("type".into(), json!("vmess"));
            outbound.insert("uuid".into(), json!(id.to_string()));
            outbound.insert("alter_id".into(), json!(alter_id));
            outbound.insert("security".into(), json!(vmess_security(*security)));
        }
        Protocol::Vless {
            id,
            flow,
            encryption,
        } => {
            outbound.insert("type".into(), json!("vless"));
            outbound.insert("uuid".into(), json!(id.to_string()));
            if let Some(flow) = flow {
                outbound.insert("flow".into(), json!(flow.as_str()));
            }
            if let Some(encryption) = encryption {
                outbound.insert("encryption".into(), json!(encryption));
            }
        }
        Protocol::Trojan { password } => {
            outbound.insert("type".into(), json!("trojan"));
            outbound.insert("password".into(), json!(password.expose()));
        }
        Protocol::Shadowsocks { method, password } => {
            outbound.insert("type".into(), json!("shadowsocks"));
            outbound.insert("method".into(), json!(method.as_str()));
            outbound.insert("password".into(), json!(password.expose()));
        }
        Protocol::Socks { username, password } => {
            outbound.insert("type".into(), json!("socks"));
            if let (Some(username), Some(password)) = (username, password) {
                outbound.insert("username".into(), json!(username));
                outbound.insert("password".into(), json!(password.expose()));
            }
        }
        Protocol::Http { username, password } => {
            outbound.insert("type".into(), json!("http"));
            if let (Some(username), Some(password)) = (username, password) {
                outbound.insert("username".into(), json!(username));
                outbound.insert("password".into(), json!(password.expose()));
            }
        }
        Protocol::Hysteria2 {
            password,
            obfs,
            up_mbps,
            down_mbps,
            ports,
        } => {
            outbound.insert("type".into(), json!("hysteria2"));
            outbound.insert("password".into(), json!(password.expose()));
            if let Some(obfs) = obfs {
                let mut value = Map::new();
                value.insert("type".into(), json!(obfs.kind.as_str()));
                value.insert("password".into(), json!(obfs.password.expose()));
                outbound.insert("obfs".into(), Value::Object(value));
            }
            if let Some(up) = up_mbps {
                outbound.insert("up_mbps".into(), json!(up));
            }
            if let Some(down) = down_mbps {
                outbound.insert("down_mbps".into(), json!(down));
            }
            if let Some(ports) = ports {
                outbound.insert("server_ports".into(), json!(ports));
            }
        }
        Protocol::Tuic(tuic) => {
            outbound.insert("type".into(), json!("tuic"));
            outbound.insert("uuid".into(), json!(tuic.uuid.to_string()));
            outbound.insert("password".into(), json!(tuic.password.expose()));
            outbound.insert(
                "congestion_control".into(),
                json!(
                    tuic.congestion
                        .map(|congestion| congestion.as_str())
                        .unwrap_or("cubic")
                ),
            );
            outbound.insert("udp_relay_mode".into(), json!(tuic.udp_relay_mode));
        }
        Protocol::WireGuard(wg) => {
            outbound.insert("type".into(), json!("wireguard"));
            outbound.insert("local_address".into(), json!(wg.local_address));
            outbound.insert("private_key".into(), json!(wg.private_key.expose()));
            outbound.insert("peer_public_key".into(), json!(wg.public_key));
            if let Some(psk) = &wg.preshared_key {
                outbound.insert("pre_shared_key".into(), json!(psk.expose()));
            }
            outbound.insert("mtu".into(), json!(wg.mtu));
        }
        Protocol::AnyTls(anytls) => {
            outbound.insert("type".into(), json!("anytls"));
            outbound.insert("password".into(), json!(anytls.password.expose()));
        }
        // Rejected by `choose_core` before we get here.
        Protocol::Naive(_) | Protocol::Chain(_) | Protocol::Custom { .. } => {}
    }

    if let Some(tls_options) = profile.tls.as_ref() {
        outbound.insert("tls".into(), tls(tls_options, &profile.address));
    }
    if let Some(transport) = transport(&profile.transport) {
        outbound.insert("transport".into(), transport);
    }
    if let Some(mux) = plan.mux.as_ref().filter(|mux| mux.enabled) {
        let mut multiplex = Map::new();
        multiplex.insert("enabled".into(), json!(true));
        multiplex.insert(
            "protocol".into(),
            json!(
                mux.protocol
                    .map(|protocol| protocol.as_str())
                    .unwrap_or("smux")
            ),
        );
        if let Some(concurrency) = mux.concurrency {
            multiplex.insert("max_streams".into(), json!(concurrency));
        }
        outbound.insert("multiplex".into(), Value::Object(multiplex));
    }
    if plan.tcp_fast_open {
        outbound.insert("tcp_fast_open".into(), json!(true));
    }
    Value::Object(outbound)
}

fn vmess_security(security: VmessSecurity) -> &'static str {
    match security {
        VmessSecurity::Auto => "auto",
        VmessSecurity::None => "none",
        VmessSecurity::Aes128Gcm => "aes-128-gcm",
        VmessSecurity::Chacha20Poly1305 => "chacha20-poly1305",
        VmessSecurity::Zero => "zero",
    }
}

fn tls(options: &TlsOptions, address: &str) -> Value {
    let mut tls = Map::new();
    tls.insert("enabled".into(), json!(true));
    tls.insert("server_name".into(), json!(options.server_name(address)));
    if options.insecure {
        tls.insert("insecure".into(), json!(true));
    }
    if !options.alpn.is_empty() {
        tls.insert("alpn".into(), json!(options.alpn));
    }
    if let Some(fingerprint) = &options.fingerprint {
        tls.insert(
            "utls".into(),
            json!({"enabled": true, "fingerprint": fingerprint}),
        );
    }
    if let Some(reality) = &options.reality {
        let mut value = Map::new();
        value.insert("enabled".into(), json!(true));
        value.insert("public_key".into(), json!(reality.public_key));
        if !reality.short_id.is_empty() {
            value.insert("short_id".into(), json!(reality.short_id));
        }
        tls.insert("reality".into(), Value::Object(value));
    }
    if let Some(ech) = &options.ech {
        tls.insert("ech".into(), json!({"enabled": true, "config": ech}));
    }
    Value::Object(tls)
}

fn transport(transport: &Transport) -> Option<Value> {
    let value = match transport {
        Transport::Raw => return None,
        Transport::Ws {
            path,
            host,
            early_data,
        } => {
            let mut value = Map::new();
            value.insert("type".into(), json!("ws"));
            value.insert("path".into(), json!(path));
            if let Some(host) = host {
                value.insert("headers".into(), json!({"Host": host}));
            }
            if let Some(early) = early_data {
                value.insert("max_early_data".into(), json!(early));
            }
            Value::Object(value)
        }
        Transport::Grpc {
            service_name,
            multi_mode: _,
        } => json!({"type": "grpc", "service_name": service_name}),
        Transport::HttpUpgrade { path, host } => {
            let mut value = Map::new();
            value.insert("type".into(), json!("httpupgrade"));
            value.insert("path".into(), json!(path));
            if let Some(host) = host {
                value.insert("host".into(), json!(host));
            }
            Value::Object(value)
        }
        Transport::Http { path, hosts } => {
            let mut value = Map::new();
            value.insert("type".into(), json!("http"));
            value.insert("path".into(), json!(path));
            if !hosts.is_empty() {
                value.insert("host".into(), json!(hosts));
            }
            Value::Object(value)
        }
        Transport::Quic { .. } => {
            // sing-box has no QUIC transport; `choose_core` rejects it, but be
            // defensive so a future caller cannot silently emit nonsense.
            return None;
        }
        Transport::Xhttp { .. } | Transport::Mkcp { .. } => return None,
    };
    Some(value)
}

fn route(plan: &SessionPlan) -> Value {
    let final_tag = match plan.policy {
        RoutingMode::Direct => "direct",
        _ => PROXY_TAG,
    };

    let mut rules = Vec::new();
    if plan.policy != RoutingMode::Direct {
        rules.push(json!({"ip_is_private": true, "outbound": "direct"}));
    }

    if plan.policy == RoutingMode::Rule {
        for rule_set in &plan.rule_sets {
            match rule_set.kind {
                RuleSetKind::CnSites | RuleSetKind::CnIp => rules.push(json!({
                    "rule_set": [rule_set.tag],
                    "outbound": "direct",
                })),
                RuleSetKind::Ads => {
                    if plan.block_ads {
                        rules.push(json!({"rule_set": [rule_set.tag], "action": "reject"}));
                    }
                }
            }
        }
        for rule in &plan.rules {
            if let Some(value) = custom_rule(rule) {
                rules.push(value);
            }
        }
    }

    let mut route = Map::new();
    route.insert("rules".into(), Value::Array(rules));
    if !plan.rule_sets.is_empty() {
        let sets: Vec<Value> = plan
            .rule_sets
            .iter()
            .map(|set| {
                json!({
                    "tag": set.tag,
                    "type": "local",
                    "format": "binary",
                    "path": set.path,
                })
            })
            .collect();
        route.insert("rule_set".into(), Value::Array(sets));
    }
    route.insert("final".into(), json!(final_tag));
    route.insert("auto_detect_interface".into(), json!(true));
    Value::Object(route)
}

fn custom_rule(rule: &crate::plan::CustomRule) -> Option<Value> {
    let outbound = match rule.action {
        RuleAction::Proxy => PROXY_TAG,
        RuleAction::Direct => "direct",
        RuleAction::Block => "block",
    };
    let mut value = Map::new();
    match rule.kind {
        crate::plan::RuleKind::Domain => {
            value.insert("domain".into(), json!([rule.value]));
        }
        crate::plan::RuleKind::DomainSuffix => {
            value.insert("domain_suffix".into(), json!([rule.value]));
        }
        crate::plan::RuleKind::IpCidr => {
            value.insert("ip_cidr".into(), json!([rule.value]));
        }
        crate::plan::RuleKind::Process => {
            value.insert("process_name".into(), json!([rule.value]));
        }
        crate::plan::RuleKind::GeoIp | crate::plan::RuleKind::GeoSite => {
            // Geo databases are rule sets in sing-box 1.11+; the value is the
            // rule set tag shipped next to the app.
            value.insert("rule_set".into(), json!([rule.value]));
        }
    }
    value.insert("outbound".into(), json!(outbound));
    Some(Value::Object(value))
}

/// Unused today but kept because the emitter will need it once QUIC transport
/// support lands on sing-box.
#[allow(dead_code)]
fn quic_security_name(security: QuicSecurity) -> &'static str {
    match security {
        QuicSecurity::None => "none",
        QuicSecurity::Aes128Gcm => "aes-128-gcm",
        QuicSecurity::Chacha20Poly1305 => "chacha20-poly1305",
    }
}

#[allow(dead_code)]
fn obfs_name(obfs: &Hysteria2Obfs) -> &'static str {
    obfs.kind.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{ApiPlan, InboundPlan, RuleKind, RuleSetRef, TunPlan};
    use v2rayr_domain::model::MuxProtocol;
    use v2rayr_domain::{MuxOptions, Secret};

    fn plan(profile: Profile) -> SessionPlan {
        SessionPlan {
            profile,
            core_override: None,
            inbound: InboundPlan {
                listen: "127.0.0.1".into(),
                port: 10808,
                http_port: Some(10809),
                allow_lan: false,
                username: None,
                password: None,
            },
            api: ApiPlan {
                port: 10812,
                secret: "s3cret".into(),
            },
            tun: None,
            policy: RoutingMode::Rule,
            dns: Default::default(),
            rules: Vec::new(),
            rule_sets: Vec::new(),
            sniffing: true,
            tcp_fast_open: true,
            mux: None,
            log_level: "info".into(),
            block_ads: false,
            data_dir: "/tmp/v2rayr".into(),
        }
    }

    fn vless_ws() -> Profile {
        Profile::new(
            "node",
            "example.com",
            443,
            Protocol::Vless {
                id: uuid::Uuid::nil(),
                flow: None,
                encryption: None,
            },
        )
        .with_transport(Transport::Ws {
            path: "/ws".into(),
            host: Some("cdn.example.com".into()),
            early_data: None,
        })
        .with_tls(v2rayr_domain::TlsOptions {
            sni: Some("cdn.example.com".into()),
            alpn: vec!["h2".into()],
            ..Default::default()
        })
    }

    fn emitted(plan: &SessionPlan) -> Value {
        let config = emit(plan).expect("emit");
        serde_json::from_str(&config.body).expect("json")
    }

    #[test]
    fn mixed_inbound_uses_the_configured_port() {
        let value = emitted(&plan(vless_ws()));
        assert_eq!(value["inbounds"][0]["type"], "mixed");
        assert_eq!(value["inbounds"][0]["listen_port"], 10808);
        assert_eq!(value["inbounds"][0]["sniff"], true);
    }

    #[test]
    fn vless_ws_keeps_transport_and_tls() {
        let value = emitted(&plan(vless_ws()));
        let outbound = &value["outbounds"][0];
        assert_eq!(outbound["type"], "vless");
        assert_eq!(outbound["transport"]["type"], "ws");
        assert_eq!(outbound["transport"]["path"], "/ws");
        assert_eq!(outbound["transport"]["headers"]["Host"], "cdn.example.com");
        assert_eq!(outbound["tls"]["server_name"], "cdn.example.com");
        assert_eq!(outbound["tls"]["alpn"][0], "h2");
    }

    #[test]
    fn clash_api_is_always_present() {
        let value = emitted(&plan(vless_ws()));
        assert_eq!(
            value["experimental"]["clash_api"]["external_controller"],
            "127.0.0.1:10812"
        );
        assert_eq!(value["experimental"]["clash_api"]["secret"], "s3cret");
    }

    #[test]
    fn tun_adds_a_tun_inbound_and_drops_the_http_port() {
        let mut plan = plan(vless_ws());
        plan.inbound.http_port = None;
        plan.tun = Some(TunPlan {
            mtu: 1500,
            interface: "utun9".into(),
        });
        let value = emitted(&plan);
        assert_eq!(value["inbounds"][1]["type"], "tun");
        assert_eq!(value["inbounds"][1]["mtu"], 1500);
        assert_eq!(value["inbounds"].as_array().expect("array").len(), 2);
    }

    #[test]
    fn rule_mode_passes_private_addresses_directly() {
        let value = emitted(&plan(vless_ws()));
        assert_eq!(value["route"]["rules"][0]["ip_is_private"], true);
        assert_eq!(value["route"]["final"], PROXY_TAG);
    }

    #[test]
    fn direct_policy_ends_on_direct_and_skips_proxy_rules() {
        let mut plan = plan(vless_ws());
        plan.policy = RoutingMode::Direct;
        let value = emitted(&plan);
        assert_eq!(value["route"]["final"], "direct");
        assert_eq!(value["dns"]["final"], "dns-local");
        assert!(
            value["route"]["rules"]
                .as_array()
                .expect("rules")
                .is_empty()
        );
    }

    #[test]
    fn custom_rules_are_emitted_in_order() {
        let mut plan = plan(vless_ws());
        plan.rules = vec![
            crate::plan::CustomRule {
                kind: RuleKind::DomainSuffix,
                value: "corp.example".into(),
                action: RuleAction::Direct,
            },
            crate::plan::CustomRule {
                kind: RuleKind::Domain,
                value: "ads.example".into(),
                action: RuleAction::Block,
            },
        ];
        let value = emitted(&plan);
        let rules = value["route"]["rules"].as_array().expect("rules");
        assert_eq!(
            rules.last().expect("last")["domain"],
            json!(["ads.example"])
        );
        assert_eq!(rules.last().expect("last")["outbound"], "block");
    }

    #[test]
    fn rule_sets_are_declared_and_referenced() {
        let mut plan = plan(vless_ws());
        plan.rule_sets = vec![RuleSetRef {
            tag: "geosite-cn".into(),
            path: "/data/geosite-cn.srs".into(),
            kind: RuleSetKind::CnSites,
        }];
        let value = emitted(&plan);
        assert_eq!(
            value["route"]["rule_set"][0]["path"],
            "/data/geosite-cn.srs"
        );
        let rules = value["route"]["rules"].as_array().expect("rules");
        assert!(
            rules
                .iter()
                .any(|rule| rule["rule_set"][0] == "geosite-cn" && rule["outbound"] == "direct")
        );
    }

    #[test]
    fn mux_is_emitted_only_when_enabled() {
        let mut plan = plan(vless_ws());
        plan.mux = Some(MuxOptions {
            enabled: true,
            concurrency: Some(8),
            protocol: Some(MuxProtocol::Smux),
        });
        let value = emitted(&plan);
        assert_eq!(value["outbounds"][0]["multiplex"]["enabled"], true);
        assert_eq!(value["outbounds"][0]["multiplex"]["max_streams"], 8);
    }

    #[test]
    fn hysteria2_obfs_and_ports_are_kept() {
        let profile = Profile::new(
            "hy2",
            "hy.example.com",
            8443,
            Protocol::Hysteria2 {
                password: Secret::new("pw"),
                obfs: Some(Hysteria2Obfs {
                    kind: v2rayr_domain::ObfsKind::Salamander,
                    password: Secret::new("obfs"),
                }),
                up_mbps: Some(50),
                down_mbps: Some(200),
                ports: Some("20000-30000".into()),
            },
        );
        let value = emitted(&plan(profile));
        let outbound = &value["outbounds"][0];
        assert_eq!(outbound["type"], "hysteria2");
        assert_eq!(outbound["obfs"]["type"], "salamander");
        assert_eq!(outbound["obfs"]["password"], "obfs");
        assert_eq!(outbound["server_ports"], "20000-30000");
    }

    #[test]
    fn a_bad_log_level_falls_back_to_info() {
        let mut plan = plan(vless_ws());
        plan.log_level = "verbose".into();
        assert_eq!(emitted(&plan)["log"]["level"], "info");
    }

    #[test]
    fn unsupported_nodes_are_rejected_before_emission() {
        let node = Profile::new(
            "x",
            "a.com",
            443,
            Protocol::Naive(v2rayr_domain::Naive {
                username: "u".into(),
                password: Secret::new("p"),
                quic: false,
            }),
        );
        assert!(emit(&plan(node)).is_err());
    }
}
