//! An unscoped supplemental resolver for ordinary system lookups. The
//! Dynamic Store entry belongs to scutil's session; it is removed on EOF or
//! process death without overwriting DHCP, VPN, or persistent network settings.
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

const SCUTIL: &str = "/usr/sbin/scutil";
use v2rayr_platform::tun::MAC_DNS_ADDRESS as SERVER;
const KEY: &str = "State:/Network/Service/com.v2rayr.tun/DNS";

pub(super) struct Lease(Child);

impl Lease {
    pub(super) fn start() -> anyhow::Result<Self> {
        // Refuse an existing registration instead of accidentally leasing
        // another session's resolver.
        if advertised()? {
            bail!("macOS TUN DNS resolver already advertised");
        }
        let mut lease = Self(
            Command::new(SCUTIL)
                .env_clear()
                .env("LC_ALL", "C")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("starting macOS TUN DNS lease")?,
        );
        let input = lease.0.stdin.as_mut().expect("piped scutil stdin");
        // Empty match domain covers default queries. More-specific split DNS
        // domains keep their existing routing.
        write!(
            input,
            "d.init\nd.add ServerAddresses * {SERVER}\nd.add SupplementalMatchDomains * \"\"\nd.add SupplementalMatchOrders * # 1\nd.add SupplementalMatchDomainsNoSearch # 1\nadd {KEY} temporary\n"
        )?;
        input.flush()?;
        wait_advertised(true)?;
        if lease.0.try_wait()?.is_some() {
            bail!("macOS TUN DNS lease process exited");
        }
        Ok(lease)
    }

    pub(super) fn stop(mut self) -> anyhow::Result<()> {
        self.close()?;
        wait_advertised(false)
    }

    fn close(&mut self) -> anyhow::Result<()> {
        drop(self.0.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.0.try_wait()?.is_none() {
            if Instant::now() >= deadline {
                self.0.kill()?;
                self.0.wait()?;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn advertised() -> anyhow::Result<bool> {
    let output = Command::new(SCUTIL)
        .arg("--dns")
        .env_clear()
        .env("LC_ALL", "C")
        .output()?;
    if !output.status.success() {
        bail!("macOS resolver readback failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split("\n\n")
        .take_while(|block| !block.contains("for scoped queries"))
        .any(|block| {
            block
                .lines()
                .any(|line| line.trim().strip_prefix("nameserver[0] : ") == Some(SERVER))
                && block.lines().any(|line| line.contains("Supplemental"))
        }))
}

fn wait_advertised(expected: bool) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if advertised()? == expected {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("macOS TUN DNS resolver state did not converge");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
