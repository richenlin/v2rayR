//! `vless://` links (including REALITY and XTLS Vision).

use uuid::Uuid;

use super::{Uri, invalid, missing, remark_of, tls_from, transport_from};
use crate::error::DomainError;
use crate::model::{Profile, Protocol, VlessFlow};

pub fn parse(link: &str) -> Result<Profile, DomainError> {
    let uri = Uri::split(link)?;

    let raw_id = uri.userinfo.as_deref().ok_or_else(|| missing("id"))?;
    let id = raw_id
        .parse::<Uuid>()
        .map_err(|_| invalid("id", "not a uuid"))?;

    // `encryption=none` is the only legal value; a future VLESS encryption
    // extension would have to be added deliberately.
    let encryption = uri.param(&["encryption"]).map(str::to_owned);
    if let Some(value) = encryption.as_deref() {
        if !value.eq_ignore_ascii_case("none") {
            return Err(invalid("encryption", value));
        }
    }

    let flow = match uri.param(&["flow"]) {
        None => None,
        Some(raw) => Some(VlessFlow::parse(raw).ok_or_else(|| invalid("flow", raw))?),
    };

    let net = uri.param(&["type", "net"]).unwrap_or_default();
    let header_type = uri.param(&["headerType"]).unwrap_or_default();
    let transport = transport_from(&uri, net, header_type)?;
    let tls = tls_from(&uri, false);

    let mut profile = Profile::new(
        remark_of(&uri, "vless"),
        uri.host.clone(),
        uri.port.ok_or_else(|| missing("port"))?,
        Protocol::Vless {
            id,
            flow,
            encryption,
        },
    );
    profile.transport = transport;
    profile.tls = tls;
    profile.validate()?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use crate::model::Transport;

    #[test]
    fn parses_plain_vless() {
        let profile =
            parse("vless://11111111-1111-1111-1111-111111111111@a.com:443#Node").expect("parse");
        assert_eq!(profile.address, "a.com");
        assert_eq!(profile.port, 443);
        assert!(profile.tls.is_none());
        assert_eq!(profile.transport, Transport::Raw);
    }

    #[test]
    fn rejects_unsupported_encryption() {
        let error =
            parse("vless://11111111-1111-1111-1111-111111111111@a.com:443?encryption=aes#x")
                .expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::InvalidValue);
    }

    #[test]
    fn xhttp_transport_is_kept_for_core_selection() {
        let profile = parse(
            "vless://11111111-1111-1111-1111-111111111111@a.com:443?type=xhttp&path=%2Fx&mode=stream-up&security=tls#x",
        )
        .expect("parse");
        assert_eq!(profile.transport.kind(), "xhttp");
        assert!(profile.transport.host_header().is_none());
    }

    #[test]
    fn missing_port_is_rejected_instead_of_defaulting() {
        let error =
            parse("vless://11111111-1111-1111-1111-111111111111@a.com#x").expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::MissingField);
    }

    #[test]
    fn grpc_service_name_falls_back_to_path() {
        let profile = parse(
            "vless://11111111-1111-1111-1111-111111111111@a.com:443?type=grpc&path=%2Fsvc&mode=multi#g",
        )
        .expect("parse");
        match profile.transport {
            Transport::Grpc {
                service_name,
                multi_mode,
            } => {
                assert_eq!(service_name, "svc");
                assert!(multi_mode);
            }
            other => panic!("unexpected transport: {other:?}"),
        }
    }
}
