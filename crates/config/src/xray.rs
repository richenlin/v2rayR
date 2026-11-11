//! Xray emitter.
//!
//! Xray is the fallback for transports sing-box dropped (xhttp, mKCP, QUIC) and
//! for VLESS + Reality deployments that rely on XTLS Vision. It cannot serve a
//! "mixed" inbound, so SOCKS and HTTP are separate ports — the emitted config
//! says so in `notes`, and the app points the system proxy at the SOCKS port.

use serde_json::{Map, Value, json};
use v2rayr_domain::{CoreKind, Profile, Protocol, Transport, VmessSecurity};

use crate::capability::choose_core;
use crate::plan::{RoutingMode, RuleKind, RuleSetKind, SessionPlan};
use v2rayr_domain::TlsOptions;

use crate::{ConfigError, EmittedConfig, Format};

pub const PROXY_TAG: &str = "proxy";

pub fn emit(plan: &SessionPlan) -> Result<EmittedConfig, ConfigError> {
    if choose_core(&plan.profile, Some(CoreKind::Xray)).is_err() {
        return Err(ConfigError::Unsupported {
            core: "xray",
            reason: format!(
                "{} over {}",
                plan.profile.protocol.kind(),
                plan.profile.transport.kind()
            ),
        });
    }

    let mut root = Map::new();
    root.insert(
        "log".into(),
        json!({"loglevel": log_level(&plan.log_level), "access": "none"}),
    );
    root.insert("inbounds".into(), inbounds(plan));
    root.insert("outbounds".into(), outbounds(plan));
    root.insert("routing".into(), routing(plan));
    root.insert("dns".into(), dns(plan));
    root.insert("stats".into(), json!({}));
    // An HTTP expvar endpoint the app polls with one kept-alive connection; the
    // gRPC StatsService could only be read by spawning `xray api` every second.
    root.insert(
        "metrics".into(),
        json!({"tag": "v2rayr-metrics", "listen": format!("127.0.0.1:{}", plan.api.port)}),
    );
    root.insert(
        "policy".into(),
        json!({"levels": {"0": {"handshake": 4, "connIdle": 300}},
            "system": {"statsInboundUplink": true, "statsInboundDownlink": true}}),
    );

    let body = serde_json::to_string_pretty(&Value::Object(root))?;
    let mut emitted = EmittedConfig::new(CoreKind::Xray, Format::Json, body)
        .with_note("Xray 不支持混合入站：SOCKS 与 HTTP 分别监听，系统代理使用 SOCKS 端口");
    if plan.tun.is_some() {
        emitted = emitted.with_note("Xray 版本不提供 TUN，请使用 sing-box 的 TUN 模式");
    }
    if plan.dns.fake_ip {
        emitted = emitted.with_note("FakeIP 需要 sing-box，本版本已忽略该设置");
    }
    Ok(emitted)
}

fn log_level(level: &str) -> &str {
    match level {
        "trace" | "debug" | "info" | "warning" | "error" | "none" => level,
        _ => "warning",
    }
}

fn sniffing(enabled: bool) -> Value {
    if enabled {
        json!({"enabled": true, "destOverride": ["http", "tls", "quic"], "routeOnly": false})
    } else {
        json!({"enabled": false})
    }
}

fn inbounds(plan: &SessionPlan) -> Value {
    let mut socks = Map::new();
    socks.insert("tag".into(), json!("socks-in"));
    socks.insert("listen".into(), json!(plan.inbound.listen));
    socks.insert("port".into(), json!(plan.inbound.port));
    socks.insert("protocol".into(), json!("socks"));
    let mut settings = Map::new();
    settings.insert("auth".into(), json!("noauth"));
    settings.insert("udp".into(), json!(true));
    socks.insert("settings".into(), Value::Object(settings));
    socks.insert("sniffing".into(), sniffing(plan.sniffing));

    let http_port = plan.inbound.http_port.unwrap_or(plan.inbound.port + 1);
    let mut http = Map::new();
    http.insert("tag".into(), json!("http-in"));
    http.insert("listen".into(), json!(plan.inbound.listen));
    http.insert("port".into(), json!(http_port));
    http.insert("protocol".into(), json!("http"));
    http.insert("settings".into(), json!({"allowTransparent": false}));
    http.insert("sniffing".into(), sniffing(plan.sniffing));

    let mut inbounds = vec![Value::Object(socks), Value::Object(http)];
    if let Some((listen, address, port)) = &plan.inbound.dokodemo {
        inbounds.push(json!({
            "tag": "dokodemo-in", "listen": "127.0.0.1", "port": listen,
            "protocol": "dokodemo-door",
            "settings": {"address": address, "port": port, "network": "tcp,udp", "followRedirect": false}
        }));
    }
    if let Some(lan) = &plan.inbound.lan {
        let accounts = json!([{"user":crate::LAN_USERNAME,"pass":lan.password}]);
        inbounds.push(
            json!({"tag":"lan-socks","listen":lan.address,"port":lan.socks_port,
            "protocol":"socks","settings":{"auth":"password","accounts":accounts,"udp":false}}),
        );
        inbounds.push(
            json!({"tag":"lan-http","listen":lan.address,"port":lan.http_port,
            "protocol":"http","settings":{"allowTransparent":false,"accounts":accounts}}),
        );
    }
    Value::Array(inbounds)
}

