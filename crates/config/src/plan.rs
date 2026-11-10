//! The intermediate representation every emitter consumes.
//!
//! `SessionPlan` is intentionally kernel-agnostic: ports, DNS, routing policy,
//! custom rules and the (already resolved) proxy node. Emitters translate; they
//! never invent policy.

use v2rayr_domain::{CoreKind, MuxOptions, Profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingMode {
    Rule,
    Global,
    Direct,
}

impl RoutingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Global => "global",
            Self::Direct => "direct",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    Domain,
    DomainSuffix,
    IpCidr,
    Process,
    GeoIp,
    GeoSite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAction {
    Proxy,
    Direct,
    Block,
}

/// A user rule, already ordered by priority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRule {
    pub kind: RuleKind,
    pub value: String,
    pub action: RuleAction,
}

/// A binary rule set shipped with the app or downloaded on first use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSetRef {
    /// Tag used in route rules (`geosite-cn`).
    pub tag: String,
    /// Absolute path to the `.srs` file.
    pub path: String,
    /// Which traffic class it belongs to; emitters use it to decide the action.
    pub kind: RuleSetKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSetKind {
    /// Domestic sites: go direct.
    CnSites,
    /// Domestic IP ranges: go direct.
    CnIp,
    /// Ads and trackers: block.
    Ads,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundPlan {
    pub listen: String,
    pub port: u16,
    pub http_port: Option<u16>,
    pub dokodemo: Option<(u16, String, u16)>,
    pub lan: Option<LanInboundPlan>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct LanInboundPlan {
    pub address: std::net::Ipv4Addr,
    pub socks_port: u16,
    pub http_port: u16,
    pub password: String,
}

impl std::fmt::Debug for LanInboundPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanInboundPlan")
            .field("address", &self.address)
            .field("socks_port", &self.socks_port)
            .field("http_port", &self.http_port)
            .finish_non_exhaustive()
    }
}

pub const LAN_USERNAME: &str = "v2rayr";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiPlan {
    pub port: u16,
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunPlan {
    pub mtu: u16,
    /// Interface name; emitters fall back to a default when empty.
    pub interface: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsPlan {
    /// Empty means "use the system resolver".
    pub servers: Vec<String>,
    /// `prefer_ipv4`, `prefer_ipv6`, `ipv4_only`, `ipv6_only`.
    pub strategy: String,
    pub fake_ip: bool,
}

impl Default for DnsPlan {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            strategy: "prefer_ipv4".to_owned(),
            fake_ip: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionPlan {
    pub profile: Profile,
    pub core_override: Option<CoreKind>,
    pub inbound: InboundPlan,
    pub api: ApiPlan,
    pub tun: Option<TunPlan>,
    pub policy: RoutingMode,
    pub dns: DnsPlan,
    pub rules: Vec<CustomRule>,
    pub rule_sets: Vec<RuleSetRef>,
    pub sniffing: bool,
    pub tcp_fast_open: bool,
    pub mux: Option<MuxOptions>,
    pub log_level: String,
    pub block_ads: bool,
    /// Working directory the kernel runs in; relative `rule_set` paths resolve
    /// against it.
    pub data_dir: String,
}

impl SessionPlan {
    pub fn check(&self) -> Result<(), super::ConfigError> {
        if self.inbound.port == 0 {
            return Err(super::ConfigError::InvalidPlan("inbound port is 0".into()));
        }
        if self.inbound.port == self.api.port {
            return Err(super::ConfigError::InvalidPlan(
                "inbound and control API share a port".into(),
            ));
        }
        if let Some(http_port) = self.inbound.http_port {
            if http_port == self.inbound.port || http_port == self.api.port {
                return Err(super::ConfigError::InvalidPlan(
                    "http and socks ports collide".into(),
                ));
            }
        }
        if self.inbound.listen != "127.0.0.1" {
            return Err(super::ConfigError::InvalidPlan(
                "the unauthenticated listener must use IPv4 loopback".into(),
            ));
        }
        if let Some(lan) = &self.inbound.lan {
            let reserved = [
                Some(self.inbound.port),
                self.inbound.http_port,
                Some(self.api.port),
            ];
            if !lan.address.is_private()
                || lan.password.len() < 24
                || [lan.socks_port, lan.http_port]
                    .iter()
                    .any(|port| *port < 1024 || reserved.contains(&Some(*port)))
                || (self.inbound.http_port.is_some() && lan.socks_port == lan.http_port)
                || (self.inbound.http_port.is_none() && lan.socks_port != lan.http_port)
            {
                return Err(super::ConfigError::InvalidPlan(
                    "invalid authenticated LAN listener".into(),
                ));
            }
        }
        if self.tun.is_some() && self.inbound.http_port.is_some() {
            // Not fatal, but TUN only needs the mixed inbound; keep the plan honest.
            return Err(super::ConfigError::InvalidPlan(
                "TUN mode does not need a separate HTTP inbound".into(),
            ));
        }
        self.profile.validate().map_err(|error| {
            super::ConfigError::InvalidPlan(format!(
                "{}: {:?}",
                error.code().as_str(),
                error.detail()
            ))
        })?;
        Ok(())
    }

    /// The tag every emitter uses for the proxy outbound.
    pub fn proxy_tag(&self) -> String {
        self.profile.outbound_tag()
    }
}

impl RuleKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Domain => "domain",
            Self::DomainSuffix => "domain-suffix",
            Self::IpCidr => "ip-cidr",
            Self::Process => "process",
            Self::GeoIp => "geo-ip",
            Self::GeoSite => "geo-site",
        }
    }
}

impl RuleAction {
    pub fn id(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Direct => "direct",
            Self::Block => "block",
        }
    }
}
