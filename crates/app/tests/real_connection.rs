//! Local-only end-to-end fixture. No Internet or OS proxy writes.
#![cfg(unix)]

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use v2rayr_app::{App, AppOptions, ConnectionRequest, ImportRequest};
use v2rayr_domain::CoreKind;
use v2rayr_engine::{CoreBinaries, KernelProcess, LaunchSpec};
use v2rayr_ipc::{ConnectionPhase, RoutingPolicy, TakeoverMode};
use v2rayr_platform::{
    AppPaths, PlatformError, ProxyEndpoint, SystemProxy, SystemProxyState, TakeoverSnapshot,
    proxy::ProxyTarget,
};

#[derive(Default)]
struct FixtureProxy {
    applies: AtomicUsize,
    restore_fails: AtomicBool,
}
impl SystemProxy for FixtureProxy {
    fn capture(&self) -> Result<SystemProxyState, PlatformError> {
        Ok(SystemProxyState::default())
    }
    fn apply(&self, target: &ProxyTarget) -> Result<TakeoverSnapshot, PlatformError> {
        assert_eq!(target.http, target.https);
        self.applies.fetch_add(1, Ordering::SeqCst);
        Ok(TakeoverSnapshot {
            services: Vec::new(),
            applied_endpoint: target.http.clone(),
            applied_pac: target.pac.clone(),
            applied_at_ms: 0,
            target: Some(target.clone()),
            native: None,
        })
    }
    fn restore(&self, _: &TakeoverSnapshot) -> Result<(), PlatformError> {
        if self.restore_fails.load(Ordering::SeqCst) {
            return Err(PlatformError::InvalidState("fixture restore failed".into()));
        }
        Ok(())
    }
    fn clear(&self, _: &ProxyEndpoint) -> Result<(), PlatformError> {
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires V2RAYR_TEST_XRAY and V2RAYR_TEST_SING_BOX pointing to real binaries"]
async fn verifies_vless_and_restores_previous_session_and_journal() {
    let binary = |name| PathBuf::from(std::env::var_os(name).expect(name));
    let xray = binary("V2RAYR_TEST_XRAY");
    let server_xray = std::env::var_os("V2RAYR_TEST_SERVER_XRAY")
        .map(PathBuf::from)
        .unwrap_or_else(|| xray.clone());
    let sing_box = binary("V2RAYR_TEST_SING_BOX");
    let temp = tempfile::tempdir().unwrap();
    let backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_port = backend.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let backend_task = {
        let hits = hits.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = backend.accept().await.unwrap();
                let hits = hits.clone();
                tokio::spawn(async move {
                    let mut buf = [0; 2048];
                    if socket.read(&mut buf).await.unwrap_or(0) > 0 {
                        hits.fetch_add(1, Ordering::SeqCst);
                        let _ = socket
                            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                            .await;
                    }
                });
            }
        })
    };
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server_port = free.local_addr().unwrap().port();
    let id = uuid::Uuid::new_v4();
    let config = temp.path().join("server.json");
    std::fs::write(
        &config,
        serde_json::json!({
            "inbounds":[{"port":server_port,"listen":"127.0.0.1","protocol":"vless",
                "settings":{"clients":[{"id":id}],"decryption":"none"}}],
            "outbounds":[{"protocol":"freedom","settings":{"redirect":format!("127.0.0.1:{backend_port}")}}]
        }).to_string(),
    ).unwrap();
    drop(free);
    let mut server = KernelProcess::launch(&LaunchSpec::new(
        CoreKind::Xray,
        &server_xray,
        vec!["run".into(), "-c".into(), config.display().to_string()],
        temp.path().join("server.log"),
    ))
    .await
    .unwrap();
    server
        .wait_ready(server_port, Duration::from_secs(8))
        .await
        .unwrap();

    let proxy = Arc::new(FixtureProxy::default());
    let paths = AppPaths::new(temp.path().join("client"));
    let options = AppOptions::new("test")
        .with_cores(CoreBinaries::default().with_overrides(
            Some(sing_box.clone()),
            Some(xray.clone()),
            None,
        ))
        .with_proxy(proxy.clone())
        .with_probe_url("http://198.51.100.20/health");
    let app = App::open(
        paths.clone(),
        options.clone().with_cores(CoreBinaries::default()),
    )
    .unwrap();
    let import = |link| {
        app.import_text(&ImportRequest {
            text: link,
            subscription_id: None,
        })
        .unwrap()
        .report
        .added[0]
            .id
    };
    let working = import(format!(
        "vless://{id}@127.0.0.1:{server_port}?encryption=none#working"
    ));
    let bad = import(format!(
        "vless://{}@127.0.0.1:{server_port}?encryption=none#bad",
        uuid::Uuid::new_v4()
    ));
    let request = ConnectionRequest {
        profile_id: working,
        mode: TakeoverMode::SystemProxy,
        policy: RoutingPolicy::Global,
    };
    let missing = app.connect(request.clone()).await.unwrap_err();
    assert_eq!(missing.code, "core_missing");
    assert_eq!(missing.required_core, Some(CoreKind::SingBox));
    assert_eq!(proxy.applies.load(Ordering::SeqCst), 0);
    std::fs::create_dir_all(paths.core_bin_dir()).unwrap();
    std::fs::copy(sing_box, paths.core_bin_dir().join("sing-box")).unwrap();
    app.rescan_cores();
    app.set_core_path(CoreKind::Xray, Some(xray));
    let snapshot = app.retry().await.unwrap();
    assert_eq!(snapshot.connection.phase, ConnectionPhase::Connected);
    assert!(snapshot.connection.takeover_applied);
    assert!(hits.load(Ordering::SeqCst) > 0);
    assert!(paths.takeover_snapshot().exists());
    verify_traffic(&app, snapshot.connection.inbound_port.unwrap()).await;

    assert!(app.switch(bad).await.is_err());
    assert_eq!(app.snapshot().unwrap().connection.profile_id, Some(working));
    assert_eq!(
        app.snapshot().unwrap().connection.phase,
        ConnectionPhase::Connected
    );
    assert_eq!(
        proxy.applies.load(Ordering::SeqCst),
        2,
        "unverified connection cannot take over"
    );

    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut inbound = app.settings().inbound;
    inbound.mixed_port = occupied.local_addr().unwrap().port();
    let problem = app
        .save_settings_and_apply(&[v2rayr_ipc::SettingsPatch {
            group: v2rayr_ipc::SettingsGroupId::Inbound,
            values: serde_json::to_value(inbound).unwrap(),
        }])
        .await
        .unwrap_err();
    assert_eq!(problem.code, "port_in_use");
    assert_eq!(app.settings().inbound.mixed_port, 0);
    assert_eq!(
        app.snapshot().unwrap().connection.phase,
        ConnectionPhase::Connected
    );

    proxy.restore_fails.store(true, Ordering::SeqCst);
    assert!(app.disconnect().await.is_err());
    assert!(paths.takeover_snapshot().exists());
    drop(app);

    let app = App::open(paths.clone(), options).unwrap();
    assert_eq!(
        app.snapshot().unwrap().connection.phase,
        ConnectionPhase::Failed
    );
    assert!(paths.takeover_snapshot().exists());
    proxy.restore_fails.store(false, Ordering::SeqCst);
    assert_eq!(
        app.retry().await.unwrap().connection.phase,
        ConnectionPhase::Disconnected
    );
    assert!(!paths.takeover_snapshot().exists());

    let mut advanced = app.settings().advanced;
    advanced.core_override = v2rayr_ipc::CoreOverride::Xray;
    app.save_settings_and_apply(&[v2rayr_ipc::SettingsPatch {
        group: v2rayr_ipc::SettingsGroupId::Advanced,
        values: serde_json::to_value(advanced).unwrap(),
    }])
    .await
    .unwrap();
    let snapshot = app.connect(request.clone()).await.unwrap();
    assert_eq!(snapshot.connection.core, Some(CoreKind::Xray));
    let journal: TakeoverSnapshot =
        serde_json::from_slice(&std::fs::read(paths.takeover_snapshot()).unwrap()).unwrap();
    let target = journal.target.unwrap();
    assert_ne!(
        target.http, target.socks,
        "Xray's HTTP and SOCKS listeners are separate"
    );
    assert_eq!(
        target.socks.unwrap().port,
        snapshot.connection.inbound_port.unwrap()
    );
    verify_traffic(&app, target.http.unwrap().port).await;
    app.disconnect().await.unwrap();

    let manual = app
        .connect(ConnectionRequest {
            mode: TakeoverMode::Manual,
            ..request.clone()
        })
        .await
        .unwrap();
    assert!(!manual.connection.takeover_applied);
    app.disconnect().await.unwrap();
    if let Ok(address) = std::env::var("V2RAYR_TEST_LAN_BIND") {
        let mut inbound = app.settings().inbound;
        inbound.allow_lan = true;
        inbound.lan_bind = address;
        for core in [
            v2rayr_ipc::CoreOverride::SingBox,
            v2rayr_ipc::CoreOverride::Xray,
        ] {
            let mut advanced = app.settings().advanced;
            advanced.core_override = core;
            app.save_settings_and_apply(&[
                v2rayr_ipc::SettingsPatch {
                    group: v2rayr_ipc::SettingsGroupId::Inbound,
                    values: serde_json::to_value(&inbound).unwrap(),
                },
                v2rayr_ipc::SettingsPatch {
                    group: v2rayr_ipc::SettingsGroupId::Advanced,
                    values: serde_json::to_value(advanced).unwrap(),
                },
            ])
            .await
            .unwrap();
            let snapshot = app
                .connect(ConnectionRequest {
                    mode: TakeoverMode::Manual,
                    ..request.clone()
                })
                .await
                .unwrap();
            let lan = snapshot.connection.lan.as_ref().unwrap();
            assert_ne!(snapshot.connection.inbound_port, Some(lan.socks_port));
            let credentials = app.reveal_lan_credentials().unwrap();
            let user = credentials["username"].as_str().unwrap();
            let password = credentials["password"].as_str().unwrap();
            assert!(!serde_json::to_string(&snapshot).unwrap().contains(password));
            assert!(
                !paths
                    .core_config(snapshot.connection.core.unwrap().as_str())
                    .exists()
            );
            let addr = (lan.address, lan.http_port);
            assert_eq!(lan_http(addr, None).await, 407);
            assert_eq!(lan_http(addr, Some((user, "wrong"))).await, 407);
            assert_eq!(lan_http(addr, Some((user, password))).await, 204);
            let mut socks = tokio::net::TcpStream::connect((lan.address, lan.socks_port))
                .await
                .unwrap();
            socks.write_all(&[5, 1, 0]).await.unwrap();
            let mut selection = [0; 2];
            socks.read_exact(&mut selection).await.unwrap();
            assert_eq!(
                selection,
                [5, 255],
                "unauthenticated SOCKS must be rejected"
            );
            let mut socks = tokio::net::TcpStream::connect((lan.address, lan.socks_port))
                .await
                .unwrap();
            socks.write_all(&[5, 1, 2]).await.unwrap();
            socks.read_exact(&mut selection).await.unwrap();
            assert_eq!(selection, [5, 2]);
            let mut auth = vec![1, user.len() as u8];
            auth.extend(user.as_bytes());
            auth.push(password.len() as u8);
            auth.extend(password.as_bytes());
            socks.write_all(&auth).await.unwrap();
            socks.read_exact(&mut selection).await.unwrap();
            assert_eq!(selection, [1, 0]);
            drop(socks);
            app.disconnect().await.unwrap();
            assert!(app.reveal_lan_credentials().is_err());
            assert!(tokio::net::TcpStream::connect(addr).await.is_err());
        }
    }
    backend_task.abort();
}