fn outbounds(plan: &SessionPlan) -> Value {
    let proxy = proxy_outbound(plan);
    let direct =
        json!({"tag": "direct", "protocol": "freedom", "settings": {"domainStrategy": "UseIP"}});
    let block = json!({"tag": "block", "protocol": "blackhole", "settings": {}});
    // Xray uses the first outbound as the default route.
    if plan.policy == RoutingMode::Direct {
        json!([direct, proxy, block])
    } else {
        json!([proxy, direct, block])
    }
}

fn proxy_outbound(plan: &SessionPlan) -> Value {
    let profile = &plan.profile;
    let mut outbound = Map::new();
    outbound.insert("tag".into(), json!(PROXY_TAG));
    outbound.insert("protocol".into(), json!(profile.protocol.kind()));
    outbound.insert("settings".into(), outbound_settings(profile));
    outbound.insert("streamSettings".into(), stream_settings(profile));
    if let Some(mux) = plan.mux.as_ref().filter(|mux| mux.enabled) {
        outbound.insert(
            "mux".into(),
            json!({"enabled": true, "concurrency": mux.concurrency}),
        );
    } else {
        outbound.insert("mux".into(), json!({"enabled": false}));
    }
    Value::Object(outbound)
}

fn outbound_settings(profile: &Profile) -> Value {
    match &profile.protocol {
        Protocol::Vless {
            id,
            flow,
            encryption,
        } => {
            let mut user = Map::new();
            user.insert("id".into(), json!(id.to_string()));
            user.insert(
                "encryption".into(),
                json!(encryption.clone().unwrap_or_else(|| "none".into())),
            );
            user.insert("level".into(), json!(0));
            if let Some(flow) = flow {
                user.insert("flow".into(), json!(flow.as_str()));
            }
            json!({"vnext": [{
                "address": profile.address,
                "port": profile.port,
                "users": [Value::Object(user)],
            }]})
        }
        Protocol::Vmess {
            id,
            alter_id,
            security,
        } => json!({"vnext": [{
            "address": profile.address,
            "port": profile.port,
            "users": [{
                "id": id.to_string(),
                "alterId": alter_id,
                "security": vmess_security(*security),
                "level": 0,
            }],
        }]}),
        Protocol::Trojan { password } => json!({"servers": [{
            "address": profile.address,
            "port": profile.port,
            "password": password.expose(),
        }]}),
        Protocol::Shadowsocks { method, password } => json!({"servers": [{
            "address": profile.address,
            "port": profile.port,
            "method": method.as_str(),
            "password": password.expose(),
        }]}),
        Protocol::Socks { username, password } => {
            let mut server = Map::new();
            server.insert("address".into(), json!(profile.address));
            server.insert("port".into(), json!(profile.port));
            if let (Some(username), Some(password)) = (username, password) {
                server.insert(
                    "users".into(),
                    json!([{"user": username, "pass": password.expose()}]),
                );
            }
            json!({"servers": [Value::Object(server)]})
        }
        Protocol::Http { username, password } => {
            let mut server = Map::new();
            server.insert("address".into(), json!(profile.address));
            server.insert("port".into(), json!(profile.port));
            if let (Some(username), Some(password)) = (username, password) {
                server.insert(
                    "users".into(),
                    json!([{"user": username, "pass": password.expose()}]),
                );
            }
            json!({"servers": [Value::Object(server)]})
        }
        // Rejected by `choose_core` first.
        _ => json!({}),
    }
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

fn stream_settings(profile: &Profile) -> Value {
    let mut stream = Map::new();
    stream.insert("network".into(), json!(network(&profile.transport)));
    stream.insert("security".into(), json!(security(profile)));

    let _ = match security(profile) {
        "tls" => profile
            .tls
            .as_ref()
            .map(|tls| stream.insert("tlsSettings".into(), tls_settings(tls, &profile.address))),
        "reality" => profile.tls.as_ref().map(|tls| {
            stream.insert(
                "realitySettings".into(),
                reality_settings(tls, &profile.address),
            )
        }),
        _ => None,
    };

    match &profile.transport {
        Transport::Raw => {}
        Transport::Ws { path, host, .. } => {
            let mut ws = Map::new();
            ws.insert("path".into(), json!(path));
            if let Some(host) = host {
                ws.insert("headers".into(), json!({"Host": host}));
            }
            stream.insert("wsSettings".into(), Value::Object(ws));
        }
        Transport::Grpc {
            service_name,
            multi_mode,
        } => {
            let mut grpc = Map::new();
            grpc.insert("serviceName".into(), json!(service_name));
            if *multi_mode {
                grpc.insert("multiMode".into(), json!(true));
            }
            stream.insert("grpcSettings".into(), Value::Object(grpc));
        }
        Transport::HttpUpgrade { path, host } => {
            stream.insert(
                "httpupgradeSettings".into(),
                json!({"path": path, "host": host.clone().unwrap_or_default()}),
            );
        }
        Transport::Http { path, hosts } => {
            stream.insert("httpSettings".into(), json!({"path": path, "host": hosts}));
        }
        Transport::Xhttp {
            path,
            host,
            mode,
            extra,
        } => {
            let mut xhttp = Map::new();
            xhttp.insert("path".into(), json!(path));
            if let Some(host) = host {
                xhttp.insert("host".into(), json!(host));
            }
            if !mode.is_empty() {
                xhttp.insert("mode".into(), json!(mode));
            }
            if !extra.is_null() {
                xhttp.insert("extra".into(), extra.clone());
            }
            stream.insert("xhttpSettings".into(), Value::Object(xhttp));
        }
        Transport::Mkcp { header, seed } => {
            let mut mkcp = Map::new();
            mkcp.insert("header".into(), json!({"type": header.as_str()}));
            if let Some(seed) = seed {
                mkcp.insert("seed".into(), json!(seed));
            }
            stream.insert("kcpSettings".into(), Value::Object(mkcp));
        }
        Transport::Quic {
            security,
            key,
            header,
        } => {
            stream.insert(
                "quicSettings".into(),
                json!({
                    "security": security.as_str(),
                    "key": key.expose(),
                    "header": {"type": header},
                }),
            );
        }
    }
    Value::Object(stream)
}

fn network(transport: &Transport) -> &'static str {
    match transport {
        Transport::Raw => "tcp",
        Transport::Ws { .. } => "ws",
        Transport::Grpc { .. } => "grpc",
        Transport::HttpUpgrade { .. } => "httpupgrade",
        Transport::Http { .. } => "http",
        Transport::Xhttp { .. } => "xhttp",
        Transport::Mkcp { .. } => "mkcp",
        Transport::Quic { .. } => "quic",
    }
}

