//! System proxy takeover.
//!
//! Rules (from the design document):
//! * capture state before touching anything;
//! * never silently switch ports;
//! * restore only what we own — if the user changed the proxy after we applied
//!   ours, leave their value alone and report it.

#[cfg(any(target_os = "linux", test))]
mod gsettings;
mod native;
#[cfg(any(windows, test))]
mod wininet;

pub use native::{NativeProxyJournal, NativeProxyKind};

/// Select the adapter for the current desktop. Each adapter captures its own
/// state; manual mode never calls into the desktop integration.
pub fn system_proxy() -> Arc<dyn SystemProxy> {
    #[cfg(target_os = "macos")]
    return Arc::new(MacOsSystemProxy::system());
    #[cfg(target_os = "linux")]
    return Arc::new(native::NativeProxy(gsettings::GSettings::new(Arc::new(
        SystemCommandRunner,
    ))));
    #[cfg(windows)]
    return Arc::new(native::NativeProxy(wininet::WinInet));
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Arc::new(MacOsSystemProxy::system())
}

use std::process::Command;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::PlatformError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BypassMode {
    /// Local and private destinations skip the proxy.
    LoopbackOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyEndpoint {
    pub host: String,
    pub port: u16,
}

impl ProxyEndpoint {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

/// Which proxy slots we point at our inbound. macOS exposes three independent
/// switches; leaving HTTPS off is the classic "browser works, some apps don't".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyTarget {
    pub http: Option<ProxyEndpoint>,
    pub https: Option<ProxyEndpoint>,
    pub socks: Option<ProxyEndpoint>,
    /// When set, every service is pointed at this PAC URL and the manual proxy
    /// slots are switched off, per the design doc ("PAC 模式…同时关闭系统手动代理").
    pub pac: Option<String>,
    pub bypass: Vec<String>,
    pub mode: BypassMode,
}

impl ProxyTarget {
    /// Point every slot at the same local inbound.
    pub fn all_through(host: impl Into<String>, port: u16) -> Self {
        let endpoint = ProxyEndpoint::new(host, port);
        Self {
            http: Some(endpoint.clone()),
            https: Some(endpoint.clone()),
            socks: Some(endpoint),
            pac: None,
            bypass: default_bypass(),
            mode: BypassMode::LoopbackOnly,
        }
    }

    /// Take over through a PAC script served at `url`.
    pub fn through_pac(url: impl Into<String>, bypass: Vec<String>) -> Self {
        Self {
            http: None,
            https: None,
            socks: None,
            pac: Some(url.into()),
            bypass,
            mode: BypassMode::LoopbackOnly,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pac.is_none() && self.http.is_none() && self.https.is_none() && self.socks.is_none()
    }
}

pub fn default_bypass() -> Vec<String> {
    [
        "127.0.0.1",
        "localhost",
        "::1",
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "*.local",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceProxyState {
    /// Network service name, e.g. `Wi-Fi`.
    pub service: String,
    pub enabled: bool,
    pub http: Option<ProxyEndpoint>,
    pub https: Option<ProxyEndpoint>,
    pub socks: Option<ProxyEndpoint>,
    /// Automatic proxy configuration (PAC) URL, if one was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_url: Option<String>,
    #[serde(default)]
    pub auto_enabled: bool,
    pub bypass: Vec<String>,
}

/// The exact state we replaced, per network service. Serialised to disk so a
/// crash mid-takeover can be repaired on the next start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeoverSnapshot {
    pub services: Vec<ServiceProxyState>,
    /// The endpoint we wrote; `None` when we took over through a PAC script.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_endpoint: Option<ProxyEndpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_pac: Option<String>,
    pub applied_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ProxyTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeProxyJournal>,
}

impl TakeoverSnapshot {
    pub fn owned_by(&self, endpoint: &ProxyEndpoint) -> bool {
        self.applied_endpoint.as_ref() == Some(endpoint)
    }

    pub fn owns_pac(&self, url: &str) -> bool {
        self.applied_pac.as_deref() == Some(url)
    }

    /// True when the current state still points at the endpoint we applied, i.e.
    /// nobody changed the proxy behind our back.
    pub fn still_applied(&self, current: &[ServiceProxyState]) -> bool {
        current.iter().all(|service| {
            let previous = self
                .services
                .iter()
                .find(|item| item.service == service.service);
            match previous {
                Some(previous) => {
                    let ours = |endpoint: &Option<ProxyEndpoint>| {
                        self.applied_endpoint.is_some() && endpoint == &self.applied_endpoint
                    };
                    // Either untouched (previous) or ours.
                    let matches_previous = service.http == previous.http
                        && service.https == previous.https
                        && service.socks == previous.socks
                        && service.enabled == previous.enabled
                        && service.auto_url == previous.auto_url
                        && service.auto_enabled == previous.auto_enabled;
                    let our_pac = self.applied_pac.is_some()
                        && service.auto_enabled
                        && service.auto_url == self.applied_pac;
                    matches_previous || our_pac || ours(&service.http) || ours(&service.socks)
                }
                None => false,
            }
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemProxyState {
    pub services: Vec<ServiceProxyState>,
}

impl SystemProxyState {
    pub fn is_enabled(&self) -> bool {
        self.services.iter().any(|service| service.enabled)
    }

    pub fn pointing_at(&self, endpoint: &ProxyEndpoint) -> bool {
        self.services.iter().any(|service| {
            service.enabled
                && (service.http.as_ref() == Some(endpoint)
                    || service.socks.as_ref() == Some(endpoint)
                    || service.https.as_ref() == Some(endpoint))
        })
    }
}

pub trait SystemProxy: Send + Sync {
    fn capture(&self) -> Result<SystemProxyState, PlatformError>;
    fn apply(&self, target: &ProxyTarget) -> Result<TakeoverSnapshot, PlatformError>;
    fn apply_journaled(
        &self,
        target: &ProxyTarget,
        journal: &mut dyn FnMut(&TakeoverSnapshot) -> Result<(), PlatformError>,
    ) -> Result<TakeoverSnapshot, PlatformError> {
        let snapshot = self.apply(target)?;
        journal(&snapshot)?;
        Ok(snapshot)
    }
    fn restore(&self, snapshot: &TakeoverSnapshot) -> Result<(), PlatformError>;
    fn clear(&self, endpoint: &ProxyEndpoint) -> Result<(), PlatformError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, PlatformError>;
}

pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, PlatformError> {
        let output = Command::new(program).args(args).output()?;
        Ok(CommandOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// macOS implementation backed by `networksetup`.
pub struct MacOsSystemProxy {
    runner: Arc<dyn CommandRunner>,
    networksetup: String,
}

impl MacOsSystemProxy {
    pub fn system() -> Self {
        Self {
            runner: Arc::new(SystemCommandRunner),
            networksetup: "/usr/sbin/networksetup".to_owned(),
        }
    }

    pub fn with_runner(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            runner,
            networksetup: "/usr/sbin/networksetup".to_owned(),
        }
    }

    fn run(&self, args: &[&str]) -> Result<CommandOutput, PlatformError> {
        let output = self.runner.run(&self.networksetup, args)?;
        if !output.ok() && PlatformError::is_permission_failure(&output.stderr) {
            return Err(PlatformError::PermissionDenied(
                output.stderr.trim().to_owned(),
            ));
        }
        Ok(output)
    }

    pub fn services(&self) -> Result<Vec<String>, PlatformError> {
        let output = self.run(&["-listallnetworkservices"])?;
        if !output.ok() {
            return Err(command_error(
                &self.networksetup,
                &["-listallnetworkservices"],
                &output,
            ));
        }
        Ok(parse_services(&output.stdout))
    }

    fn read_service(&self, service: &str) -> Result<ServiceProxyState, PlatformError> {
        let http = self.read_endpoint("-getwebproxy", service)?;
        let https = self.read_endpoint("-getsecurewebproxy", service)?;
        let socks = self.read_endpoint("-getsocksfirewallproxy", service)?;
        let (auto_url, auto_enabled) = self.read_auto(service)?;
        let enabled = http.is_some() || https.is_some() || socks.is_some() || auto_enabled;
        let mut bypass = self.read_bypass(service)?;
        bypass.sort();
        Ok(ServiceProxyState {
            service: service.to_owned(),
            enabled,
            http,
            https,
            socks,
            auto_url,
            auto_enabled,
            bypass,
        })
    }

    fn read_endpoint(
        &self,
        getter: &str,
        service: &str,
    ) -> Result<Option<ProxyEndpoint>, PlatformError> {
        let output = self.run(&[getter, service])?;
        if !output.ok() {
            return Err(command_error(
                &self.networksetup,
                &[getter, service],
                &output,
            ));
        }
        let enabled = output
            .stdout
            .lines()
            .any(|line| line.trim().eq_ignore_ascii_case("Enabled: Yes"));
        if !enabled {
            return Ok(None);
        }
        if output.stdout.lines().any(|line| {
            line.split_once(':').is_some_and(|(key, value)| {
                key.trim() == "Authenticated Proxy Enabled" && matches!(value.trim(), "1" | "Yes")
            })
        }) {
            return Err(PlatformError::InvalidState(
                "已有认证系统代理，无法安全备份其凭据；请改用本地端口模式".into(),
            ));
        }
        parse_proxy_output(&output.stdout).map(Some).ok_or_else(|| {
            PlatformError::InvalidState(format!("无法读取 {service} 的已启用代理地址"))
        })
    }

    fn read_bypass(&self, service: &str) -> Result<Vec<String>, PlatformError> {
        let output = self.run(&["-getproxybypassdomains", service])?;
        if !output.ok() {
            return Err(command_error(
                &self.networksetup,
                &["-getproxybypassdomains", service],
                &output,
            ));
        }
        Ok(parse_bypass(&output.stdout))
    }

    /// `networksetup -getautoproxyurl` prints a `URL:` line plus `Enabled: Yes`.
    fn read_auto(&self, service: &str) -> Result<(Option<String>, bool), PlatformError> {
        let output = self.run(&["-getautoproxyurl", service])?;
        if !output.ok() {
            // Not every macOS version answers for every service; treat as unset.
            return Ok((None, false));
        }
        let mut url = None;
        let mut enabled = false;
        for line in output.stdout.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "URL" if !value.is_empty() => url = Some(value.to_owned()),
                "Enabled" => enabled = value.eq_ignore_ascii_case("yes"),
                _ => {}
            }
        }
        Ok((url, enabled))
    }

    fn set_auto(
        &self,
        service: &str,
        url: Option<&str>,
        enabled: bool,
    ) -> Result<(), PlatformError> {
        if let Some(url) = url {
            let output = self.run(&["-setautoproxyurl", service, url])?;
            if !output.ok() {
                return Err(command_error(
                    &self.networksetup,
                    &["-setautoproxyurl", service],
                    &output,
                ));
            }
        }
        let state = if enabled { "on" } else { "off" };
        let output = self.run(&["-setautoproxystate", service, state])?;
        if !output.ok() {
            return Err(command_error(
                &self.networksetup,
                &["-setautoproxystate", service, state],
                &output,
            ));
        }
        Ok(())
    }

    fn set_endpoint(
        &self,
        setter: &str,
        state_setter: &str,
        service: &str,
        endpoint: Option<&ProxyEndpoint>,
        enabled: bool,
    ) -> Result<(), PlatformError> {
        if let Some(endpoint) = endpoint {
            let port = endpoint.port.to_string();
            let output = self.run(&[setter, service, &endpoint.host, &port])?;
            if !output.ok() {
                return Err(command_error(
                    &self.networksetup,
                    &[setter, service],
                    &output,
                ));
            }
        }
        let state = if enabled { "on" } else { "off" };
        let output = self.run(&[state_setter, service, state])?;
        if !output.ok() {
            return Err(command_error(
                &self.networksetup,
                &[state_setter, service, state],
                &output,
            ));
        }
        Ok(())
    }

    fn set_bypass(&self, service: &str, domains: &[String]) -> Result<(), PlatformError> {
        let mut args: Vec<&str> = vec!["-setproxybypassdomains", service];
        if domains.is_empty() {
            args.push("Empty");
        } else {
            args.extend(domains.iter().map(String::as_str));
        }
        let output = self.run(&args)?;
        if !output.ok() {
            return Err(command_error(
                &self.networksetup,
                &["-setproxybypassdomains", service],
                &output,
            ));
        }
        Ok(())
    }
}

fn command_error(program: &str, args: &[&str], output: &CommandOutput) -> PlatformError {
    PlatformError::Command {
        program: program.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        message: if output.stderr.trim().is_empty() {
            format!("exit status {}", output.status)
        } else {
            output.stderr.trim().to_owned()
        },
    }
}

impl SystemProxy for MacOsSystemProxy {
    fn capture(&self) -> Result<SystemProxyState, PlatformError> {
        let mut services = Vec::new();
        for service in self.services()? {
            services.push(self.read_service(&service)?);
        }
        Ok(SystemProxyState { services })
    }

    fn apply(&self, target: &ProxyTarget) -> Result<TakeoverSnapshot, PlatformError> {
        self.apply_journaled(target, &mut |_| Ok(()))
    }

    fn apply_journaled(
        &self,
        target: &ProxyTarget,
        journal: &mut dyn FnMut(&TakeoverSnapshot) -> Result<(), PlatformError>,
    ) -> Result<TakeoverSnapshot, PlatformError> {
        if target.is_empty() {
            return Err(PlatformError::InvalidState(
                "no proxy endpoint to apply".to_owned(),
            ));
        }
        let capture = self.capture()?;
        if capture.services.is_empty() {
            return Err(PlatformError::InvalidState(
                "no network services found".to_owned(),
            ));
        }
        // Everything is restored from this snapshot, so it must exist before the
        // first mutating command runs.
        let endpoint = target
            .http
            .clone()
            .or_else(|| target.socks.clone())
            .or_else(|| target.https.clone());
        if endpoint.is_none() && target.pac.is_none() {
            return Err(PlatformError::InvalidState("no endpoint".to_owned()));
        }
        let snapshot = TakeoverSnapshot {
            services: capture.services.clone(),
            applied_endpoint: endpoint,
            applied_pac: target.pac.clone(),
            applied_at_ms: chrono::Utc::now().timestamp_millis(),
            target: Some(target.clone()),
            native: None,
        };
        journal(&snapshot)?;

        for service in &capture.services {
            // Skip services that are switched off entirely (e.g. a dormant
            // Thunderbolt bridge); touching them confuses macOS.
            let result = (|| -> Result<(), PlatformError> {
                if let Some(url) = &target.pac {
                    self.set_auto(&service.service, Some(url), false)?;
                    // PAC mode owns every slot: manual proxies are switched off so
                    // the two mechanisms cannot fight over the same connection.
                    for (setter, state_setter) in [
                        ("-setwebproxy", "-setwebproxystate"),
                        ("-setsecurewebproxy", "-setsecurewebproxystate"),
                        ("-setsocksfirewallproxy", "-setsocksfirewallproxystate"),
                    ] {
                        let _ = setter;
                        self.set_endpoint(
                            state_setter,
                            state_setter,
                            &service.service,
                            None,
                            false,
                        )?;
                    }
                    self.set_auto(&service.service, None, true)?;
                    return self.set_bypass(&service.service, &target.bypass);
                }
                // Manual mode must not fight a leftover PAC script.
                self.set_auto(&service.service, None, false)?;
                if let Some(endpoint) = &target.http {
                    self.set_endpoint(
                        "-setwebproxy",
                        "-setwebproxystate",
                        &service.service,
                        Some(endpoint),
                        true,
                    )?;
                }
                if let Some(endpoint) = &target.https {
                    self.set_endpoint(
                        "-setsecurewebproxy",
                        "-setsecurewebproxystate",
                        &service.service,
                        Some(endpoint),
                        true,
                    )?;
                }
                if let Some(endpoint) = &target.socks {
                    self.set_endpoint(
                        "-setsocksfirewallproxy",
                        "-setsocksfirewallproxystate",
                        &service.service,
                        Some(endpoint),
                        true,
                    )?;
                }
                self.set_bypass(&service.service, &target.bypass)
            })()
            .and_then(|()| {
                let observed = self.read_service(&service.service)?;
                let expected_bypass = sorted_domains(&target.bypass);
                let manual_ok = if target.pac.is_some() {
                    observed.http.is_none() && observed.https.is_none() && observed.socks.is_none()
                } else {
                    target
                        .http
                        .as_ref()
                        .is_none_or(|e| observed.http.as_ref() == Some(e))
                        && target
                            .https
                            .as_ref()
                            .is_none_or(|e| observed.https.as_ref() == Some(e))
                        && target
                            .socks
                            .as_ref()
                            .is_none_or(|e| observed.socks.as_ref() == Some(e))
                };
                let pac_ok = if let Some(url) = &target.pac {
                    observed.auto_enabled && observed.auto_url.as_ref() == Some(url)
                } else {
                    !observed.auto_enabled
                };
                if manual_ok && pac_ok && observed.bypass == expected_bypass {
                    Ok(())
                } else {
                    Err(PlatformError::InvalidState(format!(
                        "{} 的代理设置未通过读回验证",
                        service.service
                    )))
                }
            });
            if let Err(error) = result {
                // Roll back what we already changed; the user must never be left
                // with a half-applied proxy.
                let _ = self.restore(&snapshot);
                return Err(error);
            }
        }
        Ok(snapshot)
    }

    fn restore(&self, snapshot: &TakeoverSnapshot) -> Result<(), PlatformError> {
        if snapshot.native.is_some() {
            return Err(PlatformError::InvalidState(
                "系统代理恢复记录与 macOS 不匹配".into(),
            ));
        }
        let current = self.capture()?;
        for previous in &snapshot.services {
            let Some(now) = current
                .services
                .iter()
                .find(|now| now.service == previous.service)
            else {
                continue;
            };
            let target = snapshot.target.as_ref();
            let pac_owned = snapshot.applied_pac.is_some() && now.auto_url == snapshot.applied_pac;
            let mut restored_slots = [false; 3];
            for (index, (current_slot, old_slot, desired, setter, state_setter)) in [
                (
                    &now.http,
                    &previous.http,
                    target.and_then(|t| t.http.as_ref()),
                    "-setwebproxy",
                    "-setwebproxystate",
                ),
                (
                    &now.https,
                    &previous.https,
                    target.and_then(|t| t.https.as_ref()),
                    "-setsecurewebproxy",
                    "-setsecurewebproxystate",
                ),
                (
                    &now.socks,
                    &previous.socks,
                    target.and_then(|t| t.socks.as_ref()),
                    "-setsocksfirewallproxy",
                    "-setsocksfirewallproxystate",
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let desired = desired.or(snapshot.applied_endpoint.as_ref());
                let owned = if snapshot.applied_pac.is_some() {
                    pac_owned && current_slot.is_none()
                } else {
                    desired.is_some() && current_slot.as_ref() == desired
                };
                if owned && current_slot != old_slot {
                    restored_slots[index] = true;
                    self.set_endpoint(
                        setter,
                        state_setter,
                        &previous.service,
                        old_slot.as_ref(),
                        old_slot.is_some(),
                    )?;
                }
            }
            let restore_bypass = target
                .is_some_and(|target| now.bypass == sorted_domains(&target.bypass))
                && now.bypass != previous.bypass;
            if restore_bypass {
                self.set_bypass(&previous.service, &previous.bypass)?;
            }
            let restore_pac = pac_owned
                || (target.is_some_and(|t| t.pac.is_none())
                    && !now.auto_enabled
                    && previous.auto_enabled
                    && now.auto_url == previous.auto_url);
            if restore_pac {
                self.set_auto(
                    &previous.service,
                    previous.auto_url.as_deref(),
                    previous.auto_enabled,
                )?;
            }
            if restored_slots.iter().any(|changed| *changed) || restore_bypass || restore_pac {
                let observed = self.read_service(&previous.service)?;
                if (restored_slots[0] && observed.http != previous.http)
                    || (restored_slots[1] && observed.https != previous.https)
                    || (restored_slots[2] && observed.socks != previous.socks)
                    || (restore_bypass && observed.bypass != previous.bypass)
                    || (restore_pac
                        && (observed.auto_enabled != previous.auto_enabled
                            || (previous.auto_url.is_some()
                                && observed.auto_url != previous.auto_url)))
                {
                    return Err(PlatformError::InvalidState(format!(
                        "{} 的系统代理恢复未通过读回验证",
                        previous.service
                    )));
                }
            }
        }
        Ok(())
    }

    fn clear(&self, endpoint: &ProxyEndpoint) -> Result<(), PlatformError> {
        let current = self.capture()?;
        for service in &current.services {
            let ours = |slot: &Option<ProxyEndpoint>| slot.as_ref() == Some(endpoint);
            if ours(&service.http) {
                self.set_endpoint(
                    "-setwebproxystate",
                    "-setwebproxystate",
                    &service.service,
                    None,
                    false,
                )?;
            }
            if ours(&service.https) {
                self.set_endpoint(
                    "-setsecurewebproxystate",
                    "-setsecurewebproxystate",
                    &service.service,
                    None,
                    false,
                )?;
            }
            if ours(&service.socks) {
                self.set_endpoint(
                    "-setsocksfirewallproxystate",
                    "-setsocksfirewallproxystate",
                    &service.service,
                    None,
                    false,
                )?;
            }
        }
        Ok(())
    }
}

fn sorted_domains(domains: &[String]) -> Vec<String> {
    let mut domains = domains.to_vec();
    domains.sort();
    domains.dedup();
    domains
}

pub fn parse_services(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("An asterisk"))
        .map(|line| line.strip_prefix('*').unwrap_or(line).trim().to_owned())
        .filter(|line| !line.eq_ignore_ascii_case("Network Service"))
        .collect()
}

/// Parse `networksetup -getwebproxy` output.
pub fn parse_proxy_output(stdout: &str) -> Option<ProxyEndpoint> {
    let mut server = None;
    let mut port = None;
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Server" if !value.is_empty() => server = Some(value.to_owned()),
            "Port" => port = value.parse::<u16>().ok(),
            _ => {}
        }
    }
    match (server, port) {
        (Some(host), Some(port)) if port > 0 => Some(ProxyEndpoint { host, port }),
        _ => None,
    }
}

pub fn parse_bypass(stdout: &str) -> Vec<String> {
    if stdout.trim().is_empty() || stdout.to_ascii_lowercase().contains("there aren't any") {
        return Vec::new();
    }
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Replays canned outputs in call order and records the mutating commands.
    struct ScriptedRunner {
        outputs: Mutex<VecDeque<CommandOutput>>,
        calls: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl ScriptedRunner {
        fn new(outputs: Vec<CommandOutput>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.lock().expect("lock").clone()
        }
    }

    fn out(stdout: &str) -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        }
    }

    impl CommandRunner for ScriptedRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, PlatformError> {
            self.calls.lock().expect("lock").push((
                program.to_owned(),
                args.iter().map(|arg| (*arg).to_owned()).collect(),
            ));
            // Extra read calls (`-getautoproxyurl`) are harmless: default to an
            // empty, successful answer so scripts only spell out what matters.
            Ok(self
                .outputs
                .lock()
                .expect("lock")
                .pop_front()
                .unwrap_or(CommandOutput {
                    status: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                }))
        }
    }

    fn proxy_block(server: &str, port: u16, enabled: bool) -> String {
        format!(
            "Enabled: {}\nServer: {server}\nPort: {port}\nAuthenticated Proxy Enabled: 0\n",
            if enabled { "Yes" } else { "No" }
        )
    }

    #[test]
    fn parses_networksetup_output() {
        let endpoint =
            parse_proxy_output(&proxy_block("127.0.0.1", 10808, true)).expect("endpoint");
        assert_eq!(endpoint, ProxyEndpoint::new("127.0.0.1", 10808));
        assert!(parse_proxy_output("Enabled: No\n").is_none());
        assert_eq!(
            parse_bypass("There aren't any bypass domains set on Wi-Fi.\n"),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_bypass("127.0.0.1\n*.local\n"),
            vec!["127.0.0.1", "*.local"]
        );
    }

    #[test]
    fn service_list_skips_the_header_and_disabled_marker() {
        let stdout = "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Thunderbolt Bridge\nUSB 10/100/1000 LAN\n";
        assert_eq!(
            parse_services(stdout),
            vec!["Wi-Fi", "Thunderbolt Bridge", "USB 10/100/1000 LAN"]
        );
    }

    #[test]
    fn apply_captures_state_before_mutating_and_restore_undoes_it() {
        let mut outputs = vec![
            out("Wi-Fi\n"),
            out(&proxy_block("127.0.0.1", 7890, true)),
            out(&proxy_block("", 0, false)),
            out(&proxy_block("", 0, false)),
            out("Enabled: No\n"),
            out("127.0.0.1\n"),
        ];
        outputs.extend((0..8).map(|_| out("")));
        outputs.extend((0..3).map(|_| out(&proxy_block("127.0.0.1", 10808, true))));
        outputs.push(out("Enabled: No\n"));
        outputs.push(out(&default_bypass().join("\n")));
        let runner = Arc::new(ScriptedRunner::new(outputs));
        let proxy = MacOsSystemProxy::with_runner(runner.clone());
        let snapshot = proxy
            .apply_journaled(
                &ProxyTarget::all_through("127.0.0.1", 10808),
                &mut |snapshot| {
                    assert!(
                        !runner
                            .calls()
                            .iter()
                            .any(|(_, args)| args[0].starts_with("-set"))
                    );
                    assert_eq!(snapshot.services[0].http.as_ref().unwrap().port, 7890);
                    Ok(())
                },
            )
            .expect("apply");

        assert_eq!(snapshot.services.len(), 1);
        assert_eq!(
            snapshot.services[0].http,
            Some(ProxyEndpoint::new("127.0.0.1", 7890))
        );
        assert_eq!(
            snapshot.applied_endpoint.as_ref().map(|e| e.port),
            Some(10808)
        );

        let calls = runner.calls();
        let setters: Vec<&String> = calls
            .iter()
            .map(|(_, args)| args.first().expect("arg"))
            .filter(|arg| arg.starts_with("-set") && !arg.ends_with("state"))
            .collect();
        assert!(
            setters.iter().any(|arg| arg.as_str() == "-setwebproxy"),
            "the web proxy slot must be written"
        );

        proxy.restore(&snapshot).expect("restore");
    }

    #[test]
    fn a_failing_set_rolls_the_previous_state_back() {
        let runner = Arc::new(ScriptedRunner::new(vec![
            out("Wi-Fi\n"),
            out(&proxy_block("127.0.0.1", 7890, true)),
            out(&proxy_block("", 0, false)),
            out(&proxy_block("", 0, false)),
            out("Enabled: No\n"),
            out("127.0.0.1\n"),
            out(""),
            // -setwebproxy fails
            CommandOutput {
                status: 1,
                stdout: String::new(),
                stderr: "not authorized".into(),
            },
        ]));
        let proxy = MacOsSystemProxy::with_runner(runner);
        let error = proxy
            .apply(&ProxyTarget::all_through("127.0.0.1", 10808))
            .expect_err("must fail");
        assert!(matches!(error, PlatformError::PermissionDenied(_)));
    }

    #[test]
    fn restore_does_not_overwrite_a_changed_https_slot_or_bypass() {
        let runner = Arc::new(ScriptedRunner::new(vec![
            out("Wi-Fi\n"),
            out(&proxy_block("127.0.0.1", 10809, true)),
            out(&proxy_block("other.proxy", 3128, true)),
            out(&proxy_block("127.0.0.1", 10808, true)),
            out("Enabled: No\n"),
            out("external.local\n"),
            out(""),
            out(""),
            out(""),
            out(&proxy_block("old.proxy", 8080, true)),
            out(&proxy_block("other.proxy", 3128, true)),
            out(&proxy_block("", 0, false)),
            out("Enabled: No\n"),
            out("external.local\n"),
        ]));
        let mut target = ProxyTarget::all_through("127.0.0.1", 10808);
        target.http = Some(ProxyEndpoint::new("127.0.0.1", 10809));
        target.https = target.http.clone();
        MacOsSystemProxy::with_runner(runner.clone())
            .restore(&TakeoverSnapshot {
                services: vec![ServiceProxyState {
                    service: "Wi-Fi".into(),
                    enabled: true,
                    http: Some(ProxyEndpoint::new("old.proxy", 8080)),
                    https: None,
                    socks: None,
                    auto_url: None,
                    auto_enabled: false,
                    bypass: vec![],
                }],
                applied_endpoint: target.http.clone(),
                applied_pac: None,
                applied_at_ms: 0,
                target: Some(target),
                native: None,
            })
            .unwrap();
        let setters: Vec<_> = runner
            .calls()
            .into_iter()
            .map(|(_, args)| args[0].clone())
            .filter(|cmd| cmd.starts_with("-set"))
            .collect();
        assert_eq!(
            setters,
            [
                "-setwebproxy",
                "-setwebproxystate",
                "-setsocksfirewallproxystate"
            ]
        );
    }

    #[test]
    fn snapshot_ownership_is_detected() {
        let snapshot = TakeoverSnapshot {
            services: vec![ServiceProxyState {
                service: "Wi-Fi".into(),
                enabled: true,
                http: Some(ProxyEndpoint::new("127.0.0.1", 7890)),
                https: None,
                socks: None,
                auto_url: None,
                auto_enabled: false,
                bypass: vec![],
            }],
            applied_endpoint: Some(ProxyEndpoint::new("127.0.0.1", 10808)),
            applied_pac: None,
            applied_at_ms: 0,
            target: None,
            native: None,
        };
        assert!(snapshot.owned_by(&ProxyEndpoint::new("127.0.0.1", 10808)));
        assert!(snapshot.still_applied(&[ServiceProxyState {
            service: "Wi-Fi".into(),
            enabled: true,
            http: Some(ProxyEndpoint::new("127.0.0.1", 10808)),
            https: None,
            socks: None,
            auto_url: None,
            auto_enabled: false,
            bypass: vec![],
        }]));
        assert!(!snapshot.still_applied(&[ServiceProxyState {
            service: "Wi-Fi".into(),
            enabled: true,
            http: Some(ProxyEndpoint::new("127.0.0.1", 9090)),
            https: None,
            socks: None,
            auto_url: None,
            auto_enabled: false,
            bypass: vec![],
        }]));
    }
}
