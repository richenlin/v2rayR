//! Windows 本机验收夹具：真实原生内核 + 本地隔离服务，验证 CONFIG 与 FLOW。
//!
//! 与 `real_connection.rs`（Unix 专用）的区别：这里只在上海 Windows 上跑，使用真实
//! `sing-box-*.exe` / `xray-*.exe`，不写系统代理（用 `FixtureProxy` 记录接管调用），
//! 也不访问外网：探测 URL 与业务请求都经 Xray freedom `redirect` 落到本机后端。
//!
//! 运行（普通用户即可，无需管理员）：
//! ```powershell
//! $env:V2RAYR_TEST_SING_BOX = 'C:\Users\<u>\AppData\Local\v2rayR\v2rayr-core-sing-box.exe'
//! $env:V2RAYR_TEST_XRAY = 'C:\Users\<u>\AppData\Local\v2rayR\v2rayr-core-xray.exe'
//! cargo test -p v2rayr-app --locked --test windows_loopback_flow -- --ignored --nocapture
//! ```
#![cfg(windows)]

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use v2rayr_app::{App, AppOptions, ConnectionRequest, ImportRequest};
use v2rayr_domain::CoreKind;
use v2rayr_engine::{CoreBinaries, KernelProcess, LaunchSpec};
use v2rayr_ipc::{
    ConnectionPhase, CoreOverride, RoutingPolicy, SettingsGroupId, SettingsPatch, TakeoverMode,
};
use v2rayr_platform::{
    AppPaths, PlatformError, ProxyEndpoint, SystemProxy, SystemProxyState, TakeoverSnapshot,
    proxy::ProxyTarget,
};
/// 只记录接管调用，绝不写真实系统代理。
#[derive(Default)]
struct FixtureProxy {
    applies: AtomicUsize,
}

impl SystemProxy for FixtureProxy {
    fn capture(&self) -> Result<SystemProxyState, PlatformError> {
        Ok(SystemProxyState::default())
    }
    fn apply(&self, target: &ProxyTarget) -> Result<TakeoverSnapshot, PlatformError> {
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
        Ok(())
    }
    fn clear(&self, _: &ProxyEndpoint) -> Result<(), PlatformError> {
        Ok(())
    }
}

fn core_path(name: &str) -> PathBuf {
    let value = std::env::var_os(name).unwrap_or_else(|| panic!("缺少环境变量 {name}"));
    let path = PathBuf::from(value);
    assert!(
        path.is_file(),
        "{name} 指向的文件不存在: {}",
        path.display()
    );
    path
}

/// 通过本机 HTTP 代理入口请求受控目标，返回状态码。
async fn http_via_proxy(port: u16) -> u16 {
    use tokio::io::AsyncBufReadExt;
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    socket
        .write_all(
            b"GET http://198.51.100.20/health HTTP/1.1\r\nHost: 198.51.100.20\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        tokio::io::BufReader::new(socket).read_line(&mut line),
    )
    .await
    .unwrap_or_else(|_| panic!("代理入口 {port} 10 秒内无响应"))
    .unwrap();
    line.split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("缺少代理 HTTP 状态行: {line:?}"))
}

async fn verify_flow(app: &App, http_port: u16) -> (u64, u64) {
    let before = app.poll_traffic().await.expect("内核流量统计接口");
    assert_eq!(http_via_proxy(http_port).await, 204, "经代理的真实请求");
    let after = app.poll_traffic().await.expect("请求后流量");
    assert!(
        after.total_down_bytes > before.total_down_bytes
            && after.total_up_bytes > before.total_up_bytes,
        "流经内核的字节未累计: before={before:?} after={after:?}"
    );
    (after.total_down_bytes, after.total_up_bytes)
}

