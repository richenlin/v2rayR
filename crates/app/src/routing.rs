//! Build a bounded, ordered routing snapshot outside the connection hot path.
use v2rayr_config::{CustomRule, RuleAction, RuleKind};
use v2rayr_ipc::{RoutingPolicy, TakeoverMode};
use v2rayr_platform::tun::{TunRule, TunRuleAction, TunRuleTarget};

pub(crate) struct CompiledRouting {
    pub policy: RoutingPolicy,
    pub rules: Vec<CustomRule>,
    pub tun_rules: Option<Vec<TunRule>>,
    pub warnings: Vec<String>,
}

pub(crate) fn compile(
    mode: TakeoverMode,
    requested: RoutingPolicy,
    rows: &[v2rayr_store::RuleRow],
    pac: Option<&crate::pac_rules::Rules>,
) -> CompiledRouting {
    let mut result = CompiledRouting {
        policy: requested,
        rules: Vec::new(),
        tun_rules: None,
        warnings: Vec::new(),
    };
    if mode == TakeoverMode::Tun {
        result.warnings.push("TUN 当前不接管回环、私网和链路本地网段（10/8、172.16/12、192.168/16、169.254/16、fc00::/7、fe80::/10）；这些目标无法执行代理/阻断规则，全局代理也不覆盖。节点地址同样绕过。".into());
    }
    if mode == TakeoverMode::Tun
        && (!v2rayr_platform::tun::RULE_ROUTING_SUPPORTED || requested == RoutingPolicy::Direct)
    {
        result.policy = RoutingPolicy::Global;
        if requested != RoutingPolicy::Global {
            result
                .warnings
                .push("当前平台/策略的 TUN 暂仅支持全局代理，实际已按全局代理运行。".into());
        }
    }
    if result.policy != RoutingPolicy::Rule {
        return result;
    }
    let pac_rules = pac
        .into_iter()
        .flatten()
        .flat_map(|group| {
            group.iter().enumerate().flat_map(|(index, domains)| {
                domains.iter().map(move |domain| CustomRule {
                    kind: RuleKind::DomainSuffix,
                    value: domain.clone(),
                    action: if index % 2 == 0 {
                        RuleAction::Direct
                    } else {
                        RuleAction::Proxy
                    },
                })
            })
        })
        .collect::<Vec<_>>();
    // Optimize only the lowest-priority PAC tail. Explicit user rules retain
    // their order and remain visible even when their action is direct.
    let pac_rules =
        v2rayr_config::routing::simplify_pac(v2rayr_config::routing::deduplicate(pac_rules));
    if mode == TakeoverMode::Tun {
        let tun = rows
            .iter()
            .map(crate::rules::tun_rule)
            .collect::<Result<Vec<_>, _>>();
        match tun {
            Ok(mut rules) => {
                rules.extend(
                    v2rayr_config::routing::deduplicate(pac_rules)
                        .into_iter()
                        .map(|r| TunRule {
                            applications: Vec::new(),
                            target: TunRuleTarget::DomainSuffix(r.value),
                            action: match r.action {
                                RuleAction::Direct => TunRuleAction::Direct,
                                RuleAction::Proxy => TunRuleAction::Proxy,
                                RuleAction::Block => TunRuleAction::Block,
                            },
                        }),
                );
                // Same bounds as the trusted helper protocol. Never truncate.
                let probe = v2rayr_platform::tun::TunRequest {
                    socks_port: 10808,
                    upstream: "1.1.1.1".parse().unwrap(),
                    rules: Some(rules.clone()),
                };
                if let Err(error) = probe.validate() {
                    result.policy = RoutingPolicy::Global;
                    result
                        .warnings
                        .push(format!("TUN 无法完整执行规则，已回退全局代理：{error}"));
                } else {
                    result.tun_rules = Some(rules);
                }
            }
            Err(error) => {
                result.policy = RoutingPolicy::Global;
                result
                    .warnings
                    .push(format!("TUN 无法完整执行规则，已回退全局代理：{error}"));
            }
        }
    } else {
        let unsupported = rows
            .iter()
            .filter(|row| {
                !row.applications.is_empty() || row.kind == v2rayr_store::RuleKind::Process
            })
            .count();
        if unsupported > 0 {
            result.warnings.push(format!("{unsupported} 条来源应用/进程规则在当前接入方式下未生效；请使用支持应用身份的 TUN 规则分流。"));
        }
        result.rules = v2rayr_config::routing::deduplicate(
            rows.iter()
                .filter(|r| r.applications.is_empty() && r.kind != v2rayr_store::RuleKind::Process)
                .cloned()
                .map(crate::rules::into_config_rule)
                .chain(pac_rules),
        );
    }
    result
}

