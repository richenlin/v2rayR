//! Typed profile model.
//!
//! Every protocol specific knob is carried by [`Protocol`]; the fields shared by
//! all protocols (`address`, `port`, `transport`, `tls`) live on [`Profile`].
//! Unknown fields survive a round trip through [`Profile::extra`], so importing
//! and re-exporting a node never loses information (a hard requirement of the
//! migration path in §5.4).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{DomainError, ErrorCode};

/// Placeholder written instead of a credential by every `Debug`/`Display` impl.
pub const REDACTED: &str = "<redacted>";

/// A credential that must never leak into logs, UI DTOs or crash reports.
///
/// On disk the whole profile payload is encrypted (see `v2rayr-store::vault`),
/// so the plaintext only ever exists in memory. `Debug` and `Display` are
/// deliberately redacting to make `tracing::info!("{profile:?}")` safe.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The only way to read the plaintext. Named `expose` so it stands out in
    /// review and can be grepped.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Which proxy core runs a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoreKind {
    #[default]
    SingBox,
    Xray,
    Mihomo,
}

impl CoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingBox => "sing-box",
            Self::Xray => "xray",
            Self::Mihomo => "mihomo",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sing-box" | "singbox" | "auto" => Some(Self::SingBox),
            "xray" | "v2ray" => Some(Self::Xray),
            "mihomo" | "clash" => Some(Self::Mihomo),
            _ => None,
        }
    }

    /// Values accepted by `settings.core.preferred`.
    pub const PREFERENCES: [&'static str; 3] = ["auto", "sing-box", "xray"];
}

impl fmt::Display for CoreKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VmessSecurity {
    #[default]
    Auto,
    None,
    Aes128Gcm,
    Chacha20Poly1305,
    Zero,
}

impl VmessSecurity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::None => "none",
            Self::Aes128Gcm => "aes-128-gcm",
            Self::Chacha20Poly1305 => "chacha20-poly1305",
            Self::Zero => "zero",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Self::None,
            "aes-128-gcm" | "aes128-gcm" => Self::Aes128Gcm,
            "chacha20-poly1305" | "chacha20-ietf-poly1305" => Self::Chacha20Poly1305,
            "zero" => Self::Zero,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VlessFlow {
    XtlsRprxVision,
    XtlsRprxDirect,
}

impl VlessFlow {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::XtlsRprxVision => "xtls-rprx-vision",
            Self::XtlsRprxDirect => "xtls-rprx-direct",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "xtls-rprx-vision" => Some(Self::XtlsRprxVision),
            "xtls-rprx-direct" => Some(Self::XtlsRprxDirect),
            _ => None,
        }
    }
}

/// Shadowsocks cipher. Kept as a validated string because both cores gain and
/// retire methods with every release; an enum with an `Other` arm forces an
/// application release just to accept a new cipher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SsMethod(String);

/// Ciphers that both sing-box and Xray accept today.
const KNOWN_SS_METHODS: &[&str] = &[
    "aes-128-gcm",
    "aes-192-gcm",
    "aes-256-gcm",
    "chacha20-ietf-poly1305",
    "xchacha20-ietf-poly1305",
    "aes-128-cfb",
    "aes-192-cfb",
    "aes-256-cfb",
    "rc4-md5",
    "chacha20-ietf",
    "xchacha20",
    "none",
    "2022-blake3-aes-128-gcm",
    "2022-blake3-aes-256-gcm",
    "2022-blake3-chacha20-poly1305",
];

impl SsMethod {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into().trim().to_ascii_lowercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_known(&self) -> bool {
        KNOWN_SS_METHODS.contains(&self.0.as_str()) || self.0.starts_with("2022-")
    }

    /// `2022-blake3-*` ciphers require sing-box/Xray 2022 support and a base64
    /// pre-shared key rather than a passphrase.
    pub fn is_2022(&self) -> bool {
        self.0.starts_with("2022-")
    }

    pub fn is_aead(&self) -> bool {
        self.0.ends_with("-gcm") || self.0.contains("poly1305")
    }
}

impl fmt::Display for SsMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObfsKind {
    Salamander,
    Gecko,
}

