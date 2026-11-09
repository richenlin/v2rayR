//! `naive+https://` and `naive+quic://` links.
//!
//! Naive always runs over TLS (or QUIC, which implies TLS), so `TlsOptions` is
//! always populated and `Naive::quic` records the scheme variant.

use super::{Uri, missing, remark_of, tls_from};
use crate::error::DomainError;
use crate::model::{Naive, Profile, Protocol, Secret};

pub fn parse(link: &str) -> Result<Profile, DomainError> {
    let uri = Uri::split(link)?;
    let quic = uri.scheme.ends_with("+quic");

    let (username, password) = uri
        .userinfo
        .as_deref()
        .and_then(|userinfo| userinfo.split_once(':'))
        .ok_or_else(|| missing("username:password"))?;

    let mut profile = Profile::new(
        remark_of(&uri, if quic { "naive+quic" } else { "naive" }),
        uri.host.clone(),
        uri.port_or(443),
        Protocol::Naive(Naive {
            username: username.to_owned(),
            password: Secret::new(password),
            quic,
        }),
    );

    let mut tls = tls_from(&uri, true).unwrap_or_default();
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
    fn parses_https_variant() {
        let profile = parse("naive+https://user:pw@example.com:443#n").expect("parse");
        match profile.protocol {
            Protocol::Naive(naive) => {
                assert_eq!(naive.username, "user");
                assert!(!naive.quic);
            }
            other => panic!("unexpected protocol: {other:?}"),
        }
    }

    #[test]
    fn parses_quic_variant_and_default_port() {
        let profile = parse("naive+quic://user:pw@example.com").expect("parse");
        assert_eq!(profile.port, 443);
        match profile.protocol {
            Protocol::Naive(naive) => assert!(naive.quic),
            other => panic!("unexpected protocol: {other:?}"),
        }
    }

    #[test]
    fn requires_both_credentials() {
        let error = parse("naive+https://user@example.com:443").expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::MissingField);
    }
}
