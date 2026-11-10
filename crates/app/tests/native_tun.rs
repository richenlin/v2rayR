#![cfg(any(target_os = "linux", target_os = "macos", windows))]

use std::net::Ipv4Addr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use v2rayr_app::{App, AppOptions, ConnectionRequest, ImportRequest};
use v2rayr_engine::CoreBinaries;
use v2rayr_ipc::{ConnectionPhase, RoutingPolicy, TakeoverMode};
use v2rayr_platform::{AppPaths, tun};

#[cfg(target_os = "macos")]
fn mac_tun_dns_advertised() -> bool {
    let output = std::process::Command::new("/usr/sbin/scutil")
        .arg("--dns")
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).contains(tun::MAC_DNS_ADDRESS)
}

#[tokio::test]
#[ignore = "requires the provisioned TUN helper and a disposable native test environment"]
async fn app_tun_captures_traffic_and_releases_the_network() {
    assert_eq!(
        std::env::var("V2RAYR_TUN_ISOLATED_TEST").as_deref(),
        Ok("1")
    );
    let address: Ipv4Addr = std::env::var("V2RAYR_TEST_TUN_ADDRESS")
        .expect("isolated fixture IP")
        .parse()
        .unwrap();
    assert!(tun::provisioned());
    assert!(tun::network_cleared());
    let listener = tokio::net::TcpListener::bind((address, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let fixture = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut header = [0; 2];
                socket.read_exact(&mut header).await.unwrap();
                assert_eq!(header[0], 5);
                let mut methods = vec![0; header[1] as usize];
                socket.read_exact(&mut methods).await.unwrap();
                assert!(methods.contains(&0));
                socket.write_all(&[5, 0]).await.unwrap();
                let mut target = [0; 10];
                socket.read_exact(&mut target).await.unwrap();
                // A native tunnel also captures unrelated background traffic.
                // This fixture serves only the reserved HTTP destination.
                if target != [5, 1, 0, 1, 198, 51, 100, 20, 0, 80] {
                    let _ = socket.write_all(&[5, 2, 0, 1, 0, 0, 0, 0, 0, 0]).await;
                    return;
                }
                socket
                    .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
                    .await
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await.unwrap());
                }
                assert!(request.starts_with(b"GET /health "));
                socket
                    .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
            });
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let cores = CoreBinaries::default().with_overrides(Some(tun::BRIDGE_CORE.into()), None, None);
    let app = App::open(
        AppPaths::new(temp.path()),
        AppOptions::new("fixture")
            .with_cores(cores)
            .with_probe_url("http://198.51.100.20/health"),
    )
    .unwrap();
    let import = app
        .import_text(&ImportRequest {
            text: format!("socks://{address}:{port}#TUN-fixture"),
            subscription_id: None,
        })
        .unwrap();
    let rules_mode = std::env::var("V2RAYR_TEST_TUN_RULES");
    let block_probe = rules_mode.as_deref() == Ok("block");
    let policy = if rules_mode.as_deref() == Ok("1") || block_probe {
        assert!(tun::RULE_ROUTING_SUPPORTED);
        let applications = v2rayr_app::inspect_rule_applications(&std::env::current_exe().unwrap())
            .unwrap();
        let action = if block_probe { "block" } else { "proxy" };
        app.add_rule_for_applications("ip-cidr", "198.51.100.20/32", action, &applications)
            .unwrap();
        RoutingPolicy::Rule
    } else {
        RoutingPolicy::Global
    };
    let snapshot = app
        .connect(ConnectionRequest {
            profile_id: import.report.added[0].id,
            mode: TakeoverMode::Tun,
            policy,
        })
        .await
        .unwrap();
    assert_eq!(snapshot.connection.phase, ConnectionPhase::Connected);
    assert!(snapshot.connection.takeover_applied);
    assert_eq!(snapshot.connection.policy, policy);
    if policy == RoutingPolicy::Rule {
        let result = v2rayr_engine::latency::verify_tun_route("http://198.51.100.20/health").await;
        assert_eq!(result.is_err(), block_probe, "{result:?}");
    }
    assert!(!tun::network_cleared());
    #[cfg(target_os = "macos")]
    assert!(mac_tun_dns_advertised());
    assert_eq!(
        app.disconnect().await.unwrap().connection.phase,
        ConnectionPhase::Disconnected
    );
    assert!(tun::network_cleared());
    #[cfg(target_os = "macos")]
    assert!(!mac_tun_dns_advertised());
    fixture.abort();
}
