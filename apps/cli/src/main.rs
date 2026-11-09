//! `v2rayr`: the headless front end.
//!
//! It exists for three reasons: CI needs to exercise the whole stack without a
//! GUI, power users want a scriptable client, and the desktop app shares the same
//! `app` crate — so a bug reproduced here is a bug reproduced there.
//!
//! `v2rayr run` defaults to manual mode (no OS takeover): killing a CLI process
//! must never leave the machine pointing at a dead proxy port. System-proxy
//! takeover belongs to the desktop app, which has a tray and a quit path.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use tokio::time::{Duration, sleep};
use uuid::Uuid;
use v2rayr_app::text;
use v2rayr_app::{App, AppOptions, ConnectionRequest, ImportRequest, Settings};
use v2rayr_ipc::{
    ConnectionPhase, Problem, RoutingPolicy, SettingsGroupId, SettingsPatch, TakeoverMode,
};
use v2rayr_platform::AppPaths;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[derive(Parser, Debug)]
#[command(name = "v2rayr", version, about = "v2rayR command line client")]
struct Cli {
    /// Data directory (defaults to the platform location).
    #[arg(long, global = true, env = "V2RAYR_HOME")]
    home: Option<PathBuf>,
    /// Machine readable output for every command.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Connection and core status.
    Status,
    /// List stored nodes.
    Nodes,
    /// Import share links or a base64 blob (`-` reads stdin).
    Import { source: String },
    /// Subscription management.
    Sub {
        #[command(subcommand)]
        action: SubCommand,
    },
    /// Connect to a node (id or a unique remark fragment).
    Connect {
        node: String,
        #[arg(long, default_value = "manual")]
        mode: String,
        #[arg(long, default_value = "rule")]
        policy: String,
    },
    /// Tear the session down.
    Disconnect,
    /// Connect and stream traffic until interrupted.
    Run {
        node: String,
        #[arg(long, default_value = "manual")]
        mode: String,
        #[arg(long, default_value = "rule")]
        policy: String,
    },
    /// Print the share link of a node.
    Export { node: String },
    /// Kernel discovery and versions.
    Cores,
    /// Recent log lines from this process.
    Logs {
        #[arg(default_value_t = 50)]
        limit: usize,
    },
    /// Print the resolved data directory.
    Paths,
}

#[derive(Subcommand, Debug)]
enum SubCommand {
    List,
    Add {
        url: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value_t = true)]
        auto_update: bool,
    },
    Update {
        id: Option<String>,
    },
    Remove {
        id: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = match &cli.home {
        Some(root) => AppPaths::new(root.clone()),
        None => AppPaths::discover().context("resolving the data directory")?,
    };
    let app = App::open(paths.clone(), AppOptions::new(env!("CARGO_PKG_VERSION")))
        .map_err(problem_to_anyhow)?;

    match cli.command {
        Command::Status => status(&app, cli.json),
        Command::Nodes => nodes(&app, cli.json),
        Command::Import { source } => import(&app, &source, cli.json),
        Command::Sub { action } => subscriptions(&app, action, cli.json).await,
        Command::Connect { node, mode, policy } => {
            connect(&app, &node, &mode, &policy, cli.json).await
        }
        Command::Disconnect => {
            app.disconnect().await.map_err(problem_to_anyhow)?;
            println!("已断开");
            Ok(())
        }
        Command::Run { node, mode, policy } => run(&app, &node, &mode, &policy).await,
        Command::Export { node } => {
            let link = app
                .share_link(resolve_node(&app, &node)?)
                .map_err(problem_to_anyhow)?;
            println!("{link}");
            Ok(())
        }
        Command::Cores => cores(&app).await,
        Command::Logs { limit } => {
            for entry in app.tail_logs(limit) {
                println!(
                    "{} {:<5} {:<12} {}",
                    chrono::DateTime::from_timestamp_millis(entry.ts)
                        .map(|time| time.format("%H:%M:%S").to_string())
                        .unwrap_or_else(|| "-".to_owned()),
                    entry.level.as_str(),
                    entry.target,
                    entry.message
                );
            }
            Ok(())
        }
        Command::Paths => {
            println!("{}", paths.root.display());
            println!("数据库     {}", paths.database().display());
            println!("设置       {}", paths.settings().display());
            println!("内核配置   {}", paths.config_dir().display());
            println!("内核日志   {}", paths.logs_dir().display());
            Ok(())
        }
    }
}