/// Load only resources explicitly referenced by custom rules. A resource never
/// introduces a route of its own, and its tag cannot escape the resource dir.
pub(crate) fn rule_set_refs(
    core: v2rayr_domain::CoreKind,
    rules: &mut [CustomRule],
    dir: &std::path::Path,
) -> Result<Vec<v2rayr_config::RuleSetRef>, v2rayr_ipc::Problem> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    if core != v2rayr_domain::CoreKind::SingBox {
        return Ok(result);
    }
    for rule in rules
        .iter_mut()
        .filter(|r| matches!(r.kind, RuleKind::GeoIp | RuleKind::GeoSite))
    {
        let prefix = if rule.kind == RuleKind::GeoIp {
            "geoip-"
        } else {
            "geosite-"
        };
        if !rule.value.starts_with(prefix) {
            rule.value = format!("{prefix}{}", rule.value);
        }
        let tag = &rule.value;
        if tag.is_empty()
            || !tag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(v2rayr_ipc::Problem::config(
                "sing-box Geo 规则需填写本地规则集标签，例如 geosite-cn",
            ));
        }
        let path = dir.join(format!("{tag}.srs"));
        if !path.is_file() {
            return Err(v2rayr_ipc::Problem::config(format!(
                "规则引用的本地规则集不存在：{tag}.srs"
            )));
        }
        if seen.insert(tag.clone()) {
            result.push(v2rayr_config::RuleSetRef {
                tag: tag.clone(),
                path: path.display().to_string(),
                kind: v2rayr_config::RuleSetKind::ReferenceOnly,
            });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pac_order_and_empty_default_are_explicit() {
        let pac = vec![vec![
            vec!["example.com".into()],
            vec!["example.com".into(), "proxy.net".into()],
        ]];
        let plan = compile(TakeoverMode::Manual, RoutingPolicy::Rule, &[], Some(&pac));
        // The direct example.com hides its duplicate proxy entry; after
        // deduplication neither needs an explicit route with default direct.
        assert_eq!(plan.rules.len(), 1);
        assert_eq!(plan.rules[0].value, "proxy.net");
        assert_eq!(plan.rules[0].action, RuleAction::Proxy);
        assert!(
            compile(TakeoverMode::Manual, RoutingPolicy::Rule, &[], None)
                .rules
                .is_empty()
        );
        assert!(
            compile(TakeoverMode::Manual, RoutingPolicy::Global, &[], Some(&pac))
                .rules
                .is_empty()
        );
    }
    #[test]
    fn pac_direct_exceptions_remain_after_custom_rules() {
        let app = crate::test_support::temp_app();
        app.add_rule("domain", "explicit.cn", "direct").unwrap();
        let pac = vec![vec![
            vec!["unrelated.cn".into(), "safe.example.com".into()],
            vec!["example.com".into()],
        ]];
        let plan = compile(
            TakeoverMode::Manual,
            RoutingPolicy::Rule,
            &app.db().enabled_rules().unwrap(),
            Some(&pac),
        );
        assert_eq!(
            plan.rules
                .iter()
                .map(|r| r.value.as_str())
                .collect::<Vec<_>>(),
            ["explicit.cn", "safe.example.com", "example.com"]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn removable_pac_direct_rules_do_not_force_tun_global_fallback() {
        let pac = vec![vec![
            (0..300).map(|n| format!("direct{n}.cn")).collect(),
            vec!["proxy.net".into()],
        ]];
        let plan = compile(TakeoverMode::Tun, RoutingPolicy::Rule, &[], Some(&pac));
        assert_eq!(plan.policy, RoutingPolicy::Rule);
        assert_eq!(plan.tun_rules.unwrap().len(), 1);
    }

    #[test]
    fn custom_rules_precede_pac_and_scopes_never_expand() {
        let app = crate::test_support::temp_app();
        app.add_rule("domain-suffix", "EXAMPLE.COM.", "block")
            .unwrap();
        app.add_rule_for_applications(
            "domain",
            "scoped.com",
            "proxy",
            &[v2rayr_domain::RuleApplication {
                path: "/Applications/X".into(),
                sha256: "a".repeat(64),
            }],
        )
        .unwrap();
        app.add_rule("process", "Example", "proxy").unwrap();
        let rows = app.db().enabled_rules().unwrap();
        let pac = vec![vec![vec![], vec!["example.com".into(), "other.net".into()]]];
        for mode in [TakeoverMode::SystemProxy, TakeoverMode::Manual] {
            let plan = compile(mode, RoutingPolicy::Rule, &rows, Some(&pac));
            assert_eq!(plan.rules.len(), 2);
            assert_eq!(plan.rules[0].value, "example.com");
            assert_eq!(plan.rules[0].action, RuleAction::Block);
            assert_eq!(plan.rules[1].value, "other.net");
            assert!(plan.warnings[0].contains("2 条"));
        }
    }
    #[test]
    fn explicit_geo_resources_do_not_inject_routes_or_escape_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("geosite-cn.srs"), b"fixture").unwrap();
        let mut rules = vec![CustomRule {
            kind: RuleKind::GeoSite,
            value: "cn".into(),
            action: RuleAction::Proxy,
        }];
        let refs = rule_set_refs(v2rayr_domain::CoreKind::SingBox, &mut rules, dir.path()).unwrap();
        assert_eq!(refs[0].kind, v2rayr_config::RuleSetKind::ReferenceOnly);
        assert_eq!(rules[0].value, "geosite-cn");
        rules[0].value = "../escape".into();
        assert!(rule_set_refs(v2rayr_domain::CoreKind::SingBox, &mut rules, dir.path()).is_err());
        rules[0].value = "missing".into();
        assert!(rule_set_refs(v2rayr_domain::CoreKind::SingBox, &mut rules, dir.path()).is_err());
    }
    #[test]
    fn tun_direct_fallback_is_visible() {
        let plan = compile(TakeoverMode::Tun, RoutingPolicy::Direct, &[], None);
        assert_eq!(plan.policy, RoutingPolicy::Global);
        assert!(!plan.warnings.is_empty());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn oversized_tun_pac_falls_back_without_truncation() {
        let pac = vec![vec![
            vec![],
            (0..257).map(|n| format!("d{n}.com")).collect(),
        ]];
        let plan = compile(TakeoverMode::Tun, RoutingPolicy::Rule, &[], Some(&pac));
        assert_eq!(plan.policy, RoutingPolicy::Global);
        assert!(plan.tun_rules.is_none());
        assert!(!plan.warnings.is_empty());
    }
}
