//! A deliberately lenient URI reader for share links.
//!
//! `url::Url` follows the WHATWG URL spec, which rejects a fair number of links
//! that v2rayN-family clients happily accept (empty hosts, underscores, missing
//! ports, credentials containing `@` after decoding). The parsers here need a
//! predictable split into scheme / userinfo / host / port / path / query /
//! fragment, so the split is done by hand.

use percent_encoding::percent_decode_str;

use crate::error::{DomainError, ErrorCode};

/// Percent-decodes lossily; share link fields are UTF-8 in practice.
pub fn pct_decode(input: &str) -> String {
    percent_decode_str(input).decode_utf8_lossy().into_owned()
}

/// Percent-encoding for regenerated links, using the RFC 3986 unreserved set so
/// hostnames and dotted values stay readable.
pub const UNRESERVED: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub fn pct_encode(input: &str) -> String {
    percent_encoding::utf8_percent_encode(input, UNRESERVED).to_string()
}

#[derive(Debug, Clone, Default)]
pub struct Uri {
    pub scheme: String,
    /// Decoded userinfo: everything before the last `@`.
    pub userinfo: Option<String>,
    /// Decoded host, without IPv6 brackets.
    pub host: String,
    pub port: Option<u16>,
    /// Decoded path, always starting with `/` when present.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub fragment: Option<String>,
}

impl Uri {
    pub fn split(input: &str) -> Result<Self, DomainError> {
        let input = input.trim();
        let (scheme, rest) = input
            .split_once("://")
            .ok_or_else(|| DomainError::new(ErrorCode::InvalidUrl).with_detail("missing scheme"))?;

        if scheme.is_empty()
            || !scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            return Err(DomainError::new(ErrorCode::InvalidUrl).with_detail("bad scheme"));
        }

        let (rest, fragment) = match rest.split_once('#') {
            Some((head, fragment)) => (head, Some(pct_decode(fragment))),
            None => (rest, None),
        };
        let (rest, raw_query) = match rest.split_once('?') {
            Some((head, query)) => (head, Some(query)),
            None => (rest, None),
        };
        let (authority, path) = match rest.find('/') {
            Some(index) => (&rest[..index], pct_decode(&rest[index..])),
            None => (rest, String::new()),
        };

        let (userinfo, hostport) = match authority.rfind('@') {
            Some(index) => (
                Some(pct_decode(&authority[..index])),
                &authority[index + 1..],
            ),
            None => (None, authority),
        };

        let (host, port) = split_host_port(hostport)?;

        let query = raw_query.map(parse_query).unwrap_or_default();

        Ok(Self {
            scheme: scheme.to_ascii_lowercase(),
            userinfo,
            host,
            port,
            path,
            query,
            fragment,
        })
    }

    /// First present parameter among `names`, case-insensitive keys.
    pub fn param(&self, names: &[&str]) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| names.iter().any(|name| key.eq_ignore_ascii_case(name)))
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    }

    pub fn flag(&self, names: &[&str]) -> bool {
        match self.param(names) {
            Some(value) => matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            None => self
                .query
                .iter()
                .any(|(key, _)| names.iter().any(|name| key.eq_ignore_ascii_case(name))),
        }
    }

    pub fn remark(&self) -> Option<String> {
        self.fragment
            .as_ref()
            .map(|fragment| fragment.trim().to_owned())
            .filter(|fragment| !fragment.is_empty())
    }

    pub fn port_or(&self, fallback: u16) -> u16 {
        self.port.unwrap_or(fallback)
    }

    /// User/password pair from `user:pass@host`. `None` for the password when
    /// the userinfo has no colon.
    pub fn user_pass(&self) -> (Option<String>, Option<String>) {
        match &self.userinfo {
            Some(userinfo) => match userinfo.split_once(':') {
                Some((user, pass)) => (Some(user.to_owned()), Some(pass.to_owned())),
                None => (Some(userinfo.clone()), None),
            },
            None => (None, None),
        }
    }
}