fn status(app: &App, json: bool) -> anyhow::Result<()> {
    let snapshot = app.snapshot().map_err(problem_to_anyhow)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
        return Ok(());
    }
    let connection = &snapshot.connection;
    println!("版本      v{}", snapshot.version.app_version);
    println!("状态      {}", phase_label(connection.headline()));
    if let Some(remark) = &connection.profile_remark {
        println!("节点      {remark}");
    }
    if let Some(core) = connection.core {
        println!("内核      {}", core.as_str());
    }
    if let Some(port) = connection.inbound_port {
        println!("入站端口  {port}");
    }
    println!(
        "流量      下载 {}  上传 {}  累计 下载 {} 上传 {}",
        text::rate(snapshot.traffic.down_bps),
        text::rate(snapshot.traffic.up_bps),
        text::size(snapshot.traffic.total_down_bytes),
        text::size(snapshot.traffic.total_up_bytes),
    );
    println!("节点总数  {}", snapshot.nodes.len());
    println!("订阅      {}", snapshot.subscriptions.len());
    for core in &snapshot.cores {
        println!(
            "内核      {:<8} {}",
            core.kind.as_str(),
            core.version.clone().unwrap_or_else(|| "未安装".to_owned())
        );
    }
    for warning in &snapshot.warnings {
        println!(
            "注意      {} {}",
            warning.key,
            warning.detail.clone().unwrap_or_default()
        );
    }
    Ok(())
}

fn nodes(app: &App, json: bool) -> anyhow::Result<()> {
    let nodes = app.nodes();
    if json {
        println!("{}", serde_json::to_string_pretty(&nodes)?);
        return Ok(());
    }
    if nodes.is_empty() {
        println!("还没有节点。用 `v2rayr import <链接>` 或 `v2rayr sub add <url>` 添加。");
        return Ok(());
    }
    for node in nodes {
        println!(
            "{:<20} {:<9} {:<10} {:<8} {}",
            text::truncate(&node.remark, 20),
            node.protocol,
            format!("{}:{}", node.address, node.port),
            node.latency
                .as_ref()
                .map(|latency| format!("{} ms", latency.ms))
                .unwrap_or_else(|| "未测速".to_owned()),
            if node.favorite { "★" } else { " " }
        );
    }
    Ok(())
}

fn import(app: &App, source: &str, json: bool) -> anyhow::Result<()> {
    let raw = if source == "-" {
        let mut buffer = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buffer)?;
        buffer
    } else {
        std::fs::read_to_string(source).with_context(|| format!("reading {source}"))?
    };
    let summary = app
        .import_text(&ImportRequest {
            text: raw,
            subscription_id: None,
        })
        .map_err(problem_to_anyhow)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&summary.report)?);
        return Ok(());
    }
    println!(
        "新增 {} · 重复 {} · 失败 {} · 发现订阅 {}",
        summary.report.added.len(),
        summary.report.duplicates,
        summary.report.failures.len(),
        summary.report.subscription_urls.len()
    );
    for failure in &summary.report.failures {
        println!("  ✗ {} {}", failure.source, failure.error.key);
    }
    Ok(())
}

async fn subscriptions(app: &App, action: SubCommand, json: bool) -> anyhow::Result<()> {
    match action {
        SubCommand::List => {
            let subscriptions = app.subscriptions();
            if json {
                println!("{}", serde_json::to_string_pretty(&subscriptions)?);
                return Ok(());
            }
            if subscriptions.is_empty() {
                println!("没有订阅。");
                return Ok(());
            }
            for subscription in subscriptions {
                println!(
                    "{:<20} {:<30} 节点 {:>4}  {}",
                    text::truncate(&subscription.name, 20),
                    text::truncate(&subscription.url_host, 30),
                    subscription.node_count,
                    subscription
                        .last_error
                        .as_ref()
                        .map(|error| format!("上次失败: {}", error.key))
                        .unwrap_or_else(|| "正常".to_owned())
                );
            }
            Ok(())
        }
        SubCommand::Add {
            url,
            name,
            auto_update,
        } => {
            let (_, report) = app
                .add_and_refresh(&url, name.as_deref(), auto_update)
                .await
                .map_err(problem_to_anyhow)?;
            println!(
                "已添加：新增 {} · 更新 {} · 移除 {}",
                report.added, report.updated, report.removed
            );
            Ok(())
        }
        SubCommand::Update { id } => {
            let targets = match id {
                Some(raw) => vec![parse_uuid(&raw)?],
                None => app
                    .subscriptions()
                    .into_iter()
                    .map(|view| view.id)
                    .collect(),
            };
            for id in targets {
                let report = app.refresh_subscription(id).await;
                let line = App::describe_refresh(&report);
                println!("{} {}", text::truncate(&report.name, 20), line);
            }
            Ok(())
        }
        SubCommand::Remove { id } => {
            app.remove_subscription(parse_uuid(&id)?)
                .map_err(problem_to_anyhow)?;
            println!("已删除");
            Ok(())
        }
    }
}

