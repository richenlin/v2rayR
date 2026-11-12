use serde_json::Value;
use v2rayr_config::*;
use v2rayr_domain::{CoreKind, Profile, Protocol};
fn plan(core: CoreKind, policy: RoutingMode) -> SessionPlan {
    SessionPlan {
        profile: Profile::new(
            "routing",
            "127.0.0.1",
            19999,
            Protocol::Vless {
                id: uuid::Uuid::nil(),
                flow: None,
                encryption: None,
            },
        ),
        core_override: Some(core),
        inbound: InboundPlan {
            listen: "127.0.0.1".into(),
            port: 19880,
            http_port: if core == CoreKind::Xray {
                Some(19881)
            } else {
                None
            },
            dokodemo: None,
            lan: None,
        },
        api: ApiPlan {
            port: 19882,
            secret: "test".into(),
        },
        tun: None,
        policy,
        dns: DnsPlan::default(),
        rules: vec![
            rule("example.com", RuleAction::Direct),
            rule("proxy.net", RuleAction::Proxy),
            rule("other.net", RuleAction::Proxy),
            rule("blocked.net", RuleAction::Block),
        ],
        custom_rules_first: true,
        rule_sets: Vec::new(),
        sniffing: false,
        tcp_fast_open: false,
        mux: None,
        log_level: "warn".into(),
        block_ads: false,
        data_dir: ".".into(),
    }
}
fn rule(value: &str, action: RuleAction) -> CustomRule {
    CustomRule {
        kind: RuleKind::DomainSuffix,
        value: value.into(),
        action,
    }
}
#[test]
fn json_emitters_batch_adjacent_domains_without_reordering_actions() {
    for core in [CoreKind::Xray, CoreKind::SingBox] {
        let config = emit(&plan(core, RoutingMode::Rule)).unwrap();
        let json: Value = serde_json::from_str(&config.body).unwrap();
        let xray = core == CoreKind::Xray;
        let rules = json[if xray { "routing" } else { "route" }]["rules"]
            .as_array()
            .unwrap();
        assert_eq!(rules.len(), 3);
        let key = if xray { "domain" } else { "domain_suffix" };
        assert_eq!(rules[1][key].as_array().unwrap().len(), 2);
        assert_eq!(
            rules[0][if xray { "outboundTag" } else { "outbound" }],
            "direct"
        );
        assert_eq!(
            rules[1][if xray { "outboundTag" } else { "outbound" }],
            "proxy"
        );
        if xray {
            assert_eq!(rules[2]["outboundTag"], "block");
        } else {
            assert_eq!(rules[2]["action"], "reject");
        }
    }
}
#[test]
fn all_emitters_use_only_the_selected_policy_and_rule_defaults_direct() {
    for core in [CoreKind::Xray, CoreKind::SingBox, CoreKind::Mihomo] {
        for policy in [RoutingMode::Rule, RoutingMode::Global, RoutingMode::Direct] {
            let config = emit(&plan(core, policy)).unwrap();
            if core == CoreKind::Mihomo {
                assert!(
                    config
                        .body
                        .trim_end()
                        .ends_with(if policy == RoutingMode::Global {
                            "MATCH,PROXY"
                        } else {
                            "MATCH,DIRECT"
                        })
                );
                assert_eq!(
                    config.body.contains("DOMAIN-SUFFIX,example.com,DIRECT"),
                    policy == RoutingMode::Rule
                );
            } else {
                let json: Value = serde_json::from_str(&config.body).unwrap();
                let expected = if policy == RoutingMode::Global {
                    "proxy"
                } else {
                    "direct"
                };
                let route = &json[if core == CoreKind::Xray {
                    "routing"
                } else {
                    "route"
                }];
                if core == CoreKind::Xray {
                    assert_eq!(json["outbounds"][0]["tag"], expected);
                } else {
                    assert_eq!(route["final"], expected);
                }
                assert_eq!(
                    route["rules"].as_array().unwrap().is_empty(),
                    policy != RoutingMode::Rule
                );
            }
        }
    }
}

#[test]
fn explicit_rule_set_resources_have_no_implicit_action() {
    let mut plan = plan(CoreKind::SingBox, RoutingMode::Rule);
    plan.rule_sets = vec![RuleSetRef {
        tag: "geosite-cn".into(),
        path: "/tmp/geosite-cn.srs".into(),
        kind: RuleSetKind::ReferenceOnly,
    }];
    let config = emit(&plan).unwrap();
    let json: Value = serde_json::from_str(&config.body).unwrap();
    assert_eq!(json["route"]["rule_set"][0]["tag"], "geosite-cn");
    assert!(
        json["route"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|rule| rule["rule_set"].is_null())
    );
}

#[test]
fn mihomo_preserves_ip_family_and_does_not_resolve_for_ip_matching() {
    let mut plan = plan(CoreKind::Mihomo, RoutingMode::Rule);
    plan.rules = vec![
        CustomRule {
            kind: RuleKind::IpCidr,
            value: "2001:db8::/32".into(),
            action: RuleAction::Block,
        },
        CustomRule {
            kind: RuleKind::IpCidr,
            value: "192.0.2.0/24".into(),
            action: RuleAction::Direct,
        },
        CustomRule {
            kind: RuleKind::GeoIp,
            value: "cn".into(),
            action: RuleAction::Direct,
        },
    ];
    let config = emit(&plan).unwrap();
    assert!(
        config
            .body
            .contains("IP-CIDR6,2001:db8::/32,REJECT,no-resolve")
    );
    assert!(
        config
            .body
            .contains("IP-CIDR,192.0.2.0/24,DIRECT,no-resolve")
    );
    assert!(config.body.contains("GEOIP,cn,DIRECT,no-resolve"));
}

#[test]
fn mihomo_rejects_injected_targets_from_existing_records() {
    let mut plan = plan(CoreKind::Mihomo, RoutingMode::Rule);
    for value in ["x.com,REJECT", "x.com\rREJECT", "x.com\tREJECT"] {
        plan.rules[0].value = value.into();
        assert!(v2rayr_config::mihomo::emit(&plan).is_err());
    }
}
