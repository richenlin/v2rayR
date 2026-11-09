//! Downloading kernel binaries.
//!
//! The design document treats kernel updates as an opt-in action that must
//! verify a SHA256 when one is provided, never replace a working binary with a
//! partially downloaded one, and say where it installed what. That is exactly
//! what this module does: download to `.part`, verify, extract, move, chmod.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use v2rayr_domain::CoreKind;

use crate::EngineError;

/// A known-good download for one kernel.
#[derive(Debug, Clone)]
pub struct CoreRelease {
    pub kind: CoreKind,
    pub version: &'static str,
    pub url: String,
    /// Present when we know the archive digest; verification is mandatory then.
    pub sha256: Option<&'static str>,
}

fn platform_slug() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-amd64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "x86_64") => "linux-amd64",
        _ => "darwin-arm64",
    }
}

/// Default releases. Versions are pinned; the UI lets the user paste a newer URL.
pub fn default_release(kind: CoreKind) -> Option<CoreRelease> {
    let slug = platform_slug();
    match kind {
        CoreKind::SingBox => {
            let version = "1.11.15";
            Some(CoreRelease {
                kind,
                version,
                url: format!(
                    "https://github.com/SagerNet/sing-box/releases/download/v{version}/sing-box-{version}-{slug}.tar.gz"
                ),
                sha256: None,
            })
        }
        CoreKind::Xray => {
            let version = "1.8.24";
            let arch = if slug.ends_with("arm64") {
                "arm64-v8a"
            } else {
                "64"
            };
            Some(CoreRelease {
                kind,
                version,
                url: format!(
                    "https://github.com/XTLS/Xray-core/releases/download/v{version}/Xray-macos-{arch}.zip"
                ),
                sha256: None,
            })
        }
        CoreKind::Mihomo => {
            let version = "1.19.9";
            Some(CoreRelease {
                kind,
                version,
                url: format!(
                    "https://github.com/MetaCubeX/mihomo/releases/download/v{version}/mihomo-{slug}-v{version}.gz"
                ),
                sha256: None,
            })
        }
    }
}

/// Download, verify and install `release` into `dir`.
///
/// Returns the path of the installed binary. Existing binaries are only replaced
/// after the new one has been extracted and made executable.
pub async fn download_core(
    kind: CoreKind,
    dir: &Path,
    url: Option<&str>,
    sha256: Option<&str>,
) -> Result<PathBuf, EngineError> {
    let release = default_release(kind);
    let url = match (url, &release) {
        (Some(url), _) => url.to_owned(),
        (None, Some(release)) => release.url.clone(),
        (None, None) => {
            return Err(EngineError::MissingCore {
                core: kind.as_str(),
            });
        }
    };
    let expected = sha256.map(str::to_owned).or_else(|| {
        release
            .as_ref()
            .and_then(|release| release.sha256.map(str::to_owned))
    });

    std::fs::create_dir_all(dir)?;
    let archive_path = dir.join(format!(".{}.download", kind.as_str()));

    let client = reqwest::Client::builder()
        .user_agent(concat!("v2rayR/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|error| EngineError::Network(error.to_string()))?;
    let response = client
        .get(&url)
        .header("Accept-Encoding", "identity")
        .send()
        .await
        .map_err(|error| EngineError::Network(format!("下载失败：{error}")))?;
    if !response.status().is_success() {
        return Err(EngineError::Network(format!(
            "下载失败：HTTP {}",
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| EngineError::Network(format!("下载中断：{error}")))?;
    std::fs::write(&archive_path, &bytes)?;

    if let Some(expected) = &expected {
        let digest = hex::encode(Sha256::digest(&bytes));
        if !digest.eq_ignore_ascii_case(expected.trim()) {
            let _ = std::fs::remove_file(&archive_path);
            return Err(EngineError::StartupFailed {
                core: kind.as_str(),
                message: format!("SHA256 校验失败：期望 {expected}，实际 {digest}"),
            });
        }
    } else {
        tracing::warn!(
            target: "engine",
            core = kind.as_str(),
            "该内核没有内置 SHA256，跳过校验；如来源可信度存疑请自行核对"
        );
    }

    let stage = dir.join(format!(".{}.unpack", kind.as_str()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;
    extract(&archive_path, &stage, kind)?;
    let binary = find_binary(&stage, kind).ok_or_else(|| EngineError::StartupFailed {
        core: kind.as_str(),
        message: format!("压缩包里没有找到 {} 可执行文件", kind.as_str()),
    })?;
    let target = dir.join(kind.as_str());
    let staged_target = dir.join(format!(".{}.new", kind.as_str()));
    std::fs::copy(&binary, &staged_target)?;
    make_executable(&staged_target)?;
    std::fs::rename(&staged_target, &target)?;
    if let Ok(output) = std::process::Command::new(&target).arg("version").output() {
        tracing::info!(
            target: "engine",
            core = kind.as_str(),
            banner = %String::from_utf8_lossy(&output.stdout).lines().next().unwrap_or(""),
            "内核安装完成"
        );
    }
    let _ = std::fs::remove_file(&archive_path);
    let _ = std::fs::remove_dir_all(&stage);
    Ok(target)
}

fn extract(archive: &Path, into: &Path, kind: CoreKind) -> Result<(), EngineError> {
    let name = archive.to_string_lossy().to_ascii_lowercase();
    let status = if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        std::process::Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(into)
            .status()?
    } else if name.ends_with(".zip") {
        std::process::Command::new("unzip")
            .arg("-o")
            .arg("-q")
            .arg(archive)
            .arg("-d")
            .arg(into)
            .status()?
    } else if name.ends_with(".gz") {
        let output = std::process::Command::new("gzip")
            .arg("-dc")
            .arg(archive)
            .output()?;
        if !output.status.success() {
            return Err(EngineError::StartupFailed {
                core: kind.as_str(),
                message: format!(
                    "解压失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        let file = into.join("payload");
        std::fs::write(&file, output.stdout)?;
        return Ok(());
    } else {
        return Err(EngineError::StartupFailed {
            core: kind.as_str(),
            message: "不支持的内核压缩格式（支持 .tar.gz / .zip / .gz）".to_owned(),
        });
    };
    if !status.success() {
        return Err(EngineError::StartupFailed {
            core: kind.as_str(),
            message: "解压内核失败".to_owned(),
        });
    }
    Ok(())
}

fn find_binary(root: &Path, kind: CoreKind) -> Option<PathBuf> {
    let wanted = kind.as_str();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|name| name.to_str()) == Some(wanted) {
                return Some(path);
            }
        }
    }
    // Single-binary archives are stored as `payload`.
    let payload = root.join("payload");
    payload.is_file().then_some(payload)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), EngineError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), EngineError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_are_https_and_named_after_the_kernel() {
        for kind in [CoreKind::SingBox, CoreKind::Xray, CoreKind::Mihomo] {
            let release = default_release(kind).expect("release");
            assert!(release.url.starts_with("https://"));
            assert!(
                release.url.contains(kind.as_str())
                    || kind == CoreKind::Xray && release.url.contains("Xray")
            );
        }
    }

    #[test]
    fn executable_bit_is_set() {
        let dir = tempfile::tempdir().expect("dir");
        let file = dir.path().join("bin");
        std::fs::write(&file, b"#!/bin/sh\n").expect("write");
        make_executable(&file).expect("chmod");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }
}