fn security(profile: &Profile) -> &'static str {
    match profile.tls.as_ref() {
        Some(tls) if tls.reality.is_some() => "reality",
        Some(_) => "tls",
        // Trojan is TLS by definition; a link without tls fields still speaks TLS.
        None if matches!(profile.protocol, Protocol::Trojan { .. }) => "tls",
        None => "none",
    }
}

fn tls_settings(tls: &TlsOptions, address: &str) -> Value {
    let mut settings = Map::new();
    settings.insert("serverName".into(), json!(tls.server_name(address)));
    if tls.insecure {
        settings.insert("allowInsecure".into(), json!(true));
    }
    if !tls.alpn.is_empty() {
        settings.insert("alpn".into(), json!(tls.alpn));
    }
    if let Some(fingerprint) = &tls.fingerprint {
        settings.insert(
            "settings".into(),
            json!({"fingerprint": fingerprint, "allowInsecure": tls.insecure}),
        );
    }
    Value::Object(settings)
}

fn reality_settings(tls: &TlsOptions, address: &str) -> Value {
    let mut settings = Map::new();
    settings.insert("serverName".into(), json!(tls.server_name(address)));
    settings.insert(
        "fingerprint".into(),
        json!(tls.fingerprint.clone().unwrap_or_else(|| "chrome".into())),
    );
    settings.insert("show".into(), json!(false));
    if let Some(reality) = &tls.reality {
        settings.insert("publicKey".into(), json!(reality.public_key));
        if !reality.short_id.is_empty() {
            settings.insert("shortId".into(), json!(reality.short_id));
        }
        if let Some(spider_x) = &reality.spider_x {
            settings.insert("spiderX".into(), json!(spider_x));
        }
    }
    Value::Object(settings)
}

