//! Structural validation of an emitted config.
//!
//! This is the cheap, deterministic half of "the kernel would accept this":
//! duplicate tags, dangling route targets, ports that collide. The expensive
//! half is `sing-box check` / `xray -test`, which the engine crate runs and
//! which cannot run in unit tests (no kernel binary on CI).

use serde_json::Value;
use v2rayr_domain::CoreKind;

use crate::{ConfigError, Format};

pub fn validate(core: CoreKind, format: Format, body: &str) -> Result<(), ConfigError> {
    match format {
        Format::Json => {
            let value: Value = serde_json::from_str(body)?;
            validate_json(core, &value)
        }
        Format::Yaml => validate_yaml(body),
    }
}

fn validate_json(core: CoreKind, value: &Value) -> Result<(), ConfigError> {
    let outbounds = value
        .get("outbounds")
        .and_then(Value::as_array)
        .ok_or_else(|| ConfigError::Inconsistent("config has no outbounds".into()))?;
    if outbounds.is_empty() {
        return Err(ConfigError::Inconsistent("outbounds is empty".into()));
    }

    let mut tags = Vec::new();
    for outbound in outbounds {
        let tag = outbound
            .get("tag")
            .and_then(Value::as_str)
            .ok_or_else(|| ConfigError::Inconsistent("an outbound has no tag".into()))?;
        if tags.contains(&tag.to_owned()) {
            return Err(ConfigError::Inconsistent(format!(
                "duplicate outbound tag {tag}"
            )));
        }
        tags.push(tag.to_owned());
    }

    // Route targets must exist (sing-box `outbound` / Xray `outboundTag`).
    let route = value.get("route").or_else(|| value.get("routing"));
    if let Some(route) = route {
        if let Some(rules) = route.get("rules").and_then(Value::as_array) {
            for rule in rules {
                if let Some(target) = rule
                    .get("outbound")
                    .or_else(|| rule.get("outboundTag"))
                    .and_then(Value::as_str)
                    && !tags.iter().any(|tag| tag == target)
                {
                    return Err(ConfigError::Inconsistent(format!(
                        "rule targets unknown outbound {target}"
                    )));
                }
            }
        }
        if let Some(final_tag) = route.get("final").and_then(Value::as_str)
            && !tags.iter().any(|tag| tag == final_tag)
        {
            return Err(ConfigError::Inconsistent(format!(
                "route.final targets unknown outbound {final_tag}"
            )));
        }
    }

    let mut ports = Vec::new();
    if let Some(inbounds) = value.get("inbounds").and_then(Value::as_array) {
        for inbound in inbounds {
            let port = inbound
                .get("port")
                .or_else(|| inbound.get("listen_port"))
                .and_then(Value::as_u64)
                .ok_or_else(|| ConfigError::Inconsistent("an inbound has no port".into()))?;
            if port == 0 || port > u16::MAX as u64 {
                return Err(ConfigError::Inconsistent(format!(
                    "inbound port {port} is invalid"
                )));
            }
            if ports.contains(&port) {
                return Err(ConfigError::Inconsistent(format!(
                    "two inbounds share port {port}"
                )));
            }
            ports.push(port);
        }
    }
    if ports.is_empty() {
        return Err(ConfigError::Inconsistent("config has no inbounds".into()));
    }

    if core == CoreKind::SingBox {
        // The UI reads traffic from the control API; without it the connection
        // page would show a permanently empty chart.
        let controller = value
            .pointer("/experimental/clash_api/external_controller")
            .and_then(Value::as_str)
            .ok_or_else(|| ConfigError::Inconsistent("sing-box config has no clash_api".into()))?;
        let api_port: u16 = controller
            .rsplit(':')
            .next()
            .and_then(|port| port.parse().ok())
            .ok_or_else(|| ConfigError::Inconsistent("clash_api address has no port".into()))?;
        if ports.contains(&(api_port as u64)) {
            return Err(ConfigError::Inconsistent(format!(
                "control API port {api_port} collides with an inbound"
            )));
        }
    }

    Ok(())
}

fn validate_yaml(body: &str) -> Result<(), ConfigError> {
    let has = |needle: &str| {
        body.lines()
            .any(|line| line.trim_start().starts_with(needle))
    };
    if !has("proxies:") {
        return Err(ConfigError::Inconsistent(
            "mihomo config has no proxies".into(),
        ));
    }
    if !has("rules:") {
        return Err(ConfigError::Inconsistent(
            "mihomo config has no rules".into(),
        ));
    }
    if !has("mixed-port:") {
        return Err(ConfigError::Inconsistent(
            "mihomo config has no mixed-port".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_singbox() -> Value {
        json!({
            "inbounds": [{"type": "mixed", "listen_port": 10808}],
            "outbounds": [
                {"type": "trojan", "tag": "proxy"},
                {"type": "direct", "tag": "direct"},
                {"type": "block", "tag": "block"},
            ],
            "route": {
                "rules": [{"ip_is_private": true, "outbound": "direct"}],
                "final": "proxy",
            },
            "experimental": {"clash_api": {"external_controller": "127.0.0.1:10812"}},
        })
    }

    #[test]
    fn a_well_formed_config_passes() {
        let body = serde_json::to_string(&valid_singbox()).expect("json");
        validate(CoreKind::SingBox, Format::Json, &body).expect("valid");
    }

    #[test]
    fn duplicate_tags_are_rejected() {
        let mut value = valid_singbox();
        value["outbounds"][1]["tag"] = json!("proxy");
        let body = serde_json::to_string(&value).expect("json");
        let error = validate(CoreKind::SingBox, Format::Json, &body).expect_err("must fail");
        assert!(matches!(error, ConfigError::Inconsistent(_)));
    }

    #[test]
    fn dangling_route_targets_are_rejected() {
        let mut value = valid_singbox();
        value["route"]["final"] = json!("does-not-exist");
        let body = serde_json::to_string(&value).expect("json");
        assert!(validate(CoreKind::SingBox, Format::Json, &body).is_err());

        let mut value = valid_singbox();
        value["route"]["rules"][0]["outbound"] = json!("nope");
        let body = serde_json::to_string(&value).expect("json");
        assert!(validate(CoreKind::SingBox, Format::Json, &body).is_err());
    }

    #[test]
    fn colliding_ports_are_rejected() {
        let mut value = valid_singbox();
        value["experimental"]["clash_api"]["external_controller"] = json!("127.0.0.1:10808");
        let body = serde_json::to_string(&value).expect("json");
        assert!(validate(CoreKind::SingBox, Format::Json, &body).is_err());
    }

    #[test]
    fn missing_control_api_is_rejected_for_singbox() {
        let mut value = valid_singbox();
        value
            .as_object_mut()
            .expect("object")
            .remove("experimental");
        let body = serde_json::to_string(&value).expect("json");
        assert!(validate(CoreKind::SingBox, Format::Json, &body).is_err());
    }

    #[test]
    fn mihomo_yaml_needs_proxies_and_rules() {
        assert!(validate(CoreKind::Mihomo, Format::Yaml, "mixed-port: 1\n").is_err());
        validate(
            CoreKind::Mihomo,
            Format::Yaml,
            "mixed-port: 10808\nproxies:\n  - name: proxy\nrules:\n  - MATCH,PROXY\n",
        )
        .expect("valid");
    }
}