async fn connect(
    app: &App,
    node: &str,
    mode: &str,
    policy: &str,
    json: bool,
) -> anyhow::Result<()> {
    let request = ConnectionRequest {
        profile_id: resolve_node(app, node)?,
        mode: parse_mode(mode)?,
        policy: parse_policy(policy)?,
    };
    match app.connect(request).await {
        Ok(snapshot) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                println!(
                    "已连接：{}",
                    snapshot.connection.profile_remark.unwrap_or_default()
                );
            }
            Ok(())
        }
        Err(problem) => {
            // The failed state is already published; show why.
            eprintln!(
                "连接失败：{} {}",
                problem.key,
                problem.detail.unwrap_or_default()
            );
            std::process::exit(2);
        }
    }
}

async fn run(app: &App, node: &str, mode: &str, policy: &str) -> anyhow::Result<()> {
    install_interrupt_handler();
    let request = ConnectionRequest {
        profile_id: resolve_node(app, node)?,
        mode: parse_mode(mode)?,
        policy: parse_policy(policy)?,
    };
    app.connect(request).await.map_err(problem_to_anyhow)?;
    println!("已连接，按 Ctrl-C 停止。");
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            break;
        }
        if let Some(traffic) = app.poll_traffic().await {
            println!(
                "↓ {}/s  ↑ {}/s  累计 {} / {}",
                text::rate(traffic.down_bps),
                text::rate(traffic.up_bps),
                text::size(traffic.total_down_bytes),
                text::size(traffic.total_up_bytes)
            );
        }
        sleep(Duration::from_secs(1)).await;
    }
    app.disconnect().await.map_err(problem_to_anyhow)?;
    println!("已断开，系统代理（若有）已恢复。");
    Ok(())
}

async fn cores(app: &App) -> anyhow::Result<()> {
    for status in app.refresh_core_versions().await {
        println!(
            "{:<10} {:<10} {}",
            status.kind.as_str(),
            if status.installed {
                "已安装"
            } else {
                "未找到"
            },
            status
                .path
                .clone()
                .or(status.last_error.map(|error| error.key))
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn resolve_node(app: &App, selector: &str) -> anyhow::Result<Uuid> {
    if let Ok(id) = Uuid::parse_str(selector)
        && app.node(id).is_some()
    {
        return Ok(id);
    }
    let nodes = app.nodes();
    let matches: Vec<_> = nodes
        .iter()
        .filter(|node| node.remark.contains(selector))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].id),
        0 => bail!("找不到匹配「{selector}」的节点"),
        _ => {
            let mut message = format!("「{selector}」匹配到 {} 个节点，请更精确：", matches.len());
            for node in matches.iter().take(8) {
                message.push_str(&format!("\n  {} {}", node.id, node.remark));
            }
            bail!(message)
        }
    }
}

fn parse_uuid(raw: &str) -> anyhow::Result<Uuid> {
    Uuid::parse_str(raw).with_context(|| format!("{raw} 不是有效 id"))
}

fn parse_mode(raw: &str) -> anyhow::Result<TakeoverMode> {
    Ok(match raw {
        "manual" => TakeoverMode::Manual,
        "system-proxy" | "system" => TakeoverMode::SystemProxy,
        "tun" => TakeoverMode::Tun,
        other => bail!("未知接入方式 {other}（manual / system-proxy / tun）"),
    })
}

fn parse_policy(raw: &str) -> anyhow::Result<RoutingPolicy> {
    Ok(match raw {
        "rule" => RoutingPolicy::Rule,
        "global" => RoutingPolicy::Global,
        "direct" => RoutingPolicy::Direct,
        other => bail!("未知分流策略 {other}（rule / global / direct）"),
    })
}

fn phase_label(phase: ConnectionPhase) -> &'static str {
    match phase {
        ConnectionPhase::Disconnected => "未连接",
        ConnectionPhase::Connecting => "连接中",
        ConnectionPhase::Connected => "已连接",
        ConnectionPhase::Disconnecting => "断开中",
        ConnectionPhase::Failed => "连接失败",
    }
}

fn problem_to_anyhow(problem: Problem) -> anyhow::Error {
    anyhow::anyhow!(
        "{} {}",
        problem.key,
        problem.detail.unwrap_or_else(|| problem.code.clone())
    )
}

/// SIGINT/SIGTERM set a flag instead of killing the process, so the session can
/// restore the system proxy before exiting.
fn install_interrupt_handler() {
    extern "C" fn handler(_signal: libc::c_int) {
        INTERRUPTED.store(true, Ordering::SeqCst);
    }
    unsafe {
        let pointer = handler as extern "C" fn(libc::c_int);
        libc::signal(libc::SIGINT, pointer as usize);
        libc::signal(libc::SIGTERM, pointer as usize);
    }
}

/// `Settings` is re-exported for future subcommands that edit groups in place.
#[allow(dead_code)]
fn apply_setting(
    app: &App,
    group: SettingsGroupId,
    values: serde_json::Value,
) -> anyhow::Result<()> {
    app.settings_patch(&SettingsPatch { group, values })
        .map_err(problem_to_anyhow)?;
    let _ = Settings::default();
    Ok(())
}
