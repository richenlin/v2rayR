//! Tauri shell.
//!
//! Intentionally thin: every command is a three-line adapter over `v2rayr-app`,
//! and the UI talks to nothing else. That keeps the desktop build testable in
//! CI without a display server and means the CLI and the GUI cannot drift.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;
use v2rayr_app::{App, AppOptions, ConnectionRequest, ImportRequest, Settings, SubscriptionQuota};
use v2rayr_ipc::{Problem, SessionRequest, SettingsPatch};
use v2rayr_platform::{AppPaths, InstanceGuard, InstanceOutcome, acquire_single_instance};

struct Shell {
    app: Arc<App>,
    /// Held for the lifetime of the process; dropping it releases the lock.
    _instance: Option<InstanceGuard>,
    quit_stage: AtomicU8,
    updating: AtomicBool,
}

/// Forwards app events to the webview. The frontend subscribes with `listen()`.
struct TauriEvents {
    handle: AppHandle,
}

impl v2rayr_app::EventSink for TauriEvents {
    fn emit(&self, name: &str, payload: Value) {
        let _ = self.handle.emit(name, payload);
    }
}

#[tauri::command]
fn app_snapshot(state: State<'_, Shell>) -> Result<Value, Problem> {
    state
        .app
        .snapshot()
        .map(|snapshot| serde_json::to_value(snapshot).unwrap_or(Value::Null))
}

#[tauri::command]
fn app_version(state: State<'_, Shell>) -> Value {
    serde_json::to_value(state.app.version_info()).unwrap_or(Value::Null)
}

#[tauri::command]
async fn session_connect(
    state: State<'_, Shell>,
    request: SessionRequest,
) -> Result<Value, Problem> {
    let request = ConnectionRequest {
        profile_id: request.profile_id.ok_or_else(|| {
            Problem::new("not_connected", false).with_detail("还没有选择节点；请先在节点页挑选一个")
        })?,
        mode: request.mode.unwrap_or(v2rayr_ipc::TakeoverMode::SystemProxy),
        policy: request.policy.unwrap_or(v2rayr_ipc::RoutingPolicy::Rule),
    };
    let snapshot = state.app.connect(request).await?;
    Ok(serde_json::to_value(snapshot).unwrap_or(Value::Null))
}

#[tauri::command]
fn session_lan_credentials(state: State<'_, Shell>) -> Result<Value, Problem> {
    state.app.reveal_lan_credentials()
}

#[tauri::command]
async fn session_disconnect(state: State<'_, Shell>) -> Result<Value, Problem> {
    let snapshot = state.app.disconnect().await?;
    Ok(serde_json::to_value(snapshot).unwrap_or(Value::Null))
}

#[tauri::command]
async fn session_switch(state: State<'_, Shell>, profile_id: Uuid) -> Result<Value, Problem> {
    let snapshot = state.app.switch(profile_id).await?;
    Ok(serde_json::to_value(snapshot).unwrap_or(Value::Null))
}

#[tauri::command]
async fn session_set_policy(
    state: State<'_, Shell>,
    policy: v2rayr_ipc::RoutingPolicy,
) -> Result<Value, Problem> {
    let snapshot = state.app.set_policy(policy).await?;
    Ok(serde_json::to_value(snapshot).unwrap_or(Value::Null))
}

#[tauri::command]
async fn session_set_mode(
    state: State<'_, Shell>,
    mode: v2rayr_ipc::TakeoverMode,
) -> Result<Value, Problem> {
    let snapshot = state.app.set_mode(mode).await?;
    Ok(serde_json::to_value(snapshot).unwrap_or(Value::Null))
}

#[tauri::command]
async fn session_retry(state: State<'_, Shell>) -> Result<Value, Problem> {
    let snapshot = state.app.retry().await?;
    Ok(serde_json::to_value(snapshot).unwrap_or(Value::Null))
}

#[tauri::command]
fn nodes_list(state: State<'_, Shell>) -> Value {
    serde_json::to_value(state.app.nodes()).unwrap_or(Value::Null)
}

#[tauri::command]
fn node_remove(state: State<'_, Shell>, id: Uuid) -> Result<(), Problem> {
    state.app.remove_node(id)
}