fn routing(plan: &SessionPlan) -> Value {
    let mut rules = Vec::new();
    if plan.policy != RoutingMode::Direct {
        rules.push(json!({
            "type": "field",
            "ip": ["0.0.0.0/8", "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16", "172.16.0.0/12", "192.168.0.0/16", "::1/128", "fc00::/7", "fe80::/10"],
            "outboundTag": "direct",
        }));
    }

    if plan.policy == RoutingMode::Rule {
        let custom_start = rules.len();
        for rule_set in &plan.rule_sets {
            match rule_set.kind {
                RuleSetKind::CnSites => rules.push(json!({
                    "type": "field",
                    "domain": ["geosite:cn"],
                    "outboundTag": "direct",
                })),
                RuleSetKind::CnIp => rules.push(json!({
                    "type": "field",
                    "ip": ["geoip:cn"],
                    "outboundTag": "direct",
                })),
                RuleSetKind::Ads if plan.block_ads => rules.push(json!({
                    "type": "field",
                    "domain": ["geosite:category-ads-all"],
                    "outboundTag": "block",
                })),
                RuleSetKind::Ads => {}
            }
        }
        let custom = plan.rules.iter().filter_map(custom_rule);
        if plan.custom_rules_first {
            rules.splice(custom_start..custom_start, custom);
        } else {
            rules.extend(custom);
        }
    }

    json!({
        "domainStrategy": if plan.policy == RoutingMode::Direct { "AsIs" } else { "IPIfNonMatch" },
        "rules": rules,
    })
}

fn custom_rule(rule: &crate::plan::CustomRule) -> Option<Value> {
    let outbound = match rule.action {
        crate::plan::RuleAction::Proxy => PROXY_TAG,
        crate::plan::RuleAction::Direct => "direct",
        crate::plan::RuleAction::Block => "block",
    };
    let mut value = Map::new();
    value.insert("type".into(), json!("field"));
    match rule.kind {
        RuleKind::Domain => {
            value.insert("domain".into(), json!([format!("full:{}", rule.value)]));
        }
        RuleKind::DomainSuffix => {
            value.insert("domain".into(), json!([format!("domain:{}", rule.value)]));
        }
        RuleKind::IpCidr => {
            value.insert("ip".into(), json!([rule.value]));
        }
        RuleKind::Process => {
            value.insert("process".into(), json!([rule.value]));
        }
        RuleKind::GeoIp => {
            value.insert("ip".into(), json!([format!("geoip:{}", rule.value)]));
        }
        RuleKind::GeoSite => {
            value.insert("domain".into(), json!([format!("geosite:{}", rule.value)]));
        }
    }
    value.insert("outboundTag".into(), json!(outbound));
    Some(Value::Object(value))
}

fn dns(plan: &SessionPlan) -> Value {
    let servers: Vec<Value> = if plan.dns.servers.is_empty() {
        vec![json!("1.1.1.1"), json!("8.8.8.8")]
    } else {
        plan.dns
            .servers
            .iter()
            .map(|server| json!(server))
            .collect()
    };
    json!({"servers": servers, "queryStrategy": xray_dns_strategy(&plan.dns.strategy)})
}

