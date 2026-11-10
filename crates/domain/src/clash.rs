//! Import proxies from a Clash style YAML configuration.
//!
//! There is no YAML dependency in this project (a full parser is much more code
//! than we need, and `serde_yaml` is unmaintained). Clash configs use a very
//! regular subset: block mappings, block sequences, flow mappings/sequences and
//! quoted scalars. This module parses exactly that subset and turns the
//! `proxies:` list into [`Profile`]s. Anything it does not understand is
//! reported as skipped rather than dropped silently.

use std::collections::BTreeMap;

use crate::error::{DomainError, ErrorCode};
use crate::model::{
    AnyTls, Congestion, Hysteria2Obfs, ObfsKind, Profile, Protocol, RealityOptions, Secret,
    SsMethod, TlsOptions, Transport, Tuic, VmessSecurity, WireGuard,
};

/// Result of reading the `proxies:` list of a Clash configuration.
#[derive(Debug, Clone, Default)]
pub struct ClashImport {
    pub profiles: Vec<Profile>,
    /// Human readable reasons, one per proxy we could not map.
    pub skipped: Vec<String>,
}

/// Parse a Clash configuration and map its proxies to profiles.
///
/// Returns [`ErrorCode::UnsupportedFormat`] when the document has no `proxies:`
/// key at all, so callers can tell "not Clash" from "Clash with nothing usable".
pub fn parse_clash_config(text: &str) -> Result<ClashImport, DomainError> {
    let document = Yaml::parse(text)?;
    let Some(proxies) = document.get("proxies") else {
        return Err(DomainError::new(ErrorCode::UnsupportedFormat)
            .with_detail("配置里没有 proxies 段".to_owned()));
    };
    let items = proxies.as_seq();
    let mut out = ClashImport::default();
    for item in items {
        let name = item
            .get("name")
            .and_then(Yaml::as_str)
            .unwrap_or("未命名节点");
        match proxy_to_profile(item) {
            Ok(profile) => out.profiles.push(profile),
            Err(error) => out.skipped.push(format!("{name}: {error}")),
        }
    }
    Ok(out)
}

fn parse_uuid(text: &str) -> Result<uuid::Uuid, String> {
    uuid::Uuid::parse_str(text.trim()).map_err(|_| "uuid 格式不正确".to_owned())
}