impl ObfsKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Salamander => "salamander",
            Self::Gecko => "gecko",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "salamander" => Some(Self::Salamander),
            "gecko" => Some(Self::Gecko),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hysteria2Obfs {
    pub kind: ObfsKind,
    pub password: Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Congestion {
    #[default]
    Cubic,
    NewReno,
    Bbr,
}

impl Congestion {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cubic => "cubic",
            Self::NewReno => "new_reno",
            Self::Bbr => "bbr",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cubic" => Some(Self::Cubic),
            "new_reno" | "newreno" => Some(Self::NewReno),
            "bbr" => Some(Self::Bbr),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MkcpHeader {
    #[default]
    None,
    Srtp,
    Utp,
    WechatVideo,
    Dtls,
    Wireguard,
    Dns,
}

impl MkcpHeader {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Srtp => "srtp",
            Self::Utp => "utp",
            Self::WechatVideo => "wechat-video",
            Self::Dtls => "dtls",
            Self::Wireguard => "wireguard",
            Self::Dns => "dns",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "srtp" => Self::Srtp,
            "utp" => Self::Utp,
            "wechat-video" => Self::WechatVideo,
            "dtls" => Self::Dtls,
            "wireguard" => Self::Wireguard,
            "dns" => Self::Dns,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuicSecurity {
    #[default]
    None,
    Aes128Gcm,
    Chacha20Poly1305,
}

impl QuicSecurity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Aes128Gcm => "aes-128-gcm",
            Self::Chacha20Poly1305 => "chacha20-poly1305",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "aes-128-gcm" => Self::Aes128Gcm,
            "chacha20-poly1305" => Self::Chacha20Poly1305,
            _ => Self::None,
        }
    }
}

/// REALITY parameters. `public_key` is required when `security=reality`; a link
/// without it is rejected during parsing instead of failing in the kernel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealityOptions {
    pub public_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub short_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spider_x: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TlsOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alpn: Vec<String>,
    /// uTLS fingerprint (`chrome`, `firefox`, `randomized`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reality: Option<RealityOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ech: Option<String>,
}

impl TlsOptions {
    pub fn with_sni(sni: impl Into<String>) -> Self {
        Self {
            sni: Some(sni.into()),
            ..Self::default()
        }
    }

    pub fn is_reality(&self) -> bool {
        self.reality.is_some()
    }

    /// REALITY always needs a `server_name`; fall back to the address when the
    /// link only carries `pbk`.
    pub fn server_name(&self, address: &str) -> String {
        self.sni
            .as_deref()
            .filter(|sni| !sni.is_empty())
            .unwrap_or(address)
            .to_owned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MuxProtocol {
    H2mux,
    Smux,
    Yamux,
}

impl MuxProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::H2mux => "h2mux",
            Self::Smux => "smux",
            Self::Yamux => "yamux",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuxOptions {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u16>,
    #[serde(default)]
    pub protocol: Option<MuxProtocol>,
}

/// Transport layer shared by all protocols.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Transport {
    /// Plain TCP.
    #[default]
    Raw,
    Ws {
        #[serde(default = "default_ws_path")]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        /// `maxEarlyData` / `ed` for XTLS-style 0-RTT.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        early_data: Option<u32>,
    },
    Grpc {
        #[serde(default)]
        service_name: String,
        #[serde(default)]
        multi_mode: bool,
    },
    HttpUpgrade {
        #[serde(default = "default_ws_path")]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
    /// HTTP/2 transport (`type=http` or `type=h2`).
    Http {
        #[serde(default = "default_ws_path")]
        path: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        hosts: Vec<String>,
    },
    Xhttp {
        #[serde(default = "default_ws_path")]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        #[serde(default = "default_xhttp_mode")]
        mode: String,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        extra: Value,
    },
    Mkcp {
        #[serde(default)]
        header: MkcpHeader,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<String>,
    },
    Quic {
        #[serde(default)]
        security: QuicSecurity,
        #[serde(default)]
        key: Secret,
        #[serde(default)]
        header: String,
    },
}

fn default_ws_path() -> String {
    "/".to_owned()
}

fn default_xhttp_mode() -> String {
    "auto".to_owned()
}

impl Transport {
    /// Stable identifier used by settings fixtures, logs and the UI.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Ws { .. } => "ws",
            Self::Grpc { .. } => "grpc",
            Self::HttpUpgrade { .. } => "httpupgrade",
            Self::Http { .. } => "http",
            Self::Xhttp { .. } => "xhttp",
            Self::Mkcp { .. } => "mkcp",
            Self::Quic { .. } => "quic",
        }
    }

    /// Host header overrides carried by the transport, if any.
    pub fn host_header(&self) -> Option<&str> {
        match self {
            Self::Ws { host, .. } | Self::HttpUpgrade { host, .. } | Self::Xhttp { host, .. } => {
                host.as_deref().filter(|host| !host.is_empty())
            }
            Self::Http { hosts, .. } => hosts
                .first()
                .map(String::as_str)
                .filter(|host| !host.is_empty()),
            _ => None,
        }
    }

