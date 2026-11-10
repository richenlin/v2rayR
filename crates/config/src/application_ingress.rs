//! Prototype contract between the macOS transparent provider and sing-box.
//! Ordinary proxy clients cannot supply application metadata on the mixed port.
use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::json;
use v2rayr_domain::{CoreKind, RuleApplication};

use crate::{ConfigError, EmittedConfig, RoutingMode, RuleKind, SessionPlan, singbox};

const TAG: &str = "application-in";
const UNKNOWN: &str = "unknown";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    port: u16,
    password: String,
    applications: Vec<ProviderApplication>,
    bypass_executables: Vec<String>,
}

#[derive(Serialize)]
struct ProviderApplication {
    identity: RuleApplication,
    username: String,
}

/// A scope at each position in `plan.rules`. Empty scopes apply to all ingress
/// traffic. The session owner supplies a fresh, private password and the exact
/// canonical path of the running kernel for loop protection.
pub fn emit(
    plan: &SessionPlan,
    scopes: &[Vec<RuleApplication>],
    port: u16,
    password: String,
    kernel_path: String,
) -> Result<(EmittedConfig, ProviderConfig), ConfigError> {
    plan.check()?;
    let reserved = [
        Some(plan.inbound.port),
        plan.inbound.http_port,
        Some(plan.api.port),
        plan.inbound.lan.as_ref().map(|lan| lan.http_port),
        plan.inbound.lan.as_ref().map(|lan| lan.socks_port),
        plan.inbound.dokodemo.as_ref().map(|forward| forward.0),
    ];
    if crate::choose_core(&plan.profile, plan.core_override)? != CoreKind::SingBox
        || plan.policy != RoutingMode::Rule
        || plan.tun.is_some()
        || scopes.len() != plan.rules.len()
        || port < 1024
        || reserved.contains(&Some(port))
        || password.len() < 24
        || password.len() > 255
        || !kernel_path.starts_with('/')
    {
        return Err(ConfigError::InvalidPlan(
            "invalid application ingress".into(),
        ));
    }

    let mut accounts = BTreeMap::new();
    for identity in scopes.iter().flatten() {
        let username = format!("application-{}", accounts.len());
        accounts.entry(identity.clone()).or_insert(username);
    }
    let applications = accounts
        .iter()
        .map(|(identity, username)| ProviderApplication {
            identity: identity.clone(),
            username: username.clone(),
        })
        .collect();
    let users: Vec<_> = accounts
        .values()
        .map(String::as_str)
        .chain([UNKNOWN])
        .map(|username| json!({"username": username, "password": &password}))
        .collect();
    let inbound = json!({
        "type": "socks", "tag": TAG, "listen": "127.0.0.1", "listen_port": port, "users": users
    });
    let scoped_rules = plan
        .rules
        .iter()
        .zip(scopes)
        .map(|(rule, scope)| {
            if !scope.is_empty() && rule.kind == RuleKind::Process {
                return Err(ConfigError::InvalidPlan(
                    "legacy process rules cannot carry application scopes".into(),
                ));
            }
            let mut value = singbox::custom_rule(rule)
                .ok_or_else(|| ConfigError::InvalidPlan("invalid application rule".into()))?;
            if !scope.is_empty() {
                value["inbound"] = json!([TAG]);
                value["auth_user"] = json!(
                    scope
                        .iter()
                        .map(|identity| &accounts[identity])
                        .collect::<Vec<_>>()
                );
            }
            Ok(value)
        })
        .collect::<Result<Vec<_>, ConfigError>>()?;

    let config =
        singbox::emit_with_ingress(plan, Some(inbound), Some(scoped_rules))?.validated()?;
    Ok((
        config,
        ProviderConfig {
            port,
            password,
            applications,
            bypass_executables: vec![kernel_path],
        },
    ))
}