#[tauri::command]
fn node_set_favorite(state: State<'_, Shell>, id: Uuid, favorite: bool) -> Result<(), Problem> {
    state.app.set_favorite(id, favorite)
}

#[tauri::command]
fn node_set_tags(state: State<'_, Shell>, id: Uuid, tags: Vec<String>) -> Result<(), Problem> {
    state.app.set_tags(id, &tags)
}

#[tauri::command]
fn node_share_link(state: State<'_, Shell>, id: Uuid) -> Result<String, Problem> {
    state.app.share_link(id)
}

#[tauri::command]
async fn pac_update(state: State<'_, Shell>) -> Result<usize, Problem> {
    state.app.update_pac_rules().await
}

#[tauri::command]
fn nodes_import_text(state: State<'_, Shell>, text: String) -> Result<Value, Problem> {
    let summary = state.app.import_text(&ImportRequest {
        text,
        subscription_id: None,
    })?;
    Ok(json!({
        "report": summary.report,
        "subscriptionIds": summary.subscription_ids,
    }))
}

#[tauri::command]
async fn nodes_test_latency(state: State<'_, Shell>, ids: Vec<Uuid>) -> Result<Value, Problem> {
    let results = state.app.test_latency(&ids).await?;
    Ok(serde_json::to_value(results).unwrap_or(Value::Null))
}

#[tauri::command]
fn subscriptions_list(state: State<'_, Shell>) -> Value {
    serde_json::to_value(state.app.subscriptions()).unwrap_or(Value::Null)
}

#[tauri::command]
async fn subscription_add(
    state: State<'_, Shell>,
    url: String,
    name: Option<String>,
) -> Result<Value, Problem> {
    let (_, report) = state
        .app
        .add_and_refresh(&url, name.as_deref(), true)
        .await?;
    Ok(json!({
        "added": report.added,
        "updated": report.updated,
        "removed": report.removed,
    }))
}

#[tauri::command]
async fn subscription_update(state: State<'_, Shell>, id: Uuid) -> Result<Value, Problem> {
    let report = state.app.refresh_subscription(id).await;
    Ok(json!({
        "added": report.added,
        "updated": report.updated,
        "removed": report.removed,
        "error": report.error,
    }))
}

#[tauri::command]
fn subscription_remove(state: State<'_, Shell>, id: Uuid) -> Result<(), Problem> {
    state.app.remove_subscription(id)
}

#[tauri::command]
fn subscription_set_enabled(state: State<'_, Shell>, id: Uuid, enabled: bool) -> Result<(), Problem> {
    state.app.set_subscription_enabled(id, enabled)
}

#[tauri::command]
async fn backup_export(handle: AppHandle, state: State<'_, Shell>) -> Result<bool, Problem> {
    let Some(path) = handle.dialog().file().add_filter("v2rayR backup", &["json"])
        .set_file_name("v2rayr-backup.json").blocking_save_file() else { return Ok(false) };
    let path = path.into_path().map_err(|_| Problem::config("请选择本地文件"))?;
    state.app.export_portable_backup()?.write_new(&path)?;
    Ok(true)
}

#[tauri::command]
async fn backup_preview(handle: AppHandle) -> Result<Option<Value>, Problem> {
    let Some(path) = handle.dialog().file().add_filter("v2rayR backup", &["json"])
        .blocking_pick_file() else { return Ok(None) };
    let path = path.into_path().map_err(|_| Problem::config("请选择本地文件"))?;
    let backup = v2rayr_app::backup::PortableBackup::read(&path)?;
    Ok(Some(json!({"summary":backup.summary(),"content":backup})))
}

#[tauri::command]
async fn backup_apply(state: State<'_, Shell>, content: Value) -> Result<usize, Problem> {
    let bytes = serde_json::to_vec(&content).map_err(|_| Problem::config("无效备份"))?;
    let backup = v2rayr_app::backup::PortableBackup::decode(&bytes)?;
    state.app.import_portable_backup(backup).await
}

