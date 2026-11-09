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

// Versions are pinned to releases we have actually run. Xray must be 25.9+ for
// VLESS Encryption (`mlkem768x25519plus.*`), which is why 1.8.x is not an option.
const SINGBOX_VERSION: &str = "1.14.4";
const XRAY_VERSION: &str = "26.11.9";
const MIHOMO_VERSION: &str = "1.19.33";

/// Digests we verified ourselves, for the platform this repository is developed
/// and tested on. Elsewhere the release is still HTTPS-pinned but unchecked, and
/// `download_core` logs that instead of pretending it verified anything.
fn pinned_sha(kind: CoreKind) -> Option<&'static str> {
    if !(std::env::consts::OS == "macos" && std::env::consts::ARCH == "aarch64") {
        return None;
    }
    match kind {
        CoreKind::SingBox => Some(SINGBOX_SHA256),
        CoreKind::Xray => Some(XRAY_SHA256),
        CoreKind::Mihomo => Some(MIHOMO_SHA256),
    }
}

const XRAY_SHA256: &str = "233f0c66681cd0726f81a6b2b6ed6a5710869e312047b522e46ed53035bebd26";
const SINGBOX_SHA256: &str = "0d8c7f5ed5d587b0f1a5e9772a708ec3bfa4ac4c9329604b01d8d9c2ba406838";
const MIHOMO_SHA256: &str = "060b64b0f2c9a310831db0fe4adb2878de67bfa606c311813c1f13966aa504b6";

