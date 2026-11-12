//! cargo run --release -p v2rayr-config --example routing_benchmark -- [output-dir]
//! Control-plane benchmark only; not a throughput or latency claim.
use std::time::Instant;
use v2rayr_config::*;
use v2rayr_domain::{CoreKind, Profile, Protocol};
fn main() {
    let output = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    if let Some(dir) = &output {
        std::fs::create_dir_all(dir).unwrap();
    }
    println!("domains,core,compile_emit_us,bytes,route_objects");
    for count in [1_000, 10_000, 50_000] {
        for core in [CoreKind::SingBox, CoreKind::Xray, CoreKind::Mihomo] {
            let start = Instant::now();
            let rules = routing::deduplicate((0..count).map(|n| CustomRule {
                kind: RuleKind::DomainSuffix,
                value: format!("d{n}.example.com"),
                action: RuleAction::Proxy,
            }));
            let plan = SessionPlan {
                profile: Profile::new(
                    "benchmark",
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
                    secret: "local-benchmark".into(),
                },
                tun: None,
                policy: RoutingMode::Rule,
                dns: DnsPlan::default(),
                rules,
                custom_rules_first: true,
                rule_sets: Vec::new(),
                sniffing: true,
                tcp_fast_open: false,
                mux: None,
                log_level: "warn".into(),
                block_ads: false,
                data_dir: ".".into(),
            };
            let config = emit(&plan).unwrap();
            let micros = start.elapsed().as_micros();
            let objects = if core == CoreKind::Mihomo {
                count + 1
            } else {
                let json: serde_json::Value = serde_json::from_str(&config.body).unwrap();
                json[if core == CoreKind::Xray {
                    "routing"
                } else {
                    "route"
                }]["rules"]
                    .as_array()
                    .unwrap()
                    .len()
            };
            println!(
                "{count},{},{micros},{},{objects}",
                core.as_str(),
                config.body.len()
            );
            if let Some(dir) = &output {
                std::fs::write(
                    dir.join(format!(
                        "{}-{count}.{}",
                        core.as_str(),
                        config.format.as_str()
                    )),
                    config.body,
                )
                .unwrap();
                if count == 1_000 {
                    let mut flow = plan.clone();
                    flow.rules = vec![
                        CustomRule {
                            kind: RuleKind::DomainSuffix,
                            value: "localhost".into(),
                            action: RuleAction::Direct,
                        },
                        CustomRule {
                            kind: RuleKind::DomainSuffix,
                            value: "blocked.example.com".into(),
                            action: RuleAction::Block,
                        },
                        CustomRule {
                            kind: RuleKind::DomainSuffix,
                            value: "example.com".into(),
                            action: RuleAction::Proxy,
                        },
                        CustomRule {
                            kind: RuleKind::DomainSuffix,
                            value: "localhost".into(),
                            action: RuleAction::Proxy,
                        },
                    ];
                    flow.rules = routing::deduplicate(flow.rules);
                    let fixture = emit(&flow).unwrap();
                    std::fs::write(
                        dir.join(format!(
                            "{}-flow.{}",
                            core.as_str(),
                            fixture.format.as_str()
                        )),
                        fixture.body,
                    )
                    .unwrap();
                }
            }
        }
    }
}