#[tauri::command]
async fn backup_webdav_read(url: String, username: Option<String>, password: Option<String>) -> Result<Value, Problem> {
    let (backup, _) = v2rayr_app::backup::PortableBackup::webdav_get(
        &url, username.as_deref().zip(password.as_deref()),
    ).await?;
    Ok(json!({"summary":backup.summary(),"content":backup}))
}

#[tauri::command]
async fn backup_webdav_create(state: State<'_, Shell>, url: String, username: Option<String>, password: Option<String>) -> Result<(), Problem> {
    state.app.export_portable_backup()?.webdav_put(
        &url, username.as_deref().zip(password.as_deref()), None,
    ).await
}

fn updates_configured(handle: &AppHandle) -> bool {
    handle.config().plugins.0.get("updater").is_some_and(|config| {
        config.get("requireSignedVersion").and_then(Value::as_bool) == Some(true)
        && config.get("pubkey").and_then(Value::as_str).is_some_and(|key| !key.trim().is_empty())
            && config.get("endpoints").and_then(Value::as_array).is_some_and(|endpoints| !endpoints.is_empty())
    }) && (!cfg!(target_os = "linux") || std::env::var_os("APPIMAGE").is_some())
}

fn update_error() -> Problem {
    Problem::new("app_update_failed", true).with_detail("应用更新失败：请检查网络、安装包格式或签名")
}

async fn find_app_update(handle: &AppHandle) -> Result<Option<tauri_plugin_updater::Update>, Problem> {
    if !updates_configured(handle) { return Ok(None); }
    handle.updater().map_err(|_| update_error())?.check().await.map_err(|_| update_error())
}

#[tauri::command]
fn app_updates_configured(handle: AppHandle) -> bool {
    updates_configured(&handle)
}

#[tauri::command]
async fn app_check_update(handle: AppHandle) -> Result<Option<String>, Problem> {
    Ok(find_app_update(&handle).await?.map(|update| update.version))
}

#[tauri::command]
async fn app_install_update(handle: AppHandle, state: State<'_, Shell>, expected_version: String) -> Result<(), Problem> {
    if state.updating.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return Err(Problem::new("app_update_busy", true));
    }
    struct UpdateGuard<'a>(&'a AtomicBool);
    impl Drop for UpdateGuard<'_> {
        fn drop(&mut self) { self.0.store(false, Ordering::Release); }
    }
    let _guard = UpdateGuard(&state.updating);
    let update = find_app_update(&handle).await?.filter(|update| update.version == expected_version)
        .ok_or_else(|| Problem::new("app_update_changed", true).with_detail("可用版本已变更，请重新检查后再安装"))?;
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|_| update_error())?;
    state.app.with_network_stopped(|| {
        update.install(bytes).map_err(|_| update_error())?;
        state.quit_stage.store(2, Ordering::Release);
        handle.request_restart();
        Ok(())
    }).await
}

#[tauri::command]
fn settings_get(state: State<'_, Shell>) -> Value {
    serde_json::to_value(state.app.settings_view()).unwrap_or(Value::Null)
}

#[tauri::command]
async fn settings_update(
    state: State<'_, Shell>,
    patches: Vec<SettingsPatch>,
) -> Result<Value, Problem> {
    let snapshot = state.app.save_settings_and_apply(&patches).await?;
    Ok(serde_json::to_value(snapshot.settings).unwrap_or(Value::Null))
}

#[tauri::command]
async fn settings_reset(state: State<'_, Shell>) -> Result<Value, Problem> {
    let snapshot = state.app.settings_reset().await?;
    Ok(serde_json::to_value(snapshot.settings).unwrap_or(Value::Null))
}

#[tauri::command]
fn cores_status(state: State<'_, Shell>) -> Value {
    serde_json::to_value(state.app.cores_status()).unwrap_or(Value::Null)
}

#[tauri::command]
async fn cores_probe(state: State<'_, Shell>) -> Result<Value, Problem> {
    Ok(serde_json::to_value(state.app.refresh_core_versions().await).unwrap_or(Value::Null))
}

