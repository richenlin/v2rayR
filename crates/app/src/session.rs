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
use v2rayr_platform::ports::{PortState, Ports};
use v2rayr_platform::proxy::{ProxyTarget, TakeoverSnapshot};

use crate::App;
use crate::error;
use crate::settings::write_atomic;

/// One sentence, in both failure paths, that says what is actually missing.
const TUN_DETAIL: &str = "TUN 需要以 root 运行的 helper 创建虚拟网卡；当前 helper 只处理系统代理与自启动，TUN 尚未接通。可改用系统代理或 PAC。";

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
    api_port: Option<u16>,
    api_secret: String,
    started_at: Option<chrono::DateTime<Utc>>,
    takeover_applied: bool,
    takeover_error: Option<Problem>,
    error: Option<Problem>,
    process: Option<KernelProcess>,
    snapshot: Option<TakeoverSnapshot>,
    /// Kept alive for as long as the PAC takeover is active: dropping it stops
    /// the loopback server and the system setting would point at nothing.
    pac: Option<v2rayr_platform::PacServer>,
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
            api_port: None,
            api_secret: new_secret(),
            started_at: None,
            takeover_applied: false,
            takeover_error: None,
            error: None,
            process: None,
            snapshot: None,
            pac: None,
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
            started_at: self.started_at,
            takeover_applied: self.takeover_applied,
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
            connections: 0,
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
        self.total_down = self.total_down.saturating_add(point.down_bps);
        self.total_up = self.total_up.saturating_add(point.up_bps);
        self.samples.push_back(point);
    }
}

