//! Application layer.
//!
//! Everything the UI can ask for lives here, independent of Tauri: snapshots,
//! session orchestration, imports, subscription refreshes, latency tests and
//! settings. The desktop shell is a thin adapter over this crate, and the CLI is
//! a second, thinner adapter — which is how the "same engine, two front ends"
//! promise stays honest.

pub mod backup;
pub mod error;
pub mod importer;
pub mod latency;
pub mod logging;
mod pac_rules;
mod routing;
mod rules;
pub use rules::inspect_rule_applications;
pub mod session;
pub mod settings;
pub mod subscriptions;
pub mod text;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;
use v2rayr_config::{RuleKind as ConfigRuleKind, choose_core};
use v2rayr_domain::{CoreKind, Profile};
use v2rayr_ipc::{
    API_VERSION, AppSnapshot, CoreStatus, EventName, LogEntry, NodeView, Problem, QuotaView,
    SettingsPatch, SettingsView, SubscriptionView, TrafficView, VersionInfo,
};
use v2rayr_platform::{AppPaths, SystemProxy};
use v2rayr_store::{
    Database, LatencySample, Sealer, StoredProfile, StoredSubscription, open_sealer,
};

use crate::logging::LogBus;
use crate::session::SessionRuntime;

/// Process entry for the detached core guardian. Returns immediately unless
/// this process was started with `--internal-core-guardian`.
pub fn dispatch_core_guardian() {
    v2rayr_engine::guardian::dispatch();
}

use crate::settings::{CURRENT_SCHEMA, LoadedSettings, write_atomic};

/// How a UI receives state changes. The desktop shell forwards these to Tauri
/// events; the CLI ignores them; tests record them.
pub trait EventSink: Send + Sync {
    fn emit(&self, name: &str, payload: Value);
}

#[derive(Default)]
pub struct NullEvents;

impl EventSink for NullEvents {
    fn emit(&self, _name: &str, _payload: Value) {}
}

/// Records events for assertions in tests.
#[derive(Default)]
pub struct RecordingEvents {
    events: Mutex<Vec<(String, Value)>>,
}

impl RecordingEvents {
    pub fn names(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("events")
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub fn last(&self, name: &str) -> Option<Value> {
        self.events
            .lock()
            .expect("events")
            .iter()
            .rev()
            .find(|(event, _)| event == name)
            .map(|(_, payload)| payload.clone())
    }
}

impl EventSink for RecordingEvents {
    fn emit(&self, name: &str, payload: Value) {
        self.events
            .lock()
            .expect("events")
            .push((name.to_owned(), payload));
    }
}

/// Quota embedded in a subscription row (serialised as JSON in SQLite so the
/// store does not need the wire types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubscriptionQuota {
    #[serde(default)]
    pub upload: u64,
    #[serde(default)]
    pub download: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expire: Option<i64>,
}

#[derive(Clone)]
pub struct AppOptions {
    pub version: String,
    pub events: Arc<dyn EventSink>,
    pub proxy: Arc<dyn SystemProxy>,
    pub cores: Option<v2rayr_engine::CoreBinaries>,
    bundled_cores: Option<(std::path::PathBuf, std::path::PathBuf)>,
    probe_url: String,
}

impl AppOptions {
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            events: Arc::new(NullEvents),
            proxy: v2rayr_platform::proxy::system_proxy(),
            cores: None,
            bundled_cores: None,
            probe_url: "https://www.gstatic.com/generate_204".to_owned(),
        }
    }

    pub fn with_probe_url(mut self, url: impl Into<String>) -> Self {
        self.probe_url = url.into();
        self
    }

    pub fn with_events(mut self, events: Arc<dyn EventSink>) -> Self {
        self.events = events;
        self
    }

    pub fn with_proxy(mut self, proxy: Arc<dyn SystemProxy>) -> Self {
        self.proxy = proxy;
        self
    }

    pub fn with_cores(mut self, cores: v2rayr_engine::CoreBinaries) -> Self {
        self.cores = Some(cores);
        self
    }

    pub fn with_bundled_cores(
        mut self,
        binaries: std::path::PathBuf,
        xray_assets: std::path::PathBuf,
    ) -> Self {
        self.bundled_cores = Some((binaries, xray_assets));
        self
    }
}

