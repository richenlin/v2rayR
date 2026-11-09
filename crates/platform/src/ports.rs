//! Port probing.
//!
//! The design document is explicit: a fixed port that is already taken must be
//! reported, never silently replaced by another one. `probe` therefore separates
//! "free" from "in use" and `find_free` is only used when the user asked for an
//! automatic port.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket};

use crate::PlatformError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortState {
    Free,
    InUse,
}

pub struct Ports;

impl Ports {
    /// Can we bind both TCP and UDP on `port`?
    pub fn probe(port: u16) -> PortState {
        if port == 0 {
            return PortState::InUse;
        }
        let tcp = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
        if tcp.is_err() {
            return PortState::InUse;
        }
        let udp = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
        match udp {
            Ok(_) => PortState::Free,
            Err(_) => PortState::InUse,
        }
    }

    pub fn is_free(port: u16) -> bool {
        Self::probe(port) == PortState::Free
    }

    /// First free port at or after `start`, scanning at most 200 candidates.
    pub fn find_free(start: u16) -> Result<u16, PlatformError> {
        (start..start.saturating_add(200))
            .find(|port| Self::is_free(*port))
            .ok_or_else(|| {
                PlatformError::InvalidState(format!("no free port in {start}..{}", start + 200))
            })
    }

    /// Best-effort: which process listens on this port. Used by the UI to say
    /// "port 10808 is used by another v2rayR instance" instead of a bare error.
    pub fn holder(port: u16) -> Option<u32> {
        let output = std::process::Command::new("/usr/sbin/lsof")
            .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-t"])
            .arg(format!("-i:{port}"))
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines()
            .find_map(|line| line.trim().parse::<u32>().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bound_port_is_reported_in_use() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        // The listener holds UDP too? Not necessarily: only assert TCP behaviour.
        assert_eq!(Ports::probe(port), PortState::InUse);
        drop(listener);
    }

    #[test]
    fn zero_is_never_a_valid_port() {
        assert_eq!(Ports::probe(0), PortState::InUse);
    }

    #[test]
    fn find_free_returns_something_bindable() {
        // Pick a high range to keep the test fast and avoid privileged ports.
        let port = Ports::find_free(49_000).expect("free port");
        assert!(port >= 49_000);
        assert!(Ports::is_free(port));
    }
}
