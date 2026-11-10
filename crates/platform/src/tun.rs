//! Narrow unprivileged client for the platform TUN bridge. The administrator
//! provisions the helper and its core separately from the user's core store.
use serde::{Deserialize, Serialize};

use crate::PlatformError;

#[cfg(target_os = "linux")]
pub const HELPER: &str = "/usr/local/libexec/v2rayr/v2rayr-helper";
#[cfg(target_os = "linux")]
pub const BRIDGE_CORE: &str = "/usr/local/libexec/v2rayr/sing-box";
#[cfg(target_os = "macos")]
pub const HELPER: &str = "/Library/PrivilegedHelperTools/v2rayr/v2rayr-helper";
#[cfg(target_os = "macos")]
pub const BRIDGE_CORE: &str = "/Library/PrivilegedHelperTools/v2rayr/sing-box";
#[cfg(windows)]
pub const HELPER: &str = r"C:\Program Files\v2rayR\windows-tun\v2rayr-helper.exe";
#[cfg(windows)]
pub const BRIDGE_CORE: &str = r"C:\Program Files\v2rayR\windows-tun\sing-box.exe";
pub const ADDRESS: &str = "172.30.254.1/30";
pub const ADDRESS_V4: &str = "172.30.254.1";
pub const ADDRESS_V6: &str = "fd5b:67f3:2a9c::1";
#[cfg(target_os = "macos")]
pub const MAC_DNS_ADDRESS: &str = "198.18.0.53";
#[cfg(windows)]
pub mod windows;
pub const INTERFACE: &str = "v2rayr-tun0";
pub const ROUTE_TABLE: u32 = 18622;
pub const RULE_INDEX: u32 = 18622;

#[cfg(target_os = "linux")]
pub fn ip_query(args: &[&str]) -> Option<String> {
    let ip = ["/usr/sbin/ip", "/usr/bin/ip"]
        .into_iter()
        .find(|path| trusted_executable(path.as_ref()).is_ok())?;
    let output = std::process::Command::new(ip)
        .args(args)
        .env_clear()
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(target_os = "linux")]
pub fn network_cleared() -> bool {
    if std::path::Path::new("/sys/class/net")
        .join(INTERFACE)
        .exists()
    {
        return false;
    }
    ["-4", "-6"].into_iter().all(|family| {
        ip_query(&[family, "-N", "rule", "show"]).is_some_and(|text| {
            !text.lines().any(|line| {
                line.split_ascii_whitespace()
                    .collect::<Vec<_>>()
                    .windows(2)
                    .any(|pair| pair == ["lookup", &ROUTE_TABLE.to_string()])
            })
        }) && ip_query(&[family, "-N", "route", "show", "table", "all"]).is_some_and(|text| {
            !text.lines().any(|line| {
                line.split_ascii_whitespace()
                    .collect::<Vec<_>>()
                    .windows(2)
                    .any(|pair| pair == ["table", &ROUTE_TABLE.to_string()])
            })
        })
    })
}

#[cfg(windows)]
pub fn network_cleared() -> bool {
    windows::network_cleared()
}

#[cfg(any(target_os = "macos", test))]
fn mac_tun_interfaces(output: &str) -> Vec<String> {
    let mut name = "";
    let mut matches = Vec::new();
    for line in output.lines() {
        if !line.starts_with(char::is_whitespace) {
            name = line.split_once(':').map_or("", |(name, _)| name);
            continue;
        }
        let mut parts = line.split_ascii_whitespace();
        let kind = parts.next();
        let ip = parts.next().unwrap_or("").split('%').next().unwrap_or("");
        if name.starts_with("utun")
            && ((kind == Some("inet") && ip == ADDRESS_V4)
                || (kind == Some("inet6") && ip == ADDRESS_V6))
            && !matches.iter().any(|known| known == name)
        {
            matches.push(name.to_owned());
        }
    }
    matches
}