/// Default releases. Versions are pinned; the UI lets the user paste a newer URL.
pub fn default_release(kind: CoreKind) -> Option<CoreRelease> {
    let slug = platform_slug();
    match kind {
        CoreKind::SingBox => {
            let version = SINGBOX_VERSION;
            Some(CoreRelease {
                kind,
                version,
                url: format!(
                    "https://github.com/SagerNet/sing-box/releases/download/v{version}/sing-box-{version}-{slug}.tar.gz"
                ),
                sha256: pinned_sha(kind),
            })
        }
        CoreKind::Xray => {
            let version = XRAY_VERSION;
            let (os, arch) = match (std::env::consts::OS, std::env::consts::ARCH) {
                ("linux", "aarch64") => ("linux", "arm64-v8a"),
                ("linux", _) => ("linux", "64"),
                ("windows", _) => ("windows", "64"),
                (_, "aarch64") => ("macos", "arm64-v8a"),
                _ => ("macos", "64"),
            };
            Some(CoreRelease {
                kind,
                version,
                url: format!(
                    "https://github.com/XTLS/Xray-core/releases/download/v{version}/Xray-{os}-{arch}.zip"
                ),
                sha256: pinned_sha(kind),
            })
        }
        CoreKind::Mihomo => {
            let version = MIHOMO_VERSION;
            Some(CoreRelease {
                kind,
                version,
                url: format!(
                    "https://github.com/MetaCubeX/mihomo/releases/download/v{version}/mihomo-{slug}-v{version}.gz"
                ),
                sha256: pinned_sha(kind),
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
    // Keep the URL's extension: `extract` picks the tool from the file name, so a
    // bare `.download` file would look like an unsupported archive.
    let archive_path = dir.join(format!(
        ".{}.download{}",
        kind.as_str(),
        archive_suffix(&url)
    ));

    let bytes = fetch(&url).await?;
    std::fs::write(&archive_path, &bytes)?;

    if let Some(expected) = &expected {
        if let Err(message) = verify_digest(&bytes, expected) {
            let _ = std::fs::remove_file(&archive_path);
            return Err(EngineError::StartupFailed {
                core: kind.as_str(),
                message,
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
    install_geodata(&stage, &binary, dir);
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

/// Download the release bytes.
///
/// reqwest is the normal path, but on some networks the redirect target
/// (`release-assets.githubusercontent.com`) only answers HTTP/1.1 clients with
/// the system trust store, so a `curl` retry keeps kernel installation usable
/// instead of failing with a bare "error sending request".
async fn fetch(url: &str) -> Result<Vec<u8>, EngineError> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("v2rayR/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|error| EngineError::Network(error.to_string()))?;
    let direct = async {
        let response = client
            .get(url)
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
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| EngineError::Network(format!("下载中断：{error}")))
    }
    .await;
    match direct {
        Ok(bytes) => Ok(bytes),
        Err(error) => {
            tracing::warn!(target: "engine", %error, "内置下载失败，改用系统 curl 重试");
            fetch_with_curl(url).map_err(|fallback| {
                EngineError::Network(format!("{error}；curl 重试也失败：{fallback}"))
            })
        }
    }
}

fn fetch_with_curl(url: &str) -> Result<Vec<u8>, String> {
    let output = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "300", url])
        .output()
        .map_err(|error| format!("无法执行 curl：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "curl 退出码 {:?}：{}",
            output.status.code(),
            stderr.trim().lines().next().unwrap_or("无输出")
        ));
    }
    if output.stdout.is_empty() {
        return Err("curl 返回了空响应".to_owned());
    }
    Ok(output.stdout)
}

fn verify_digest(bytes: &[u8], expected: &str) -> Result<(), String> {
    let digest = hex::encode(Sha256::digest(bytes));
    if digest.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(format!("SHA256 校验失败：期望 {expected}，实际 {digest}"))
    }
}

fn archive_suffix(url: &str) -> &'static str {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    if path.ends_with(".tar.gz") {
        ".tar.gz"
    } else if path.ends_with(".tgz") {
        ".tgz"
    } else if path.ends_with(".zip") {
        ".zip"
    } else if path.ends_with(".gz") {
        ".gz"
    } else {
        ""
    }
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

/// Xray refuses to start routing rules without `geoip.dat` / `geosite.dat`, and
/// ships both in its release archive. Copy them next to the binary so an
/// installed kernel is actually runnable, instead of failing later with
/// "failed to open geoip.dat".
fn install_geodata(stage: &Path, binary: &Path, dir: &Path) {
    let Some(root) = binary.parent() else {
        return;
    };
    for name in ["geoip.dat", "geosite.dat"] {
        let source = [
            root.join(name),
            stage.join(name),
            stage.join("payload").join(name),
        ]
        .into_iter()
        .find(|path| path.is_file());
        if let Some(source) = source {
            let target = dir.join(name);
            match std::fs::copy(&source, &target) {
                Ok(_) => tracing::info!(target: "engine", file = name, "已安装内核数据文件"),
                Err(error) => {
                    tracing::warn!(target: "engine", file = name, %error, "内核数据文件复制失败")
                }
            }
        }
    }
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
    fn a_mismatched_digest_is_refused() {
        let bytes = b"kernel";
        let good = hex::encode(Sha256::digest(bytes));
        assert!(verify_digest(bytes, &good).is_ok());
        assert!(verify_digest(bytes, &good.to_uppercase()).is_ok());
        let error = verify_digest(bytes, &"0".repeat(64)).expect_err("must fail");
        assert!(error.contains("SHA256"));
    }

    #[test]
    fn the_archive_keeps_the_url_extension() {
        assert_eq!(archive_suffix("https://x/Xray-macos-arm64-v8a.zip"), ".zip");
        assert_eq!(
            archive_suffix("https://x/sing-box-1.11.15-darwin-arm64.tar.gz?download=1"),
            ".tar.gz"
        );
        assert_eq!(archive_suffix("https://x/mihomo.gz#frag"), ".gz");
        assert_eq!(archive_suffix("https://x/binary"), "");
    }

    #[test]
    fn geodata_next_to_the_binary_is_installed() {
        let stage = tempfile::tempdir().expect("stage");
        let dir = tempfile::tempdir().expect("dir");
        let nested = stage.path().join("Xray-macos-arm64-v8a");
        std::fs::create_dir_all(&nested).expect("mkdir");
        let binary = nested.join("xray");
        std::fs::write(&binary, b"#!/bin/sh\n").expect("binary");
        std::fs::write(nested.join("geoip.dat"), b"geo").expect("geoip");
        std::fs::write(nested.join("geosite.dat"), b"geosite").expect("geosite");

        install_geodata(stage.path(), &binary, dir.path());

        assert!(dir.path().join("geoip.dat").is_file());
        assert!(dir.path().join("geosite.dat").is_file());
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