    /// Only `raw` and `ws` are legal for Shadowsocks (plugin semantics differ).
    pub fn is_ss_compatible(&self) -> bool {
        matches!(self, Self::Raw | Self::Ws { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireGuard {
    pub private_key: Secret,
    pub public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preshared_key: Option<Secret>,
    #[serde(default = "default_wg_mtu")]
    pub mtu: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_address: Vec<String>,
}

fn default_wg_mtu() -> u16 {
    1420
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tuic {
    pub uuid: Uuid,
    pub password: Secret,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub congestion: Option<Congestion>,
    #[serde(default)]
    pub udp_relay_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnyTls {
    pub password: Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Naive {
    pub username: String,
    pub password: Secret,
    #[serde(default)]
    pub quic: bool,
}

/// Protocol specific parameters.
///
/// Serde uses an internal tag so a payload stays readable and survives schema
/// upgrades: `{"kind":"vless","id":"...","flow":"xtls-rprx-vision"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Protocol {
    Vmess {
        id: Uuid,
        #[serde(default)]
        alter_id: u16,
        #[serde(default)]
        security: VmessSecurity,
    },
    Vless {
        id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flow: Option<VlessFlow>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encryption: Option<String>,
    },
    Trojan {
        password: Secret,
    },
    Shadowsocks {
        method: SsMethod,
        password: Secret,
    },
    Socks {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<Secret>,
    },
    Http {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<Secret>,
    },
    Hysteria2 {
        password: Secret,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        obfs: Option<Hysteria2Obfs>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        up_mbps: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        down_mbps: Option<u32>,
        /// `"443,8443"` or `"443-8443"`; `None` means a single `port`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ports: Option<String>,
    },
    Tuic(Tuic),
    WireGuard(WireGuard),
    AnyTls(AnyTls),
    Naive(Naive),
    /// Proxy chain: the first hop dials the second, and so on (advanced).
    Chain(Vec<Profile>),
    /// Raw JSON produced by the user or imported from another client.
    Custom {
        core: CoreKind,
        json: Value,
    },
}

impl Protocol {
    /// Stable identifier; also the value stored in `profile.protocol`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Vmess { .. } => "vmess",
            Self::Vless { .. } => "vless",
            Self::Trojan { .. } => "trojan",
            Self::Shadowsocks { .. } => "shadowsocks",
            Self::Socks { .. } => "socks",
            Self::Http { .. } => "http",
            Self::Hysteria2 { .. } => "hysteria2",
            Self::Tuic(_) => "tuic",
            Self::WireGuard(_) => "wireguard",
            Self::AnyTls(_) => "anytls",
            Self::Naive(_) => "naive",
            Self::Chain(_) => "chain",
            Self::Custom { .. } => "custom",
        }
    }

    /// Protocols that only sing-box implements.
    pub fn requires_singbox(&self) -> bool {
        matches!(
            self,
            Self::Hysteria2 { .. } | Self::Tuic(_) | Self::AnyTls(_) | Self::Naive(_)
        )
    }
}

/// A proxy node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_id: Option<Uuid>,
    pub remark: String,
    pub address: String,
    pub port: u16,
    pub protocol: Protocol,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mux: Option<MuxOptions>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Unknown inbound fields, kept so import → export is lossless.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub extra: Value,
}

impl Profile {
    /// Builds a profile with sensible defaults; used by parsers and the CLI.
    pub fn new(
        remark: impl Into<String>,
        address: impl Into<String>,
        port: u16,
        protocol: Protocol,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            sub_id: None,
            remark: remark.into(),
            address: address.into(),
            port,
            protocol,
            transport: Transport::Raw,
            tls: None,
            mux: None,
            tags: Vec::new(),
            extra: Value::Null,
        }
    }

    pub fn with_transport(mut self, transport: Transport) -> Self {
        self.transport = transport;
        self
    }

    pub fn with_tls(mut self, tls: TlsOptions) -> Self {
        self.tls = Some(tls);
        self
    }

    pub fn protocol_kind(&self) -> &'static str {
        self.protocol.kind()
    }

    pub fn transport_kind(&self) -> &'static str {
        self.transport.kind()
    }

    /// Minimal sanity check that does not need a kernel. Kernel specific
    /// capability checks live in `v2rayr-config`.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.address.trim().is_empty() {
            return Err(DomainError::new(ErrorCode::MissingField).with_detail("address"));
        }
        if self.port == 0 {
            return Err(DomainError::new(ErrorCode::InvalidPort).with_detail("0"));
        }
        if self.remark.trim().is_empty() {
            return Err(DomainError::new(ErrorCode::MissingField).with_detail("remark"));
        }
        if let Some(tls) = &self.tls {
            if let Some(reality) = &tls.reality {
                if reality.public_key.trim().is_empty() {
                    return Err(DomainError::new(ErrorCode::RealityWithoutPublicKey));
                }
            }
        }
        if matches!(self.protocol, Protocol::Shadowsocks { .. })
            && !self.transport.is_ss_compatible()
        {
            return Err(DomainError::new(ErrorCode::UnsupportedTransport)
                .with_detail(self.transport.kind()));
        }
        Ok(())
    }

    /// Everything that changes the bytes on the wire, and nothing that only
    /// changes how a node is displayed. Used for dedupe (§10.3).
    pub fn canonical_connection(&self) -> Value {
        serde_json::json!({
            "address": self.address.to_ascii_lowercase(),
            "port": self.port,
            "protocol": self.protocol,
            "transport": self.transport,
            "tls": self.tls,
            "extra": self.extra,
            "version": crate::PROFILE_PAYLOAD_VERSION,
        })
    }

    /// `tag` used inside generated kernel configs. Kept short and stable so the
    /// Clash API selector can address it across restarts.
    pub fn outbound_tag(&self) -> String {
        format!("{}-proxy", self.id)
    }
}

