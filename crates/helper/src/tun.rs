//! Privileged platform bridge. It never reads a user config or executes a path
//! supplied by the client. The user kernel remains unprivileged.
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use v2rayr_platform::tun::{ADDRESS, ADDRESS_V6, BRIDGE_CORE, TunRequest};
#[cfg(target_os = "linux")]
use v2rayr_platform::tun::{INTERFACE, ROUTE_TABLE, RULE_INDEX, ip_query};
#[cfg(unix)]
use v2rayr_platform::tun::{network_cleared, trusted_executable};

#[cfg(target_os = "macos")]
#[path = "tun_macos_dns.rs"]
mod macos_dns;

pub fn bridge_config(request: &TunRequest) -> serde_json::Value {
    let config = serde_json::json!({
        "log": {"disabled": true},
        "dns": {"servers": [{"type":"https","tag":"remote","server":"1.1.1.1","detour":"bridge"}],"final":"remote"},
        "inbounds": [{
            "type": "tun", "tag":"tun",
            "address": [ADDRESS, format!("{ADDRESS_V6}/126")],
            "mtu": 1500, "auto_route": true,
            "stack": "system",
            "route_exclude_address": [format!("{}/32", request.upstream), "127.0.0.0/8",
                "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16",
                "::1/128", "fe80::/10", "fc00::/7"]
        }],
        "outbounds": [{"type":"socks","tag":"bridge","server":"127.0.0.1","server_port":request.socks_port}],
        "route": {"rules":[
            {"ip_cidr": [format!("{}/32", v2rayr_platform::tun::ADDRESS_V4), format!("{ADDRESS_V6}/128")], "action":"reject","method":"drop"},
            {"action":"sniff"},{"protocol":"dns","action":"hijack-dns"}],
            "final":"bridge"}
    });
    let mut config = config;
    #[cfg(target_os = "macos")]
    {
        config["inbounds"][0]["stack"] = serde_json::json!("gvisor");
        config["route"]["rules"]
            .as_array_mut()
            .expect("fixed route rules")
            .insert(
                0,
                serde_json::json!({
                    "ip_cidr": [format!("{}/32", v2rayr_platform::tun::MAC_DNS_ADDRESS)],
                    "port": 53,
                    "action": "hijack-dns"
                }),
            );
    }
    #[cfg(target_os = "linux")]
    {
        let inbound = &mut config["inbounds"][0];
        inbound["interface_name"] = serde_json::json!(INTERFACE);
        inbound["strict_route"] = serde_json::json!(true);
        inbound["iproute2_table_index"] = serde_json::json!(ROUTE_TABLE);
        inbound["iproute2_rule_index"] = serde_json::json!(RULE_INDEX);
    }
    #[cfg(windows)]
    {
        let inbound = &mut config["inbounds"][0];
        inbound["interface_name"] = serde_json::json!(v2rayr_platform::tun::INTERFACE);
        inbound["strict_route"] = serde_json::json!(true);
    }
    config
}

struct Bridge(
    Child,
    #[cfg(windows)]
    #[allow(dead_code)]
    v2rayr_platform::tun::windows::Job,
);

impl Bridge {
    fn spawn(mut command: Command) -> anyhow::Result<Self> {
        #[cfg(windows)]
        {
            let job = v2rayr_platform::tun::windows::Job::new()?;
            v2rayr_platform::tun::windows::prepare_core(&mut command)?;
            let mut child = command.spawn()?;
            if let Err(error) = job.assign(&child) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
            Ok(Self(child, job))
        }
        #[cfg(unix)]
        Ok(Self(command.spawn()?))
    }
}

impl Bridge {
    fn stop(&mut self) -> anyhow::Result<()> {
        if self.0.try_wait()?.is_none() {
            #[cfg(unix)]
            {
                // SAFETY: the child is owned and unreaped, so the PID cannot be reused.
                unsafe { libc::kill(self.0.id() as libc::pid_t, libc::SIGTERM) };
            }
            #[cfg(windows)]
            v2rayr_platform::tun::windows::request_core_stop(&self.0);
            let deadline = Instant::now() + Duration::from_secs(4);
            while self.0.try_wait()?.is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(30));
            }
            if self.0.try_wait()?.is_none() {
                self.0.kill()?;
            }
        }
        self.0.wait()?;
        Ok(())
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(unix)]
pub fn serve() -> anyhow::Result<()> {
    // SAFETY: querying process identity has no side effects.
    if unsafe { libc::geteuid() } != 0 {
        bail!("TUN helper must be invoked by the administrator's sudo policy");
    }
    #[cfg(target_os = "macos")]
    v2rayr_platform::tun::validate_apple_install()?;
    serve_io(std::io::stdin(), std::io::stdout())
}

