//! Session orchestration.
//!
//! One connection at a time, and every state change is serialised: the design
//! document requires that a rapid connect/connect/switch sequence can never
//! leave two kernels running or the system proxy pointing at a dead port.
//!
//! The failure rules implemented here:
//! * the config is written atomically and checked by the kernel *before* `run`;
//! * a port that is already taken is reported, never silently replaced;
//! * if the system proxy cannot be applied, the kernel is stopped again and the
//!   state becomes `Failed` — "已连接" without routing would be a lie;
//! * disconnect restores only the proxy state we captured, never a hardcoded one.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;

use chrono::Utc;
use uuid::Uuid;
use v2rayr_config::{
    ApiPlan, CustomRule, DnsPlan, InboundPlan, RoutingMode, RuleAction as ConfigRuleAction,
    RuleSetKind, RuleSetRef, SessionPlan, emit,
};
use v2rayr_domain::{CoreKind, ErrorCode, MuxOptions, MuxProtocol, Profile};
use v2rayr_engine::{KernelProcess, LaunchSpec, check::rejection, cores::launch_arguments};
use v2rayr_ipc::{
    AppSnapshot, ConnectionPhase, ConnectionState, Problem, RoutingPolicy, TakeoverMode,
    TrafficPoint, TrafficView,
};
use v2rayr_platform::ports::Ports;
use v2rayr_platform::proxy::{ProxyTarget, TakeoverSnapshot};

use crate::App;
use crate::error;
use crate::settings::write_atomic;

struct RemoveLanConfig(std::path::PathBuf);

impl Drop for RemoveLanConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const SAMPLE_CAPACITY: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionRequest {
    pub profile_id: Uuid,
    pub mode: TakeoverMode,
    pub policy: RoutingPolicy,
}

#[derive(Debug)]
pub(crate) struct SessionRuntime {
    phase: ConnectionPhase,
    mode: TakeoverMode,
    policy: RoutingPolicy,
    profile_id: Option<Uuid>,
    profile_remark: Option<String>,
    core: Option<CoreKind>,
    inbound_port: Option<u16>,
    http_port: Option<u16>,
    api_port: Option<u16>,
    api_secret: String,
    pub(crate) traffic_program: Option<std::path::PathBuf>,
    traffic_baseline: Option<(std::time::Instant, v2rayr_engine::traffic::Counters)>,
    traffic_connections: u32,
    started_at: Option<chrono::DateTime<Utc>>,
    takeover_applied: bool,
    takeover_error: Option<Problem>,
    error: Option<Problem>,
    process: Option<KernelProcess>,
    snapshot: Option<TakeoverSnapshot>,
    /// Kept alive for as long as the PAC takeover is active: dropping it stops
    /// the loopback server and the system setting would point at nothing.
    pac: Option<v2rayr_platform::PacServer>,
    tun: Option<v2rayr_platform::tun::TunLease>,
    lan: Option<v2rayr_config::LanInboundPlan>,
    pub total_down: u64,
    pub total_up: u64,
    samples: VecDeque<TrafficPoint>,
}

impl SessionRuntime {
    pub(crate) fn new() -> Self {
        Self {
            phase: ConnectionPhase::Disconnected,
            mode: TakeoverMode::SystemProxy,
            policy: RoutingPolicy::Rule,
            profile_id: None,
            profile_remark: None,
            core: None,
            inbound_port: None,
            http_port: None,
            api_port: None,
            api_secret: new_secret(),
            traffic_program: None,
            traffic_baseline: None,
            traffic_connections: 0,
            started_at: None,
            takeover_applied: false,
            takeover_error: None,
            error: None,
            process: None,
            snapshot: None,
            pac: None,
            tun: None,
            lan: None,
            total_down: 0,
            total_up: 0,
            samples: VecDeque::with_capacity(SAMPLE_CAPACITY),
        }
    }

    pub(crate) fn phase(&self) -> ConnectionPhase {
        self.phase
    }

    pub(crate) fn api_port(&self) -> Option<u16> {
        self.api_port
    }

    pub(crate) fn api_secret(&self) -> &str {
        &self.api_secret
    }