pub(crate) fn split_host_port(authority: &str) -> Result<(String, Option<u16>), DomainError> {
    if authority.is_empty() {
        return Err(DomainError::new(ErrorCode::InvalidUrl).with_detail("missing host"));
    }

    // IPv6 literal: `[::1]:443`
    if let Some(stripped) = authority.strip_prefix('[') {
        let (host, tail) = stripped.split_once(']').ok_or_else(|| {
            DomainError::new(ErrorCode::InvalidUrl).with_detail("unterminated ipv6")
        })?;
        let port = match tail.strip_prefix(':') {
            Some(port) if !port.is_empty() => Some(parse_port(port)?),
            _ => None,
        };
        return Ok((host.to_ascii_lowercase(), port));
    }

    // A bare IPv6 address without brackets has several colons and no port.
    if authority.matches(':').count() > 1 {
        return Ok((authority.to_ascii_lowercase(), None));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() => Ok((
            pct_decode(host).trim_end_matches('.').to_ascii_lowercase(),
            Some(parse_port(port)?),
        )),
        Some((host, "")) => Ok((
            pct_decode(host).trim_end_matches('.').to_ascii_lowercase(),
            None,
        )),
        _ => Ok((
            pct_decode(authority)
                .trim_end_matches('.')
                .to_ascii_lowercase(),
            None,
        )),
    }
}

/// Like [`split_host_port`] but a port is mandatory (Shadowsocks, Socks).
pub(crate) fn host_port_required(authority: &str) -> Result<(String, u16), DomainError> {
    let (host, port) = split_host_port(authority)?;
    match port {
        Some(port) => Ok((host, port)),
        None => Err(DomainError::new(ErrorCode::MissingField).with_detail("port")),
    }
}

fn parse_port(raw: &str) -> Result<u16, DomainError> {
    raw.parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| {
            DomainError::new(ErrorCode::InvalidPort)
                .with_detail(raw.chars().take(8).collect::<String>())
        })
}

fn parse_query(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (pct_decode(key), pct_decode(value)),
            None => (pct_decode(pair), String::new()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_typical_link() {
        let uri = Uri::split("vless://uuid@Example.COM:8443/a/b?type=ws&path=%2Fws#Tokyo%20%231")
            .expect("parse");
        assert_eq!(uri.scheme, "vless");
        assert_eq!(uri.userinfo.as_deref(), Some("uuid"));
        assert_eq!(uri.host, "example.com");
        assert_eq!(uri.port, Some(8443));
        assert_eq!(uri.path, "/a/b");
        assert_eq!(uri.param(&["TYPE"]), Some("ws"));
        assert_eq!(uri.param(&["path"]), Some("/ws"));
        assert_eq!(uri.remark().as_deref(), Some("Tokyo #1"));
    }

    #[test]
    fn keeps_colons_inside_userinfo() {
        let uri = Uri::split("hysteria2://user:pa:ss@host:443/?sni=host").expect("parse");
        assert_eq!(uri.userinfo.as_deref(), Some("user:pa:ss"));
        assert_eq!(uri.host, "host");
        assert_eq!(uri.param(&["sni"]), Some("host"));
    }

    #[test]
    fn parses_ipv6_literal() {
        let uri = Uri::split("trojan://pw@[2001:db8::1]:443#v6").expect("parse");
        assert_eq!(uri.host, "2001:db8::1");
        assert_eq!(uri.port, Some(443));
    }

    #[test]
    fn treats_bare_ipv6_as_host() {
        let uri = Uri::split("trojan://pw@2001:db8::1").expect("parse");
        assert_eq!(uri.host, "2001:db8::1");
        assert_eq!(uri.port, None);
    }

    #[test]
    fn allows_missing_port() {
        let uri = Uri::split("hysteria2://auth@host").expect("parse");
        assert_eq!(uri.host, "host");
        assert_eq!(uri.port, None);
        assert_eq!(uri.port_or(443), 443);
    }

    #[test]
    fn rejects_non_numeric_port() {
        let error = Uri::split("trojan://pw@host:abc").expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::InvalidPort);
    }

    #[test]
    fn decodes_percent_encoded_host() {
        let uri = Uri::split("trojan://pw@%E4%BE%8B.example:443").expect("parse");
        assert_eq!(uri.host, "例.example");
    }
}