fn new_secret() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl App {
    fn set_phase(&self, phase: ConnectionPhase, error: Option<Problem>) {
        let mut session = self.session.lock().expect("session");
        session.phase = phase;
        session.error = error;
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
        {
            let mut session = self.session.lock().expect("session");
            session.phase = ConnectionPhase::Connecting;
            session.mode = request.mode;
            session.policy = request.policy;
            session.error = None;
            session.takeover_error = None;
            session.takeover_applied = false;
        }
        self.emit_session();

        match self.start_session(&request).await {
            Ok(()) => {
                self.logs.info("session", "已连接");
                self.emit_session();
                self.snapshot()
            }
            Err(problem) => {
                self.cleanup_after_failure().await;
                self.fail(problem.clone());
                self.emit_session();
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
        let core = v2rayr_config::choose_core(&profile, settings.core_override())
            .map_err(error::from_config)?;
        let binaries = self.cores.read().expect("cores").clone();
        let program = binaries
            .require(core)
            .map_err(error::from_engine)?
            .to_path_buf();

        let inbound_port = settings.desired_inbound_port();
        if Ports::probe(inbound_port) != PortState::Free {
            let mut problem = Problem::new(ErrorCode::PortInUse.as_str(), false).with_detail(
                format!("本地端口 {inbound_port} 已被占用；请释放该端口或修改「本地混合端口」"),
            );
            if let Some(pid) = Ports::holder(inbound_port) {
                problem = problem.with_detail(format!(
                    "本地端口 {inbound_port} 被进程 {pid} 占用；请结束它或修改「本地混合端口」"
                ));
            }
            return Err(problem);
        }

        // TUN is requested either by the access mode or by the persistent
        // setting; both mean the same thing, so refuse once with one reason.
        let tun = settings.inbound.tun_enabled || request.mode == TakeoverMode::Tun;
        if tun {
            return Err(Problem::new(ErrorCode::PermissionDenied.as_str(), false)
                .with_detail(TUN_DETAIL.to_owned()));
        }
        // The control API needs a free port of its own; the documented default is
        // 10812, drifting by one when TUN is on.
        let api_port = if tun { 10813 } else { 10812 };
        if Ports::probe(api_port) != PortState::Free {
            return Err(
                Problem::new(ErrorCode::PortInUse.as_str(), false).with_detail(format!(
                    "控制端口 {api_port} 已被占用；请关闭其他 v2rayR 实例或代理内核"
                )),
            );
        }

        let http_port = (core == CoreKind::Xray).then_some(inbound_port.saturating_add(1));
        if let Some(http_port) = http_port
            && Ports::probe(http_port) != PortState::Free
        {
            return Err(Problem::new(ErrorCode::PortInUse.as_str(), false)
                .with_detail(format!("Xray 需要额外的 HTTP 端口 {http_port}，它已被占用")));
        }

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

        let plan = SessionPlan {
            profile,
            core_override: settings.core_override(),
            inbound: InboundPlan {
                listen: if settings.inbound.allow_lan {
                    "0.0.0.0".to_owned()
                } else {
                    "127.0.0.1".to_owned()
                },
                port: inbound_port,
                http_port,
                allow_lan: settings.inbound.allow_lan,
                username: None,
                password: None,
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
            rule_sets: self.rule_sets(),
            sniffing: settings.advanced.sniffing,
            tcp_fast_open: settings.advanced.tcp_fast_open,
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
        write_atomic(&config_path, emitted.body.as_bytes())
            .map_err(|error| error::internal(format!("无法写入内核配置: {error}")))?;
        for note in &emitted.notes {
            self.logs.info("config", note.clone());
        }

        let data_dir = self.paths().data_dir();
        let checked = v2rayr_engine::check_config(core, &program, &config_path, &data_dir)
            .await
            .map_err(error::from_engine)?;
        if !checked.ok {
            return Err(error::from_engine(rejection(core, &checked.output)));
        }

        let spec = LaunchSpec::new(
            core,
            program,
            launch_arguments(core, &config_path, &data_dir),
            self.paths().core_log(core.as_str()),
        );
        let mut process = KernelProcess::launch(&spec)
            .await
            .map_err(error::from_engine)?;
        process
            .wait_ready(inbound_port, READY_TIMEOUT)
            .await
            .map_err(error::from_engine)?;

        // The kernel is up. Take over the OS only now, so a failed takeover can
        // still stop a kernel that would otherwise proxy nothing.
        let mut takeover_applied = true;
        let mut takeover_error = None;
        let mut snapshot = None;
        let mut pac = None;
        match request.mode {
            TakeoverMode::Manual => {}
            TakeoverMode::SystemProxy => {
                let mut target = ProxyTarget::all_through("127.0.0.1", inbound_port);
                let mut bypass = v2rayr_platform::proxy::default_bypass();
                bypass.extend(settings.bypass_extra());
                if !settings.inbound.bypass_local {
                    bypass.clear();
                }
                target.bypass = bypass;
                match self.proxy.apply(&target) {
                    Ok(applied) => {
                        self.persist_takeover(Some(&applied));
                        snapshot = Some(applied);
                    }
                    Err(error) => {
                        takeover_applied = false;
                        takeover_error = Some(error::from_platform(error));
                    }
                }
            }
            TakeoverMode::Pac => {
                let mut bypass = v2rayr_platform::proxy::default_bypass();
                bypass.extend(settings.bypass_extra());
                if !settings.inbound.bypass_local {
                    bypass.clear();
                }
                let script = v2rayr_platform::render_pac("127.0.0.1", inbound_port, &bypass);
                // PAC port is 10811 unless something already holds it; we never
                // silently reuse an occupied port, we just pick the next free one
                // and say so in the log.
                let preferred = 10811u16;
                let port = if v2rayr_platform::Ports::probe(preferred)
                    == v2rayr_platform::PortState::Free
                {
                    preferred
                } else {
                    let port =
                        v2rayr_platform::Ports::find_free(preferred + 1).unwrap_or(preferred + 1);
                    self.logs.warn(
                        "session",
                        format!("PAC 端口 {preferred} 已被占用，改用 {port}"),
                    );
                    port
                };
                match v2rayr_platform::PacServer::start(port, script) {
                    Ok(server) => {
                        let target = ProxyTarget::through_pac(server.url(), bypass);
                        match self.proxy.apply(&target) {
                            Ok(applied) => {
                                self.persist_takeover(Some(&applied));
                                snapshot = Some(applied);
                                pac = Some(server);
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
                takeover_applied = false;
                takeover_error = Some(
                    Problem::new(ErrorCode::PermissionDenied.as_str(), false)
                        .with_detail(TUN_DETAIL.to_owned()),
                );
            }
        }

        if !takeover_applied {
            let _ = process.stop(std::time::Duration::from_secs(2)).await;
            self.persist_takeover(None);
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
            session.api_port = Some(api_port);
            session.started_at = Some(Utc::now());
            session.takeover_applied = takeover_applied;
            session.takeover_error = takeover_error;
            session.error = None;
            session.process = Some(process);
            session.snapshot = snapshot;
            session.pac = pac;
            session.total_down = total_down;
            session.total_up = total_up;
            session.samples.clear();
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

    async fn cleanup_after_failure(&self) {
        let (process, snapshot, pac) = {
            let mut session = self.session.lock().expect("session");
            let process = session.process.take();
            let snapshot = session.snapshot.take();
            let pac = session.pac.take();
            session.inbound_port = None;
            session.api_port = None;
            session.started_at = None;
            session.takeover_applied = false;
            (process, snapshot, pac)
        };
        if let Some(snapshot) = snapshot {
            if let Err(error) = self.proxy.restore(&snapshot) {
                self.logs
                    .warn("takeover", format!("回滚系统代理失败: {error}"));
            }
        }
        self.persist_takeover(None);
        // Stop serving the PAC script only after the OS setting is restored,
        // otherwise a browser may fetch the URL while it is already gone.
        drop(pac);
        if let Some(mut process) = process {
            let _ = process.stop(std::time::Duration::from_secs(3)).await;
        }
    }

    pub async fn disconnect(&self) -> Result<AppSnapshot, Problem> {
        let _guard = self.op_lock.lock().await;
        self.set_phase(ConnectionPhase::Disconnecting, None);
        self.emit_session();

        let (process, snapshot, pac) = {
            let mut session = self.session.lock().expect("session");
            let process = session.process.take();
            let snapshot = session.snapshot.take();
            let pac = session.pac.take();
            session.phase = ConnectionPhase::Disconnecting;
            session.inbound_port = None;
            session.api_port = None;
            session.takeover_applied = false;
            session.samples.clear();
            (process, snapshot, pac)
        };

        if let Some(snapshot) = &snapshot {
            if let Err(error) = self.proxy.restore(snapshot) {
                self.logs
                    .warn("takeover", format!("恢复系统代理失败: {error}"));
            }
        }
        self.persist_takeover(None);
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
        let (mode, policy) = {
            let session = self.session.lock().expect("session");
            (session.mode, session.policy)
        };
        let _ = self.disconnect().await;
        self.connect(ConnectionRequest {
            profile_id,
            mode,
            policy,
        })
        .await
    }

    pub async fn retry(&self) -> Result<AppSnapshot, Problem> {
        let (profile_id, mode, policy) = {
            let session = self.session.lock().expect("session");
            (session.profile_id, session.mode, session.policy)
        };
        let profile_id = profile_id
            .or_else(|| self.db().active_profile())
            .ok_or_else(|| {
                Problem::new(ErrorCode::NotConnected.as_str(), false)
                    .with_detail("没有可重试的节点".to_owned())
            })?;
        self.connect(ConnectionRequest {
            profile_id,
            mode,
            policy,
        })
        .await
    }

    /// Changing mode or policy while connected requires a new config, so the
    /// session is re-established instead of pretending the old kernel adapts.
    pub async fn set_mode(&self, mode: TakeoverMode) -> Result<AppSnapshot, Problem> {
        let (profile_id, policy, phase) = {
            let session = self.session.lock().expect("session");
            (session.profile_id, session.policy, session.phase)
        };
        {
            let mut session = self.session.lock().expect("session");
            session.mode = mode;
        }
        match (phase, profile_id) {
            (ConnectionPhase::Connected, Some(profile_id)) => {
                let _ = self.disconnect().await;
                self.connect(ConnectionRequest {
                    profile_id,
                    mode,
                    policy,
                })
                .await
            }
            _ => {
                self.emit_session();
                self.snapshot()
            }
        }
    }

    pub async fn set_policy(&self, policy: RoutingPolicy) -> Result<AppSnapshot, Problem> {
        let (profile_id, mode, phase) = {
            let session = self.session.lock().expect("session");
            (session.profile_id, session.mode, session.phase)
        };
        {
            let mut session = self.session.lock().expect("session");
            session.policy = policy;
        }
        if phase == ConnectionPhase::Connected
            && let Some(profile_id) = profile_id
        {
            let _ = self.disconnect().await;
            return self
                .connect(ConnectionRequest {
                    profile_id,
                    mode,
                    policy,
                })
                .await;
        }
        self.emit_session();
        self.snapshot()
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
    fn samples_are_capped_and_totals_accumulate() {
        let mut runtime = SessionRuntime::new();
        for index in 0..(SAMPLE_CAPACITY + 5) {
            runtime.push_sample(TrafficPoint {
                ts: index as i64,
                down_bps: 10,
                up_bps: 1,
            });
        }
        assert_eq!(runtime.samples().len(), SAMPLE_CAPACITY);
        assert_eq!(runtime.total_down, 10 * (SAMPLE_CAPACITY as u64 + 5));
    }
}
