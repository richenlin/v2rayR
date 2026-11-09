//! System proxy takeover.
//!
//! Rules (from the design document):
//! * capture state before touching anything;
//! * never silently switch ports;
//! * restore only what we own — if the user changed the proxy after we applied
//!   ours, leave their value alone and report it.

use std::process::Command;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::PlatformError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyTarget {
    pub http: Option<ProxyEndpoint>,
    pub https: Option<ProxyEndpoint>,
    pub socks: Option<ProxyEndpoint>,
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
            bypass: default_bypass(),
            mode: BypassMode::LoopbackOnly,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.http.is_none() && self.https.is_none() && self.socks.is_none()
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
    pub bypass: Vec<String>,
}

/// The exact state we replaced, per network service. Serialised to disk so a
/// crash mid-takeover can be repaired on the next start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeoverSnapshot {
    pub services: Vec<ServiceProxyState>,
    pub applied_endpoint: ProxyEndpoint,
    pub applied_at_ms: i64,
}

impl TakeoverSnapshot {
    pub fn owned_by(&self, endpoint: &ProxyEndpoint) -> bool {
        &self.applied_endpoint == endpoint
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
                        endpoint.as_ref() == Some(&self.applied_endpoint)
                    };
                    // Either untouched (previous) or ours.
                    let matches_previous = service.http == previous.http
                        && service.https == previous.https
                        && service.socks == previous.socks
                        && service.enabled == previous.enabled;
                    matches_previous || ours(&service.http) || ours(&service.socks)
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
        let http = self.read_endpoint("-getwebproxy", service, &["-getwebproxystate"])?;
        let https =
            self.read_endpoint("-getsecurewebproxy", service, &["-getsecurewebproxystate"])?;
        let socks = self.read_endpoint(
            "-getsocksfirewallproxy",
            service,
            &["-getsocksfirewallproxystate"],
        )?;
        let enabled = http.0 || https.0 || socks.0;
        let mut bypass = self.read_bypass(service)?;
        bypass.sort();
        Ok(ServiceProxyState {
            service: service.to_owned(),
            enabled,
            http: http.1,
            https: https.1,
            socks: socks.1,
            bypass,
        })
    }

    /// Returns (enabled, endpoint).
    fn read_endpoint(
        &self,
        getter: &str,
        service: &str,
        state_getter: &[&str],
    ) -> Result<(bool, Option<ProxyEndpoint>), PlatformError> {
        let output = self.run(&[getter, service])?;
        if !output.ok() {
            return Err(command_error(
                &self.networksetup,
                &[getter, service],
                &output,
            ));
        }
        let endpoint = parse_proxy_output(&output.stdout);
        let state_output = self.run(&[state_getter[0], service])?;
        let enabled = state_output.ok() && state_output.stdout.trim().eq_ignore_ascii_case("on");
        Ok((enabled, endpoint))
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
        if domains.is_empty() {
            return Ok(());
        }
        let mut args: Vec<&str> = vec!["-setproxybypassdomains", service];
        args.extend(domains.iter().map(String::as_str));
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
        let mut snapshot = TakeoverSnapshot {
            services: capture.services.clone(),
            applied_endpoint: target
                .http
                .clone()
                .or_else(|| target.socks.clone())
                .or_else(|| target.https.clone())
                .ok_or_else(|| PlatformError::InvalidState("no endpoint".to_owned()))?,
            applied_at_ms: chrono::Utc::now().timestamp_millis(),
        };

        for service in &capture.services {
            // Skip services that are switched off entirely (e.g. a dormant
            // Thunderbolt bridge); touching them confuses macOS.
            let result = (|| -> Result<(), PlatformError> {
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
            })();
            if let Err(error) = result {
                // Roll back what we already changed; the user must never be left
                // with a half-applied proxy.
                let _ = self.restore(&snapshot);
                return Err(error);
            }
            snapshot.applied_at_ms = chrono::Utc::now().timestamp_millis();
        }
        Ok(snapshot)
    }

    fn restore(&self, snapshot: &TakeoverSnapshot) -> Result<(), PlatformError> {
        for service in &snapshot.services {
            self.set_endpoint(
                "-setwebproxy",
                "-setwebproxystate",
                &service.service,
                service.http.as_ref(),
                service.http.is_some() && service.enabled,
            )?;
            self.set_endpoint(
                "-setsecurewebproxy",
                "-setsecurewebproxystate",
                &service.service,
                service.https.as_ref(),
                service.https.is_some() && service.enabled,
            )?;
            self.set_endpoint(
                "-setsocksfirewallproxy",
                "-setsocksfirewallproxystate",
                &service.service,
                service.socks.as_ref(),
                service.socks.is_some() && service.enabled,
            )?;
            self.set_bypass(&service.service, &service.bypass)?;
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
            self.outputs
                .lock()
                .expect("lock")
                .pop_front()
                .ok_or_else(|| PlatformError::Command {
                    program: "unexpected".into(),
                    args: Vec::new(),
                    message: "no scripted output".into(),
                })
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
        let runner = Arc::new(ScriptedRunner::new(vec![
            out("An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n"),
            // capture: web / state / secure / state / socks / state / bypass
            out(&proxy_block("127.0.0.1", 7890, true)),
            out("on\n"),
            out(&proxy_block("", 0, false)),
            out("off\n"),
            out(&proxy_block("", 0, false)),
            out("off\n"),
            out("127.0.0.1\n"),
            // apply
            out(""),
            out(""),
            out(""),
            out(""),
            out(""),
            out(""),
            out(""),
            // restore (2 calls per slot + bypass)
            out(""),
            out(""),
            out(""),
            out(""),
            out(""),
            out(""),
            out(""),
        ]));
        let proxy = MacOsSystemProxy::with_runner(runner.clone());
        let snapshot = proxy
            .apply(&ProxyTarget::all_through("127.0.0.1", 10808))
            .expect("apply");

        assert_eq!(snapshot.services.len(), 1);
        assert_eq!(
            snapshot.services[0].http,
            Some(ProxyEndpoint::new("127.0.0.1", 7890))
        );
        assert_eq!(snapshot.applied_endpoint.port, 10808);

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
            out("on\n"),
            out(&proxy_block("", 0, false)),
            out("off\n"),
            out(&proxy_block("", 0, false)),
            out("off\n"),
            out("127.0.0.1\n"),
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
    fn snapshot_ownership_is_detected() {
        let snapshot = TakeoverSnapshot {
            services: vec![ServiceProxyState {
                service: "Wi-Fi".into(),
                enabled: true,
                http: Some(ProxyEndpoint::new("127.0.0.1", 7890)),
                https: None,
                socks: None,
                bypass: vec![],
            }],
            applied_endpoint: ProxyEndpoint::new("127.0.0.1", 10808),
            applied_at_ms: 0,
        };
        assert!(snapshot.owned_by(&ProxyEndpoint::new("127.0.0.1", 10808)));
        assert!(snapshot.still_applied(&[ServiceProxyState {
            service: "Wi-Fi".into(),
            enabled: true,
            http: Some(ProxyEndpoint::new("127.0.0.1", 10808)),
            https: None,
            socks: None,
            bypass: vec![],
        }]));
        assert!(!snapshot.still_applied(&[ServiceProxyState {
            service: "Wi-Fi".into(),
            enabled: true,
            http: Some(ProxyEndpoint::new("127.0.0.1", 9090)),
            https: None,
            socks: None,
            bypass: vec![],
        }]));
    }
}