    pub(crate) fn connection_state(&self) -> ConnectionState {
        ConnectionState {
            phase: self.phase,
            mode: self.mode,
            policy: self.policy,
            profile_id: self.profile_id,
            profile_remark: self.profile_remark.clone(),
            core: self.core,
            inbound_port: self.inbound_port,
            api_port: self.api_port,
            pac_url: self.pac.as_ref().map(v2rayr_platform::PacServer::url),
            started_at: self.started_at,
            takeover_applied: self.takeover_applied,
            lan: self.lan.as_ref().map(|lan| v2rayr_ipc::LanEndpoint {
                address: lan.address,
                socks_port: lan.socks_port,
                http_port: lan.http_port,
            }),
            takeover_error: self.takeover_error.clone(),
            error: self.error.clone(),
        }
    }

    pub(crate) fn traffic_view(&self) -> TrafficView {
        TrafficView {
            down_bps: self.samples.back().map(|point| point.down_bps).unwrap_or(0),
            up_bps: self.samples.back().map(|point| point.up_bps).unwrap_or(0),
            total_down_bytes: self.total_down,
            total_up_bytes: self.total_up,
            connections: self.traffic_connections,
            samples: self.samples(),
        }
    }

    pub(crate) fn samples(&self) -> Vec<TrafficPoint> {
        self.samples.iter().cloned().collect()
    }

    pub(crate) fn push_sample(&mut self, point: TrafficPoint) {
        if self.samples.len() == SAMPLE_CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back(point);
    }

    pub(crate) fn record_counters(
        &mut self,
        at: std::time::Instant,
        ts: i64,
        counters: v2rayr_engine::traffic::Counters,
    ) -> Option<(u64, u64)> {
        let (previous_at, previous) = self.traffic_baseline?;
        let elapsed = at.checked_duration_since(previous_at)?.as_secs_f64();
        if elapsed <= 0.0 {
            return None;
        }
        self.traffic_baseline = Some((at, counters));
        let down = counters.down.saturating_sub(previous.down);
        let up = counters.up.saturating_sub(previous.up);
        self.total_down = self.total_down.saturating_add(down);
        self.total_up = self.total_up.saturating_add(up);
        self.traffic_connections = u32::try_from(counters.connections).unwrap_or(u32::MAX);
        self.push_sample(TrafficPoint {
            ts,
            down_bps: (down as f64 / elapsed).round() as u64,
            up_bps: (up as f64 / elapsed).round() as u64,
        });
        Some((down, up))
    }
}

