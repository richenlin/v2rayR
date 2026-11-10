//! Pins the JSON the UI consumes.
//!
//! `ui/src/lib/contract.ts` is hand written; this test is the tripwire that
//! keeps it honest. When a field is renamed deliberately, update both files and
//! bump `API_VERSION` if the old name disappears.

use serde_json::Value;
use uuid::Uuid;
use v2rayr_domain::CoreKind;
use v2rayr_ipc::{
    AppSnapshot, ConnectionPhase, ConnectionState, NodeView, Problem, RoutingPolicy, SettingsView,
    TrafficView, VersionInfo,
};

#[test]
fn connection_state_field_names() {
    let state = ConnectionState {
        phase: ConnectionPhase::Connected,
        policy: RoutingPolicy::Global,
        takeover_applied: true,
        lan: None,
        ..ConnectionState::default()
    };
    let json: Value = serde_json::to_value(&state).expect("serialize");
    for key in ["phase", "mode", "policy", "takeoverApplied"] {
        assert!(json.get(key).is_some(), "missing {key}");
    }
    assert_eq!(json["phase"], "connected");
    assert_eq!(json["policy"], "global");
    assert_eq!(json["mode"], "system-proxy");
}

#[test]
fn snapshot_is_camel_case_and_carries_the_pages_data() {
    let snapshot = AppSnapshot {
        version: VersionInfo {
            app_version: "0.1.0".into(),
            api_version: 1,
            settings_schema: 1,
            db_schema: 1,
            commit: None,
        },
        connection: ConnectionState::default(),
        tun_available: false,
        tun_rule_routing: false,
        settings: SettingsView::default(),
        active_node: None,
        nodes: vec![NodeView {
            id: Uuid::nil(),
            remark: "n".into(),
            protocol: "vless".into(),
            transport: "ws".into(),
            address: "a.com".into(),
            port: 443,
            tls: true,
            subscription_id: None,
            subscription_name: None,
            tags: vec!["stream".into()],
            favorite: true,
            core: CoreKind::SingBox,
            latency: None,
            last_used_at: None,
        }],
        subscriptions: vec![],
        traffic: TrafficView::default(),
        cores: vec![],
        warnings: vec![Problem::internal("no core")],
    };
    let json: Value = serde_json::to_value(&snapshot).expect("serialize");
    assert_eq!(json["version"]["appVersion"], "0.1.0");
    assert_eq!(json["nodes"][0]["core"], "sing-box");
    assert_eq!(
        json["nodes"][0]["subscriptionId"],
        Value::Null as Value,
        "omitted, not null"
    );
    assert!(
        json.get("activeNode").is_none(),
        "None is skipped, not null"
    );
}