fn proxy_to_profile(proxy: &Yaml) -> Result<Profile, String> {
    let kind = proxy
        .get("type")
        .and_then(Yaml::as_str)
        .ok_or_else(|| "缺少 type".to_owned())?;
    let server = proxy
        .get("server")
        .and_then(Yaml::as_str)
        .ok_or_else(|| "缺少 server".to_owned())?;
    let port = proxy
        .get("port")
        .and_then(Yaml::as_u16)
        .ok_or_else(|| "缺少合法 port".to_owned())?;
    let remark = proxy
        .get("name")
        .and_then(Yaml::as_str)
        .unwrap_or(server)
        .to_owned();

    let transport = transport_of(proxy)?;
    let protocol = match kind {
        "ss" | "shadowsocks" => {
            let method = proxy
                .get("cipher")
                .and_then(Yaml::as_str)
                .ok_or_else(|| "缺少 cipher".to_owned())?;
            let password = proxy
                .get("password")
                .and_then(Yaml::as_str)
                .ok_or_else(|| "缺少 password".to_owned())?;
            Protocol::Shadowsocks {
                method: SsMethod::new(method),
                password: Secret::new(password),
            }
        }
        "vmess" => {
            let id = proxy
                .get("uuid")
                .and_then(Yaml::as_str)
                .ok_or_else(|| "缺少 uuid".to_owned())?;
            let alter_id = proxy
                .get("alterId")
                .or_else(|| proxy.get("alter-id"))
                .and_then(Yaml::as_u64)
                .unwrap_or(0) as u16;
            let security = proxy
                .get("cipher")
                .and_then(Yaml::as_str)
                .map(VmessSecurity::parse)
                .unwrap_or(VmessSecurity::Auto);
            Protocol::Vmess {
                id: parse_uuid(id)?,
                alter_id,
                security,
            }
        }
        "vless" => {
            let id = proxy
                .get("uuid")
                .and_then(Yaml::as_str)
                .ok_or_else(|| "缺少 uuid".to_owned())?;
            let flow = proxy
                .get("flow")
                .and_then(Yaml::as_str)
                .and_then(crate::model::VlessFlow::parse);
            Protocol::Vless {
                id: parse_uuid(id)?,
                flow,
                encryption: Some("none".to_owned()),
            }
        }
        "trojan" => Protocol::Trojan {
            password: Secret::new(
                proxy
                    .get("password")
                    .and_then(Yaml::as_str)
                    .ok_or_else(|| "缺少 password".to_owned())?,
            ),
        },
        "hysteria2" | "hy2" => Protocol::Hysteria2 {
            password: Secret::new(
                proxy
                    .get("password")
                    .and_then(Yaml::as_str)
                    .unwrap_or_default(),
            ),
            obfs: proxy
                .get("obfs")
                .and_then(Yaml::as_str)
                .filter(|kind| !kind.is_empty())
                .map(|kind| {
                    Ok::<_, String>(Hysteria2Obfs {
                        kind: ObfsKind::parse(kind)
                            .ok_or_else(|| "不支持的 Hysteria2 混淆类型".to_owned())?,
                        password: Secret::new(
                            proxy
                                .get("obfs-password")
                                .and_then(Yaml::as_str)
                                .unwrap_or_default(),
                        ),
                    })
                })
                .transpose()?,
            up_mbps: proxy.get("up").and_then(Yaml::as_u64).map(|v| v as u32),
            down_mbps: proxy.get("down").and_then(Yaml::as_u64).map(|v| v as u32),
            ports: proxy.get("ports").and_then(Yaml::as_str).map(str::to_owned),
        },
        "tuic" => Protocol::Tuic(Tuic {
            uuid: parse_uuid(
                proxy
                    .get("uuid")
                    .and_then(Yaml::as_str)
                    .ok_or_else(|| "缺少 uuid".to_owned())?,
            )?,
            password: Secret::new(
                proxy
                    .get("password")
                    .and_then(Yaml::as_str)
                    .unwrap_or_default(),
            ),
            congestion: proxy
                .get("congestion-controller")
                .and_then(Yaml::as_str)
                .and_then(Congestion::parse),
            udp_relay_mode: proxy
                .get("udp-relay-mode")
                .and_then(Yaml::as_str)
                .map(str::to_owned),
        }),
        "anytls" => Protocol::AnyTls(AnyTls {
            password: Secret::new(
                proxy
                    .get("password")
                    .and_then(Yaml::as_str)
                    .unwrap_or_default(),
            ),
        }),
        "socks5" => Protocol::Socks {
            username: proxy
                .get("username")
                .and_then(Yaml::as_str)
                .map(str::to_owned),
            password: proxy
                .get("password")
                .and_then(Yaml::as_str)
                .map(Secret::new),
        },
        "http" => Protocol::Http {
            username: proxy
                .get("username")
                .and_then(Yaml::as_str)
                .map(str::to_owned),
            password: proxy
                .get("password")
                .and_then(Yaml::as_str)
                .map(Secret::new),
        },
        "wireguard" => Protocol::WireGuard(WireGuard {
            private_key: Secret::new(
                proxy
                    .get("private-key")
                    .and_then(Yaml::as_str)
                    .unwrap_or_default(),
            ),
            public_key: proxy
                .get("public-key")
                .and_then(Yaml::as_str)
                .unwrap_or_default()
                .to_owned(),
            preshared_key: proxy
                .get("pre-shared-key")
                .and_then(Yaml::as_str)
                .map(Secret::new),
            mtu: proxy.get("mtu").and_then(Yaml::as_u16).unwrap_or(1420),
            local_address: proxy.get("ip").map(Yaml::as_str_list).unwrap_or_default(),
        }),
        other => return Err(format!("暂不支持 {other}")),
    };

    let tls = tls_of(proxy, kind);
    let mut profile = Profile::new(remark, server.to_owned(), port, protocol);
    profile.transport = transport;
    profile.tls = tls;
    profile
        .validate()
        .map_err(|error| error.code().as_str().to_string())?;
    Ok(profile)
}

