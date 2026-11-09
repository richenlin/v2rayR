//! `tuic://` links: `tuic://uuid:password@host:port?congestion_control=bbr`.

use super::{Uri, invalid, missing, remark_of, tls_from};
use crate::error::DomainError;
use crate::model::{Congestion, Profile, Protocol, Secret, Tuic};

pub fn parse(link: &str) -> Result<Profile, DomainError> {
    let uri = Uri::split(link)?;

    let (uuid_raw, password) = uri
        .userinfo
        .as_deref()
        .and_then(|userinfo| userinfo.split_once(':'))
        .ok_or_else(|| missing("uuid:password"))?;

    let uuid = uuid_raw
        .parse()
        .map_err(|_| invalid("uuid", "not a uuid"))?;
    let port = uri.port.ok_or_else(|| missing("port"))?;

    let congestion = match uri.param(&["congestion_control", "congestion", "congestion-controller"])
    {
        None => None,
        Some(raw) => {
            Some(Congestion::parse(raw).ok_or_else(|| invalid("congestion_control", raw))?)
        }
    };

    let mut profile = Profile::new(
        remark_of(&uri, "tuic"),
        uri.host.clone(),
        port,
        Protocol::Tuic(Tuic {
            uuid,
            password: Secret::new(password),
            congestion,
            udp_relay_mode: uri
                .param(&["udp_relay_mode", "udp-relay-mode"])
                .map(str::to_owned),
        }),
    );

    let mut tls = tls_from(&uri, true).unwrap_or_default();
    if tls.sni.is_none() {
        tls.sni = Some(uri.host.clone());
    }
    if tls.alpn.is_empty() {
        tls.alpn = vec!["h3".to_owned()];
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
    fn parses_uuid_password_and_congestion() {
        let profile = parse(
            "tuic://11111111-1111-1111-1111-111111111111:pw@example.com:443?congestion_control=bbr&udp_relay_mode=native&alpn=h2#t",
        )
        .expect("parse");
        match profile.protocol {
            Protocol::Tuic(tuic) => {
                assert_eq!(tuic.congestion, Some(Congestion::Bbr));
                assert_eq!(tuic.udp_relay_mode.as_deref(), Some("native"));
            }
            other => panic!("unexpected protocol: {other:?}"),
        }
        assert_eq!(
            profile.tls.as_ref().expect("tls").alpn,
            vec!["h2".to_owned()]
        );
    }

    #[test]
    fn defaults_alpn_to_h3() {
        let profile =
            parse("tuic://11111111-1111-1111-1111-111111111111:pw@example.com:443").expect("parse");
        assert_eq!(
            profile.tls.as_ref().expect("tls").alpn,
            vec!["h3".to_owned()]
        );
    }

    #[test]
    fn rejects_unknown_congestion_controller() {
        let error = parse("tuic://11111111-1111-1111-1111-111111111111:pw@example.com:443?congestion_control=reno2")
            .expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::InvalidValue);
    }
}