async fn verify_traffic(app: &App, http_port: u16) {
    let before = app.poll_traffic().await.expect("kernel traffic API");
    let request_target = (std::net::Ipv4Addr::LOCALHOST, http_port);
    assert_eq!(lan_http(request_target, None).await, 204);
    let after = app
        .poll_traffic()
        .await
        .expect("traffic after closed request");
    assert!(after.down_bps > 0 && after.up_bps > 0, "{after:?}");
    assert!(
        after.total_down_bytes > before.total_down_bytes,
        "{after:?}"
    );
    assert!(after.total_up_bytes > before.total_up_bytes, "{after:?}");
    assert_eq!(
        app.traffic_totals(),
        (after.total_down_bytes, after.total_up_bytes)
    );
    let snapshot = app.snapshot().unwrap().traffic;
    assert_eq!(snapshot.total_down_bytes, after.total_down_bytes);
    assert_eq!(snapshot.total_up_bytes, after.total_up_bytes);
    assert!(snapshot.samples.last().unwrap().down_bps > 0);
}

async fn lan_http(address: (std::net::Ipv4Addr, u16), auth: Option<(&str, &str)>) -> u16 {
    use tokio::io::AsyncBufReadExt;
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let auth = auth
        .map(|(user, password)| {
            let data = format!("{user}:{password}");
            format!(
                "Proxy-Authorization: Basic {}\r\n",
                v2rayr_domain::base64::encode(data.as_bytes())
            )
        })
        .unwrap_or_default();
    socket.write_all(format!("GET http://198.51.100.20/health HTTP/1.1\r\nHost: 198.51.100.20\r\n{auth}Connection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio::io::BufReader::new(socket).read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    line.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("missing proxy HTTP status: {line:?}"))
}
