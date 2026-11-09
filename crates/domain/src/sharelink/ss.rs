//! `ss://` links, in all three shapes found in the wild.
//!
//! * SIP002: `ss://base64(method:password)@host:port?plugin=...`
//! * SIP002 plain: `ss://method:password@host:port`
//! * legacy: `ss://base64(method:password@host:port)`
//!
//! The legacy form is why this parser cannot use the generic [`Uri`] splitter:
//! its base64 payload may contain `/`, which would be mistaken for a path.

use super::uri::{host_port_required, pct_decode};
use super::{Uri, invalid, missing, transport_from};
use crate::error::{DomainError, ErrorCode};
use crate::model::{Profile, Protocol, Secret, SsMethod, Transport};

pub fn parse(link: &str) -> Result<Profile, DomainError> {
    let rest = link
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(|| DomainError::new(ErrorCode::InvalidUrl).with_detail("missing scheme"))?;

    let (rest, fragment) = split_once(rest, '#');
    let (authority, query) = split_once(rest, '?');

    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((userinfo, hostport)) => (pct_decode(userinfo), pct_decode(hostport)),
        None => {
            // Legacy: the whole authority is base64(method:password@host:port).
            let decoded = crate::base64::decode_str(authority)
                .ok_or_else(|| DomainError::new(ErrorCode::InvalidBase64).with_detail("ss"))?;
            match decoded.rsplit_once('@') {
                Some((userinfo, hostport)) => (userinfo.to_owned(), hostport.to_owned()),
                None => {
                    return Err(
                        DomainError::new(ErrorCode::InvalidEntry).with_detail("ss authority")
                    );
                }
            }
        }
    };

    let (method, password) = parse_method_password(&userinfo)?;
    let (host, port) = host_port_required(&hostport)?;

    let mut profile = Profile::new(
        fragment
            .map(|fragment| pct_decode(fragment).trim().to_owned())
            .filter(|remark| !remark.is_empty())
            .unwrap_or_else(|| crate::model::derive_remark("ss", &host, port)),
        host,
        port,
        Protocol::Shadowsocks { method, password },
    );

    // `plugin=obfs-local;obfs=http;obfs-host=...` is percent-encoded as a whole.
    if let Some(plugin) = query.and_then(|query| param(query, "plugin")) {
        let plugin = pct_decode(&plugin);
        let (name, opts) = plugin.split_once(';').unwrap_or((plugin.as_str(), ""));
        if !name.is_empty() {
            profile.transport = Transport::Raw;
            profile.extra = serde_json::json!({
                "plugin": name,
                "plugin_opts": opts,
            });
        }
    }
    if let Some(net) = query.and_then(|query| param(query, "type")) {
        profile.transport = transport_from(&Uri::split(link)?, &net, "")?;
    }

    profile.validate()?;
    Ok(profile)
}

fn parse_method_password(userinfo: &str) -> Result<(SsMethod, Secret), DomainError> {
    let decoded = if userinfo.contains(':') {
        userinfo.to_owned()
    } else {
        crate::base64::decode_str(userinfo)
            .ok_or_else(|| DomainError::new(ErrorCode::InvalidBase64).with_detail("ss method"))?
    };
    let (method, password) = decoded.split_once(':').ok_or_else(|| {
        DomainError::new(ErrorCode::InvalidEntry).with_detail("ss method:password")
    })?;
    if method.is_empty() {
        return Err(missing("method"));
    }
    let method = SsMethod::new(method);
    if !method.is_known() {
        return Err(invalid("method", method.as_str()));
    }
    Ok((method, Secret::new(password)))
}

fn split_once(value: &str, separator: char) -> (&str, Option<&str>) {
    match value.split_once(separator) {
        Some((head, tail)) => (head, Some(tail)),
        None => (value, None),
    }
}

fn param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|pair| {
            pair.split_once('=')
                .filter(|(name, _)| name.eq_ignore_ascii_case(key))
        })
        .map(|(_, value)| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sip002_base64_userinfo() {
        let userinfo = crate::base64::encode(b"aes-256-gcm:secret");
        let profile = parse(&format!("ss://{userinfo}@example.com:8388#HK")).expect("parse");
        assert_eq!(profile.remark, "HK");
        assert_eq!(profile.port, 8388);
        match profile.protocol {
            Protocol::Shadowsocks { method, password } => {
                assert_eq!(method.as_str(), "aes-256-gcm");
                assert_eq!(password.expose(), "secret");
            }
            other => panic!("unexpected protocol: {other:?}"),
        }
    }

    #[test]
    fn parses_plain_sip002() {
        let profile =
            parse("ss://2022-blake3-aes-128-gcm:a2V5@example.com:8388#v6").expect("parse");
        match profile.protocol {
            Protocol::Shadowsocks { method, .. } => assert!(method.is_2022()),
            other => panic!("unexpected protocol: {other:?}"),
        }
    }

    #[test]
    fn parses_legacy_base64_payload_with_slash() {
        let payload = crate::base64::encode(b"chacha20-ietf-poly1305:pw@example.com:8388");
        let profile = parse(&format!("ss://{payload}")).expect("parse");
        assert_eq!(profile.address, "example.com");
        assert_eq!(profile.port, 8388);
    }

    #[test]
    fn keeps_obfs_plugin_for_the_emitter() {
        let userinfo = crate::base64::encode(b"aes-128-gcm:pw");
        let plugin =
            super::super::uri::pct_encode("obfs-local;obfs=http;obfs-host=cdn.example.com");
        let profile = parse(&format!(
            "ss://{userinfo}@example.com:8388?plugin={plugin}#p"
        ))
        .expect("parse");
        assert_eq!(profile.extra["plugin"], "obfs-local");
        assert_eq!(
            profile.extra["plugin_opts"],
            "obfs=http;obfs-host=cdn.example.com"
        );
    }

    #[test]
    fn rejects_unknown_method() {
        let userinfo = crate::base64::encode(b"rc5:secret");
        let error = parse(&format!("ss://{userinfo}@example.com:8388")).expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::InvalidValue);
    }

    #[test]
    fn rejects_missing_port() {
        let userinfo = crate::base64::encode(b"aes-256-gcm:secret");
        let error = parse(&format!("ss://{userinfo}@example.com")).expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::MissingField);
    }
}
