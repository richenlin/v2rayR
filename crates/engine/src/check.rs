//! `sing-box check` / `xray -test` / `mihomo -t`.
//!
//! The emitters validate structure; this validates *meaning* with the actual
//! kernel. It runs before we hand the config to `run`, so a config the kernel
//! rejects never becomes a "connected" state.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use v2rayr_domain::CoreKind;

use crate::EngineError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    pub ok: bool,
    pub output: String,
}

pub async fn check_config(
    kind: CoreKind,
    program: &Path,
    config: &Path,
    data_dir: &Path,
) -> Result<CheckOutcome, EngineError> {
    let program: PathBuf = program.to_path_buf();
    let config: PathBuf = config.to_path_buf();
    let data_dir: PathBuf = data_dir.to_path_buf();
    let task = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(&program);
        match kind {
            CoreKind::SingBox => {
                command.arg("check").arg("-c").arg(&config);
                command.arg("-D").arg(&data_dir);
            }
            CoreKind::Xray => {
                command.arg("-test").arg("-c").arg(&config);
            }
            CoreKind::Mihomo => {
                command.arg("-t");
                command.arg("-d").arg(&data_dir);
                command.arg("-f").arg(&config);
            }
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        command.output()
    });
    let output = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .map_err(|_| EngineError::Timeout(Duration::from_secs(20)))?
        .map_err(|error| EngineError::Io(std::io::Error::other(error.to_string())))??;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let combined = match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (true, false) => stderr,
        (false, true) => stdout,
        (false, false) => format!("{stdout}\n{stderr}"),
    };
    Ok(CheckOutcome {
        ok: output.status.success(),
        output: combined,
    })
}

/// Turn a failed check into the error the UI shows, truncated so a wall of
/// kernel output does not become a toast.
pub fn rejection(kind: CoreKind, output: &str) -> EngineError {
    let message: String = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join(" / ");
    EngineError::ConfigRejected {
        core: kind.as_str(),
        message: if message.is_empty() {
            "no output".to_owned()
        } else {
            message
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_missing_binary_surfaces_as_io_error() {
        let error = check_config(
            CoreKind::SingBox,
            Path::new("/nonexistent/sing-box"),
            Path::new("/tmp/x.json"),
            Path::new("/tmp"),
        )
        .await
        .expect_err("must fail");
        assert!(matches!(error, EngineError::Io(_)));
    }

    #[test]
    fn rejection_keeps_the_first_lines_only() {
        let output = "FATAL decode config at line 3\nsecond\nthird\nfourth\nfifth\nsixth\nseventh";
        let error = rejection(CoreKind::SingBox, output);
        let text = error.to_string();
        assert!(text.contains("line 3"));
        assert!(!text.contains("seventh"));
    }
}