fn xray_dns_strategy(value: &str) -> &str {
    match value {
        "prefer_ipv4" => "UseIP",
        "prefer_ipv6" => "UseIPv6",
        "ipv4_only" => "UseIPv4",
        "ipv6_only" => "UseIPv6",
        _ => "UseIP",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{ApiPlan, CustomRule, InboundPlan, RuleAction, RuleSetRef};
    use v2rayr_domain::{MkcpHeader, RealityOptions, Secret};

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
                secret: "s".into(),
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

    fn vless_reality() -> Profile {
        Profile::new(
            "node",
            "reality.example.com",
            443,
            Protocol::Vless {
                id: uuid::Uuid::nil(),
                flow: Some(v2rayr_domain::VlessFlow::XtlsRprxVision),
                encryption: None,
            },
        )
        .with_tls(TlsOptions {
            sni: Some("www.microsoft.com".into()),
            fingerprint: Some("chrome".into()),
            reality: Some(RealityOptions {
                public_key: "pbk".into(),
                short_id: "ab12".into(),
                spider_x: Some("/".into()),
            }),
            ..Default::default()
        })
    }

    fn emitted(plan: &SessionPlan) -> Value {
        let config = emit(plan).expect("emit");
        serde_json::from_str(&config.body).expect("json")
    }

    #[test]
    fn socks_and_http_get_separate_ports() {
        let value = emitted(&plan(vless_reality()));
        assert_eq!(value["inbounds"][0]["port"], 10808);
        assert_eq!(value["inbounds"][0]["protocol"], "socks");
        assert_eq!(value["inbounds"][1]["port"], 10809);
        assert_eq!(value["inbounds"][1]["protocol"], "http");
    }

    #[test]
    fn reality_is_expressed_in_stream_settings() {
        let value = emitted(&plan(vless_reality()));
        let proxy = &value["outbounds"][0];
        assert_eq!(proxy["protocol"], "vless");
        assert_eq!(proxy["streamSettings"]["security"], "reality");
        assert_eq!(
            proxy["streamSettings"]["realitySettings"]["publicKey"],
            "pbk"
        );
        assert_eq!(
            proxy["streamSettings"]["realitySettings"]["shortId"],
            "ab12"
        );
        assert_eq!(
            proxy["settings"]["vnext"][0]["users"][0]["flow"],
            "xtls-rprx-vision"
        );
    }

    #[test]
    fn xhttp_and_mkcp_are_kept() {
        let node = Profile::new(
            "x",
            "a.com",
            443,
            Protocol::Vless {
                id: uuid::Uuid::nil(),
                flow: None,
                encryption: None,
            },
        )
        .with_transport(Transport::Xhttp {
            path: "/x".into(),
            host: Some("cdn".into()),
            mode: "packet-up".into(),
            extra: serde_json::Value::Null,
        });
        let value = emitted(&plan(node));
        assert_eq!(value["outbounds"][0]["streamSettings"]["network"], "xhttp");
        assert_eq!(
            value["outbounds"][0]["streamSettings"]["xhttpSettings"]["mode"],
            "packet-up"
        );

        let mkcp = Profile::new(
            "k",
            "a.com",
            443,
            Protocol::Vless {
                id: uuid::Uuid::nil(),
                flow: None,
                encryption: None,
            },
        )
        .with_transport(Transport::Mkcp {
            header: MkcpHeader::Srtp,
            seed: Some("seed".into()),
        });
        let value = emitted(&plan(mkcp));
        assert_eq!(value["outbounds"][0]["streamSettings"]["network"], "mkcp");
        assert_eq!(
            value["outbounds"][0]["streamSettings"]["kcpSettings"]["header"]["type"],
            "srtp"
        );
    }

    #[test]
    fn direct_policy_puts_freedom_first() {
        let mut plan = plan(vless_reality());
        plan.policy = RoutingMode::Direct;
        let value = emitted(&plan);
        assert_eq!(value["outbounds"][0]["protocol"], "freedom");
        assert_eq!(value["routing"]["domainStrategy"], "AsIs");
    }

    #[test]
    fn custom_rules_use_xray_matchers() {
        let mut plan = plan(vless_reality());
        plan.rules = vec![CustomRule {
            kind: RuleKind::Domain,
            value: "exact.example".into(),
            action: RuleAction::Direct,
        }];
        let value = emitted(&plan);
        assert_eq!(
            value["routing"]["rules"][1]["domain"][0],
            "full:exact.example"
        );
    }

    #[test]
    fn rule_sets_map_to_geosite_and_geoip() {
        let mut plan = plan(vless_reality());
        plan.rule_sets = vec![
            RuleSetRef {
                tag: "cn".into(),
                path: "/x.srs".into(),
                kind: RuleSetKind::CnSites,
            },
            RuleSetRef {
                tag: "cnip".into(),
                path: "/y.srs".into(),
                kind: RuleSetKind::CnIp,
            },
        ];
        let value = emitted(&plan);
        let rules = value["routing"]["rules"].as_array().expect("rules");
        assert!(rules.iter().any(|rule| rule["domain"][0] == "geosite:cn"));
        assert!(rules.iter().any(|rule| rule["ip"][0] == "geoip:cn"));
    }

    #[test]
    fn trojan_password_goes_to_servers() {
        let node = Profile::new(
            "t",
            "t.example.com",
            443,
            Protocol::Trojan {
                password: Secret::new("hunter2"),
            },
        );
        let value = emitted(&plan(node));
        assert_eq!(
            value["outbounds"][0]["settings"]["servers"][0]["password"],
            "hunter2"
        );
        assert_eq!(value["outbounds"][0]["streamSettings"]["security"], "tls");
    }
}