#[tokio::test]
#[ignore = "使用本机真实内核起本地服务；设置 V2RAYR_TEST_SING_BOX 与 V2RAYR_TEST_XRAY 后显式运行"]
async fn windows_loopback_vless_flows_through_both_cores() {
    let sing_box = core_path("V2RAYR_TEST_SING_BOX");
    let xray = core_path("V2RAYR_TEST_XRAY");
    let temp = tempfile::tempdir().unwrap();

    // 本机后端：固定返回 204，用于证明请求确实穿过了代理链路。
    let backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_port = backend.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let backend_hits = hits.clone();
    let backend_task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = backend.accept().await.unwrap();
            let hits = backend_hits.clone();
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
    });

    // 受控"远端"：本机 Xray 提供 VLESS 入站，freedom 把流量重定向到后端。
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server_port = free.local_addr().unwrap().port();
    drop(free);
    let id = uuid::Uuid::new_v4();
    let config = temp.path().join("server.json");
    std::fs::write(
        &config,
        serde_json::json!({
            "inbounds":[{"port":server_port,"listen":"127.0.0.1","protocol":"vless",
                "settings":{"clients":[{"id":id}],"decryption":"none"}}],
            "outbounds":[{"protocol":"freedom","settings":{"redirect":format!("127.0.0.1:{backend_port}"),
                "finalRules":[{"action":"allow","ip":["127.0.0.1/32"],"port":backend_port}]}}]
        })
        .to_string(),
    )
    .unwrap();
    let server_log = temp.path().join("server.log");
    let mut server = KernelProcess::launch(&LaunchSpec::new(
        CoreKind::Xray,
        &xray,
        vec!["run".into(), "-c".into(), config.display().to_string()],
        server_log.clone(),
    ))
    .await
    .unwrap();
    server
        .wait_ready(server_port, Duration::from_secs(15))
        .await
        .unwrap();

    let proxy = Arc::new(FixtureProxy::default());
    let paths = AppPaths::new(temp.path().join("client"));
    let options = AppOptions::new("0.1.0")
        .with_cores(CoreBinaries::default())
        .with_proxy(proxy.clone())
        .with_probe_url("http://198.51.100.20/health");
    let app = App::open(paths.clone(), options.clone()).unwrap();
    let import = |link: String| {
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
        "vless://{id}@127.0.0.1:{server_port}?type=tcp&security=none#loopback-vless"
    ));
    let bad = import(format!(
        "vless://{}@127.0.0.1:{server_port}?type=tcp&security=none#loopback-bad-uuid",
        uuid::Uuid::new_v4()
    ));

    let request = ConnectionRequest {
        profile_id: working,
        mode: TakeoverMode::Manual,
        policy: RoutingPolicy::Global,
    };

    // 1) 没有内核时必须明确失败，且不声称接管。
    let missing = app.connect(request.clone()).await.unwrap_err();
    assert_eq!(missing.code, "core_missing");
    assert_eq!(missing.required_core, Some(CoreKind::SingBox));
    assert_eq!(proxy.applies.load(Ordering::SeqCst), 0);
    let after_missing = app.snapshot().unwrap().connection;
    // 产品语义：失败连接进入 Failed 阶段（不是 Disconnected），且未接管、无入站端口。
    assert_eq!(after_missing.phase, ConnectionPhase::Failed);
    assert!(!after_missing.takeover_applied);
    assert_eq!(after_missing.inbound_port, None);

    // 2) sing-box 真实转发（默认自动选择 sing-box）。
    // 先放入数据目录的内核目录再重扫描，最后显式指定路径（rescan 会重置路径覆盖）。
    std::fs::create_dir_all(paths.core_bin_dir()).unwrap();
    std::fs::copy(&sing_box, paths.core_bin_dir().join("sing-box.exe")).unwrap();
    std::fs::copy(&xray, paths.core_bin_dir().join("xray.exe")).unwrap();
    app.rescan_cores();
    app.set_core_path(CoreKind::SingBox, Some(sing_box.clone()));
    app.set_core_path(CoreKind::Xray, Some(xray.clone()));
    let connected = app.retry().await.unwrap_or_else(|error| {
        panic!(
            "sing-box 连接失败: {error:?}\n应用日志: {:?}\n服务端日志: {}",
            app.tail_logs(30),
            std::fs::read_to_string(&server_log).unwrap_or_default()
        );
    });
    assert_eq!(connected.connection.phase, ConnectionPhase::Connected);
    assert_eq!(connected.connection.core, Some(CoreKind::SingBox));
    assert!(hits.load(Ordering::SeqCst) > 0, "连接探测必须真实到达后端");
    let inbound = connected.connection.inbound_port.expect("入站端口");
    let sing_box_totals = verify_flow(&app, inbound).await;

    // 3) 切换到一个不可用节点必须失败并保留原会话。
    assert!(app.switch(bad).await.is_err(), "坏节点不应切换成功");
    let after_bad = app.snapshot().unwrap().connection;
    assert_eq!(after_bad.phase, ConnectionPhase::Connected);
    assert_eq!(after_bad.profile_id, Some(working));

    // 4) 断开后端口释放。
    app.disconnect().await.unwrap();
    assert_eq!(
        app.snapshot().unwrap().connection.phase,
        ConnectionPhase::Disconnected
    );
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", inbound))
            .await
            .is_err(),
        "断开后入站端口应释放"
    );

    // 5) 强制 Xray：HTTP 与 SOCKS 端口分离，真实转发可用。
    let mut advanced = app.settings().advanced;
    advanced.core_override = CoreOverride::Xray;
    app.save_settings_and_apply(&[SettingsPatch {
        group: SettingsGroupId::Advanced,
        values: serde_json::to_value(advanced).unwrap(),
    }])
    .await
    .unwrap();
    let snapshot = app.connect(request.clone()).await.unwrap_or_else(|error| {
        panic!(
            "Xray 连接失败: {error:?}\n应用日志: {:?}",
            app.tail_logs(30)
        );
    });
    assert_eq!(snapshot.connection.core, Some(CoreKind::Xray));
    let http_port = snapshot
        .connection
        .inbound_port
        .expect("Xray HTTP 入站端口");
    let xray_totals = verify_flow(&app, http_port).await;
    assert!(
        xray_totals.0 > sing_box_totals.0 || xray_totals.1 > sing_box_totals.1,
        "两个内核都应产生累计字节"
    );
    app.disconnect().await.unwrap();
    assert_eq!(
        app.snapshot().unwrap().connection.phase,
        ConnectionPhase::Disconnected
    );

    // 手动模式从不调用系统代理接管。
    assert_eq!(
        proxy.applies.load(Ordering::SeqCst),
        0,
        "manual 模式不得写系统代理"
    );
    drop(server);
    backend_task.abort();
}