fn new_secret() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl App {
    pub(crate) fn reload_active_pac(&self, rules: &crate::pac_rules::Rules) {
        let settings = self.settings();
        let mut bypass = if settings.inbound.bypass_local {
            v2rayr_platform::proxy::default_bypass()
        } else {
            Vec::new()
        };
        bypass.extend(settings.bypass_extra());
        let session = self.session.lock().expect("session");
        if let (Some(server), Some(port)) =
            (&session.pac, session.http_port.or(session.inbound_port))
        {
            server.replace_body(v2rayr_platform::render_pac_with_rules(
                "127.0.0.1",
                port,
                &bypass,
                Some(rules),
            ));
        }
    }

    fn set_phase(&self, phase: ConnectionPhase, error: Option<Problem>) {
        let mut session = self.session.lock().expect("session");
        session.phase = phase;
        session.error = error;
    }

    pub(crate) fn keep_recovery(&self, snapshot: TakeoverSnapshot, problem: Problem) {
        let mut session = self.session.lock().expect("session");
        session.phase = ConnectionPhase::Failed;
        session.snapshot = Some(snapshot);
        session.error = Some(problem);
    }

    fn fail(&self, problem: Problem) {
        {
            let mut session = self.session.lock().expect("session");
            session.phase = ConnectionPhase::Failed;
            session.error = Some(problem.clone());
            session.takeover_applied = false;
        }
        self.logs.error("session", format!("连接失败: {problem:?}"));
        self.emit_session();
    }

    /// Connect. Returns `Err` on failure *after* publishing the failed state, so
    /// the UI both shows a toast and keeps a truthful status card.
    pub async fn connect(&self, request: ConnectionRequest) -> Result<AppSnapshot, Problem> {
        let _guard = self.op_lock.lock().await;
        self.replace_session(request).await
    }

    // All callers hold op_lock. Capture the active request before changing it.
    async fn replace_session(&self, request: ConnectionRequest) -> Result<AppSnapshot, Problem> {
        let previous = self.active_request();
        let cleanup_pending = {
            let session = self.session.lock().expect("session");
            session.snapshot.is_some() || session.tun.is_some()
        };
        if previous.is_some() || cleanup_pending {
            self.disconnect_locked().await?;
        }
        match self.connect_locked(&request).await {
            Ok(snapshot) => Ok(snapshot),
            Err(problem) => {
                let can_restore = {
                    let session = self.session.lock().expect("session");
                    session.snapshot.is_none() && session.tun.is_none()
                };
                if let Some(previous) = previous.filter(|previous| previous != &request)
                    && can_restore
                    && self.connect_locked(&previous).await.is_ok()
                {
                    self.logs.warn("session", "切换失败，已恢复上一个连接");
                }
                Err(problem)
            }
        }
    }

    pub(crate) fn active_request(&self) -> Option<ConnectionRequest> {
        let session = self.session.lock().expect("session");
        if session.phase != ConnectionPhase::Connected {
            return None;
        }
        Some(ConnectionRequest {
            profile_id: session.profile_id?,
            mode: session.mode,
            policy: session.policy,
        })
    }

    pub(crate) async fn connect_locked(
        &self,
        request: &ConnectionRequest,
    ) -> Result<AppSnapshot, Problem> {
        {
            let mut session = self.session.lock().expect("session");
            session.phase = ConnectionPhase::Connecting;
            session.profile_id = Some(request.profile_id);
            session.profile_remark = None;
            session.mode = request.mode;
            session.policy = request.policy;
            session.error = None;
            session.takeover_error = None;
            session.takeover_applied = false;
        }
        self.emit_session();

        match self.start_session(request).await {
            Ok(()) => {
                self.logs.info("session", "已连接");
                self.emit_session();
                self.snapshot()
            }
            Err(problem) => {
                self.fail(problem.clone());
                Err(problem)
            }
        }
    }

    async fn start_session(&self, request: &ConnectionRequest) -> Result<(), Problem> {
        let stored = self.node(request.profile_id).ok_or_else(|| {
            Problem::new(ErrorCode::NotFound.as_str(), false).with_detail("节点不存在".to_owned())
        })?;
        let profile: Profile = stored.profile.clone();
        let settings = self.settings();
        let forward_port = settings.inbound.dokodemo_listen_port;
        let core_override = if forward_port != 0 {
            if request.mode == TakeoverMode::Tun
                || settings
                    .core_override()
                    .is_some_and(|core| core != CoreKind::Xray)
            {
                return Err(Problem::config(
                    "Dokodemo-door 需要 Xray，不能与 TUN 或其他内核同时使用",
                ));
            }
            Some(CoreKind::Xray)
        } else {
            settings.core_override()
        };
        let core =
            v2rayr_config::choose_core(&profile, core_override).map_err(error::from_config)?;
        let binaries = self.cores.read().expect("cores").clone();
        let program = binaries
            .require(core)
            .map_err(|error| {
                let mut problem = error::from_engine(error);
                problem.required_core = Some(core);
                problem
            })?
            .to_path_buf();

        let tun_upstream = if request.mode == TakeoverMode::Tun {
            if !v2rayr_platform::tun::provisioned() {
                return Err(Problem::new(ErrorCode::PermissionDenied.as_str(), false)
                    .with_detail("当前平台尚未部署 TUN helper；请改用系统代理或本地端口"));
            }
            if request.policy != RoutingPolicy::Global || core != CoreKind::SingBox {
                return Err(Problem::config("TUN 当前需要全局分流和 sing-box 节点"));
            }
            let address = profile.address.parse::<std::net::Ipv4Addr>().map_err(|_| {
                Problem::config(
                    "TUN 当前仅接受使用固定 IPv4 的节点，避免节点域名变更后形成路由回环",
                )
            })?;
            Some(address)
        } else {
            None
        };
        let desired = settings.desired_inbound_port();
        let automatic = settings.inbound.mixed_port == 0;
        let inbound_port = if automatic {
            (desired..desired.saturating_add(200))
                .find(|port| Ports::is_free(*port))
                .ok_or_else(|| port_problem(desired))?
        } else if Ports::is_free(desired) {
            desired
        } else {
            return Err(port_problem(desired));
        };
        let mut reserved = vec![inbound_port];
        if forward_port != 0 {
            if reserved.contains(&forward_port)
                || !Ports::is_free(forward_port)
                || std::net::UdpSocket::bind(("127.0.0.1", forward_port)).is_err()
            {
                return Err(port_problem(forward_port));
            }
            reserved.push(forward_port);
        }
        let http_port = if core == CoreKind::Xray {
            Some(auxiliary_port(10809, &reserved)?)
        } else {
            None
        };
        reserved.extend(http_port);
        let api_port = auxiliary_port(10812, &reserved)?;
        reserved.push(api_port);

        let lan = if settings.inbound.allow_lan {
            if core == CoreKind::Mihomo {
                return Err(Problem::config("认证 LAN 当前需要 sing-box 或 Xray"));
            }
            let address = settings
                .inbound
                .lan_bind
                .trim()
                .parse::<std::net::Ipv4Addr>()
                .ok()
                .filter(std::net::Ipv4Addr::is_private)
                .ok_or_else(|| Problem::config("请先填写本机可信网络接口的私有 IPv4 地址"))?;
            let socks_port = lan_port(address, &reserved)?;
            reserved.push(socks_port);
            let http_port = if core == CoreKind::Xray {
                let port = lan_port(address, &reserved)?;
                reserved.push(port);
                port
            } else {
                socks_port
            };
            Some(v2rayr_config::LanInboundPlan {
                address,
                socks_port,
                http_port,
                password: new_secret(),
            })
        } else {
            None
        };

        let rules = self
            .db()
            .enabled_rules()
            .map_err(error::from_store)?
            .into_iter()
            .map(|row| CustomRule {
                kind: crate::config_rule_kind(row.kind),
                value: row.value,
                action: match row.action {
                    v2rayr_store::RuleAction::Proxy => ConfigRuleAction::Proxy,
                    v2rayr_store::RuleAction::Direct => ConfigRuleAction::Direct,
                    v2rayr_store::RuleAction::Block => ConfigRuleAction::Block,
                },
            })
            .collect();

        // TCP Fast Open on Darwin loopback can break concurrent browser dials.
        let loopback_node = profile.address.eq_ignore_ascii_case("localhost")
            || profile
                .address
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        let plan = SessionPlan {
            profile,
            core_override,
            inbound: InboundPlan {
                listen: "127.0.0.1".to_owned(),
                port: inbound_port,
                http_port,
                dokodemo: (forward_port != 0).then(|| {
                    (
                        forward_port,
                        settings.inbound.dokodemo_address.trim().to_owned(),
                        settings.inbound.dokodemo_target_port,
                    )
                }),
                lan: lan.clone(),
            },
            api: ApiPlan {
                port: api_port,
                secret: {
                    let mut session = self.session.lock().expect("session");
                    session.api_secret = new_secret();
                    session.api_secret.clone()
                },
            },
            tun: None,
            policy: match request.policy {
                RoutingPolicy::Rule => RoutingMode::Rule,
                RoutingPolicy::Global => RoutingMode::Global,
                RoutingPolicy::Direct => RoutingMode::Direct,
            },
            dns: DnsPlan {
                servers: settings.dns_servers(),
                strategy: settings.routing.dns_strategy.clone(),
                fake_ip: settings.routing.fake_ip,
            },
            rules,
            rule_sets: if settings.routing.rule_set_enabled {
                self.rule_sets()
            } else {
                Vec::new()
            },
            sniffing: settings.advanced.sniffing,
            tcp_fast_open: settings.advanced.tcp_fast_open
                && !(cfg!(target_os = "macos") && loopback_node),
            mux: settings.advanced.mux_enabled.then_some(MuxOptions {
                enabled: true,
                concurrency: Some(settings.advanced.mux_concurrency),
                protocol: Some(MuxProtocol::Smux),
            }),
            log_level: settings.log_level().as_str().to_owned(),
            block_ads: settings.routing.block_ads,
            data_dir: self.paths().data_dir().display().to_string(),
        };

        let emitted = emit(&plan).map_err(error::from_config)?;
        let config_path = self.paths().core_config(core.as_str());
        // The core parses its config before readiness. Avoid retaining the
        // generated LAN password on disk after startup (or failed startup).
        let _remove_lan_config = lan.as_ref().map(|_| RemoveLanConfig(config_path.clone()));
        write_atomic(&config_path, emitted.body.as_bytes())
            .map_err(|error| error::internal(format!("无法写入内核配置: {error}")))?;
        for note in &emitted.notes {
            self.logs.info("config", note.clone());
        }

        let data_dir = self.paths().data_dir();
        let xray_assets = self.bundled_cores.as_ref().and_then(|(binaries, assets)| {
            (core == CoreKind::Xray && program.parent() == Some(binaries.as_path()))
                .then_some(assets.as_path())
        });
        let checked = v2rayr_engine::check::check_config_with_assets(
            core,
            &program,
            &config_path,
            &data_dir,
            xray_assets,
        )
        .await
        .map_err(error::from_engine)?;
        if !checked.ok {
            return Err(error::from_engine(rejection(core, &checked.output)));
        }

        let mut spec = LaunchSpec::new(
            core,
            program,
            launch_arguments(core, &config_path, &data_dir),
            self.paths().core_log(core.as_str()),
        );
        if let Some(assets) = xray_assets {
            spec = spec.with_env("XRAY_LOCATION_ASSET", assets.to_string_lossy());
        }
        let traffic_started = std::time::Instant::now();
        let mut process = KernelProcess::launch(&spec)
            .await
            .map_err(error::from_engine)?;
        process
            .wait_ready(inbound_port, READY_TIMEOUT)
            .await
            .map_err(error::from_engine)?;

        v2rayr_engine::latency::verify_inbound(http_port.unwrap_or(inbound_port), &self.probe_url)
            .await
            .map_err(error::from_engine)?;
        if let Some(error) = process.exit_error() {
            return Err(error::from_engine(error));
        }

        // The kernel is up. Take over the OS only now, so a failed takeover can
        // still stop a kernel that would otherwise proxy nothing.
        let mut takeover_applied = request.mode != TakeoverMode::Manual;
        let mut takeover_error = None;
        let mut snapshot = None;
        let mut pending = None;
        let mut journal = |prepared: &TakeoverSnapshot| {
            self.write_takeover_journal(prepared)?;
            pending = Some(prepared.clone());
            Ok(())
        };
        let mut pac = None;
        let mut tun = None;
        match request.mode {
            TakeoverMode::Manual => {}
            TakeoverMode::SystemProxy => {
                let mut target = ProxyTarget::all_through("127.0.0.1", inbound_port);
                let mut bypass = if settings.inbound.bypass_local {
                    v2rayr_platform::proxy::default_bypass()
                } else {
                    Vec::new()
                };
                bypass.extend(settings.bypass_extra());
                if let Some(http_port) = http_port {
                    let endpoint = v2rayr_platform::ProxyEndpoint::new("127.0.0.1", http_port);
                    target.http = Some(endpoint.clone());
                    target.https = Some(endpoint);
                }
                target.bypass = bypass;
                match self.proxy.apply_journaled(&target, &mut journal) {
                    Ok(applied) => {
                        snapshot = Some(applied);
                    }
                    Err(error) => {
                        takeover_applied = false;
                        takeover_error = Some(error::from_platform(error));
                    }
                }
            }
            TakeoverMode::Pac => {
                let mut bypass = if settings.inbound.bypass_local {
                    v2rayr_platform::proxy::default_bypass()
                } else {
                    Vec::new()
                };
                bypass.extend(settings.bypass_extra());
                let script = v2rayr_platform::render_pac_with_rules(
                    "127.0.0.1",
                    http_port.unwrap_or(inbound_port),
                    &bypass,
                    self.pac_rules().as_deref(),
                );
                // PAC port is 10811 unless something already holds it; we never
                // silently reuse an occupied port, we just pick the next free one
                // and say so in the log.
                let port = auxiliary_port(10811, &reserved)?;
                match v2rayr_platform::PacServer::start(port, script) {
                    Ok(server) => {
                        let target = ProxyTarget::through_pac(server.url(), bypass);
                        pac = Some(server);
                        match self.proxy.apply_journaled(&target, &mut journal) {
                            Ok(applied) => {
                                snapshot = Some(applied);
                            }
                            Err(error) => {
                                takeover_applied = false;
                                takeover_error = Some(error::from_platform(error));
                            }
                        }
                    }
                    Err(error) => {
                        takeover_applied = false;
                        takeover_error = Some(error::from_platform(error));
                    }
                }
            }
            TakeoverMode::Tun => {
                let request = v2rayr_platform::tun::TunRequest {
                    socks_port: inbound_port,
                    upstream: tun_upstream.expect("TUN validated above"),
                };
                let mut lease = v2rayr_platform::tun::TunLease::start(&request)
                    .await
                    .map_err(error::from_platform)?;
                let result = v2rayr_engine::latency::verify_tun_route(&self.probe_url).await;
                if result.is_err() || lease.exited() {
                    if let Err(error) = lease.stop().await {
                        let mut session = self.session.lock().expect("session");
                        session.tun = Some(lease);
                        session.process = Some(process);
                        return Err(recovery_problem(error));
                    }
                    result.map_err(error::from_engine)?;
                    return Err(Problem::network("TUN helper 在探测期间退出"));
                }
                tun = Some(lease);
            }
        }

        if request.mode != TakeoverMode::Manual && !takeover_applied {
            if let Some(pending) = pending {
                if let Err(error) = self.proxy.restore(&pending) {
                    let problem = recovery_problem(error);
                    self.keep_recovery(pending, problem.clone());
                    let mut session = self.session.lock().expect("session");
                    session.process = Some(process);
                    session.pac = pac;
                    return Err(problem);
                }
                self.clear_takeover_journal();
            }
            return Err(takeover_error.unwrap_or_else(|| {
                Problem::new(ErrorCode::TakeoverFailed.as_str(), true)
                    .with_detail("系统代理接管失败".to_owned())
            }));
        }

        let (total_down, total_up) = self.traffic_totals();
        {
            let mut session = self.session.lock().expect("session");
            session.phase = ConnectionPhase::Connected;
            session.profile_id = Some(stored.id);
            session.profile_remark = Some(stored.profile.remark.clone());
            session.core = Some(core);
            session.inbound_port = Some(inbound_port);
            session.http_port = http_port;
            session.api_port = Some(api_port);
            session.started_at = Some(Utc::now());
            session.takeover_applied = takeover_applied;
            session.takeover_error = takeover_error;
            session.error = None;
            session.process = Some(process);
            session.snapshot = snapshot;
            session.pac = pac;
            session.tun = tun;
            session.lan = lan;
            session.total_down = total_down;
            session.total_up = total_up;
            session.samples.clear();
            session.traffic_program = (core == CoreKind::Xray).then_some(spec.program);
            session.traffic_baseline = Some((traffic_started, Default::default()));
            session.traffic_connections = 0;
        }
        {
            let db = self.db();
            let _ = db.set_active_profile(Some(stored.id));
            let _ = db.set_last_mode(mode_str(request.mode));
            let _ = db.mark_used(stored.id);
        }
        self.logs.info(
            "session",
            format!(
                "{} 已启动：{}，入站端口 {}，控制端口 {}",
                core.as_str(),
                stored.profile.remark,
                inbound_port,
                api_port
            ),
        );
        Ok(())
    }

    /// Keep session operations serialized until a verified maintenance
    /// action has finished replacing the installed application.
    pub async fn with_network_stopped<T>(
        &self,
        action: impl FnOnce() -> Result<T, Problem>,
    ) -> Result<T, Problem> {
        let _guard = self.op_lock.lock().await;
        self.disconnect_locked().await?;
        action()
    }

    pub async fn disconnect(&self) -> Result<AppSnapshot, Problem> {
        let _guard = self.op_lock.lock().await;
        self.disconnect_locked().await
    }

    pub(crate) async fn disconnect_locked(&self) -> Result<AppSnapshot, Problem> {
        self.set_phase(ConnectionPhase::Disconnecting, None);
        self.emit_session();

        let snapshot = self.session.lock().expect("session").snapshot.clone();
        if let Some(snapshot) = &snapshot {
            if let Err(error) = self.proxy.restore(snapshot) {
                let problem = recovery_problem(error);
                self.fail(problem.clone());
                return Err(problem);
            }
        }
        let mut tun = self.session.lock().expect("session").tun.take();
        if let Some(lease) = tun.as_mut() {
            if let Err(error) = lease.stop().await {
                self.session.lock().expect("session").tun = tun;
                let problem = recovery_problem(error);
                self.fail(problem.clone());
                return Err(problem);
            }
        }
        self.clear_takeover_journal();
        let (process, pac) = {
            let mut session = self.session.lock().expect("session");
            session.snapshot = None;
            session.takeover_applied = false;
            session.inbound_port = None;
            session.http_port = None;
            session.api_port = None;
            session.samples.clear();
            session.traffic_program = None;
            session.traffic_baseline = None;
            session.traffic_connections = 0;
            session.lan = None;
            (session.process.take(), session.pac.take())
        };
        // Stop serving the PAC script only after the OS setting is restored,
        // otherwise a browser may fetch the URL while it is already gone.
        drop(pac);
        if let Some(mut process) = process {
            if let Err(error) = process.stop(std::time::Duration::from_secs(5)).await {
                self.logs.warn("session", format!("停止内核失败: {error}"));
            }
        }
        {
            let mut session = self.session.lock().expect("session");
            session.phase = ConnectionPhase::Disconnected;
            session.profile_id = None;
            session.profile_remark = None;
            session.core = None;
            session.started_at = None;
            session.error = None;
            session.takeover_error = None;
            session.snapshot = None;
        }
        self.logs.info("session", "已断开");
        self.emit_session();
        self.snapshot()
    }

    pub async fn switch(&self, profile_id: Uuid) -> Result<AppSnapshot, Problem> {
        let _guard = self.op_lock.lock().await;
        if self.node(profile_id).is_none() {
            return Err(Problem::new(ErrorCode::NotFound.as_str(), false)
                .with_detail("节点不存在".to_owned()));
        }
        let (mode, policy) = {
            let session = self.session.lock().expect("session");
            (session.mode, session.policy)
        };
        self.replace_session(ConnectionRequest {
            profile_id,
            mode,
            policy,
        })
        .await
    }

    pub async fn retry(&self) -> Result<AppSnapshot, Problem> {
        let _guard = self.op_lock.lock().await;
        let (profile_id, mode, policy, recovery_pending) = {
            let session = self.session.lock().expect("session");
            (
                session.profile_id,
                session.mode,
                session.policy,
                session.phase == ConnectionPhase::Failed
                    && (session.snapshot.is_some() || session.tun.is_some()),
            )
        };
        if recovery_pending {
            return self.disconnect_locked().await;
        }
        let profile_id = profile_id
            .or_else(|| self.db().active_profile())
            .ok_or_else(|| {
                Problem::new(ErrorCode::NotConnected.as_str(), false)
                    .with_detail("没有可重试的节点".to_owned())
            })?;
        self.replace_session(ConnectionRequest {
            profile_id,
            mode,
            policy,
        })
        .await
    }

    pub async fn set_mode(&self, mode: TakeoverMode) -> Result<AppSnapshot, Problem> {
        let _guard = self.op_lock.lock().await;
        if let Some(mut request) = self.active_request() {
            if request.mode != mode {
                request.mode = mode;
                return self.replace_session(request).await;
            }
        } else {
            self.session.lock().expect("session").mode = mode;
            self.emit_session();
        }
        self.snapshot()
    }

    pub async fn set_policy(&self, policy: RoutingPolicy) -> Result<AppSnapshot, Problem> {
        let _guard = self.op_lock.lock().await;
        if let Some(mut request) = self.active_request() {
            if request.policy != policy {
                request.policy = policy;
                return self.replace_session(request).await;
            }
        } else {
            self.session.lock().expect("session").policy = policy;
            self.emit_session();
        }
        self.snapshot()
    }

    pub(crate) async fn check_kernel_exit(&self) -> bool {
        let Ok(_guard) = self.op_lock.try_lock() else {
            return false;
        };
        let exited = {
            let mut session = self.session.lock().expect("session");
            if session.phase != ConnectionPhase::Connected {
                return false;
            }
            if session
                .tun
                .as_mut()
                .is_some_and(v2rayr_platform::tun::TunLease::exited)
            {
                Some(Problem::network("TUN helper 已退出"))
            } else {
                session
                    .process
                    .as_mut()
                    .and_then(KernelProcess::exit_error)
                    .map(error::from_engine)
            }
        };
        let Some(exited) = exited else { return false };
        let profile_id = self.active_request().map(|r| r.profile_id);
        if self.disconnect_locked().await.is_ok() {
            self.session.lock().expect("session").profile_id = profile_id;
            self.fail(exited);
        }
        true
    }

    pub(crate) fn set_idle_policy(&self, policy: RoutingPolicy) {
        let mut session = self.session.lock().expect("session");
        if session.phase == ConnectionPhase::Disconnected {
            session.policy = policy;
        }
    }

    pub fn reveal_lan_credentials(&self) -> Result<serde_json::Value, Problem> {
        let session = self.session.lock().expect("session");
        let lan = session
            .lan
            .as_ref()
            .filter(|_| session.phase == ConnectionPhase::Connected)
            .ok_or_else(|| Problem::new(ErrorCode::NotConnected.as_str(), false))?;
        Ok(serde_json::json!({"username":v2rayr_config::LAN_USERNAME,"password":lan.password}))
    }

    fn rule_sets(&self) -> Vec<RuleSetRef> {
        let dir = self.paths().rule_sets_dir();
        [
            ("geosite-cn", "geosite-cn.srs", RuleSetKind::CnSites),
            ("geoip-cn", "geoip-cn.srs", RuleSetKind::CnIp),
            ("geosite-ads", "geosite-ads.srs", RuleSetKind::Ads),
        ]
        .into_iter()
        .filter_map(|(tag, file, kind)| {
            let path = dir.join(file);
            path.is_file().then(|| RuleSetRef {
                tag: tag.to_owned(),
                path: path.display().to_string(),
                kind,
            })
        })
        .collect()
    }

    /// Emit a session event after external mutations (e.g. import).
    pub fn touch_session(&self) {
        let _ = self.settings_revision.fetch_add(0, Ordering::Relaxed);
        self.emit_session();
    }
}

