//! `wireguard://` links.
//!
//! `wireguard://<private-key>@host:port?publickey=<server-key>&address=10.0.0.2/32&mtu=1420`.
//! Base64 keys contain `=` padding, so the URI is split before the query is
//! touched and the userinfo is used verbatim.

use super::{Uri, missing, remark_of};
use crate::error::DomainError;
use crate::model::{Profile, Protocol, Secret, WireGuard};

pub fn parse(link: &str) -> Result<Profile, DomainError> {
    let uri = Uri::split(link)?;

    let private_key = uri
        .userinfo
        .as_deref()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| missing("private_key"))?;
    let public_key = uri
        .param(&["publickey", "public_key", "peer_public_key"])
        .ok_or_else(|| missing("publickey"))?;
    let port = uri.port.ok_or_else(|| missing("port"))?;

    let local_address = uri
        .param(&["address", "ip"])
        .map(|address| {
            address
                .split(',')
                .map(|part| part.trim().to_owned())
                .filter(|part| !part.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut profile = Profile::new(
        remark_of(&uri, "wireguard"),
        uri.host.clone(),
        port,
        Protocol::WireGuard(WireGuard {
            private_key: Secret::new(private_key),
            public_key: public_key.to_owned(),
            preshared_key: uri
                .param(&["presharedkey", "preshared_key"])
                .map(Secret::new),
            mtu: uri
                .param(&["mtu"])
                .and_then(|mtu| mtu.parse().ok())
                .filter(|mtu| *mtu >= 576)
                .unwrap_or(1420),
            local_address,
        }),
    );
    if uri.param(&["reserved"]).is_some() || uri.param(&["udp"]).is_some() {
        profile.extra = serde_json::json!({
            "reserved": uri.param(&["reserved"]),
            "udp": uri.param(&["udp"]),
        });
    }
    profile.validate()?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn key(bytes: &[u8]) -> String {
        crate::base64::encode(bytes)
    }

    #[test]
    fn parses_keys_address_and_mtu() {
        let private = key(&[1u8; 32]);
        let public = key(&[2u8; 32]);
        let profile = parse(&format!(
            "wireguard://{private}@example.com:51820?publickey={public}&address=10.0.0.2%2F32,fd00::2%2F128&mtu=1280#wg"
        ))
        .expect("parse");
        match profile.protocol {
            Protocol::WireGuard(wg) => {
                assert_eq!(wg.public_key, public);
                assert_eq!(wg.mtu, 1280);
                assert_eq!(wg.local_address.len(), 2);
                assert_eq!(wg.private_key.expose(), private);
            }
            other => panic!("unexpected protocol: {other:?}"),
        }
    }

    #[test]
    fn requires_public_key() {
        let error = parse("wireguard://key@example.com:51820").expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::MissingField);
    }

    #[test]
    fn mtu_below_minimum_falls_back() {
        let profile =
            parse("wireguard://key@example.com:51820?publickey=x&mtu=100").expect("parse");
        match profile.protocol {
            Protocol::WireGuard(wg) => assert_eq!(wg.mtu, 1420),
            other => panic!("unexpected protocol: {other:?}"),
        }
    }
}
