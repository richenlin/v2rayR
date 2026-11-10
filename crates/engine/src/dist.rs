//! Downloading kernel binaries.
//!
//! The design document treats kernel updates as an opt-in action that must
//! verify the pinned or user-provided SHA-256, never replace a working binary with a
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

const SINGBOX_VERSION: &str = "1.14.4";
const XRAY_VERSION: &str = "26.11.9";
const MIHOMO_VERSION: &str = "1.19.33";

/// Official assets and SHA-256 digests for the supported OS/architecture pairs.
pub fn default_release(kind: CoreKind) -> Option<CoreRelease> {
    release_for(kind, std::env::consts::OS, std::env::consts::ARCH)
}

fn release_for(kind: CoreKind, os: &str, arch: &str) -> Option<CoreRelease> {
    let (slug, xray, extension, digests) = match (os, arch) {
        ("macos", "aarch64") => (
            "darwin-arm64",
            "macos-arm64-v8a",
            ["tar.gz", "gz"],
            [
                "0d8c7f5ed5d587b0f1a5e9772a708ec3bfa4ac4c9329604b01d8d9c2ba406838",
                "233f0c66681cd0726f81a6b2b6ed6a5710869e312047b522e46ed53035bebd26",
                "060b64b0f2c9a310831db0fe4adb2878de67bfa606c311813c1f13966aa504b6",
            ],
        ),
        ("macos", "x86_64") => (
            "darwin-amd64",
            "macos-64",
            ["tar.gz", "gz"],
            [
                "eae86b9b033bba50b7c44ab719e4bfac0aca05efd28483390ed3aa510f32db25",
                "488963e676e06f5e48547f9dd38116aaca8de4234959b4272ae8f284e658dfd0",
                "551ae54ef8d77a5e21f7a8185a59b441fb13a0fb0514fcdef5ca2816f6f50015",
            ],
        ),
        ("linux", "x86_64") => (
            "linux-amd64",
            "linux-64",
            ["tar.gz", "gz"],
            [
                "699a1b510aa4396b2176d32d1181920399ebf1e267835da6f3b598d0f7c24968",
                "b651f7e1d9966ee2c1b1ee1bf8b52c72771824d3065dce96bbc8bcc19ed95f82",
                "132f6f3edf9a65e62f04853aed3358472e0b2b7666daa8419edfa2ddf5bb6323",
            ],
        ),
        ("linux", "aarch64") => (
            "linux-arm64",
            "linux-arm64-v8a",
            ["tar.gz", "gz"],
            [
                "436441f76ee2f7fe21b5b2d426d3cb261a480455a704865789ad533e4273ec26",
                "64ed6858982a7f5eb85d88b811089063bd4f17561f4c8f44e0f1dc8e6f6430be",
                "ba408b317080ee265c46cb923a50018c1b82cbd708551cdd46a1e3e307396187",
            ],
        ),
        ("windows", "x86_64") => (
            "windows-amd64",
            "windows-64",
            ["zip", "zip"],
            [
                "99185d75d314a07dbba4a130c8ad986cbeb629a07927975636b341ca61f991df",
                "1262522266337b824f07beb8e1375786b437683b3890105469088f8e17e0a69d",
                "6ab2180875f9f6fff8b19386d760daec257f66b45f72b3afc650634272fa837a",
            ],
        ),
        _ => return None,
    };
    let (version, repo, asset, digest) = match kind {
        CoreKind::SingBox => (
            SINGBOX_VERSION,
            "SagerNet/sing-box",
            format!("sing-box-{SINGBOX_VERSION}-{slug}.{}", extension[0]),
            digests[0],
        ),
        CoreKind::Xray => (
            XRAY_VERSION,
            "XTLS/Xray-core",
            format!("Xray-{xray}.zip"),
            digests[1],
        ),
        CoreKind::Mihomo => (
            MIHOMO_VERSION,
            "MetaCubeX/mihomo",
            format!("mihomo-{slug}-v{MIHOMO_VERSION}.{}", extension[1]),
            digests[2],
        ),
    };
    Some(CoreRelease {
        kind,
        version,
        url: format!("https://github.com/{repo}/releases/download/v{version}/{asset}"),
        sha256: Some(digest),
    })
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
    if !reqwest::Url::parse(&url).is_ok_and(|parsed| {
        parsed.scheme() == "https"
            && parsed.host().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
    }) {
        return Err(EngineError::StartupFailed {
            core: kind.as_str(),
            message: "内核下载地址必须是无账号密码的 HTTPS 地址".into(),
        });
    }
    let expected = release
        .as_ref()
        .filter(|release| release.url == url)
        .and_then(|release| release.sha256)
        .or(sha256)
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| EngineError::StartupFailed {
            core: kind.as_str(),
            message: "自定义内核地址必须提供有效的 SHA-256".into(),
        })?;
    std::fs::create_dir_all(dir)?;
    let stage = tempfile::tempdir_in(dir)?;
    let archive_path = stage
        .path()
        .join(format!("download{}", archive_suffix(&url)));
    let official = release.as_ref().is_some_and(|release| release.url == url);
    let bytes = fetch(&url, official).await?;
    verify_digest(&bytes, expected).map_err(|message| EngineError::StartupFailed {
        core: kind.as_str(),
        message,
    })?;
    std::fs::write(&archive_path, bytes)?;
    let unpack = stage.path().join("unpack");
    std::fs::create_dir(&unpack)?;
    extract(&archive_path, &unpack, kind)?;
    let binary = find_binary(&unpack, kind).ok_or_else(|| EngineError::StartupFailed {
        core: kind.as_str(),
        message: format!("压缩包里没有找到 {} 可执行文件", kind.as_str()),
    })?;
    let target = dir.join(crate::cores::binary_name(kind.as_str()));
    let staged_target = stage.path().join(crate::cores::binary_name(kind.as_str()));
    std::fs::copy(&binary, &staged_target)?;
    make_executable(&staged_target)?;
    std::fs::rename(&staged_target, &target)?;
    install_geodata(&unpack, &binary, dir);
    if let Some(license) = binary.parent().map(|path| path.join("LICENSE"))
        && license.is_file()
    {
        std::fs::copy(license, dir.join(format!("{}-LICENSE", kind.as_str())))?;
    }
    Ok(target)
}

