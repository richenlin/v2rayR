//! `trojan://` links.

use super::{Uri, missing, remark_of, tls_from, transport_from};
use crate::error::DomainError;
use crate::model::{Profile, Protocol, Secret};

pub fn parse(link: &str) -> Result<Profile, DomainError> {
    let uri = Uri::split(link)?;

    let password = uri
        .userinfo
        .as_deref()
        .filter(|password| !password.is_empty())
        .ok_or_else(|| missing("password"))?;

    let net = uri.param(&["type", "net"]).unwrap_or_default();
    let header_type = uri.param(&["headerType"]).unwrap_or_default();
    let transport = transport_from(&uri, net, header_type)?;
    // Trojan is TLS by definition; `security=none` is honoured as an escape
    // hatch for non-standard servers behind a TLS terminator.
    let tls = tls_from(&uri, true);

    let mut profile = Profile::new(
        remark_of(&uri, "trojan"),
        uri.host.clone(),
        uri.port.ok_or_else(|| missing("port"))?,
        Protocol::Trojan {
            password: Secret::new(password),
        },
    );
    profile.transport = transport;
    profile.tls = tls;
    if let Some(tls) = profile.tls.as_mut() {
        if tls.sni.is_none() {
            tls.sni = Some(uri.host.clone());
        }
    }
    profile.validate()?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn parses_with_default_tls_and_sni() {
        let profile = parse("trojan://p%40ss@example.com:8443#Tokyo").expect("parse");
        assert_eq!(profile.remark, "Tokyo");
        assert_eq!(
            profile.tls.as_ref().and_then(|tls| tls.sni.as_deref()),
            Some("example.com")
        );
        match &profile.protocol {
            Protocol::Trojan { password } => assert_eq!(password.expose(), "p@ss"),
            other => panic!("unexpected protocol: {other:?}"),
        }
    }

    #[test]
    fn respects_security_none() {
        let profile = parse("trojan://pw@example.com:443?security=none#x").expect("parse");
        assert!(profile.tls.is_none());
    }

    #[test]
    fn requires_password() {
        let error = parse("trojan://example.com:443").expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::MissingField);
    }

    #[test]
    fn insecure_flag_is_read_from_allow_insecure() {
        let profile = parse("trojan://pw@example.com:443?allowInsecure=1#x").expect("parse");
        assert!(profile.tls.expect("tls").insecure);
    }
}
