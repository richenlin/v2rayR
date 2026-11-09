//! `v2rayr-helper`: the only binary that may touch privileged system state.
//!
//! Every invocation is authenticated with a per-install token (mode 0600) and
//! prints exactly one JSON `HelperResponse` on stdout, so the caller never has to
//! parse human text. Exit codes: 0 success, 64 usage, 77 unauthorized, 70 failure.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use v2rayr_helper::authorize::{Token, default_token_path};
use v2rayr_helper::ops::execute;
use v2rayr_helper::protocol::{HelperRequest, HelperResponse, ProxySpec};
use v2rayr_platform::autostart::LaunchAgentAutostart;
use v2rayr_platform::proxy::MacOsSystemProxy;

#[derive(Parser, Debug)]
#[command(name = "v2rayr-helper", version, about = "v2rayR privileged helper")]
struct Cli {
    /// Token file created by the app.
    #[arg(long, env = "V2RAYR_HELPER_TOKEN_FILE", global = true)]
    token_file: Option<PathBuf>,
    /// Token value, for scripted use.
    #[arg(long, env = "V2RAYR_HELPER_TOKEN", global = true)]
    token: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Point the system proxy at a local inbound.
    ApplyProxy {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long)]
        port: u16,
        #[arg(long)]
        socks: bool,
        #[arg(long, value_delimiter = ',')]
        bypass: Vec<String>,
    },
    /// Restore a snapshot printed by a previous `apply-proxy`.
    RestoreProxy {
        #[arg(long)]
        snapshot_file: PathBuf,
    },
    /// Remove the proxy, but only if it still points at this endpoint.
    ClearProxy {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long)]
        port: u16,
    },
    /// Launch-at-login via a user LaunchAgent.
    Autostart {
        #[command(subcommand)]
        action: AutostartAction,
    },
    /// Report what the helper sees.
    Status,
    /// Read newline-delimited JSON requests from stdin and answer each one.
    Serve,
}

#[derive(Subcommand, Debug)]
enum AutostartAction {
    Enable {
        #[arg(long)]
        program: PathBuf,
    },
    Disable,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let token = load_token(&cli)?;
    let proxy = MacOsSystemProxy::system();
    let autostart = LaunchAgentAutostart::system()?;

    if let Command::Serve = cli.command {
        return serve(&token, &proxy, &autostart);
    }

    let request = into_request(cli.command)?;
    let response = execute(&request, &proxy, &autostart);
    print_response(&response)
}

fn load_token(cli: &Cli) -> anyhow::Result<Token> {
    let path = cli
        .token_file
        .clone()
        .or_else(|| {
            std::env::var_os("V2RAYR_HOME")
                .map(|root| default_token_path(&PathBuf::from(root).join("runtime")))
        })
        .context("either --token-file or V2RAYR_HOME must be provided")?;
    let stored = Token::load(&path).with_context(|| format!("loading {}", path.display()))?;
    if let Some(candidate) = &cli.token
        && !stored.matches(candidate)
    {
        let response = HelperResponse::error("permission_denied", "helper token mismatch");
        println!("{}", serde_json::to_string(&response)?);
        std::process::exit(77);
    }
    Ok(stored)
}

fn into_request(command: Command) -> anyhow::Result<HelperRequest> {
    Ok(match command {
        Command::ApplyProxy {
            host,
            port,
            socks,
            bypass,
        } => HelperRequest::ProxyApply {
            proxy: ProxySpec {
                host,
                port,
                bypass,
                socks,
            },
        },
        Command::RestoreProxy { snapshot_file } => {
            let raw = std::fs::read_to_string(&snapshot_file)
                .with_context(|| format!("reading {}", snapshot_file.display()))?;
            HelperRequest::ProxyRestore {
                snapshot: serde_json::from_str(&raw).context("parsing snapshot")?,
            }
        }
        Command::ClearProxy { host, port } => HelperRequest::ProxyClear { host, port },
        Command::Autostart { action } => match action {
            AutostartAction::Enable { program } => HelperRequest::AutostartEnable {
                program: program.display().to_string(),
            },
            AutostartAction::Disable => HelperRequest::AutostartDisable,
        },
        Command::Status => HelperRequest::Status,
        Command::Serve => bail!("serve is handled separately"),
    })
}

fn serve(
    token: &Token,
    proxy: &MacOsSystemProxy,
    autostart: &LaunchAgentAutostart,
) -> anyhow::Result<()> {
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer)?;
    let mut out = std::io::stdout();
    for line in buffer.lines().filter(|line| !line.trim().is_empty()) {
        let response = match serde_json::from_str::<HelperRequest>(line) {
            Ok(request) => execute(&request, proxy, autostart),
            Err(error) => HelperResponse::error("invalid_value", error.to_string()),
        };
        serde_json::to_writer(&mut out, &response)?;
        out.write_all(b"\n")?;
        out.flush()?;
    }
    let _ = token;
    Ok(())
}

fn print_response(response: &HelperResponse) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(response)?);
    match response {
        HelperResponse::Ok { .. } => Ok(()),
        HelperResponse::Error { .. } => std::process::exit(70),
    }
}