#[tauri::command]
async fn cores_install(
    state: State<'_, Shell>,
    kind: v2rayr_domain::CoreKind,
    url: Option<String>,
    sha256: Option<String>,
) -> Result<Value, Problem> {
    let status = state.app.install_core(kind, url, sha256).await?;
    Ok(serde_json::to_value(status).unwrap_or(Value::Null))
}

#[tauri::command]
fn rules_list(state: State<'_, Shell>) -> Result<Value, Problem> {
    let rules = state.app.rules()?;
    Ok(serde_json::to_value(rules).unwrap_or(Value::Null))
}

#[tauri::command]
fn rule_add(
    state: State<'_, Shell>,
    kind: String,
    value: String,
    action: String,
) -> Result<Value, Problem> {
    let rules = state.app.add_rule(&kind, &value, &action)?;
    Ok(serde_json::to_value(rules).unwrap_or(Value::Null))
}

#[tauri::command]
fn rule_set_enabled(
    state: State<'_, Shell>,
    id: uuid::Uuid,
    enabled: bool,
) -> Result<Value, Problem> {
    let rules = state.app.set_rule_enabled(id, enabled)?;
    Ok(serde_json::to_value(rules).unwrap_or(Value::Null))
}

#[tauri::command]
fn rule_remove(state: State<'_, Shell>, id: uuid::Uuid) -> Result<Value, Problem> {
    let rules = state.app.remove_rule(id)?;
    Ok(serde_json::to_value(rules).unwrap_or(Value::Null))
}

#[tauri::command]
fn logs_tail(state: State<'_, Shell>, limit: usize) -> Value {
    serde_json::to_value(state.app.tail_logs(limit)).unwrap_or(Value::Null)
}

#[tauri::command]
fn paths(state: State<'_, Shell>) -> Value {
    let paths = state.app.paths();
    json!({
        "root": paths.root.display().to_string(),
        "database": paths.database().display().to_string(),
        "settings": paths.settings().display().to_string(),
        "logs": paths.logs_dir().display().to_string(),
        "cores": paths.core_bin_dir().display().to_string(),
    })
}