pub(crate) fn recovery_problem(error: v2rayr_platform::PlatformError) -> Problem {
    Problem::new("proxy_recovery_pending", true)
        .with_detail(format!("系统网络尚未恢复，请重试恢复网络。{error}"))
}

fn lan_port(address: std::net::Ipv4Addr, reserved: &[u16]) -> Result<u16, Problem> {
    let mut listeners = Vec::new();
    for _ in 0..8 {
        let listener = std::net::TcpListener::bind((address, 0)).map_err(|_| {
            Problem::config(format!("无法在 {address} 监听 LAN，请核对本机网卡地址"))
        })?;
        let port = listener.local_addr().map_err(error::internal)?.port();
        if !reserved.contains(&port) {
            return Ok(port);
        }
        listeners.push(listener);
    }
    Err(Problem::config("无法分配独立的 LAN 端口"))
}

fn port_problem(port: u16) -> Problem {
    Problem::new(ErrorCode::PortInUse.as_str(), false)
        .with_detail(format!("本地端口 {port} 不可用，请释放端口或修改端口设置"))
}

fn auxiliary_port(start: u16, reserved: &[u16]) -> Result<u16, Problem> {
    (start..start.saturating_add(200))
        .find(|port| !reserved.contains(port) && Ports::is_free(*port))
        .ok_or_else(|| port_problem(start))
}

