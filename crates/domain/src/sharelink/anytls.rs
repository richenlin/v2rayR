//! `anytls://` links: `anytls://password@host:port?sni=...&insecure=1`.

use super::{Uri, missing, remark_of, tls_from};
use crate::error::DomainError;
use crate::model::{AnyTls, Profile, Protocol, Secret};

pub fn parse(link: &str) -> Result<Profile, DomainError> {
    let uri = Uri::split(link)?;

    let password = uri
        .userinfo
        .as_deref()
        .filter(|password| !password.is_empty())
        .ok_or_else(|| missing("password"))?;

    let mut profile = Profile::new(
        remark_of(&uri, "anytls"),
        uri.host.clone(),
        uri.port.ok_or_else(|| missing("port"))?,
        Protocol::AnyTls(AnyTls {
            password: Secret::new(password),
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

    #[test]
    fn parses_password_and_tls() {
        let profile = parse("anytls://pw@example.com:443?sni=cdn.example.com&allowInsecure=1#a")
            .expect("parse");
        assert_eq!(profile.remark, "a");
        let tls = profile.tls.expect("tls");
        assert_eq!(tls.sni.as_deref(), Some("cdn.example.com"));
        assert!(tls.insecure);
    }
}