/// Entry point shared by `main.rs` and mobile targets.
pub fn run() {
    tracing::info!("starting v2rayR desktop");
    let app_paths = AppPaths::discover().unwrap_or_else(|_| AppPaths::new("."));

    // One instance per user. A second launch raises the window that is already
    // open instead of starting a second kernel and a second proxy takeover.
    let instance = match acquire_single_instance(&app_paths.runtime_dir().join("v2rayr.lock")) {
        Ok(InstanceOutcome::Primary(guard)) => Some(guard),
        Ok(InstanceOutcome::AlreadyRunning(pid)) => {
            tracing::info!(pid, "another instance is running; asking it to come forward");
            activate_running_instance();
            return;
        }
        Err(error) => {
            tracing::error!(%error, "single-instance lock unavailable");
            return;
        }
    };

    // Deep links and share links on the command line are queued for import.
    // The shell cannot know whether the UI is ready yet, so the import runs on
    // the app object and the UI picks up the result on its next snapshot.
    let clipboard = pending_imports();

    let context = tauri::generate_context!();
    let mut builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());
    if context.config().plugins.0.contains_key("updater") {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    builder.setup(move |handle| {
            let app_paths = app_paths.clone();
            let events = Arc::new(TauriEvents {
                handle: handle.handle().clone(),
            });
            let mut options = AppOptions::new(env!("CARGO_PKG_VERSION")).with_events(events);
            let executable = std::env::current_exe()?;
            if let Some(binaries) = executable.parent() {
                options = options.with_bundled_cores(
                    binaries.to_path_buf(),
                    handle.path().resource_dir()?.join("xray-assets"),
                );
            }
            let app = App::open(
                app_paths,
                options,
            )
            .map_err(|problem| Box::<dyn std::error::Error>::from(problem.to_string()))?;

            // Traffic pump: one tick per second, independent of the UI being open.
            let pump = Arc::clone(&app);
            tauri::async_runtime::spawn(async move {
                loop {
                    let _ = pump.poll_traffic().await;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            });

            let scheduler = Arc::clone(&app);
            tauri::async_runtime::spawn(async move {
                if !scheduler.settings().subscription.update_on_start {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
                loop {
                    let _ = scheduler.refresh_due_subscriptions().await;
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            });

            if updates_configured(handle.handle()) {
                let updater = handle.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(15)).await;
                    loop {
                        if let Ok(Some(update)) = find_app_update(&updater).await {
                            let _ = updater.emit("app-update-available", update.version);
                        }
                        tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
                    }
                });
            }

            if !clipboard.is_empty() {
                let importer = Arc::clone(&app);
                tauri::async_runtime::spawn(async move {
                    match importer.import_text(&ImportRequest {
                        text: clipboard,
                        subscription_id: None,
                    }) {
                        Ok(summary) => tracing::info!(added = summary.report.added.len(), "imported from command line"),
                        Err(problem) => tracing::warn!(?problem, "command-line import failed"),
                    }
                });
            }

            if app.settings().startup.show_tray_icon {
                install_tray(handle.handle())?;
            }

            // "Start minimised" means: the tray is the UI until the user asks
            // for the window, which is how the login item should behave.
            if app.settings().startup.start_minimized {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.hide();
                }
            }

            // "Connect on launch" only fires when there is a node to reconnect
            // to; a fresh install should land on the connection page instead of
            // failing at startup.
            let auto_connect = app.settings().startup.connect_on_launch;
            let app_for_launch = Arc::clone(&app);
            handle.manage(Shell {
                app,
                quit_stage: AtomicU8::new(0),
                updating: AtomicBool::new(false),
                _instance: instance,
            });
            if auto_connect {
                tauri::async_runtime::spawn(async move {
                    let remembered = app_for_launch.remembered_profile();
                    let Some(profile_id) = remembered else {
                        tracing::info!("connect-on-launch: no remembered node");
                        return;
                    };
                    let mode = app_for_launch
                        .remembered_mode()
                        .as_deref()
                        .map(|mode| match mode {
                            "pac" => v2rayr_ipc::TakeoverMode::Pac,
                            "tun" => v2rayr_ipc::TakeoverMode::Tun,
                            "manual" => v2rayr_ipc::TakeoverMode::Manual,
                            _ => v2rayr_ipc::TakeoverMode::SystemProxy,
                        })
                        .unwrap_or(v2rayr_ipc::TakeoverMode::SystemProxy);
                    let policy = app_for_launch.settings().policy();
                    if let Err(problem) = app_for_launch
                        .connect(ConnectionRequest {
                            profile_id,
                            mode,
                            policy,
                        })
                        .await
                    {
                        tracing::warn!(?problem, "connect-on-launch failed");
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the proxy running, like every other
            // menu-bar proxy: the quit action is the tray menu.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let Some(shell) = window.app_handle().try_state::<Shell>() else {
                    return;
                };
                if shell.app.settings().startup.close_to_tray && window.app_handle().tray_by_id("main").is_some() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_snapshot,
            app_version,
            session_connect,
            session_disconnect,
            session_lan_credentials,
            session_switch,
            session_set_policy,
            session_set_mode,
            session_retry,
            nodes_list,
            node_remove,
            node_set_favorite,
            node_set_tags,
            node_share_link,
            nodes_import_text,
            pac_update,
            nodes_test_latency,
            subscriptions_list,
            subscription_add,
            subscription_update,
            subscription_remove,
            subscription_set_enabled,
            backup_export,
            backup_preview,
            backup_apply,
            backup_webdav_read,
            backup_webdav_create,
            app_updates_configured,
            app_check_update,
            app_install_update,
            settings_get,
            settings_update,
            settings_reset,
            cores_status,
            cores_probe,
            cores_install,
            rules_list,
            rule_add,
            rule_set_enabled,
            rule_remove,
            logs_tail,
            paths,
        ])
        .build(context)
        .expect("error while building v2rayR")
        .run(|handle, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                let Some(shell) = handle.try_state::<Shell>() else { return };
                if shell.quit_stage.load(Ordering::Acquire) == 2 { return; }
                api.prevent_exit();
                if shell.quit_stage.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
                    return;
                }
                let app = Arc::clone(&shell.app);
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let result = app.disconnect().await;
                    let Some(shell) = handle.try_state::<Shell>() else { return };
                    match result {
                        Ok(_) => {
                            shell.quit_stage.store(2, Ordering::Release);
                            handle.exit(0);
                        }
                        Err(problem) => {
                            shell.quit_stage.store(0, Ordering::Release);
                            tracing::warn!(?problem, "cannot quit before restoring system proxy");
                            show_main(&handle);
                        }
                    }
                });
            }
        });
}