#[cfg(target_os = "macos")]
pub fn mac_interface() -> Option<String> {
    let interfaces = mac_interfaces()?;
    if interfaces.len() == 1 {
        interfaces.into_iter().next()
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn mac_interfaces() -> Option<Vec<String>> {
    let output = std::process::Command::new("/sbin/ifconfig")
        .arg("-a")
        .env_clear()
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| mac_tun_interfaces(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(target_os = "macos")]
fn mac_route_interface(destination: &str) -> std::io::Result<Option<String>> {
    let mut command = std::process::Command::new("/sbin/route");
    command.args(["-n", "get"]);
    if destination.contains(':') {
        command.arg("-inet6");
    }
    let output = command
        .arg(destination)
        .env_clear()
        .env("LC_ALL", "C")
        .output()?;
    parse_mac_route(
        output.status.success(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

#[cfg(any(target_os = "macos", test))]
fn parse_mac_route(success: bool, stdout: &str, stderr: &str) -> std::io::Result<Option<String>> {
    // Darwin route(8) can report a missing route on stderr and still exit 0.
    if stdout.trim().is_empty()
        && matches!(
            stderr.trim(),
            "route: writing to routing socket: not in table"
                | "route: writing to routing socket: Network is unreachable"
        )
    {
        return Ok(None);
    }
    if !success || !stderr.trim().is_empty() {
        return Err(std::io::Error::other("macOS route readback failed"));
    }
    stdout
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("interface:")
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .map(Some)
        .ok_or_else(|| std::io::Error::other("macOS route readback is missing an interface"))
}

#[cfg(target_os = "macos")]
pub fn mac_has_capture_routes(name: &str) -> bool {
    ["198.51.100.20", "2001:db8::20", MAC_DNS_ADDRESS]
        .into_iter()
        .all(|destination| {
            mac_route_interface(destination)
                .is_ok_and(|interface| interface.as_deref() == Some(name))
        })
}

#[cfg(target_os = "macos")]
pub fn mac_has_residual_routes(name: &str) -> bool {
    ["198.51.100.20", "2001:db8::20", MAC_DNS_ADDRESS]
        .into_iter()
        .any(|destination| {
            match mac_route_interface(destination) {
                Ok(interface) => interface.as_deref() == Some(name),
                Err(_) => true, // Unknown is not evidence of successful cleanup.
            }
        })
}

#[cfg(target_os = "macos")]
pub fn validate_apple_install() -> std::io::Result<()> {
    for path in [HELPER, BRIDGE_CORE] {
        trusted_executable(path.as_ref())?;
        for entry in std::path::Path::new(path).ancestors() {
            let output = std::process::Command::new("/bin/ls")
                .arg("-lde")
                .arg(entry)
                .env_clear()
                .env("LC_ALL", "C")
                .output()?;
            // POSIX mode alone does not exclude inherited ACL writes.
            if !output.status.success()
                || String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .skip(1)
                    .any(|line| line.split_ascii_whitespace().any(|token| token == "allow"))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "unexpected ACL on administrator-installed TUN path",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn network_cleared() -> bool {
    mac_interfaces().is_some_and(|interfaces| interfaces.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn darwin_missing_ipv6_route_can_exit_successfully() {
        assert_eq!(
            parse_mac_route(true, "", "route: writing to routing socket: not in table\n").unwrap(),
            None
        );
        assert_eq!(
            parse_mac_route(true, "  interface: utun5\n", "")
                .unwrap()
                .as_deref(),
            Some("utun5")
        );
        assert!(parse_mac_route(true, "", "route: unexpected error").is_err());
        assert!(parse_mac_route(true, "", "").is_err());
    }
    #[test]
    fn identifies_only_utun_interfaces_with_the_session_addresses() {
        let output = "utun0: flags=8051
	inet6 fe80::1%utun0 prefixlen 64
utun8: flags=8051
	inet 172.30.254.1 --> 172.30.254.1 netmask 0xfffffffc
	inet6 fd5b:67f3:2a9c::1%utun8 prefixlen 126
en0: flags=8863
	inet 172.30.254.1
";
        assert_eq!(mac_tun_interfaces(output), ["utun8"]);
    }
}

pub const RULE_ROUTING_SUPPORTED: bool = cfg!(target_os = "macos");
pub const MAX_REQUEST_BYTES: u64 = 128 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunRule {
    pub applications: Vec<String>,
    pub target: TunRuleTarget,
    pub action: TunRuleAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum TunRuleTarget {
    Any,
    Domain(String),
    DomainSuffix(String),
    IpCidr(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TunRuleAction {
    Proxy,
    Direct,
    Block,
}

impl TunRule {
    fn valid(&self) -> bool {
        let target =
            match &self.target {
                TunRuleTarget::Any => !self.applications.is_empty(),
                TunRuleTarget::Domain(value) | TunRuleTarget::DomainSuffix(value) => {
                    !value.is_empty()
                        && value.len() <= 253
                        && value.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_')
                        })
                }
                TunRuleTarget::IpCidr(value) => value.split_once('/').is_some_and(|(ip, bits)| {
                    match (ip.parse::<std::net::IpAddr>(), bits.parse::<u8>()) {
                        (Ok(std::net::IpAddr::V4(_)), Ok(bits)) => bits <= 32,
                        (Ok(std::net::IpAddr::V6(_)), Ok(bits)) => bits <= 128,
                        _ => false,
                    }
                }),
            };
        target
            && self.applications.len() <= 32
            && self.applications.iter().all(|path| {
                path.starts_with('/') && path.len() <= 4096 && !path.chars().any(char::is_control)
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunRequest {
    pub socks_port: u16,
    pub upstream: std::net::Ipv4Addr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<TunRule>>,
}

impl TunRequest {
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.socks_port < 1024
            || self.upstream.is_unspecified()
            || self.upstream.is_multicast()
            || self.upstream.is_broadcast()
            || self.upstream.is_loopback()
        {
            return Err(PlatformError::InvalidState(
                "TUN 需要有效的 IPv4 节点和本机 SOCKS 端口".into(),
            ));
        }
        if let Some(rules) = &self.rules {
            if !RULE_ROUTING_SUPPORTED {
                return Err(PlatformError::Unsupported("此平台的 TUN 暂只支持全局代理"));
            }
            if rules.len() > 256
                || rules.iter().any(|rule| !rule.valid())
                || serde_json::to_vec(self)
                    .map_err(std::io::Error::other)?
                    .len() as u64
                    >= MAX_REQUEST_BYTES
            {
                return Err(PlatformError::InvalidState(
                    "TUN 规则无效或超出大小限制".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn trusted_executable(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    for (index, entry) in path.ancestors().enumerate() {
        let metadata = std::fs::symlink_metadata(entry)?;
        if metadata.uid() != 0
            || metadata.mode() & 0o022 != 0
            || metadata.is_symlink()
            || (index == 0
                && (!metadata.is_file()
                    || metadata.mode() & 0o111 == 0
                    || metadata.mode() & 0o6000 != 0))
            || (index > 0 && !metadata.is_dir())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "untrusted TUN executable path",
            ));
        }
    }
    Ok(())
}

pub fn provisioned() -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    return trusted_executable(HELPER.as_ref()).is_ok()
        && trusted_executable(BRIDGE_CORE.as_ref()).is_ok();
    #[cfg(windows)]
    return windows::provisioned();
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    false
}

#[cfg(target_os = "macos")]
fn rule_helper_compatible() -> bool {
    std::process::Command::new(HELPER)
        .arg("tun-capabilities")
        .env_clear()
        .output()
        .is_ok_and(|output| output.status.success() && output.stdout == b"tun-rules-v1\n")
}

#[derive(Debug)]
pub struct TunLease {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    child: Option<std::process::Child>,
    #[cfg(target_os = "macos")]
    interface: Option<String>,
    #[cfg(windows)]
    windows: windows::Lease,
}

impl TunLease {
    #[cfg(windows)]
    pub async fn launch_windows(request: &TunRequest) -> Result<Self, PlatformError> {
        request.validate()?;
        Ok(Self {
            windows: windows::Lease::launch().await?,
        })
    }

    #[cfg(windows)]
    pub async fn wait_windows_ready(&mut self, request: &TunRequest) -> Result<(), PlatformError> {
        self.windows.wait_ready(request).await
    }

    #[cfg(not(windows))]
    pub async fn start(request: &TunRequest) -> Result<Self, PlatformError> {
        request.validate()?;
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        return Err(PlatformError::Unsupported("此平台尚未提供 TUN 特权服务"));
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use std::io::{BufRead, BufReader, Read, Write};
            use std::process::{Command, Stdio};
            use std::time::Duration;
            if !provisioned() {
                return Err(PlatformError::PermissionDenied(
                    "请由管理员部署 TUN helper 与专用内核".into(),
                ));
            }
            #[cfg(target_os = "macos")]
            validate_apple_install()?;
            #[cfg(target_os = "macos")]
            if request.rules.is_some() && !rule_helper_compatible() {
                return Err(PlatformError::Unsupported(
                    "请更新已部署的 TUN helper 以启用规则分流",
                ));
            }
            let mut child = Command::new("/usr/bin/sudo")
                .args(["-n", "--", HELPER, "tun-stdio"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?;
            let stdout = child.stdout.take().expect("piped stdout");
            let mut lease = Self {
                child: Some(child),
                #[cfg(target_os = "macos")]
                interface: None,
            };
            let input = lease
                .child
                .as_mut()
                .unwrap()
                .stdin
                .as_mut()
                .expect("piped stdin");
            serde_json::to_writer(&mut *input, request).map_err(std::io::Error::other)?;
            input.write_all(b"\n")?;
            input.flush()?;
            let (sender, mut receiver) = tokio::sync::oneshot::channel();
            std::thread::spawn(move || {
                let mut line = String::new();
                let read = BufReader::new(stdout).take(256).read_line(&mut line);
                let _ = sender.send(read.map(|_| line));
            });
            let answer = tokio::time::timeout(Duration::from_secs(30), &mut receiver)
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(Result::ok);
            if answer.as_deref() != Some("ready\n") || lease.exited() {
                return Err(PlatformError::PermissionDenied(
                    "TUN 未就绪：检查管理员授权、虚拟网卡、路由冲突和专用内核".into(),
                ));
            }
            #[cfg(target_os = "macos")]
            {
                lease.interface = mac_interface();
                if !lease
                    .interface
                    .as_deref()
                    .is_some_and(mac_has_capture_routes)
                {
                    lease.stop().await?;
                    return Err(PlatformError::InvalidState(
                        "macOS utun 接口或路由读回失败".into(),
                    ));
                }
            }
            Ok(lease)
        }
    }

    pub fn exited(&mut self) -> bool {
        #[cfg(windows)]
        return self.windows.exited();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        return self
            .child
            .as_mut()
            .is_none_or(|child| !matches!(child.try_wait(), Ok(None)));
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        true
    }

    pub async fn stop(&mut self) -> Result<(), PlatformError> {
        #[cfg(windows)]
        self.windows.stop().await?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(child) = self.child.as_mut() {
            drop(child.stdin.take());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
            while child.try_wait()?.is_none() {
                if std::time::Instant::now() >= deadline {
                    return Err(PlatformError::InvalidState(
                        "TUN helper 未退出，请重试恢复网络".into(),
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            }
            loop {
                #[cfg(target_os = "macos")]
                let residual = self
                    .interface
                    .as_deref()
                    .is_some_and(mac_has_residual_routes);
                #[cfg(target_os = "linux")]
                let residual = false;
                if network_cleared() && !residual {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(PlatformError::InvalidState(
                        "TUN 接口或路由尚未清理，请由管理员检查".into(),
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            }
        }
        Ok(())
    }
}

impl Drop for TunLease {
    fn drop(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(mut child) = self.child.take() {
            drop(child.stdin.take());
            if matches!(child.try_wait(), Ok(None)) {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
        }
    }
}