fn tls_of(proxy: &Yaml, kind: &str) -> Option<TlsOptions> {
    let tls_on = proxy.get("tls").and_then(Yaml::as_bool).unwrap_or(false);
    let always_tls = matches!(kind, "trojan" | "hysteria2" | "hy2" | "anytls" | "tuic");
    if !tls_on && !always_tls {
        return None;
    }
    let mut tls = TlsOptions {
        sni: proxy
            .get("servername")
            .or_else(|| proxy.get("sni"))
            .and_then(Yaml::as_str)
            .map(str::to_owned),
        insecure: proxy
            .get("skip-cert-verify")
            .and_then(Yaml::as_bool)
            .unwrap_or(false),
        alpn: proxy.get("alpn").map(Yaml::as_str_list).unwrap_or_default(),
        fingerprint: proxy
            .get("client-fingerprint")
            .and_then(Yaml::as_str)
            .map(str::to_owned),
        reality: None,
        ech: None,
    };
    if let Some(reality) = proxy.get("reality-opts") {
        let public_key = reality
            .get("public-key")
            .and_then(Yaml::as_str)
            .unwrap_or_default()
            .to_owned();
        let short_id = reality
            .get("short-id")
            .and_then(Yaml::as_str)
            .unwrap_or_default()
            .to_owned();
        if !public_key.is_empty() {
            tls.reality = Some(RealityOptions {
                public_key,
                short_id,
                spider_x: reality
                    .get("spider-x")
                    .and_then(Yaml::as_str)
                    .map(str::to_owned),
            });
        }
    }
    Some(tls)
}

fn transport_of(proxy: &Yaml) -> Result<Transport, String> {
    let network = proxy.get("network").and_then(Yaml::as_str).unwrap_or("tcp");
    let header = proxy
        .get("header-type")
        .and_then(Yaml::as_str)
        .map(str::to_owned);
    let transport = match network {
        "tcp" | "raw" => Transport::Raw,
        "ws" => {
            let options = proxy.get("ws-opts");
            Transport::Ws {
                path: options
                    .and_then(|o| o.get("path"))
                    .and_then(Yaml::as_str)
                    .unwrap_or("/")
                    .to_owned(),
                host: options
                    .and_then(|o| o.get("headers"))
                    .and_then(|h| h.get("Host").or_else(|| h.get("host")))
                    .and_then(Yaml::as_str)
                    .map(str::to_owned),
                early_data: None,
            }
        }
        "grpc" => {
            let options = proxy.get("grpc-opts");
            Transport::Grpc {
                service_name: options
                    .and_then(|o| o.get("grpc-service-name"))
                    .and_then(Yaml::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                multi_mode: false,
            }
        }
        "h2" | "http" => {
            let options = proxy.get("h2-opts").or_else(|| proxy.get("http-opts"));
            Transport::Http {
                path: options
                    .and_then(|o| o.get("path"))
                    .and_then(Yaml::as_str)
                    .unwrap_or("/")
                    .to_owned(),
                hosts: options.map(Yaml::as_str_list).unwrap_or_default(),
            }
        }
        "httpupgrade" => {
            let options = proxy.get("http-opts").or_else(|| proxy.get("ws-opts"));
            Transport::HttpUpgrade {
                path: options
                    .and_then(|o| o.get("path"))
                    .and_then(Yaml::as_str)
                    .unwrap_or("/")
                    .to_owned(),
                host: options
                    .and_then(|o| o.get("host"))
                    .and_then(Yaml::as_str)
                    .map(str::to_owned),
            }
        }
        other => return Err(format!("暂不支持 network={other}")),
    };
    let _ = header;
    Ok(transport)
}

// ---------------------------------------------------------------------------
// A YAML subset parser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Yaml {
    Null,
    Scalar(String),
    Seq(Vec<Yaml>),
    Map(BTreeMap<String, Yaml>),
}

impl Yaml {
    fn parse(text: &str) -> Result<Self, DomainError> {
        let lines = Lines::collect(text);
        if lines.is_empty() {
            return Err(DomainError::new(ErrorCode::EmptyInput));
        }
        let mut cursor = 0;
        let (value, _) = Lines::parse_block(&lines, &mut cursor, lines[0].indent)?;
        Ok(value)
    }

