//! `socks://` and `socks5://` links.
//!
//! v2rayN accepts both `socks://base64(user:pass)@host:port` and the legacy
//! `socks://base64(user:pass@host:port)` form, and both are common in exported
//! node lists, so both are recognised here.

use super::uri::{host_port_required, pct_decode};
use crate::error::{DomainError, ErrorCode};
use crate::model::{Profile, Protocol, Secret};

pub fn parse(link: &str) -> Result<Profile, DomainError> {
    let (scheme, rest) = link
        .split_once("://")
        .ok_or_else(|| DomainError::new(ErrorCode::InvalidUrl).with_detail("missing scheme"))?;
    let scheme = scheme.to_ascii_lowercase();

    let (rest, fragment) = match rest.split_once('#') {
        Some((head, fragment)) => (head, Some(fragment)),
        None => (rest, None),
    };

    // Either `userinfo@host:port`, the legacy whole-payload form, or no auth.
    let (encoded_userinfo, hostport) = match rest.rsplit_once('@') {
        Some((userinfo, hostport)) => (Some(pct_decode(userinfo)), pct_decode(hostport)),
        None => match crate::base64::decode_str(rest).filter(|decoded| decoded.contains('@')) {
            Some(decoded) => match decoded.rsplit_once('@') {
                Some((userinfo, hostport)) => (Some(userinfo.to_owned()), hostport.to_owned()),
                None => (None, pct_decode(rest)),
            },
            None => (None, pct_decode(rest)),
        },
    };

    let (host, port) = host_port_required(&hostport)?;

    let (username, password) = match encoded_userinfo {
        Some(userinfo) => {
            let decoded = if userinfo.contains(':') {
                userinfo
            } else {
                crate::base64::decode_str(&userinfo).unwrap_or(userinfo)
            };
            match decoded.split_once(':') {
                Some((user, pass)) => (
                    Some(user.to_owned()).filter(|user| !user.is_empty()),
                    Some(Secret::new(pass)),
                ),
                None => (Some(decoded).filter(|user| !user.is_empty()), None),
            }
        }
        None => (None, None),
    };

    let remark = fragment
        .map(|fragment| pct_decode(fragment).trim().to_owned())
        .filter(|remark| !remark.is_empty())
        .unwrap_or_else(|| crate::model::derive_remark(&scheme, &host, port));

    let profile = Profile::new(remark, host, port, Protocol::Socks { username, password });
    profile.validate()?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_base64_credentials_and_their_port() {
        let userinfo = crate::base64::encode(b"user:pa:ss");
        let profile = parse(&format!("socks://{userinfo}@example.com:1080#s")).expect("parse");
        assert_eq!(profile.port, 1080);
        match profile.protocol {
            Protocol::Socks { username, password } => {
                assert_eq!(username.as_deref(), Some("user"));
                assert_eq!(password.expect("password").expose(), "pa:ss");
            }
            other => panic!("unexpected protocol: {other:?}"),
        }
    }

    #[test]
    fn parses_plain_credentials() {
        let profile = parse("socks5://user:pass@example.com:1080").expect("parse");
        assert_eq!(profile.remark, "socks5 example.com:1080");
    }

    #[test]
    fn parses_legacy_payload_without_at_sign() {
        let payload = crate::base64::encode(b"user:pass@example.com:1080");
        let profile = parse(&format!("socks://{payload}")).expect("parse");
        assert_eq!(profile.address, "example.com");
        assert_eq!(profile.port, 1080);
    }

    #[test]
    fn parses_without_auth() {
        let profile = parse("socks://example.com:1080#no-auth").expect("parse");
        match profile.protocol {
            Protocol::Socks { username, .. } => assert!(username.is_none()),
            other => panic!("unexpected protocol: {other:?}"),
        }
    }
}