/// 本机受控服务：后端 HTTP(204) + Xray VLESS 入站（freedom 重定向到后端）。
async fn start_loopback(
    xray: &Path,
) -> (
    tempfile::TempDir,
    u16,
    uuid::Uuid,
    Arc<AtomicUsize>,
    KernelProcess,
) {
    let temp = tempfile::tempdir().unwrap();
    let backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_port = backend.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = backend.accept().await.unwrap();
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buf = [0; 2048];
                if socket.read(&mut buf).await.unwrap_or(0) > 0 {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let _ = socket
                        .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                        .await;
                }
            });
        }
    });
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server_port = free.local_addr().unwrap().port();
    drop(free);
    let id = uuid::Uuid::new_v4();
    let config = temp.path().join("server.json");
    std::fs::write(
        &config,
        serde_json::json!({
            "inbounds":[{"port":server_port,"listen":"127.0.0.1","protocol":"vless",
                "settings":{"clients":[{"id":id}],"decryption":"none"}}],
            "outbounds":[{"protocol":"freedom","settings":{"redirect":format!("127.0.0.1:{backend_port}"),
                "finalRules":[{"action":"allow","ip":["127.0.0.1/32"],"port":backend_port}]}}]
        })
        .to_string(),
    )
    .unwrap();
    let mut server = KernelProcess::launch(&LaunchSpec::new(
        CoreKind::Xray,
        xray,
        vec!["run".into(), "-c".into(), config.display().to_string()],
        temp.path().join("server.log"),
    ))
    .await
    .unwrap();
    server
        .wait_ready(server_port, Duration::from_secs(15))
        .await
        .unwrap();
    (temp, server_port, id, hits, server)
}