/// Default remark when a link has no fragment: `protocol host:port`.
pub fn derive_remark(protocol: &str, address: &str, port: u16) -> String {
    format!("{protocol} {address}:{port}")
}

/// Helper for serde: accept both `"443"` and `443` for numeric fields (vmess
/// JSON exported by various panels is inconsistent; upstream uses
/// `AllowReadingFromString` for the same reason).
pub fn de_u64_from_any<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Num(u64),
        Float(f64),
        Str(String),
    }

    match NumOrStr::deserialize(deserializer)? {
        NumOrStr::Num(value) => Ok(value),
        NumOrStr::Float(value) if value >= 0.0 => Ok(value as u64),
        NumOrStr::Float(_) => Err(serde::de::Error::custom("negative port")),
        NumOrStr::Str(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|_| serde::de::Error::custom(format!("not a number: {value}"))),
    }
}

/// Serializes a `Secret` without exposing it when the target type is `Debug`.
pub fn serde_redacted<S: Serializer>(_: &Secret, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(REDACTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vless() -> Profile {
        Profile::new(
            "test",
            "example.com",
            443,
            Protocol::Vless {
                id: Uuid::nil(),
                flow: Some(VlessFlow::XtlsRprxVision),
                encryption: None,
            },
        )
    }

    #[test]
    fn secret_never_leaks_in_debug_or_display() {
        let secret = Secret::new("hunter2");
        assert_eq!(format!("{secret:?}"), REDACTED);
        assert_eq!(format!("{secret}"), REDACTED);
        assert!(
            !format!(
                "{:?}",
                Protocol::Trojan {
                    password: secret.clone()
                }
            )
            .contains("hunter2")
        );
        assert_eq!(secret.expose(), "hunter2");
    }

    #[test]
    fn profile_roundtrips_through_json_without_losing_unknown_fields() {
        let mut profile = vless();
        profile.extra = serde_json::json!({"panel": "unknown-field"});
        let encoded = serde_json::to_string(&profile).expect("serialize");
        let decoded: Profile = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(profile, decoded);
        assert_eq!(decoded.extra["panel"], "unknown-field");
    }

    #[test]
    fn fingerprint_ignores_display_only_fields() {
        let a = vless();
        let mut b = a.clone();
        b.remark = "another name".into();
        b.tags = vec!["favourite".into()];
        assert_eq!(
            crate::fingerprint::fingerprint(&a, b"salt"),
            crate::fingerprint::fingerprint(&b, b"salt")
        );
        b.port = 8443;
        assert_ne!(
            crate::fingerprint::fingerprint(&a, b"salt"),
            crate::fingerprint::fingerprint(&b, b"salt")
        );
    }

    #[test]
    fn tls_reality_requires_public_key() {
        let mut profile = vless();
        profile.tls = Some(TlsOptions {
            reality: Some(RealityOptions {
                public_key: String::new(),
                short_id: String::new(),
                spider_x: None,
            }),
            ..TlsOptions::default()
        });
        assert_eq!(
            profile.validate().expect_err("must fail").code(),
            ErrorCode::RealityWithoutPublicKey
        );
    }

    #[test]
    fn shadowsocks_rejects_grpc_transport() {
        let mut profile = Profile::new(
            "ss",
            "example.com",
            443,
            Protocol::Shadowsocks {
                method: SsMethod::new("aes-256-gcm"),
                password: Secret::new("pw"),
            },
        );
        profile.transport = Transport::Grpc {
            service_name: "x".into(),
            multi_mode: false,
        };
        assert_eq!(
            profile.validate().expect_err("must fail").code(),
            ErrorCode::UnsupportedTransport
        );
    }

    #[test]
    fn numeric_fields_accept_strings() {
        #[derive(Deserialize)]
        struct V {
            #[serde(deserialize_with = "de_u64_from_any")]
            port: u64,
        }
        let from_str: V = serde_json::from_str(r#"{"port":"443"}"#).expect("string port");
        let from_num: V = serde_json::from_str(r#"{"port":443}"#).expect("numeric port");
        assert_eq!(from_str.port, 443);
        assert_eq!(from_num.port, 443);
    }
}