    fn get(&self, key: &str) -> Option<&Yaml> {
        match self {
            Yaml::Map(map) => map.get(key),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Yaml::Scalar(text) => Some(text.as_str()),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self.as_str()? {
            "true" | "True" | "yes" | "on" => Some(true),
            "false" | "False" | "no" | "off" => Some(false),
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        self.as_str()?.parse().ok()
    }

    fn as_u16(&self) -> Option<u16> {
        self.as_u64()?.try_into().ok()
    }

    fn as_seq(&self) -> &[Yaml] {
        match self {
            Yaml::Seq(items) => items,
            _ => &[],
        }
    }

    /// A scalar, a sequence of scalars, or (for `ip`) a comma separated string.
    fn as_str_list(&self) -> Vec<String> {
        match self {
            Yaml::Scalar(text) => text
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect(),
            Yaml::Seq(items) => items
                .iter()
                .filter_map(Yaml::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        }
    }
}

struct Line {
    indent: usize,
    text: String,
}

struct Lines;

impl Lines {
    fn collect(text: &str) -> Vec<Line> {
        let mut out = Vec::new();
        for raw in text.lines() {
            let without_comment = strip_comment(raw);
            let trimmed = without_comment.trim_end();
            if trimmed.trim().is_empty() {
                continue;
            }
            if trimmed.trim_start().starts_with("---") || trimmed.trim_start().starts_with("...") {
                continue;
            }
            let indent = trimmed.len() - trimmed.trim_start().len();
            out.push(Line {
                indent,
                text: trimmed.trim_start().to_owned(),
            });
        }
        out
    }

    fn parse_block(
        lines: &[Line],
        cursor: &mut usize,
        indent: usize,
    ) -> Result<(Yaml, usize), DomainError> {
        if *cursor >= lines.len() {
            return Ok((Yaml::Null, *cursor));
        }
        if lines[*cursor].text.starts_with("- ") || lines[*cursor].text == "-" {
            return Self::parse_seq(lines, cursor, indent);
        }
        Self::parse_map(lines, cursor, indent)
    }

    fn parse_seq(
        lines: &[Line],
        cursor: &mut usize,
        indent: usize,
    ) -> Result<(Yaml, usize), DomainError> {
        let mut items = Vec::new();
        while *cursor < lines.len()
            && lines[*cursor].indent == indent
            && (lines[*cursor].text.starts_with("- ") || lines[*cursor].text == "-")
        {
            let rest = lines[*cursor].text[1..].trim_start().to_owned();
            *cursor += 1;
            if rest.is_empty() {
                let (value, next) =
                    Self::parse_block(lines, cursor, Self::child_indent(lines, *cursor))?;
                *cursor = next;
                items.push(value);
                continue;
            }
            if let Some((key, value)) = split_pair(&rest) {
                // `- key: value` plus any following keys of the same mapping.
                let mut map = BTreeMap::new();
                let inline = Self::parse_inline_or_nested(lines, cursor, value)?;
                map.insert(key, inline);
                while *cursor < lines.len()
                    && lines[*cursor].indent > indent
                    && !lines[*cursor].text.starts_with("- ")
                {
                    let child_indent = lines[*cursor].indent;
                    let (more, next) = Self::parse_map(lines, cursor, child_indent)?;
                    if let Yaml::Map(extra) = more {
                        map.extend(extra);
                    }
                    *cursor = next;
                }
                items.push(Yaml::Map(map));
            } else {
                items.push(Yaml::Scalar(unquote(&rest)));
            }
        }
        Ok((Yaml::Seq(items), *cursor))
    }

    fn parse_map(
        lines: &[Line],
        cursor: &mut usize,
        indent: usize,
    ) -> Result<(Yaml, usize), DomainError> {
        let mut map = BTreeMap::new();
        while *cursor < lines.len()
            && lines[*cursor].indent == indent
            && !lines[*cursor].text.starts_with("- ")
        {
            let text = lines[*cursor].text.clone();
            let Some((key, value)) = split_pair(&text) else {
                *cursor += 1;
                continue;
            };
            *cursor += 1;
            let parsed = Self::parse_inline_or_nested(lines, cursor, value)?;
            map.insert(key, parsed);
        }
        Ok((Yaml::Map(map), *cursor))
    }

    fn parse_inline_or_nested(
        lines: &[Line],
        cursor: &mut usize,
        value: &str,
    ) -> Result<Yaml, DomainError> {
        let value = value.trim();
        if value.is_empty() {
            if *cursor < lines.len() && lines[*cursor].indent > lines[*cursor - 1].indent {
                let child = lines[*cursor].indent;
                let (parsed, next) = Self::parse_block(lines, cursor, child)?;
                *cursor = next;
                Ok(parsed)
            } else {
                Ok(Yaml::Null)
            }
        } else if value.starts_with('[') || value.starts_with('{') {
            let (parsed, _rest) = parse_flow(value)?;
            Ok(parsed)
        } else {
            Ok(Yaml::Scalar(unquote(value)))
        }
    }

    fn child_indent(lines: &[Line], cursor: usize) -> usize {
        lines
            .get(cursor)
            .map(|line| line.indent)
            .unwrap_or_default()
    }
}

/// Split `key: value` at the first colon that is not inside quotes or brackets.
fn split_pair(text: &str) -> Option<(String, &str)> {
    let bytes = text.as_bytes();
    let mut quote: Option<u8> = None;
    let mut depth = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'"' | b'\'' => {
                if quote == Some(*byte) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(*byte);
                }
            }
            b'[' | b'{' if quote.is_none() => depth += 1,
            b']' | b'}' if quote.is_none() => depth = depth.saturating_sub(1),
            b':' if quote.is_none() && depth == 0 => {
                let next = bytes.get(index + 1).copied();
                if next == Some(b' ') || next.is_none() {
                    let key = unquote(text[..index].trim());
                    if key.is_empty() {
                        return None;
                    }
                    return Some((key, &text[index + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_comment(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut quote: Option<u8> = None;
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'"' | b'\'' => {
                if quote == Some(*byte) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(*byte);
                }
            }
            b'#' if quote.is_none()
                && (index == 0 || bytes[index - 1] == b' ' || bytes[index - 1] == b'\t') =>
            {
                return raw[..index].to_owned();
            }
            _ => {}
        }
    }
    raw.to_owned()
}

fn unquote(text: &str) -> String {
    let text = text.trim();
    if text.len() >= 2 {
        let first = text.as_bytes()[0];
        let last = text.as_bytes()[text.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return text[1..text.len() - 1]
                .replace("\\\"", "\"")
                .replace("\\n", "\n");
        }
    }
    text.to_owned()
}

fn parse_flow(text: &str) -> Result<(Yaml, &str), DomainError> {
    let text = text.trim_start();
    let bytes = text.as_bytes();
    if bytes.first() == Some(&b'[') {
        let mut rest = &text[1..];
        let mut items = Vec::new();
        loop {
            rest = rest.trim_start();
            if let Some(tail) = rest.strip_prefix(']') {
                return Ok((Yaml::Seq(items), tail));
            }
            if rest.is_empty() {
                return Err(invalid_flow());
            }
            let (value, tail) = if rest.starts_with('{') || rest.starts_with('[') {
                parse_flow(rest)?
            } else {
                parse_flow_scalar(rest)
            };
            items.push(value);
            rest = tail.trim_start();
            if rest.starts_with(',') {
                rest = &rest[1..];
            } else if !rest.starts_with(']') {
                return Err(invalid_flow());
            }
        }
    }
    if bytes.first() == Some(&b'{') {
        let mut rest = &text[1..];
        let mut map = BTreeMap::new();
        loop {
            rest = rest.trim_start();
            if let Some(tail) = rest.strip_prefix('}') {
                return Ok((Yaml::Map(map), tail));
            }
            if rest.is_empty() {
                return Err(invalid_flow());
            }
            let Some((key, tail)) = split_pair(rest) else {
                return Err(invalid_flow());
            };
            let tail = tail.trim_start();
            let (value, tail) = if tail.starts_with('{') || tail.starts_with('[') {
                parse_flow(tail)?
            } else {
                parse_flow_scalar(tail)
            };
            map.insert(key, value);
            rest = tail.trim_start();
            if rest.starts_with(',') {
                rest = &rest[1..];
            } else if !rest.starts_with('}') {
                return Err(invalid_flow());
            }
        }
    }
    Ok(parse_flow_scalar(text))
}

fn parse_flow_scalar(text: &str) -> (Yaml, &str) {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        return (Yaml::Scalar(rest[..end].to_owned()), &rest[end + 1..]);
    }
    if let Some(rest) = text.strip_prefix('\'')
        && let Some(end) = rest.find('\'')
    {
        return (Yaml::Scalar(rest[..end].to_owned()), &rest[end + 1..]);
    }
    let end = text.find([',', ']', '}']).unwrap_or(text.len());
    let value = text[..end].trim();
    (Yaml::Scalar(unquote(value)), &text[end..])
}

fn invalid_flow() -> DomainError {
    DomainError::new(ErrorCode::InvalidValue).with_detail("无法解析 YAML 的流式写法".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Protocol, Transport};

    const SAMPLE: &str = r#"
port: 7890
proxies:
  - name: "Tokyo · 01"
    type: vless
    server: node01.example.com
    port: 443
    uuid: 8a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d
    tls: true
    servername: node01.example.com
    client-fingerprint: chrome
    reality-opts:
      public-key: abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG
      short-id: "0123abcd"
    network: ws
    ws-opts:
      path: /ws
      headers:
        Host: cdn.example.com
  - name: HK SS
    type: ss
    server: hk.example.com
    port: 8443
    cipher: aes-256-gcm
    password: "p@ss:word"
    plugin: obfs
    plugin-opts:
      mode: http
      host: bing.com
  - name: Trojan
    type: trojan
    server: tj.example.com
    port: 443
    password: secret
    sni: tj.example.com
    skip-cert-verify: true
  - name: Hy2
    type: hysteria2
    server: hy.example.com
    port: 443
    password: pass
    obfs: salamander
    obfs-password: obfspass
    up: 100
    down: 200
  - name: Unsupported
    type: snell
    server: snell.example.com
    port: 443
proxy-groups:
  - name: PROXY
    type: select
    proxies: [Tokyo · 01, HK SS]
"#;

    #[test]
    fn parses_the_common_proxy_types() {
        let import = parse_clash_config(SAMPLE).expect("parse");
        assert_eq!(import.profiles.len(), 4, "skipped: {:?}", import.skipped);
        let vless = &import.profiles[0];
        assert_eq!(vless.remark, "Tokyo · 01");
        assert!(matches!(vless.protocol, Protocol::Vless { .. }));
        assert_eq!(
            vless.transport,
            Transport::Ws {
                path: "/ws".to_owned(),
                host: Some("cdn.example.com".to_owned()),
                early_data: None,
            }
        );
        let tls = vless.tls.as_ref().expect("tls");
        assert_eq!(tls.sni.as_deref(), Some("node01.example.com"));
        assert_eq!(tls.fingerprint.as_deref(), Some("chrome"));
        assert_eq!(
            tls.reality.as_ref().map(|r| r.short_id.as_str()),
            Some("0123abcd")
        );

        let ss = &import.profiles[1];
        assert!(matches!(
            &ss.protocol,
            Protocol::Shadowsocks { method, .. } if method.as_str() == "aes-256-gcm"
        ));

        let trojan = &import.profiles[2];
        assert!(trojan.tls.as_ref().is_some_and(|tls| tls.insecure));

        let hy2 = &import.profiles[3];
        assert!(matches!(
            &hy2.protocol,
            Protocol::Hysteria2 { obfs: Some(obfs), .. } if obfs.password.expose() == "obfspass"
        ));

        assert_eq!(import.skipped.len(), 1);
        assert!(import.skipped[0].contains("Unsupported"));
    }

    #[test]
    fn accepts_flow_style_proxies() {
        let text = r#"
proxies: [{name: "Flow", type: ss, server: a.example.com, port: 8388, cipher: chacha20-ietf-poly1305, password: pw}]
"#;
        let import = parse_clash_config(text).expect("parse");
        assert_eq!(import.profiles.len(), 1, "skipped: {:?}", import.skipped);
        assert_eq!(import.profiles[0].remark, "Flow");
    }

    #[test]
    fn quotes_and_comments_do_not_break_fields() {
        let text = r#"
proxies:
  # a comment
  - name: "A # not a comment"
    type: trojan
    server: a.example.com
    port: 443
    password: 'p#w'
"#;
        let import = parse_clash_config(text).expect("parse");
        assert_eq!(import.profiles[0].remark, "A # not a comment");
        assert!(matches!(
            &import.profiles[0].protocol,
            Protocol::Trojan { password } if password.expose() == "p#w"
        ));
    }

    #[test]
    fn missing_proxies_is_a_format_error() {
        let error = parse_clash_config("port: 1\nmode: rule\n").expect_err("no proxies");
        assert_eq!(error.code(), ErrorCode::UnsupportedFormat);
    }
}
