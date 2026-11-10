//! Cumulative counters from the running node kernel.
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use crate::EngineError;

#[derive(Debug, Default, Clone, Copy)]
pub struct Counters {
    pub down: u64,
    pub up: u64,
    pub connections: u64,
}

pub async fn xray_totals(program: &Path, port: u16) -> Result<Counters, EngineError> {
    let program = program.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut command = Command::new(program);
        command
            .args([
                "api",
                "statsquery",
                &format!("--server=127.0.0.1:{port}"),
                "-timeout",
                "2",
                "-pattern",
                "inbound>>>",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(15))
                }
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(match result {
                        Err(error) => error.into(),
                        _ => EngineError::Timeout(Duration::from_secs(3)),
                    });
                }
            }
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(EngineError::ControlApi(
                "Xray StatsService query failed".into(),
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| EngineError::ControlApi(error.to_string()))?;
        let mut counters = Counters::default();
        if let Some(stats) = value.get("stat").and_then(serde_json::Value::as_array) {
            for stat in stats {
                let name = stat["name"].as_str().unwrap_or_default();
                let Some(("inbound", suffix)) = name.split_once(">>>") else {
                    continue;
                };
                let Some((tag, direction)) = suffix.split_once(">>>traffic>>>") else {
                    continue;
                };
                if !matches!(
                    tag,
                    "socks-in" | "http-in" | "lan-socks" | "lan-http" | "dokodemo-in"
                ) {
                    continue;
                }
                let bytes = stat["value"]
                    .as_u64()
                    .or_else(|| stat["value"].as_str()?.parse().ok())
                    .unwrap_or(0);
                match direction {
                    "uplink" => counters.up = counters.up.saturating_add(bytes),
                    "downlink" => counters.down = counters.down.saturating_add(bytes),
                    _ => (),
                }
            }
        }
        Ok(counters)
    })
    .await
    .map_err(|error| EngineError::ControlApi(error.to_string()))?
}
