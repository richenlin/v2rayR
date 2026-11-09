//! `hysteria2://` / `hy2://` links.
//!
//! The userinfo is the authentication string and may legitimately contain `:`;
//! it is therefore taken verbatim instead of being split into user/password.

use super::{Uri, invalid, missing, remark_of, tls_from};
use crate::error::DomainError;
use crate::model::{Hysteria2Obfs, ObfsKind, Profile, Protocol, Secret};

pub fn parse(link: &str) -> Result<Profile, DomainError> {
    let uri = Uri::split(link)?;

    let password = uri
        .userinfo
        .as_deref()
        .filter(|auth| !auth.is_empty())
        .ok_or_else(|| missing("auth"))?;

    let obfs = match uri.param(&["obfs"]) {
        None => None,
        Some(raw) => {
            let kind = ObfsKind::parse(raw).ok_or_else(|| invalid("obfs", raw))?;
            let password = uri
                .param(&["obfs-password", "obfs_password", "obfsParam"])
                .ok_or_else(|| missing("obfs-password"))?;
            Some(Hysteria2Obfs {
                kind,
                password: Secret::new(password),
            })
        }
    };

    let mut profile = Profile::new(
        remark_of(&uri, "hysteria2"),
        uri.host.clone(),
        uri.port_or(443),
        Protocol::Hysteria2 {
            password: Secret::new(password),
            obfs,
            up_mbps: uri
                .param(&["upmbps", "up"])
                .and_then(|value| value.parse().ok()),
            down_mbps: uri
                .param(&["downmbps", "down"])
                .and_then(|value| value.parse().ok()),
            ports: uri.param(&["mport", "ports"]).map(str::to_owned),
        },
    );

    let mut tls = tls_from(&uri, true).unwrap_or_default();
    // `pinSHA256` implies certificate pinning; sing-box only exposes
    // `insecure` and the certificate fields, so a pinned link is marked
    // insecure and the pin is preserved for a later implementation.
    if uri.param(&["pinSHA256", "pin"]).is_some() {
        tls.insecure = true;
        profile.extra =
            serde_json::json!({ "pinned_cert_sha256": uri.param(&["pinSHA256", "pin"]) });
    }
    if tls.sni.is_none() {
        tls.sni = Some(uri.host.clone());
    }
    profile.tls = Some(tls);
    profile.validate()?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn parses_password_obfs_and_port_hopping() {
        let profile = parse("hysteria2://auth:pw@example.com:8443?obfs=salamander&obfs-password=abc&mport=443,8443-8445&insecure=1#hy")
            .expect("parse");
        assert_eq!(profile.port, 8443);
        assert!(profile.tls.as_ref().expect("tls").insecure);
        match profile.protocol {
            Protocol::Hysteria2 { obfs, ports, .. } => {
                assert_eq!(obfs.expect("obfs").kind, ObfsKind::Salamander);
                assert_eq!(ports.as_deref(), Some("443,8443-8445"));
            }
            other => panic!("unexpected protocol: {other:?}"),
        }
    }

    #[test]
    fn defaults_to_443() {
        let profile = parse("hy2://pw@example.com").expect("parse");
        assert_eq!(profile.port, 443);
        assert_eq!(profile.transport.kind(), "raw");
    }

    #[test]
    fn obfs_without_password_is_rejected() {
        let error = parse("hy2://pw@example.com:443?obfs=salamander").expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::MissingField);
    }

    #[test]
    fn requires_authentication() {
        let error = parse("hy2://example.com:443").expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::MissingField);
    }
}
