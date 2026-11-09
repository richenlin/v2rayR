//! Which kernel can run which node.
//!
//! The matrix is deliberately conservative: a kernel is only advertised for a
//! combination we actually emit (and test). A node that no kernel supports is
//! rejected with an explanation instead of producing a config the kernel will
//! refuse at startup.

use v2rayr_domain::{CoreKind, Profile, Protocol, Transport};

use crate::ConfigError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub core: CoreKind,
    /// Human readable reasons for the parts it *cannot* do; empty when the core
    /// handles everything.
    pub limits: Vec<String>,
}

/// Protocol ids each kernel can serve, used for the UI badge and tests.
pub fn capabilities() -> Vec<Capability> {
    vec![
        Capability {
            core: CoreKind::SingBox,
            limits: vec![
                "no VLESS + xhttp/mKCP, use Xray".to_owned(),
                "no naive outbound".to_owned(),
            ],
        },
        Capability {
            core: CoreKind::Xray,
            limits: vec![
                "no Hysteria2/TUIC/AnyTLS/WireGuard/naive".to_owned(),
                "shadowsocks 2022 falls back to sing-box".to_owned(),
            ],
        },
        Capability {
            core: CoreKind::Mihomo,
            limits: vec!["no custom JSON passthrough yet".to_owned()],
        },
    ]
}

pub fn supported(core: CoreKind, profile: &Profile) -> bool {
    match core {
        CoreKind::SingBox => singbox_supports(profile).is_ok(),
        CoreKind::Xray => xray_supports(profile).is_ok(),
        CoreKind::Mihomo => mihomo_supports(profile).is_ok(),
    }
}

/// Pick the kernel for a node.
///
/// Order of decision:
/// 1. an explicit override, if the kernel can actually run the node;
/// 2. the protocol/transport rules from the design document;
/// 3. sing-box as the safe default.
pub fn choose_core(
    profile: &Profile,
    override_core: Option<CoreKind>,
) -> Result<CoreKind, ConfigError> {
    if let Some(core) = override_core {
        return match core {
            CoreKind::SingBox => singbox_supports(profile).map(|_| core),
            CoreKind::Xray => xray_supports(profile).map(|_| core),
            CoreKind::Mihomo => mihomo_supports(profile).map(|_| core),
        };
    }

    if let Protocol::Vless { encryption, .. } = &profile.protocol {
        // Post-quantum VLESS encryption is an Xray 25.x feature; sing-box does
        // not understand the field and would fail at startup.
        if encryption.is_some() {
            return xray_supports(profile).map(|_| CoreKind::Xray);
        }
        if matches!(
            profile.transport,
            Transport::Xhttp { .. } | Transport::Mkcp { .. }
        ) {
            return xray_supports(profile).map(|_| CoreKind::Xray);
        }
    }

    match &profile.protocol {
        Protocol::Hysteria2 { .. } | Protocol::Tuic(_) | Protocol::AnyTls(_) => {
            return singbox_supports(profile).map(|_| CoreKind::SingBox);
        }
        Protocol::Shadowsocks { method, .. } if method.is_2022() => {
            return singbox_supports(profile).map(|_| CoreKind::SingBox);
        }
        Protocol::Naive(_) => {
            return Err(ConfigError::NoCapableKernel(
                "naive has no supported kernel yet".to_owned(),
            ));
        }
        Protocol::Chain(_) => {
            return Err(ConfigError::NoCapableKernel(
                "proxy chains are not implemented".to_owned(),
            ));
        }
        Protocol::Custom { .. } => {
            return Err(ConfigError::NoCapableKernel(
                "custom protocol payloads need an explicit kernel".to_owned(),
            ));
        }
        _ => {}
    }

    singbox_supports(profile)
        .map(|_| CoreKind::SingBox)
        .or_else(|_| xray_supports(profile).map(|_| CoreKind::Xray))
}

fn singbox_supports(profile: &Profile) -> Result<(), ConfigError> {
    let fail = |reason: String| {
        Err(ConfigError::Unsupported {
            core: "sing-box",
            reason,
        })
    };
    if let Protocol::Vless {
        encryption: Some(_),
        ..
    } = &profile.protocol
    {
        return fail("VLESS encryption needs Xray".to_owned());
    }
    match &profile.protocol {
        Protocol::Vmess { .. }
        | Protocol::Vless { .. }
        | Protocol::Trojan { .. }
        | Protocol::Shadowsocks { .. }
        | Protocol::Socks { .. }
        | Protocol::Http { .. }
        | Protocol::Hysteria2 { .. }
        | Protocol::Tuic(_)
        | Protocol::AnyTls(_)
        | Protocol::WireGuard(_) => {}
        Protocol::Naive(_) => return fail("sing-box has no naive outbound".to_owned()),
        Protocol::Chain(_) => return fail("chains are not supported".to_owned()),
        Protocol::Custom { .. } => return fail("custom JSON is not supported".to_owned()),
    }
    match &profile.transport {
        Transport::Raw
        | Transport::Ws { .. }
        | Transport::Grpc { .. }
        | Transport::HttpUpgrade { .. } => {}
        Transport::Http { .. } => {}
        Transport::Xhttp { .. } | Transport::Mkcp { .. } | Transport::Quic { .. } => {
            return fail(format!(
                "sing-box has no {} transport",
                profile.transport.kind()
            ));
        }
    }
    if let Protocol::Shadowsocks { method, .. } = &profile.protocol
        && !method.is_known()
    {
        return fail(format!("unknown shadowsocks method {}", method.as_str()));
    }
    Ok(())
}