/// 读取应用写下的恢复日志（接管失败或断言中断时靠它还原）。
fn load_journal(paths: &AppPaths) -> Option<TakeoverSnapshot> {
    let body = std::fs::read_to_string(paths.takeover_snapshot()).ok()?;
    serde_json::from_str(&body).ok()
}

/// 断言失败（panic）时也要把系统代理还原回基线。
///
/// 断言中断时手上的 `TakeoverSnapshot` 可能还没拿到，所以回落到磁盘上的恢复日志；
/// 这条路径和应用自己的崩溃恢复走的是同一份数据。
struct RestoreOnDrop {
    proxy: Arc<dyn SystemProxy>,
    journal: PathBuf,
    snapshot: Option<TakeoverSnapshot>,
}

impl Drop for RestoreOnDrop {
    fn drop(&mut self) {
        let snapshot = self.snapshot.take().or_else(|| {
            let body = std::fs::read_to_string(&self.journal).ok()?;
            serde_json::from_str(&body).ok()
        });
        let Some(snapshot) = snapshot else { return };
        if let Err(error) = self.proxy.restore(&snapshot) {
            eprintln!("系统代理还原失败，需要手工恢复: {error}");
        }
    }
}

/// 用真正读取 WinINet 默认代理的客户端请求受控目标。
///
/// `Invoke-WebRequest` 走 .NET 的 `DefaultWebProxy`（Windows PowerShell 5.1 上是
/// IE/WinINet 设置）；返回 (状态码, 诊断信息)，诊断里带上 .NET 解析到的代理地址。
fn system_proxy_client() -> (Option<u16>, String) {
    // `-DisableKeepAlive`：Xray 的 HTTP 入站会把 204 转回来却保留代理跳的
    // keep-alive，.NET 于是等一个永远不来的响应体（实测 12s 超时，加这个参数 97ms）。
    // `$ProgressPreference`：stderr 被父进程接成管道时，Invoke-WebRequest 的进度条
    // 渲染会一直卡住到超时（同一脚本在交互终端里 100ms 就能返回）。
    let script = "$ProgressPreference='SilentlyContinue'; $ErrorActionPreference='Stop';         $uri=[Uri]'http://198.51.100.20/health';         \"proxy=$([System.Net.WebRequest]::GetSystemWebProxy().GetProxy($uri))\";         try { $r=Invoke-WebRequest -Uri $uri -TimeoutSec 15 -UseBasicParsing -DisableKeepAlive; \"status=$([int]$r.StatusCode)\" }         catch { \"error=$($_.Exception.GetType().Name): $($_.Exception.Message)\" }";
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .expect("run powershell");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let diagnostic = format!(
        "stdout={} stderr={}",
        stdout.trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let code = stdout.lines().find_map(|line| {
        line.trim()
            .strip_prefix("status=")
            .and_then(|value| value.parse().ok())
    });
    (code, diagnostic)
}

/// 接管系统代理的完整往返：写入 → 读回 → 真实客户端访问 → 断开逐字段还原。
#[tokio::test]
#[ignore = "写真实系统代理；需要在独占的本机账户上显式运行"]
async fn windows_system_proxy_takeover_writes_reads_back_and_restores() {
    let xray = core_path("V2RAYR_TEST_XRAY");
    let (temp, server_port, id, hits, server) = start_loopback(&xray).await;
    let proxy = v2rayr_platform::proxy::system_proxy();
    // 基线：接管前原样记下，断开后要逐字段回到这里。
    let before = proxy.capture().expect("capture baseline");

    let paths = AppPaths::new(temp.path().join("client"));
    let options = AppOptions::new("0.1.0")
        .with_cores(CoreBinaries::default())
        .with_proxy(proxy.clone())
        .with_probe_url("http://198.51.100.20/health");
    let app = App::open(paths.clone(), options.clone()).unwrap();
    // 断言中途 panic 也要还原：先挂上守护，拿到快照后再填进去。
    let mut guard = RestoreOnDrop {
        proxy: proxy.clone(),
        journal: paths.takeover_snapshot(),
        snapshot: None,
    };
    let profile = app
        .import_text(&ImportRequest {
            text: format!(
                "vless://{id}@127.0.0.1:{server_port}?type=tcp&security=none#system-proxy"
            ),
            subscription_id: None,
        })
        .unwrap()
        .report
        .added[0]
        .id;
    std::fs::create_dir_all(paths.core_bin_dir()).unwrap();
    std::fs::copy(&xray, paths.core_bin_dir().join("xray.exe")).unwrap();
    app.rescan_cores();
    app.set_core_path(CoreKind::Xray, Some(xray.clone()));
    // 只装了 Xray：显式固定内核，避免默认首选 sing-box 报 core_missing。
    let mut advanced = app.settings().advanced;
    advanced.core_override = CoreOverride::Xray;
    app.save_settings_and_apply(&[SettingsPatch {
        group: SettingsGroupId::Advanced,
        values: serde_json::to_value(advanced).unwrap(),
    }])
    .await
    .unwrap();

    // 1) 系统代理模式下连接：会写真实代理并留下恢复日志。
    let connected = app
        .connect(ConnectionRequest {
            profile_id: profile,
            mode: TakeoverMode::SystemProxy,
            policy: RoutingPolicy::Global,
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "系统代理模式连接失败: {error:?}
应用日志: {:?}",
                app.tail_logs(30)
            );
        });
    guard.snapshot = load_journal(&paths);
    assert_eq!(connected.connection.phase, ConnectionPhase::Connected);
    assert_eq!(connected.connection.core, Some(CoreKind::Xray));
    assert!(connected.connection.takeover_applied, "必须声称接管成功");
    let inbound = connected.connection.inbound_port.expect("入站端口");
    assert!(paths.takeover_snapshot().exists(), "接管后必须留下恢复日志");

    // 2) 读回：系统代理必须真的指向本应用入站，且与基线不同。
    let live = proxy.capture().expect("capture during takeover");
    assert_ne!(live, before, "接管后系统代理必须发生变化");
    let endpoint = ProxyEndpoint::new("127.0.0.1", inbound);
    assert!(live.is_enabled(), "接管后系统代理必须启用: {live:?}");
    assert!(
        live.pointing_at(&endpoint),
        "系统代理必须指向本应用入站 {endpoint:?}: {live:?}"
    );

    // 调试用：接管窗口内留出时间，供外部手工验证其它客户端（不影响 normal 断言）。
    if let Some(secs) = std::env::var("V2RAYR_TEST_HOLD_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        tokio::time::sleep(Duration::from_secs(secs)).await;
    }

    // 3) 真正使用系统代理的客户端请求成功（不是 curl 的显式代理）。
    // 必须放到阻塞线程池里跑：`#[tokio::test]` 是单线程运行时，同步等待子进程会
    // 把服务本机后端的任务一起卡住，客户端只能等到超时。
    let (code, diagnostic) = tokio::task::spawn_blocking(system_proxy_client)
        .await
        .expect("join client task");
    assert_eq!(
        code,
        Some(204),
        "读取 WinINet 默认代理的客户端必须经本应用访问到受控目标: {diagnostic}"
    );

    // 4) 断开：字段恢复、journal 删除、端口释放。
    app.disconnect().await.expect("disconnect");
    guard.snapshot = None;
    let after = proxy.capture().expect("capture after");
    assert_eq!(after, before, "断开后必须逐字段恢复到基线");
    assert!(
        !paths.takeover_snapshot().exists(),
        "成功恢复后必须删除 journal"
    );
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", inbound))
            .await
            .is_err(),
        "断开后入站端口必须释放"
    );
    assert!(hits.load(Ordering::SeqCst) > 0, "连接探测必须真实到达后端");
    drop(server);
}