#[cfg(windows)]
pub fn serve_windows(nonce: &str, caller: u32) -> anyhow::Result<()> {
    let pipe = v2rayr_platform::tun::windows::open_for_helper(nonce, caller)?;
    serve_io(pipe.try_clone()?, pipe)
}

fn serve_io(reader: impl Read + Send + 'static, mut writer: impl Write) -> anyhow::Result<()> {
    #[cfg(unix)]
    trusted_executable(BRIDGE_CORE.as_ref())
        .context("validating administrator-installed TUN core")?;
    #[cfg(windows)]
    v2rayr_platform::tun::windows::validate_install()
        .context("validating administrator-installed TUN core")?;
    #[cfg(windows)]
    use v2rayr_platform::tun::windows::network_cleared;
    #[cfg(target_os = "linux")]
    let runtime = "/run";
    #[cfg(target_os = "macos")]
    let runtime = "/private/var/run";
    #[cfg(windows)]
    let runtime = v2rayr_platform::tun::windows::RUNTIME;
    let lock = v2rayr_platform::acquire_single_instance(
        &std::path::Path::new(runtime).join("v2rayr-tun-bridge.lock"),
    )?;
    let v2rayr_platform::InstanceOutcome::Primary(_lock) = lock else {
        bail!("TUN bridge already in use")
    };
    if !network_cleared() {
        bail!("TUN device or routes already present, or network state unavailable");
    }
    #[cfg(target_os = "linux")]
    if !std::path::Path::new("/dev/net/tun").exists() {
        bail!("TUN device unavailable");
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let (eof_sender, eof) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut input = BufReader::new(reader);
        let mut frame = Vec::new();
        let read = (&mut input).take(512).read_until(b'\n', &mut frame);
        if read.is_err() || frame.last() != Some(&b'\n') {
            return;
        }
        let Ok(request) = serde_json::from_slice::<TunRequest>(&frame) else {
            return;
        };
        if sender.send(request).is_err() {
            return;
        }
        // EOF is the lease; any extra data terminates it, never a new command.
        let mut byte = [0];
        let _ = input.read(&mut byte);
        let _ = eof_sender.send(());
    });
    let request = receiver.recv_timeout(Duration::from_secs(5))?;
    request.validate()?;
    let directory = tempfile::Builder::new()
        .prefix("v2rayr-tun-")
        .tempdir_in(runtime)?;
    let config = directory.path().join("config.json");
    std::fs::write(&config, serde_json::to_vec(&bridge_config(&request))?)?;
    let build = |action: &str| {
        let mut command = Command::new(BRIDGE_CORE);
        command
            .arg(action)
            .arg("-c")
            .arg(&config)
            .arg("-D")
            .arg(directory.path())
            .env_clear()
            .current_dir(directory.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(target_os = "linux")]
        {
            // SAFETY: getpid/prctl/getppid are async-signal-safe in the pre-exec
            // child. The parent identity check closes the fork-to-prctl race.
            let parent = unsafe { libc::getpid() };
            unsafe {
                command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0
                        || libc::getppid() != parent
                    {
                        return Err(std::io::Error::from_raw_os_error(libc::EIO));
                    }
                    Ok(())
                });
            }
        }
        command
    };
    let mut check = Bridge::spawn(build("check"))?;
    let check_deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = check.0.try_wait()? {
            if !status.success() {
                bail!("bridge core rejected configuration")
            }
            break;
        }
        if Instant::now() >= check_deadline || lease_ended(&eof) {
            bail!("bridge check interrupted")
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let mut bridge = Bridge::spawn(build("run"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let owned_interface = loop {
        if bridge.0.try_wait()?.is_some() {
            bail!("bridge core exited before TUN was ready")
        }
        if lease_ended(&eof) || Instant::now() >= deadline {
            bail!("TUN startup interrupted")
        }
        if let Some(interface) = interface_ready() {
            break interface;
        }
        std::thread::sleep(Duration::from_millis(80));
    };
    #[cfg(target_os = "macos")]
    let dns = macos_dns::Lease::start()?;
    writer.write_all(b"ready\n")?;
    writer.flush()?;
    loop {
        if bridge.0.try_wait()?.is_some() || lease_ended(&eof) {
            break;
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    #[cfg(target_os = "macos")]
    dns.stop()?;
    bridge.stop()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while !network_cleared() || route_still_owned(&owned_interface) {
        if Instant::now() >= deadline {
            bail!("TUN interface or route still exists")
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    Ok(())
}

fn lease_ended(eof: &std::sync::mpsc::Receiver<()>) -> bool {
    !matches!(eof.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty))
}

#[cfg(target_os = "linux")]
fn interface_ready() -> Option<String> {
    let table = ROUTE_TABLE.to_string();
    let ready = ip_query(&["-o", "addr", "show", "dev", INTERFACE])
        .is_some_and(|text| text.contains(ADDRESS))
        && ["-4", "-6"].into_iter().all(|family| {
            ip_query(&[family, "route", "show", "table", &table])
                .is_some_and(|text| text.contains(INTERFACE))
                && ip_query(&[family, "-N", "rule", "show"]).is_some_and(|text| {
                    text.lines().any(|line| {
                        line.split_ascii_whitespace()
                            .collect::<Vec<_>>()
                            .windows(2)
                            .any(|pair| pair == ["lookup", &table])
                    })
                })
        });
    ready.then(|| INTERFACE.to_owned())
}

#[cfg(target_os = "macos")]
fn interface_ready() -> Option<String> {
    let interface = v2rayr_platform::tun::mac_interface()?;
    v2rayr_platform::tun::mac_has_capture_routes(&interface).then_some(interface)
}

#[cfg(windows)]
fn interface_ready() -> Option<String> {
    v2rayr_platform::tun::windows::interface_ready()
        .then(|| v2rayr_platform::tun::INTERFACE.to_owned())
}

fn route_still_owned(interface: &str) -> bool {
    #[cfg(target_os = "macos")]
    return v2rayr_platform::tun::mac_has_residual_routes(interface);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = interface;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_configuration_keeps_the_privileged_bridge_narrow() {
        let request = TunRequest {
            socks_port: 10808,
            upstream: "192.0.2.99".parse().unwrap(),
        };
        let config = bridge_config(&request);
        let inbound = &config["inbounds"][0];
        assert_eq!(config["outbounds"][0]["server"], "127.0.0.1");
        assert_eq!(config["outbounds"][0]["server_port"], 10808);
        assert!(
            inbound["route_exclude_address"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "192.0.2.99/32")
        );
        let rules = config["route"]["rules"].as_array().unwrap();
        assert!(rules.iter().any(|rule| rule["action"] == "reject"
            && rule["method"] == "drop"
            && rule["ip_cidr"][0] == format!("{}/32", v2rayr_platform::tun::ADDRESS_V4)));
        #[cfg(target_os = "macos")]
        {
            assert!(inbound.get("interface_name").is_none());
            assert!(inbound.get("iproute2_table_index").is_none());
            assert_eq!(rules[0]["action"], "hijack-dns");
            assert_eq!(rules[0]["port"], 53);
            assert_eq!(
                rules[0]["ip_cidr"][0],
                format!("{}/32", v2rayr_platform::tun::MAC_DNS_ADDRESS)
            );
        }
        #[cfg(windows)]
        {
            assert_eq!(inbound["interface_name"], v2rayr_platform::tun::INTERFACE);
            assert_eq!(inbound["strict_route"], true);
            assert!(inbound.get("iproute2_table_index").is_none());
        }
    }

    #[test]
    #[ignore = "set V2RAYR_TEST_SING_BOX to a real local sing-box executable"]
    fn installed_sing_box_accepts_the_generated_bridge_config() {
        let core = std::env::var_os("V2RAYR_TEST_SING_BOX").expect("core executable");
        let request = TunRequest {
            socks_port: 10808,
            upstream: "192.0.2.99".parse().unwrap(),
        };
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&bridge_config(&request)).unwrap(),
        )
        .unwrap();
        let output = Command::new(core)
            .args(["check", "-c"])
            .arg(config)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