pub struct App {
    paths: AppPaths,
    db: Mutex<Database>,
    sealer: Box<dyn Sealer>,
    settings: RwLock<Settings>,
    settings_revision: AtomicU64,
    pub(crate) session: Mutex<SessionRuntime>,
    pub(crate) op_lock: tokio::sync::Mutex<()>,
    subscription_refresh_lock: tokio::sync::Mutex<()>,
    pub(crate) events: Arc<dyn EventSink>,
    pub(crate) logs: Arc<LogBus>,
    pub(crate) proxy: Arc<dyn SystemProxy>,
    pub(crate) cores: RwLock<v2rayr_engine::CoreBinaries>,
    pub(crate) bundled_cores: Option<(std::path::PathBuf, std::path::PathBuf)>,
    warnings: Mutex<Vec<String>>,
    version: String,
    probe_url: String,
}

impl App {
    pub fn open(paths: AppPaths, options: AppOptions) -> Result<Arc<Self>, Problem> {
        paths.ensure().map_err(error::from_platform)?;
        let db = Database::open(paths.database()).map_err(error::from_store)?;
        let sealer = open_sealer(&paths.secrets_dir()).map_err(error::from_store)?;
        let LoadedSettings { settings, warnings } = Settings::load(&paths.settings());
        let cores = options.cores.unwrap_or_else(|| {
            v2rayr_engine::CoreBinaries::discover_with_bundle(
                &paths,
                options
                    .bundled_cores
                    .as_ref()
                    .map(|(binaries, _)| binaries.as_path()),
            )
        });
        let logs = LogBus::new();
        for warning in &warnings {
            logs.warn("settings", warning.clone());
        }
        let app = Arc::new(Self {
            paths: paths.clone(),
            db: Mutex::new(db),
            sealer,
            settings: RwLock::new(settings),
            settings_revision: AtomicU64::new(1),
            session: Mutex::new(SessionRuntime::new()),
            op_lock: tokio::sync::Mutex::new(()),
            subscription_refresh_lock: tokio::sync::Mutex::new(()),
            events: options.events,
            logs,
            proxy: options.proxy,
            cores: RwLock::new(cores),
            bundled_cores: options.bundled_cores,
            warnings: Mutex::new(warnings),
            version: options.version,
            probe_url: options.probe_url,
        });
        app.set_idle_policy(app.settings().policy());
        app.recover_takeover();
        Ok(app)
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub fn logs(&self) -> Arc<LogBus> {
        Arc::clone(&self.logs)
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().expect("settings").clone()
    }

    pub fn settings_view(&self) -> SettingsView {
        self.settings()
            .view(self.settings_revision.load(Ordering::Relaxed))
    }

    pub fn settings_patch(&self, patch: &SettingsPatch) -> Result<AppSnapshot, Problem> {
        let mut next = self.settings();
        next.apply(patch)?;
        if patch.group == v2rayr_ipc::SettingsGroupId::Startup {
            apply_autostart(&next)?;
        }
        next.save(&self.paths.settings())
            .map_err(|error| error::internal(format!("无法保存设置: {error}")))?;
        *self.settings.write().expect("settings") = next;
        self.settings_revision.fetch_add(1, Ordering::Relaxed);
        self.logs.info("settings", "设置已保存");
        self.emit_session();
        self.snapshot()
    }

    /// Save patches and, if a session is running, reconnect so the change is real
    /// instead of "saved but not applied".
    pub async fn save_settings_and_apply(
        &self,
        patches: &[SettingsPatch],
    ) -> Result<AppSnapshot, Problem> {
        let _guard = self.op_lock.lock().await;
        let previous = self.settings();
        let mut next = previous.clone();
        for patch in patches {
            next.apply(patch)?;
        }
        self.apply_settings_locked(previous, next).await
    }

    async fn apply_settings_locked(
        &self,
        previous: Settings,
        next: Settings,
    ) -> Result<AppSnapshot, Problem> {
        if next == previous {
            return self.snapshot();
        }
        let request = self.active_request().filter(|_| {
            next.inbound != previous.inbound
                || next.routing != previous.routing
                || next.advanced != previous.advanced
        });
        let autostart_changed = next.startup.launch_at_login != previous.startup.launch_at_login;
        if autostart_changed {
            apply_autostart(&next)?;
        }
        if let Err(problem) = self.commit_settings(next) {
            if autostart_changed {
                let _ = apply_autostart(&previous);
            }
            return Err(problem);
        }
        if let Some(request) = request {
            if let Err(problem) = self.disconnect_locked().await {
                self.restore_settings(previous)?;
                return Err(problem);
            }
            if let Err(problem) = self.connect_locked(&request).await {
                self.restore_settings(previous)?;
                // A failed takeover can leave a recovery journal. Repair it
                // before starting the previous configuration.
                if self.disconnect_locked().await.is_ok() {
                    let _ = self.connect_locked(&request).await;
                }
                self.emit_session();
                return Err(problem);
            }
        }
        self.logs.info("settings", "设置已保存");
        self.emit_session();
        self.snapshot()
    }

    fn commit_settings(&self, next: Settings) -> Result<(), Problem> {
        next.save(&self.paths.settings())
            .map_err(|error| Problem::storage(format!("无法保存设置: {error}")))?;
        *self.settings.write().expect("settings") = next;
        self.settings_revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn restore_settings(&self, previous: Settings) -> Result<(), Problem> {
        let startup = self.settings().startup.launch_at_login != previous.startup.launch_at_login;
        self.commit_settings(previous.clone())?;
        if startup {
            apply_autostart(&previous)?;
        }
        Ok(())
    }

    pub async fn settings_reset(&self) -> Result<AppSnapshot, Problem> {
        let _guard = self.op_lock.lock().await;
        self.apply_settings_locked(self.settings(), Settings::default())
            .await
    }

    /// Kernel status for the settings page. Versions are probed lazily by
    /// `refresh_core_versions`; this only reports presence.
    pub fn cores_status(&self) -> Vec<CoreStatus> {
        let cores = self.cores.read().expect("cores").clone();
        [CoreKind::SingBox, CoreKind::Xray, CoreKind::Mihomo]
            .into_iter()
            .map(|kind| {
                let path = cores.path(kind).map(|path| path.display().to_string());
                CoreStatus {
                    kind,
                    installed: path.is_some(),
                    version: None,
                    path,
                    protocols: protocol_ids(kind),
                    last_error: None,
                }
            })
            .collect()
    }

    pub async fn refresh_core_versions(&self) -> Vec<CoreStatus> {
        let cores = self.cores.read().expect("cores").clone();
        let mut statuses = Vec::new();
        for kind in [CoreKind::SingBox, CoreKind::Xray, CoreKind::Mihomo] {
            let mut status = CoreStatus {
                kind,
                installed: false,
                version: None,
                path: None,
                protocols: protocol_ids(kind),
                last_error: None,
            };
            if let Some(path) = cores.path(kind) {
                status.installed = true;
                status.path = Some(path.display().to_string());
                match v2rayr_engine::cores::probe(kind, path).await {
                    Ok(probe) => status.version = probe.version,
                    Err(error) => status.last_error = Some(error::from_engine(error)),
                }
            }
            statuses.push(status);
        }
        statuses
    }

    /// Download and install a kernel, then re-probe it.
    ///
    /// `url` and `sha256` are optional overrides; without them the pinned
    /// release in `v2rayr_engine::default_release` is used. When a digest is
    /// known it is always verified before anything is replaced.
    pub async fn install_core(
        &self,
        kind: CoreKind,
        url: Option<String>,
        sha256: Option<String>,
    ) -> Result<CoreStatus, Problem> {
        let dir = self.paths.core_bin_dir();
        let path = v2rayr_engine::download_core(kind, &dir, url.as_deref(), sha256.as_deref())
            .await
            .map_err(error::from_engine)?;
        self.logs.info(
            "cores",
            format!("{} 已安装到 {}", kind.as_str(), path.display()),
        );
        self.rescan_cores();
        // Probe the fresh binary so the caller gets a version instead of `None`;
        // otherwise the UI shows a blank version until the next rescan.
        self.refresh_core_versions()
            .await
            .into_iter()
            .find(|status| status.kind == kind)
            .ok_or_else(|| {
                Problem::new(v2rayr_domain::ErrorCode::CoreMissing.as_str(), false)
                    .with_detail(format!("{} 安装后仍不可用", kind.as_str()))
            })
    }

    /// Point the app at a user-chosen kernel binary.
    pub fn set_core_path(&self, kind: CoreKind, path: Option<std::path::PathBuf>) {
        let mut cores = self.cores.write().expect("cores");
        *cores = cores.clone().with_overrides(
            (kind == CoreKind::SingBox).then(|| path.clone()).flatten(),
            (kind == CoreKind::Xray).then(|| path.clone()).flatten(),
            (kind == CoreKind::Mihomo).then(|| path.clone()).flatten(),
        );
    }

    pub fn rescan_cores(&self) {
        let discovered = v2rayr_engine::CoreBinaries::discover_with_bundle(
            &self.paths,
            self.bundled_cores
                .as_ref()
                .map(|(binaries, _)| binaries.as_path()),
        );
        *self.cores.write().expect("cores") = discovered;
    }

    pub fn version_info(&self) -> VersionInfo {
        let db_schema = self
            .db
            .lock()
            .expect("db")
            .schema_version()
            .unwrap_or_default();
        VersionInfo {
            app_version: self.version.clone(),
            api_version: API_VERSION,
            settings_schema: CURRENT_SCHEMA,
            db_schema,
            commit: option_env!("V2RAYR_COMMIT").map(str::to_owned),
        }
    }

    pub fn nodes(&self) -> Vec<NodeView> {
        let db = self.db.lock().expect("db");
        let profiles = db
            .list_profiles(self.sealer.as_ref(), None)
            .unwrap_or_default();
        let names = self.subscription_names(&db);
        profiles
            .into_iter()
            .map(|stored| self.node_view(stored, &names))
            .collect()
    }

    pub fn node(&self, id: Uuid) -> Option<StoredProfile> {
        let db = self.db.lock().expect("db");
        db.profile(id, self.sealer.as_ref()).ok().flatten()
    }

    pub fn remove_node(&self, id: Uuid) -> Result<(), Problem> {
        let removed = self.db().delete_profile(id).map_err(error::from_store)?;
        if !removed {
            return Err(
                Problem::new(v2rayr_domain::ErrorCode::NotFound.as_str(), false)
                    .with_detail("节点不存在".to_owned()),
            );
        }
        self.logs.info("nodes", "已删除节点");
        self.emit_session();
        Ok(())
    }

    pub fn set_favorite(&self, id: Uuid, favorite: bool) -> Result<(), Problem> {
        self.db()
            .set_favorite(id, favorite)
            .map_err(error::from_store)?;
        self.emit_session();
        Ok(())
    }

    pub fn set_tags(&self, id: Uuid, tags: &[String]) -> Result<(), Problem> {
        self.db().set_tags(id, tags).map_err(error::from_store)?;
        self.emit_session();
        Ok(())
    }

    /// Share link for a stored node, for the "copy link" action. Some protocols
    /// (chains, raw custom JSON) have no stable URI form and are refused.
    pub fn share_link(&self, id: Uuid) -> Result<String, Problem> {
        let stored = self.node(id).ok_or_else(|| {
            Problem::new(v2rayr_domain::ErrorCode::NotFound.as_str(), false)
                .with_detail("节点不存在".to_owned())
        })?;
        v2rayr_domain::export_share_link(&stored.profile).ok_or_else(|| {
            Problem::new(
                v2rayr_domain::ErrorCode::UnsupportedProtocol.as_str(),
                false,
            )
            .with_detail("该节点没有可分享的链接形式".to_owned())
        })
    }

    pub fn subscriptions(&self) -> Vec<SubscriptionView> {
        let db = self.db.lock().expect("db");
        let stored = db.list_subscriptions().unwrap_or_default();
        stored
            .into_iter()
            .map(|subscription| self.subscription_view(&db, subscription))
            .collect()
    }

    pub fn snapshot(&self) -> Result<AppSnapshot, Problem> {
        let settings = self.settings();
        let (connection, traffic) = {
            let session = self.session.lock().expect("session");
            (session.connection_state(), session.traffic_view())
        };
        let active_node = connection
            .profile_id
            .and_then(|id| self.node(id))
            .map(|stored| self.node_view_with(&self.nodes(), stored));
        let nodes = self.nodes();
        let subscriptions = self.subscriptions();
        let mut warnings: Vec<Problem> = self
            .warnings
            .lock()
            .expect("warnings")
            .iter()
            .map(|warning| Problem::internal(warning.clone()))
            .collect();
        if self.cores.read().expect("cores").installed().is_empty() {
            warnings.push(
                Problem::new(v2rayr_domain::ErrorCode::CoreMissing.as_str(), false)
                    .with_detail("未找到 sing-box / xray / mihomo 内核".to_owned()),
            );
        }
        Ok(AppSnapshot {
            version: self.version_info(),
            connection,
            tun_available: v2rayr_platform::tun::provisioned(),
            tun_rule_routing: v2rayr_platform::tun::RULE_ROUTING_SUPPORTED,
            settings: settings.view(self.settings_revision.load(Ordering::Relaxed)),
            active_node,
            nodes,
            subscriptions,
            traffic,
            cores: self.cores_status(),
            warnings,
        })
    }

    /// One traffic tick: read the control API, persist totals, emit.
    pub async fn poll_traffic(&self) -> Option<TrafficView> {
        if self.check_kernel_exit().await {
            return None;
        }
        let (port, secret, probe) = {
            let mut session = self.session.lock().expect("session");
            if session.phase() != v2rayr_ipc::ConnectionPhase::Connected {
                return None;
            }
            (
                session.api_port()?,
                session.api_secret().to_owned(),
                session.traffic_probe.take()?,
            )
        };
        let counters = probe.totals().await;
        let at = std::time::Instant::now();
        let now = Utc::now();
        let guard = self.op_lock.try_lock();
        let recorded = {
            let mut session = self.session.lock().expect("session");
            let current = session.phase() == v2rayr_ipc::ConnectionPhase::Connected
                && session.api_port() == Some(port)
                && session.api_secret() == secret;
            if current {
                session.traffic_probe = Some(probe);
            }
            if guard.is_err() || !current {
                None
            } else {
                match counters {
                    Ok(counters) => session
                        .record_counters(at, now.timestamp_millis(), counters)
                        .map(|delta| {
                            let prune = session.traffic_pruned_at.is_none_or(|seen| {
                                seen.elapsed() >= std::time::Duration::from_secs(60)
                            });
                            if prune {
                                session.traffic_pruned_at = Some(at);
                            }
                            (session.traffic_view(), delta, prune)
                        }),
                    Err(error) => {
                        if !session.traffic_warned {
                            session.traffic_warned = true;
                            self.logs.warn("traffic", format!("读取流量失败: {error}"));
                        }
                        None
                    }
                }
            }
        };
        let (view, (down, up), prune) = recorded?;
        {
            let db = self.db.lock().expect("db");
            let _ = db.record_tick(
                now,
                down,
                up,
                prune.then_some(now - chrono::Duration::hours(1)),
            );
        }
        self.events.emit(EventName::TRAFFIC, json!(view));
        Some(view)
    }

    /// Total traffic counters from the store, used to seed a fresh session.
    pub fn traffic_totals(&self) -> (u64, u64) {
        self.db.lock().expect("db").totals()
    }

    pub fn tail_logs(&self, limit: usize) -> Vec<LogEntry> {
        self.logs.tail(limit)
    }

    pub fn emit_session(&self) {
        match self.snapshot() {
            Ok(snapshot) => self.events.emit(EventName::SESSION, json!(snapshot)),
            Err(error) => self
                .logs
                .error("session", format!("构建快照失败: {error:?}")),
        }
    }

    pub fn emit(&self, name: &str, payload: Value) {
        self.events.emit(name, payload);
    }

    pub fn warn(&self, warning: impl Into<String>) {
        let warning = warning.into();
        self.logs.warn("app", warning.clone());
        self.warnings.lock().expect("warnings").push(warning);
    }

    fn subscription_names(&self, db: &Database) -> Vec<(Uuid, String)> {
        db.list_subscriptions()
            .unwrap_or_default()
            .into_iter()
            .map(|subscription| (subscription.id, subscription.name))
            .collect()
    }

    fn node_view(&self, stored: StoredProfile, names: &[(Uuid, String)]) -> NodeView {
        let core = capability_core(&stored.profile);
        node_view_of(stored, names, core)
    }

    fn node_view_with(&self, _nodes: &[NodeView], stored: StoredProfile) -> NodeView {
        let db = self.db.lock().expect("db");
        let names = self.subscription_names(&db);
        let core = capability_core(&stored.profile);
        node_view_of(stored, &names, core)
    }

    fn subscription_view(
        &self,
        db: &Database,
        subscription: StoredSubscription,
    ) -> SubscriptionView {
        let node_count = db
            .subscription_profile_count(subscription.id)
            .unwrap_or_default();
        let quota = subscription
            .quota
            .as_deref()
            .and_then(|quota| serde_json::from_str::<SubscriptionQuota>(quota).ok())
            .map(|quota| QuotaView {
                upload_bytes: quota.upload,
                download_bytes: quota.download,
                total_bytes: quota.total,
                expire_at: quota
                    .expire
                    .and_then(|stamp| DateTime::<Utc>::from_timestamp(stamp, 0)),
            });
        let last_error = subscription.last_error.as_deref().and_then(|raw| {
            serde_json::from_str::<Problem>(raw)
                .ok()
                .or_else(|| Some(Problem::internal(raw.to_owned())))
        });
        SubscriptionView {
            id: subscription.id,
            name: subscription.name,
            url: subscription.url.clone(),
            url_host: url_host(&subscription.url),
            enabled: subscription.enabled,
            auto_update: subscription.auto_update,
            interval_hours: subscription.interval_hours,
            node_count,
            last_updated_at: subscription.last_updated_at,
            next_update_at: subscription.next_update_at,
            last_error,
            quota,
            format: subscription.format,
        }
    }

    fn recover_takeover(&self) {
        let path = self.paths.takeover_snapshot();
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return;
        };
        match serde_json::from_str::<v2rayr_platform::TakeoverSnapshot>(&raw) {
            Ok(snapshot) => {
                let result = if snapshot.target.is_some() {
                    self.proxy.restore(&snapshot)
                } else {
                    self.proxy.capture().and_then(|current| {
                        if snapshot.still_applied(&current.services) {
                            self.proxy.restore(&snapshot)
                        } else {
                            Ok(())
                        }
                    })
                };
                match result {
                    Ok(()) => {
                        self.clear_takeover_journal();
                        self.logs
                            .info("takeover", "已检查并恢复上次会话持有的系统代理");
                    }
                    Err(error) => {
                        self.logs
                            .warn("takeover", format!("上次会话的系统代理未能恢复: {error}"));
                        self.keep_recovery(snapshot, session::recovery_problem(error));
                    }
                }
            }
            Err(error) => {
                self.logs
                    .warn("takeover", format!("接管快照损坏，已忽略: {error}"));
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    pub(crate) fn write_takeover_journal(
        &self,
        snapshot: &v2rayr_platform::TakeoverSnapshot,
    ) -> Result<(), v2rayr_platform::PlatformError> {
        let body = serde_json::to_vec_pretty(snapshot)
            .map_err(|error| v2rayr_platform::PlatformError::InvalidState(error.to_string()))?;
        write_atomic(&self.paths.takeover_snapshot(), &body)?;
        Ok(())
    }

    pub(crate) fn clear_takeover_journal(&self) {
        let _ = std::fs::remove_file(self.paths.takeover_snapshot());
    }

    /// The profile the last session used, for "connect on launch".
    pub fn remembered_profile(&self) -> Option<Uuid> {
        self.db.lock().ok().and_then(|db| db.active_profile())
    }

    /// The takeover mode the last session used, as its stored string.
    pub fn remembered_mode(&self) -> Option<String> {
        self.db.lock().ok().and_then(|db| db.last_mode())
    }

    pub(crate) fn db(&self) -> std::sync::MutexGuard<'_, Database> {
        self.db.lock().expect("db")
    }

    pub(crate) fn sealer(&self) -> &dyn Sealer {
        self.sealer.as_ref()
    }
}

/// Which kernel a node will run on, mirroring the capability matrix. Nodes the
/// installed kernels cannot serve are still listed with their preferred kernel
/// so the UI can explain why they fail to connect.
pub fn capability_core(profile: &Profile) -> CoreKind {
    choose_core(profile, None).unwrap_or(CoreKind::SingBox)
}

pub fn node_view_of(stored: StoredProfile, names: &[(Uuid, String)], core: CoreKind) -> NodeView {
    let profile = &stored.profile;
    NodeView {
        id: stored.id,
        remark: profile.remark.clone(),
        protocol: profile.protocol_kind().to_owned(),
        transport: profile.transport_kind().to_owned(),
        address: profile.address.clone(),
        port: profile.port,
        tls: profile.tls.is_some(),
        subscription_id: stored.sub_id,
        subscription_name: stored.sub_id.and_then(|id| {
            names
                .iter()
                .find(|(key, _)| *key == id)
                .map(|(_, name)| name.clone())
        }),
        tags: stored.tags.clone(),
        favorite: stored.favorite,
        core,
        latency: stored.latency.as_ref().map(latency_view),
        last_used_at: stored.last_used_at,
    }
}

pub fn latency_view(sample: &LatencySample) -> v2rayr_ipc::LatencyView {
    v2rayr_ipc::LatencyView {
        ms: sample.ms,
        method: match sample.method.as_str() {
            "http" => v2rayr_ipc::LatencyMethod::Http,
            "through-kernel" => v2rayr_ipc::LatencyMethod::ThroughKernel,
            _ => v2rayr_ipc::LatencyMethod::Tcp,
        },
        tested_at: sample.tested_at,
    }
}

pub fn url_host(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .to_owned()
}

fn protocol_ids(kind: CoreKind) -> Vec<String> {
    match kind {
        CoreKind::SingBox => [
            "vless",
            "vmess",
            "trojan",
            "shadowsocks",
            "hysteria2",
            "tuic",
            "anytls",
            "naive",
            "wireguard",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        CoreKind::Xray => ["vless", "vmess", "trojan", "shadowsocks", "socks", "http"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        CoreKind::Mihomo => [
            "vless",
            "vmess",
            "trojan",
            "shadowsocks",
            "hysteria2",
            "tuic",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    }
}

/// Translate a user rule row into the emitter's rule kind.
pub fn config_rule_kind(kind: v2rayr_store::RuleKind) -> ConfigRuleKind {
    match kind {
        v2rayr_store::RuleKind::Domain => ConfigRuleKind::Domain,
        v2rayr_store::RuleKind::DomainSuffix => ConfigRuleKind::DomainSuffix,
        v2rayr_store::RuleKind::IpCidr => ConfigRuleKind::IpCidr,
        v2rayr_store::RuleKind::Process => ConfigRuleKind::Process,
        v2rayr_store::RuleKind::GeoIp => ConfigRuleKind::GeoIp,
        v2rayr_store::RuleKind::GeoSite => ConfigRuleKind::GeoSite,
    }
}

/// Apply the launch-at-login preference through the platform layer.
///
/// Failures are reported instead of silently flipping the switch back: the user
/// asked for a system change, and "saved but not applied" is a lie.
#[cfg_attr(test, allow(unused_variables))]
fn apply_autostart(settings: &Settings) -> Result<(), Problem> {
    #[cfg(not(test))]
    {
        let program = std::env::current_exe()
            .map_err(|error| error::internal(format!("无法定位可执行文件: {error}")))?;
        v2rayr_platform::autostart::set_login_start(settings.startup.launch_at_login, &program)
            .map_err(error::from_platform)?;
    }
    Ok(())
}

pub use importer::{ImportRequest, ImportSummary};
pub use session::ConnectionRequest;
pub use settings::Settings;
pub use subscriptions::RefreshReport;

pub(crate) fn fingerprint_for(profile: &Profile) -> String {
    v2rayr_domain::fingerprint::fingerprint(profile, v2rayr_domain::fingerprint::DEFAULT_SALT)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// A proxy implementation that records nothing and changes nothing — headless
    /// tests must never touch the developer's real network settings.
    pub struct NoopProxy;

    impl SystemProxy for NoopProxy {
        fn capture(
            &self,
        ) -> Result<v2rayr_platform::SystemProxyState, v2rayr_platform::PlatformError> {
            Ok(v2rayr_platform::SystemProxyState::default())
        }

        fn apply(
            &self,
            target: &v2rayr_platform::proxy::ProxyTarget,
        ) -> Result<v2rayr_platform::TakeoverSnapshot, v2rayr_platform::PlatformError> {
            Ok(v2rayr_platform::TakeoverSnapshot {
                services: Vec::new(),
                applied_endpoint: target.http.clone(),
                applied_pac: target.pac.clone(),
                applied_at_ms: 0,
                target: None,
                native: None,
            })
        }

        fn restore(
            &self,
            _snapshot: &v2rayr_platform::TakeoverSnapshot,
        ) -> Result<(), v2rayr_platform::PlatformError> {
            Ok(())
        }

        fn clear(
            &self,
            _endpoint: &v2rayr_platform::ProxyEndpoint,
        ) -> Result<(), v2rayr_platform::PlatformError> {
            Ok(())
        }
    }

    pub fn temp_app() -> Arc<App> {
        let root = std::env::temp_dir().join(format!(
            "v2rayr-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let paths = AppPaths::new(root);
        let options = AppOptions::new("0.0.0-test").with_proxy(Arc::new(NoopProxy));
        App::open(paths, options).expect("open test app")
    }
}