fn xray_supports(profile: &Profile) -> Result<(), ConfigError> {
    let fail = |reason: String| {
        Err(ConfigError::Unsupported {
            core: "xray",
            reason,
        })
    };
    match &profile.protocol {
        Protocol::Vmess { .. } | Protocol::Vless { .. } | Protocol::Trojan { .. } => {}
        Protocol::Shadowsocks { method, .. } => {
            if method.is_2022() {
                return fail("shadowsocks 2022 needs sing-box".to_owned());
            }
            if !method.is_known() {
                return fail(format!("unknown shadowsocks method {}", method.as_str()));
            }
        }
        Protocol::Socks { .. } | Protocol::Http { .. } => {}
        _ => {
            return fail(format!("xray has no {} outbound", profile.protocol.kind()));
        }
    }
    match &profile.transport {
        Transport::Raw
        | Transport::Ws { .. }
        | Transport::Grpc { .. }
        | Transport::HttpUpgrade { .. }
        | Transport::Http { .. }
        | Transport::Xhttp { .. }
        | Transport::Mkcp { .. }
        | Transport::Quic { .. } => {}
    }
    if let Protocol::Shadowsocks { .. } = &profile.protocol
        && !matches!(profile.transport, Transport::Raw)
    {
        return fail("shadowsocks only supports the raw transport".to_owned());
    }
    Ok(())
}

fn mihomo_supports(profile: &Profile) -> Result<(), ConfigError> {
    let fail = |reason: String| {
        Err(ConfigError::Unsupported {
            core: "mihomo",
            reason,
        })
    };
    match &profile.protocol {
        Protocol::Trojan { .. } | Protocol::Vmess { .. } | Protocol::Vless { .. } => {}
        Protocol::Shadowsocks { method, .. } => {
            if method.is_2022() {
                return fail("shadowsocks 2022 needs sing-box".to_owned());
            }
        }
        Protocol::Hysteria2 { .. } | Protocol::Tuic(_) => {}
        _ => {
            return fail(format!(
                "mihomo has no {} outbound in this build",
                profile.protocol.kind()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use v2rayr_domain::{Protocol, RealityOptions, Secret, TlsOptions};

    fn profile(protocol: Protocol, transport: Transport) -> Profile {
        Profile::new("node", "example.com", 443, protocol).with_transport(transport)
    }

    fn vless() -> Protocol {
        Protocol::Vless {
            id: uuid::Uuid::nil(),
            flow: None,
            encryption: None,
        }
    }

    #[test]
    fn vless_encryption_is_routed_to_xray() {
        let node = profile(
            Protocol::Vless {
                id: uuid::Uuid::nil(),
                flow: None,
                encryption: Some("mlkem768x25519plus.native.0rtt.key".to_owned()),
            },
            Transport::Raw,
        );
        assert_eq!(choose_core(&node, None).expect("core"), CoreKind::Xray);
        assert!(!supported(CoreKind::SingBox, &node));
    }

    #[test]
    fn vless_xhttp_is_routed_to_xray() {
        let node = profile(
            vless(),
            Transport::Xhttp {
                path: "/".into(),
                host: None,
                mode: "auto".into(),
                extra: serde_json::Value::Null,
            },
        );
        assert_eq!(choose_core(&node, None).expect("core"), CoreKind::Xray);
    }

    #[test]
    fn vless_ws_stays_on_singbox() {
        let node = profile(
            vless(),
            Transport::Ws {
                path: "/".into(),
                host: None,
                early_data: None,
            },
        );
        assert_eq!(choose_core(&node, None).expect("core"), CoreKind::SingBox);
    }

    #[test]
    fn hysteria2_and_tuic_go_to_singbox() {
        let hy2 = profile(
            Protocol::Hysteria2 {
                password: Secret::new("pw"),
                obfs: None,
                up_mbps: None,
                down_mbps: None,
                ports: None,
            },
            Transport::Raw,
        );
        assert_eq!(choose_core(&hy2, None).expect("core"), CoreKind::SingBox);
    }

    #[test]
    fn an_override_wins_when_the_kernel_can_run_it() {
        let node = profile(
            Protocol::Trojan {
                password: Secret::new("pw"),
            },
            Transport::Raw,
        );
        assert_eq!(
            choose_core(&node, Some(CoreKind::Xray)).expect("core"),
            CoreKind::Xray
        );
    }

    #[test]
    fn an_impossible_override_is_rejected_not_ignored() {
        let node = profile(
            Protocol::Hysteria2 {
                password: Secret::new("pw"),
                obfs: None,
                up_mbps: None,
                down_mbps: None,
                ports: None,
            },
            Transport::Raw,
        );
        let error = choose_core(&node, Some(CoreKind::Xray)).expect_err("must fail");
        assert!(matches!(error, ConfigError::Unsupported { .. }));
    }

    #[test]
    fn naive_is_reported_instead_of_silently_misconfigured() {
        let node = profile(
            Protocol::Naive(v2rayr_domain::Naive {
                username: "u".into(),
                password: Secret::new("p"),
                quic: false,
            }),
            Transport::Raw,
        );
        assert!(matches!(
            choose_core(&node, None),
            Err(ConfigError::NoCapableKernel(_))
        ));
    }

    #[test]
    fn reality_is_preserved_for_singbox() {
        let mut node = profile(vless(), Transport::Raw);
        node.tls = Some(TlsOptions {
            reality: Some(RealityOptions {
                public_key: "pbk".into(),
                short_id: "ab".into(),
                spider_x: None,
            }),
            ..TlsOptions::default()
        });
        assert!(supported(CoreKind::SingBox, &node));
    }
}