fn mode_str(mode: TakeoverMode) -> &'static str {
    match mode {
        TakeoverMode::SystemProxy => "system-proxy",
        TakeoverMode::Pac => "pac",
        TakeoverMode::Tun => "tun",
        TakeoverMode::Manual => "manual",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_runtime_is_disconnected() {
        let runtime = SessionRuntime::new();
        assert_eq!(runtime.phase(), ConnectionPhase::Disconnected);
        assert!(runtime.api_port().is_none());
        assert_eq!(runtime.traffic_view().samples.len(), 0);
    }

    #[test]
    fn samples_are_capped_without_treating_rates_as_bytes() {
        let mut runtime = SessionRuntime::new();
        for index in 0..(SAMPLE_CAPACITY + 5) {
            runtime.push_sample(TrafficPoint {
                ts: index as i64,
                down_bps: 10,
                up_bps: 1,
            });
        }
        assert_eq!(runtime.samples().len(), SAMPLE_CAPACITY);
        assert_eq!(runtime.total_down, 0);
    }

    #[test]
    fn counters_use_elapsed_time_for_rate_and_byte_deltas_for_totals() {
        let mut runtime = SessionRuntime::new();
        let start = std::time::Instant::now();
        runtime.traffic_baseline = Some((start, Default::default()));
        assert_eq!(
            runtime.record_counters(
                start + std::time::Duration::from_secs(2),
                2000,
                v2rayr_engine::traffic::Counters {
                    down: 600,
                    up: 200,
                    connections: 1
                },
            ),
            Some((600, 200))
        );
        let view = runtime.traffic_view();
        assert_eq!((view.down_bps, view.up_bps), (300, 100));
        assert_eq!((view.total_down_bytes, view.total_up_bytes), (600, 200));
        assert_eq!(view.connections, 1);
    }
}
