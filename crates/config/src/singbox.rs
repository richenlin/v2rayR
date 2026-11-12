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
    emit_with_ingress(plan, None, None)
}

pub(crate) fn emit_with_ingress(
    plan: &SessionPlan,
    application_inbound: Option<Value>,
    application_rules: Option<Vec<Value>>,
) -> Result<EmittedConfig, ConfigError> {
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
    root.insert("dns".into(), dns(plan)?);

    let mut inbounds = vec![mixed_inbound(plan)];
    if let Some(lan) = &plan.inbound.lan {
        inbounds.push(
            json!({"type":"mixed","tag":"lan-in","listen":lan.address,"listen_port":lan.http_port,
            "users":[{"username":crate::LAN_USERNAME,"password":lan.password}]}),
        );
    }
    if let Some(tun) = &plan.tun {
        inbounds.push(tun_inbound(tun));
    }
    inbounds.extend(application_inbound);
    root.insert("inbounds".into(), Value::Array(inbounds));
    if let Protocol::WireGuard(wg) = &plan.profile.protocol {
        if wg.local_address.is_empty() {
            return Err(ConfigError::Inconsistent(
                "WireGuard 节点缺少本地地址前缀".into(),
            ));
        }
        let mut peer = json!({
            "address": plan.profile.address, "port": plan.profile.port, "public_key": wg.public_key,
            "allowed_ips": ["0.0.0.0/0", "::/0"]
        });
        if let Some(key) = &wg.preshared_key {
            peer["pre_shared_key"] = json!(key.expose());
        }
        if let Some(reserved) = plan.profile.extra.get("reserved").and_then(Value::as_str) {
            let bytes: Result<Vec<u8>, _> = reserved
                .split(',')
                .map(|part| part.trim().parse())
                .collect();
            let bytes = bytes.ok().filter(|bytes| bytes.len() == 3).ok_or_else(|| {
                ConfigError::Inconsistent("WireGuard reserved 须为三个 0–255 的字节".into())
            })?;
            peer["reserved"] = json!(bytes);
        }
        root.insert(
            "endpoints".into(),
            json!([{
                "type": "wireguard", "tag": PROXY_TAG, "system": false, "address": wg.local_address,
                "private_key": wg.private_key.expose(), "mtu": wg.mtu, "peers": [peer]
            }]),
        );
        root.insert(
            "outbounds".into(),
            json!([{"type":"direct","tag":"direct"}]),
        );
    } else {
        root.insert("outbounds".into(), outbounds(plan)?);
    }
    root.insert("route".into(), route(plan, application_rules));
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
    if plan.dns.fake_ip {
        emitted = emitted.with_note("FakeIP 需要 TUN，本版本未启用，已忽略该设置");
    }
    if plan.rule_sets.is_empty() && plan.policy == RoutingMode::Rule {
        emitted = emitted.with_note("规则模式按有序路由计划匹配，未命中直连");
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
    Value::Object(inbound)
}

fn tun_inbound(tun: &crate::plan::TunPlan) -> Value {
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
    Value::Object(inbound)
}

fn dns(plan: &SessionPlan) -> Result<Value, ConfigError> {
    let mut servers = vec![json!({"tag": "dns-local", "type": "local"})];
    let addresses: Vec<&str> = if plan.dns.servers.is_empty() {
        vec!["https://1.1.1.1/dns-query"]
    } else {
        plan.dns.servers.iter().map(String::as_str).collect()
    };
    if plan.policy != RoutingMode::Direct {
        for (index, address) in addresses.into_iter().enumerate() {
            servers.push(dns_server(address, index)?);
        }
    }
    Ok(json!({
        "servers": servers,
        "final": if plan.policy == RoutingMode::Direct { "dns-local" } else { "dns-remote-0" },
        "strategy": dns_strategy(&plan.dns.strategy),
    }))
}

fn dns_server(address: &str, index: usize) -> Result<Value, ConfigError> {
    let invalid = || ConfigError::InvalidPlan(format!("第 {} 个 DNS 地址不受支持", index + 1));
    let address = if let Ok(ip) = address.parse::<std::net::Ipv6Addr>() {
        format!("udp://[{ip}]")
    } else if !address.contains("://") {
        format!("udp://{address}")
    } else {
        address.to_owned()
    };
    let url = url::Url::parse(&address).map_err(|_| invalid())?;
    let kind = url.scheme();
    if !matches!(kind, "https" | "tls" | "tcp" | "udp" | "quic" | "h3")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    let host = url.host().ok_or_else(invalid)?;
    let server = match &host {
        url::Host::Domain(domain) => domain.to_string(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => ip.to_string(),
    };
    let mut value = json!({"type": kind, "tag": format!("dns-remote-{index}"), "server": server, "detour": PROXY_TAG});
    if matches!(host, url::Host::Domain(_)) {
        value["domain_resolver"] = json!("dns-local");
    }
    if let Some(port) = url.port() {
        value["server_port"] = json!(port);
    }
    if matches!(kind, "https" | "h3") && !matches!(url.path(), "" | "/") {
        value["path"] = json!(url.path());
    }
    Ok(value)
}

fn dns_strategy(value: &str) -> &str {
    match value {
        "prefer_ipv4" | "prefer_ipv6" | "ipv4_only" | "ipv6_only" => value,
        _ => "prefer_ipv4",
    }
}

fn outbounds(plan: &SessionPlan) -> Result<Value, ConfigError> {
    let mut outbounds = vec![proxy_outbound(plan)?];
    outbounds.push(json!({"type": "direct", "tag": "direct"}));
    Ok(Value::Array(outbounds))
}

fn proxy_outbound(plan: &SessionPlan) -> Result<Value, ConfigError> {
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
            if profile.extra.get("pinned_cert_sha256").is_some() {
                return Err(ConfigError::Unsupported {
                    core: "sing-box",
                    reason: "Hysteria2 的 pinSHA256 是证书指纹，不能作为 sing-box 的公钥指纹；请使用可验证的 TLS 证书".into(),
                });
            }
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
                outbound.remove("server_port");
                outbound.insert("server_ports".into(), json!(hopping_ports(ports)?));
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
        Protocol::WireGuard(_) => unreachable!("WireGuard is emitted as an endpoint"),
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
    Ok(Value::Object(outbound))
}

fn hopping_ports(raw: &str) -> Result<Vec<String>, ConfigError> {
    let port = |value: &str| value.trim().parse::<u16>().ok().filter(|&port| port > 0);
    raw.split(',')
        .map(|item| {
            let range = item.split_once(['-', ':']).unwrap_or((item, item));
            match (port(range.0), port(range.1)) {
                (Some(start), Some(end)) if start <= end => Ok(format!("{start}:{end}")),
                _ => Err(ConfigError::Inconsistent(
                    "Hysteria2 端口跳跃须使用 1–65535 的端口或升序范围".into(),
                )),
            }
        })
        .collect()
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

fn route(plan: &SessionPlan, application_rules: Option<Vec<Value>>) -> Value {
    let final_tag = match plan.policy {
        RoutingMode::Global => PROXY_TAG,
        _ => "direct",
    };

    let mut rules = Vec::new();
    if plan.sniffing {
        rules.push(json!({"action": "sniff"}));
    }

    if plan.policy == RoutingMode::Rule {
        let custom = application_rules.unwrap_or_else(|| {
            crate::routing::batches(&plan.rules)
                .into_iter()
                .filter_map(|batch| {
                    let mut value = custom_rule(&batch[0])?;
                    if batch.len() > 1 {
                        let key = if batch[0].kind == crate::RuleKind::Domain {
                            "domain"
                        } else {
                            "domain_suffix"
                        };
                        value[key] = json!(batch.iter().map(|r| &r.value).collect::<Vec<_>>());
                    }
                    Some(value)
                })
                .collect()
        });
        let custom_start = rules.len();
        for rule_set in &plan.rule_sets {
            match rule_set.kind {
                RuleSetKind::CnSites | RuleSetKind::CnIp => rules.push(json!({
                    "rule_set": [rule_set.tag],
                    "outbound": "direct",
                })),
                RuleSetKind::ReferenceOnly => {}
                RuleSetKind::Ads => {
                    if plan.block_ads {
                        rules.push(json!({"rule_set": [rule_set.tag], "action": "reject"}));
                    }
                }
            }
        }
        if plan.custom_rules_first {
            rules.splice(custom_start..custom_start, custom);
        } else {
            rules.extend(custom);
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
    route.insert("default_domain_resolver".into(), json!("dns-local"));
    route.insert("auto_detect_interface".into(), json!(true));
    Value::Object(route)
}

pub(crate) fn custom_rule(rule: &crate::plan::CustomRule) -> Option<Value> {
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
    if outbound == "block" {
        value.insert("action".into(), json!("reject"));
    } else {
        value.insert("outbound".into(), json!(outbound));
    }
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
                dokodemo: None,
                lan: None,
            },
            api: ApiPlan {
                port: 10812,
                secret: "s3cret".into(),
            },
            tun: None,
            policy: RoutingMode::Rule,
            dns: Default::default(),
            rules: Vec::new(),
            custom_rules_first: false,
            rule_sets: Vec::new(),
            sniffing: true,
            tcp_fast_open: true,
            mux: None,
            log_level: "info".into(),
            block_ads: false,
            data_dir: "/tmp/v2rayr".into(),
        }
    }

    #[test]
    fn application_ingress_preserves_first_match_and_isolates_the_mixed_port() {
        use crate::CustomRule;
        use crate::application_ingress;
        use v2rayr_domain::RuleApplication;

        let ports = std::env::var("V2RAYR_APPLICATION_PROBE_PORTS").ok();
        let ports: Vec<u16> = ports
            .as_deref()
            .unwrap_or("10808,10809,10812,10813,10814")
            .split(',')
            .map(|port| port.parse().unwrap())
            .collect();
        let mut plan = plan(Profile::new(
            "probe",
            "127.0.0.1",
            ports[4],
            Protocol::Socks {
                username: None,
                password: None,
            },
        ));
        plan.inbound.port = ports[0];
        plan.inbound.http_port = Some(ports[1]);
        plan.api.port = ports[2];
        plan.sniffing = false;
        let actions = [RuleAction::Proxy, RuleAction::Block, RuleAction::Direct];
        plan.rules = actions
            .into_iter()
            .map(|action| CustomRule {
                kind: RuleKind::Domain,
                value: "localhost".into(),
                action,
            })
            .collect();
        let identity = |path: &str| RuleApplication {
            path: path.into(),
            sha256: "a".repeat(64),
        };
        let scopes = vec![
            vec![identity("/one/Client")],
            vec![identity("/two/Client")],
            vec![],
        ];
        let secret = "local-probe-only".repeat(2);
        let (config, provider) =
            application_ingress::emit(&plan, &scopes, ports[3], secret, "/usr/bin/sing-box".into())
                .unwrap();
        let json: Value = serde_json::from_str(&config.body).unwrap();
        assert!(json["inbounds"][0]["users"].is_null());
        assert_eq!(json["inbounds"][1]["users"].as_array().unwrap().len(), 3);
        let rules = json["route"]["rules"].as_array().unwrap();
        assert_eq!(rules[0]["auth_user"], json!(["application-0"]));
        assert_eq!(rules[0]["inbound"], json!(["application-in"]));
        assert_eq!(rules[0]["outbound"], "proxy");
        assert_eq!(rules[1]["auth_user"], json!(["application-1"]));
        assert_eq!(rules[1]["action"], "reject");
        assert_eq!(rules[2]["outbound"], "direct");
        assert!(rules[2]["auth_user"].is_null());
        plan.policy = RoutingMode::Global;
        assert!(
            application_ingress::emit(
                &plan,
                &scopes,
                ports[3],
                "x".repeat(32),
                "/usr/bin/sing-box".into(),
            )
            .is_err()
        );

        // The native probe supplies a private temp directory and unused ports.
        if let Some(path) = std::env::var_os("V2RAYR_APPLICATION_PROBE_DIR") {
            let path = std::path::Path::new(&path);
            std::fs::write(path.join("kernel.json"), config.body).unwrap();
            std::fs::write(
                path.join("provider.json"),
                serde_json::to_vec(&provider).unwrap(),
            )
            .unwrap();
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
    #[ignore = "requires the release sing-box and Xray binaries in V2RAYR_TEST_SING_BOX and V2RAYR_TEST_XRAY"]
    fn imported_profiles_pass_release_kernel_checks() {
        use std::process::Command;
        let sing_box = std::env::var_os("V2RAYR_TEST_SING_BOX").unwrap();
        let xray = std::env::var_os("V2RAYR_TEST_XRAY").unwrap();
        let output = |binary: &std::ffi::OsStr, args: &[&str]| {
            let result = Command::new(binary).args(args).output().unwrap();
            assert!(result.status.success(), "key generation failed");
            String::from_utf8(result.stdout).unwrap()
        };
        let keys = output(&xray, &["x25519"]);
        let public = keys
            .lines()
            .find_map(|line| line.strip_prefix("Password (PublicKey): "))
            .unwrap();
        let encryption = output(&xray, &["vlessenc"]);
        let encryption = encryption
            .lines()
            .find_map(|line| line.strip_prefix("\"encryption\": \""))
            .unwrap()
            .trim_end_matches('"');
        let keys = output(&sing_box, &["generate", "wg-keypair"]);
        let key = |prefix| {
            let value = keys
                .lines()
                .find_map(|line| line.strip_prefix(prefix))
                .unwrap();
            url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
        };
        let id = "11111111-1111-4111-8111-111111111111";
        let fixtures = [
            ("Vision/TLS", format!("vless://{id}@example.com:8443?type=tcp&security=tls&sni=example.com&flow=xtls-rprx-vision"), CoreKind::SingBox),
            ("Vision/PQ REALITY", format!("vless://{id}@example.com:3389?type=tcp&security=reality&sni=example.com&fp=chrome&flow=xtls-rprx-vision&sid=04&pbk={public}&encryption={encryption}"), CoreKind::Xray),
            ("Hysteria2", "hy2://test@example.com:3389?sni=example.com&obfs=gecko&obfs-password=test&mport=3389,3390-3392".into(), CoreKind::SingBox),
            ("WireGuard", format!("wg://{}@192.0.2.1:51820?publickey={}&address=10.0.0.2%2F32&reserved=0,1,2", key("PrivateKey: "), key("PublicKey: ")), CoreKind::SingBox),
        ];
        let directory =
            std::env::temp_dir().join(format!("v2rayr-config-check-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let config = directory.join("fixture.json");
        for (name, link, expected) in fixtures {
            let profile = v2rayr_domain::parse_share_link(&link)
                .unwrap_or_else(|_| panic!("{name} import failed"));
            let mut plan = plan(profile);
            plan.data_dir = directory.display().to_string();
            let emitted = crate::emit(&plan).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(emitted.core, expected, "{name}");
            std::fs::write(&config, emitted.body).unwrap();
            let output = if expected == CoreKind::Xray {
                Command::new(&xray)
                    .arg("-test")
                    .arg("-c")
                    .arg(&config)
                    .output()
            } else {
                Command::new(&sing_box)
                    .arg("check")
                    .arg("-c")
                    .arg(&config)
                    .arg("-D")
                    .arg(&directory)
                    .output()
            }
            .unwrap();
            assert!(
                output.status.success(),
                "{name}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn mixed_inbound_uses_the_configured_port() {
        let value = emitted(&plan(vless_ws()));
        assert_eq!(value["inbounds"][0]["type"], "mixed");
        assert_eq!(value["inbounds"][0]["listen_port"], 10808);
        assert!(value["inbounds"][0].get("sniff").is_none());
        assert_eq!(value["route"]["rules"][0]["action"], "sniff");
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
    fn rule_mode_defaults_direct_without_preempting_custom_private_rules() {
        let value = emitted(&plan(vless_ws()));
        assert!(
            value["route"]["rules"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["ip_is_private"].is_null())
        );
        assert_eq!(value["route"]["final"], "direct");
    }

    #[test]
    fn direct_policy_ends_on_direct_and_skips_proxy_rules() {
        let mut plan = plan(vless_ws());
        plan.policy = RoutingMode::Direct;
        let value = emitted(&plan);
        assert_eq!(value["route"]["final"], "direct");
        assert_eq!(value["dns"]["final"], "dns-local");
        assert_eq!(value["route"]["rules"], json!([{"action": "sniff"}]));
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
        assert_eq!(rules.last().expect("last")["action"], "reject");
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
        assert_eq!(outbound["server_ports"], json!(["20000:30000"]));
        assert!(outbound.get("server_port").is_none());
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