/// Download the release bytes.
///
/// reqwest is the normal path, but on some networks the redirect target
/// (`release-assets.githubusercontent.com`) only answers HTTP/1.1 clients with
/// the system trust store, so a `curl` retry keeps kernel installation usable
/// instead of failing with a bare "error sending request".
async fn fetch(url: &str, official: bool) -> Result<Vec<u8>, EngineError> {
    // The OS-wide proxy can be an older instance or an unreachable listener.
    // On macOS, use the system downloader first for pinned releases: it uses
    // shell proxy configuration and does not inherit that OS-wide proxy.
    let first_curl_error = if cfg!(target_os = "macos") && official {
        match run_curl(url).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => Some(error),
        }
    } else {
        None
    };
    let client = reqwest::Client::builder()
        .user_agent(concat!("v2rayR/", env!("CARGO_PKG_VERSION")))
        // An unreachable system proxy should not hold a first-run install
        // until the full archive download deadline before trying curl.
        .connect_timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_secs(15))
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
            if let Some(fallback) = first_curl_error {
                return Err(EngineError::Network(format!(
                    "{error}；系统 curl 也失败：{fallback}"
                )));
            }
            tracing::warn!(target: "engine", %error, "内置下载失败，改用系统 curl 重试");
            run_curl(url).await.map_err(|fallback| {
                EngineError::Network(format!("{error}；curl 重试也失败：{fallback}"))
            })
        }
    }
}

async fn run_curl(url: &str) -> Result<Vec<u8>, String> {
    let url = url.to_owned();
    tokio::task::spawn_blocking(move || fetch_with_curl(&url))
        .await
        .map_err(|error| error.to_string())?
}

fn fetch_with_curl(url: &str) -> Result<Vec<u8>, String> {
    let program = if cfg!(target_os = "macos") {
        "/usr/bin/curl"
    } else {
        "curl"
    };
    let output = std::process::Command::new(program)
        .args([
            "-fsSL",
            "--connect-timeout",
            "10",
            "--speed-limit",
            "1024",
            "--speed-time",
            "15",
            "--max-time",
            "300",
            url,
        ])
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
    } else if name.ends_with(".zip") && cfg!(windows) {
        std::process::Command::new("tar.exe")
            .arg("-xf")
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
    let wanted = crate::cores::binary_name(kind.as_str());
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.eq_ignore_ascii_case(&wanted)
                        || cfg!(windows)
                            && kind == CoreKind::Mihomo
                            && name
                                .to_ascii_lowercase()
                                .starts_with("mihomo-windows-amd64")
                            && name.to_ascii_lowercase().ends_with(".exe")
                })
            {
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
    fn supported_assets_have_exact_archive_formats_and_digests() {
        for kind in [CoreKind::SingBox, CoreKind::Xray, CoreKind::Mihomo] {
            let release = release_for(kind, "windows", "x86_64").unwrap();
            assert!(release.url.ends_with(".zip"));
            assert_eq!(release.sha256.unwrap().len(), 64);
            assert!(release_for(kind, "windows", "aarch64").is_none());
            assert!(
                release_for(kind, "linux", "aarch64")
                    .unwrap()
                    .url
                    .contains("arm64")
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
