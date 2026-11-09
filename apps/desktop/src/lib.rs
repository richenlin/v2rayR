//! Tauri shell.
//!
//! Intentionally thin: every command is a three-line adapter over `v2rayr-app`,
//! and the UI talks to nothing else. That keeps the desktop build testable in
//! CI without a display server and means the CLI and the GUI cannot drift.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
use v2rayr_app::{App, AppOptions, ConnectionRequest, ImportRequest, Settings, SubscriptionQuota};
use v2rayr_ipc::{Problem, SessionRequest, SettingsPatch};
use v2rayr_platform::AppPaths;

struct Shell {
    app: Arc<App>,
    handle: AppHandle,
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
        profile_id: request.profile_id,
        mode: request.mode,
        policy: request.policy,
    };
    let snapshot = state.app.connect(request).await?;
    Ok(serde_json::to_value(snapshot).unwrap_or(Value::Null))
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
    state.app.set_tags(id, tags)
}

#[tauri::command]
fn node_share_link(state: State<'_, Shell>, id: Uuid) -> Result<String, Problem> {
    state.app.share_link(id)
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
fn settings_reset(state: State<'_, Shell>) -> Result<Value, Problem> {
    let snapshot = state.app.settings_reset()?;
    Ok(serde_json::to_value(snapshot.settings).unwrap_or(Value::Null))
}

#[tauri::command]
fn cores_status(state: State<'_, Shell>) -> Value {
    serde_json::to_value(state.app.cores_status()).unwrap_or(Value::Null)
}

#[tauri::command]
async fn cores_probe(state: State<'_, Shell>) -> Value {
    serde_json::to_value(state.app.refresh_core_versions().await).unwrap_or(Value::Null)
}

#[tauri::command]
fn cores_install(state: State<'_, Shell>, kind: v2rayr_domain::CoreKind) -> Result<Value, Problem> {
    let status = state.app.install_core(kind)?;
    Ok(serde_json::to_value(status).unwrap_or(Value::Null))
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
    let paths = AppPaths::discover().unwrap_or_else(|_| AppPaths::new("."));
    tauri::Builder::default()
        .setup(move |handle| {
            let app_paths = paths.clone();
            let events = Arc::new(TauriEvents {
                handle: handle.handle().clone(),
            });
            let app = App::open(
                app_paths,
                AppOptions::new(env!("CARGO_PKG_VERSION")).with_events(events),
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

            handle.manage(Shell {
                app,
                handle: handle.handle().clone(),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_snapshot,
            app_version,
            session_connect,
            session_disconnect,
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
            nodes_test_latency,
            subscriptions_list,
            subscription_add,
            subscription_update,
            subscription_remove,
            subscription_set_enabled,
            settings_get,
            settings_update,
            settings_reset,
            cores_status,
            cores_probe,
            cores_install,
            logs_tail,
            paths,
        ])
        .run(tauri::generate_context!())
        .expect("error while running v2rayR");
}

/// Re-exported so `main.rs` and downstream tooling can name the types without
/// depending on the app crate directly.
pub use v2rayr_app::Settings as AppSettings;
pub type ShellSettings = Settings;
pub type ShellQuota = SubscriptionQuota;