/// Bring the already-running window to the front.
///
/// macOS gives us `activate`; elsewhere there is no portable answer, so this is
/// best effort and the second launch exits quietly either way.
fn activate_running_instance() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/bin/osascript")
            .args(["-e", "tell application \"v2rayR\" to activate"])
            .status();
    }
}

/// Collect share links / `v2rayr://import/<base64>` arguments.
fn pending_imports() -> String {
    std::env::args()
        .skip(1)
        .filter_map(|arg| {
            if let Some(rest) = arg.strip_prefix("v2rayr://import/") {
                return v2rayr_domain::base64::decode_str(rest).or(Some(rest.to_owned()));
            }
            let scheme = arg.split_once("://").map(|(scheme, _)| scheme)?;
            let known = [
                "vless", "vmess", "trojan", "trojan-go", "ss", "shadowsocks", "socks", "socks5",
                "hysteria2", "hy2", "tuic", "wireguard", "wg", "anytls", "naive+https", "naive+quic",
            ];
            known.contains(&scheme).then_some(arg)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Menu-bar icon: show, connect/disconnect and quit.
fn install_tray(handle: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(handle, "show", "显示主窗口", true, None::<&str>)?;
    let connect = MenuItem::with_id(handle, "connect", "重新连接", true, None::<&str>)?;
    let disconnect = MenuItem::with_id(handle, "disconnect", "断开连接", true, None::<&str>)?;
    let quit = MenuItem::with_id(handle, "quit", "退出 v2rayR", true, None::<&str>)?;
    let menu = Menu::with_items(handle, &[&show, &connect, &disconnect, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main").menu(&menu);
    if let Some(icon) = handle.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let tray = builder
        .show_menu_on_left_click(false)
        .on_menu_event(|handle, event| match event.id().as_ref() {
            "show" => show_main(handle),
            "quit" => handle.exit(0),
            "connect" => {
                let Some(shell) = handle.try_state::<Shell>() else {
                    return;
                };
                let Some(profile) = shell.app.snapshot().ok().and_then(|s| s.active_node.map(|n| n.id))
                else {
                    tracing::info!("tray connect: no node selected yet");
                    return;
                };
                let app = Arc::clone(&shell.app);
                tauri::async_runtime::spawn(async move {
                    let stored = app.remembered_mode().unwrap_or_default();
                    let mode = match stored.as_str() {
                        "pac" => v2rayr_ipc::TakeoverMode::Pac,
                        "tun" => v2rayr_ipc::TakeoverMode::Tun,
                        "manual" => v2rayr_ipc::TakeoverMode::Manual,
                        _ => v2rayr_ipc::TakeoverMode::SystemProxy,
                    };
                    let policy = app.settings().policy();
                    let _ = app
                        .connect(ConnectionRequest {
                            profile_id: profile,
                            mode,
                            policy,
                        })
                        .await;
                });
            }
            "disconnect" => {
                let Some(shell) = handle.try_state::<Shell>() else {
                    return;
                };
                let app = Arc::clone(&shell.app);
                tauri::async_runtime::spawn(async move {
                    let _ = app.disconnect().await;
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(&tray.app_handle());
            }
        })
        .build(handle)?;

    // The tray lives as long as the app; keeping it in managed state would be
    // noise, so leak the handle deliberately.
    std::mem::forget(tray);
    Ok(())
}

fn show_main(handle: &AppHandle) {
    if let Some(window) = handle.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Re-exported so `main.rs` and downstream tooling can name the types without
/// depending on the app crate directly.
pub use v2rayr_app::Settings as AppSettings;
pub type ShellSettings = Settings;
pub type ShellQuota = SubscriptionQuota;
