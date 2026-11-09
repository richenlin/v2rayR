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

/// Translate the kernel's own words into something a person can act on.
///
/// The raw line is still shown — hiding it would make a novel failure
/// un-debuggable — but a known shape gets a hint in front of it.
pub fn explain(output: &str) -> Option<&'static str> {
    let lower = output.to_ascii_lowercase();
    let patterns: &[(&str, &str)] = &[
        (
            "address already in use",
            "端口已被其它进程占用，请换端口或关闭占用它的程序",
        ),
        (
            "permission denied",
            "权限不足：TUN 需要特权 helper，或端口小于 1024",
        ),
        (
            "operation not permitted",
            "权限不足：TUN 需要特权 helper，或端口小于 1024",
        ),
        (
            "no such file or directory",
            "引用的文件不存在：规则集、证书或内核路径可能没准备好",
        ),
        (
            "unknown field",
            "该内核不支持这个字段；切换内核或升级内核版本",
        ),
        (
            "unknown transport",
            "该内核不支持这个传输方式；VLESS + xhttp/mkcp 需要 Xray",
        ),
        (
            "failed to load rule-set",
            "规则集文件缺失或格式不对；缺少规则集时请关闭「启用内置规则集」",
        ),
        ("dns: ", "DNS 配置被拒绝：检查服务器地址与解析策略"),
        (
            "decode config",
            "配置文件无法解析：这通常是导出器的 bug，请附上配置反馈",
        ),
        ("panic", "内核崩溃；把日志附上反馈"),
    ];
    patterns
        .iter()
        .find_map(|(needle, hint)| lower.contains(needle).then_some(*hint))
}

/// Turn a failed check into the error the UI shows, truncated so a wall of
/// kernel output does not become a toast.
pub fn rejection(kind: CoreKind, output: &str) -> EngineError {
    let mut message: String = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join(" / ");
    if let Some(hint) = explain(output) {
        if message.is_empty() {
            message = format!("提示：{hint}");
        } else {
            message = format!("提示：{hint} / {message}");
        }
    }
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
    fn known_failures_get_a_hint() {
        assert!(explain("FATAL start inbound: address already in use").is_some());
        assert_eq!(explain("everything is fine"), None);
        let text = rejection(CoreKind::SingBox, "FATAL address already in use").to_string();
        assert!(text.contains("端口"));
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
