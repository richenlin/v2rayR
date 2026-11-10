//! Starting, watching and stopping a kernel process.
//!
//! A kernel that exits within a second of starting is the classic failure mode
//! (bad config, port taken). `wait_ready` therefore watches both the control API
//! port *and* the child process, and reports the tail of the kernel log when it
//! dies — that log is the only thing that explains a rejected config.
//!
//! `std::process` is used deliberately: tokio's `process` feature drags in
//! `signal-hook-registry`, and a kernel supervisor only needs `try_wait`, which
//! is non-blocking anyway.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use v2rayr_domain::CoreKind;

use crate::{EngineError, logs};

#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub core: CoreKind,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub log_path: PathBuf,
    pub env: Vec<(String, String)>,
}

impl LaunchSpec {
    pub fn new(
        core: CoreKind,
        program: impl Into<PathBuf>,
        args: Vec<String>,
        log_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            core,
            program: program.into(),
            args,
            log_path: log_path.into(),
            env: Vec::new(),
        }
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

#[derive(Debug)]
pub struct KernelProcess {
    core: CoreKind,
    child: Child,
    log_path: PathBuf,
    started_at: Instant,
}

impl KernelProcess {
    pub async fn launch(spec: &LaunchSpec) -> Result<Self, EngineError> {
        if let Some(parent) = spec.log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&spec.log_path)?;
        let stderr = stdout.try_clone()?;

        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        let child = command.spawn().map_err(|error| EngineError::Spawn {
            core: spec.core.as_str(),
            message: error.to_string(),
        })?;
        tracing::info!(
            core = spec.core.as_str(),
            pid = child.id(),
            "kernel started"
        );
        Ok(Self {
            core: spec.core,
            child,
            log_path: spec.log_path.clone(),
            started_at: Instant::now(),
        })
    }

    pub fn core(&self) -> CoreKind {
        self.core
    }

    pub fn pid(&self) -> Option<u32> {
        Some(self.child.id())
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Wait until the control API accepts connections, or the process dies.
    pub async fn wait_ready(&mut self, port: u16, timeout: Duration) -> Result<(), EngineError> {
        let deadline = Instant::now() + timeout;
        let mut backoff = Duration::from_millis(50);
        loop {
            if let Some(error) = self.exit_error() {
                return Err(error);
            }
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                // The socket may accept before the API can answer; give the
                // kernel a beat so the first /traffic call does not race it.
                tokio::time::sleep(Duration::from_millis(80)).await;
                if let Some(error) = self.exit_error() {
                    return Err(error);
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                let tail = logs::tail(&self.log_path, 8).unwrap_or_default();
                let _ = self.stop(Duration::from_secs(2)).await;
                return Err(EngineError::StartupFailed {
                    core: self.core.as_str(),
                    message: format!(
                        "control API on port {port} did not come up within {:?}{}",
                        timeout,
                        if tail.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", tail.join(" / "))
                        }
                    ),
                });
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_millis(400));
        }
    }

    /// Non-blocking "did it die?" — `None` while it is still running.
    pub fn exit_error(&mut self) -> Option<EngineError> {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                let tail = logs::tail(&self.log_path, 8).unwrap_or_default();
                Some(EngineError::StartupFailed {
                    core: self.core.as_str(),
                    message: format!(
                        "exited with status {}{}",
                        status
                            .code()
                            .map(|code| code.to_string())
                            .unwrap_or_else(|| "signal".to_owned()),
                        if tail.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", tail.join(" / "))
                        }
                    ),
                })
            }
            Ok(None) => None,
            Err(error) => Some(EngineError::Io(error)),
        }
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn exit_code(&mut self) -> Option<i32> {
        match self.child.try_wait() {
            Ok(Some(status)) => status.code(),
            _ => None,
        }
    }

    /// SIGTERM first (the kernel flushes its cache), SIGKILL after `grace`.
    pub async fn stop(&mut self, grace: Duration) -> Result<(), EngineError> {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return Ok(());
        }
        #[cfg(unix)]
        {
            let _ = Command::new("/bin/kill")
                .arg("-TERM")
                .arg(self.child.id().to_string())
                .status();
        }
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                tracing::info!(core = self.core.as_str(), "kernel stopped");
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tracing::warn!(core = self.core.as_str(), "kernel ignored SIGTERM, killing");
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for KernelProcess {
    fn drop(&mut self) {
        // Covers launch/probe cancellation and error paths before the child
        // becomes part of SessionRuntime. Normal shutdown has already reaped it.
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn launching_a_missing_binary_reports_spawn_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = LaunchSpec::new(
            CoreKind::SingBox,
            "/nonexistent/sing-box",
            vec!["run".into()],
            dir.path().join("sing-box.log"),
        );
        let error = KernelProcess::launch(&spec).await.expect_err("must fail");
        assert!(matches!(error, EngineError::Spawn { .. }));
        assert_eq!(error.code(), v2rayr_domain::ErrorCode::CoreFailed);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_process_that_dies_immediately_is_reported_as_startup_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("killer.log");
        let spec = LaunchSpec::new(
            CoreKind::SingBox,
            "/bin/sh",
            vec!["-c".into(), "echo 'bad config' >&2; exit 1".into()],
            log_path,
        );
        let mut process = KernelProcess::launch(&spec).await.expect("launch");
        let error = process
            .wait_ready(9, Duration::from_secs(3))
            .await
            .expect_err("must fail");
        let message = error.to_string();
        assert!(message.contains("exited with status 1"), "{message}");
        assert!(message.contains("bad config"), "{message}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_terminates_a_long_running_process() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = LaunchSpec::new(
            CoreKind::SingBox,
            "/bin/sh",
            vec!["-c".into(), "sleep 30".into()],
            dir.path().join("sleep.log"),
        );
        let mut process = KernelProcess::launch(&spec).await.expect("launch");
        // `sh -c "sleep 30"` may exec sleep directly; either way it stays alive.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(process.is_alive());
        process.stop(Duration::from_secs(2)).await.expect("stop");
        assert!(!process.is_alive());
    }
}
