//! Locating kernel binaries and asking them what they are.
//!
//! For desktop builds, signed sidecars next to the executable take priority.
//! Other callers look in the data directory and then PATH.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use v2rayr_domain::CoreKind;
use v2rayr_platform::AppPaths;

use crate::EngineError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreProbe {
    pub kind: CoreKind,
    pub path: PathBuf,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CoreBinaries {
    sing_box: Option<PathBuf>,
    xray: Option<PathBuf>,
    mihomo: Option<PathBuf>,
}

impl CoreBinaries {
    pub fn discover(paths: &AppPaths) -> Self {
        Self::discover_with_bundle(paths, None)
    }

    pub fn discover_with_bundle(paths: &AppPaths, bundled: Option<&Path>) -> Self {
        let bin = paths.core_bin_dir();
        Self {
            sing_box: bundled_core(bundled, "sing-box").or_else(|| locate(&bin, "sing-box")),
            xray: bundled_core(bundled, "xray").or_else(|| locate(&bin, "xray")),
            mihomo: locate(&bin, "mihomo"),
        }
    }

    pub fn with_overrides(
        mut self,
        sing_box: Option<PathBuf>,
        xray: Option<PathBuf>,
        mihomo: Option<PathBuf>,
    ) -> Self {
        if sing_box.is_some() {
            self.sing_box = sing_box;
        }
        if xray.is_some() {
            self.xray = xray;
        }
        if mihomo.is_some() {
            self.mihomo = mihomo;
        }
        self
    }

    pub fn path(&self, core: CoreKind) -> Option<&Path> {
        match core {
            CoreKind::SingBox => self.sing_box.as_deref(),
            CoreKind::Xray => self.xray.as_deref(),
            CoreKind::Mihomo => self.mihomo.as_deref(),
        }
    }

    pub fn require(&self, core: CoreKind) -> Result<&Path, EngineError> {
        self.path(core).ok_or(EngineError::MissingCore {
            core: core.as_str(),
        })
    }

    /// Protocol ids each installed kernel can serve, for the settings page.
    pub fn installed(&self) -> Vec<CoreKind> {
        [CoreKind::SingBox, CoreKind::Xray, CoreKind::Mihomo]
            .into_iter()
            .filter(|core| self.path(*core).is_some())
            .collect()
    }
}

fn bundled_core(dir: Option<&Path>, name: &str) -> Option<PathBuf> {
    let path = dir?.join(binary_name(&format!("v2rayr-core-{name}")));
    is_executable(&path).then_some(path)
}

fn locate(preferred_dir: &Path, name: &str) -> Option<PathBuf> {
    let name = binary_name(name);
    let candidate = preferred_dir.join(&name);
    if is_executable(&candidate) {
        return Some(candidate);
    }
    which(&name)
}

pub(crate) fn binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// `sing-box version` / `xray version` / `mihomo -v`.
pub async fn probe(kind: CoreKind, program: &Path) -> Result<CoreProbe, EngineError> {
    let path = program.to_path_buf();
    let moved = path.clone();
    let task = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(&moved);
        match kind {
            CoreKind::Mihomo => {
                command.arg("-v");
            }
            _ => {
                command.arg("version");
            }
        }
        #[cfg(windows)]
        crate::process::hide_console(&mut command);
        command.output()
    });
    let output = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .map_err(|_| EngineError::Timeout(Duration::from_secs(5)))?
        .map_err(|error| EngineError::Io(std::io::Error::other(error.to_string())))??;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        return Err(EngineError::Spawn {
            core: kind.as_str(),
            message: if stderr.is_empty() { stdout } else { stderr },
        });
    }
    let version = parse_version(&stdout)
        .or_else(|| parse_version(&stderr))
        .map(str::to_owned);
    Ok(CoreProbe {
        kind,
        path,
        version,
    })
}

/// Pull `1.11.3` out of `sing-box version 1.11.3` or `Xray 1.8.24 (Xray, Penetrates Everything.)`.
pub fn parse_version(output: &str) -> Option<&str> {
    output.split_whitespace().find_map(|token| {
        let trimmed = token.trim_start_matches('v').trim_end_matches(',');
        let looks_like_version = trimmed
            .split('.')
            .take(2)
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
            && trimmed.contains('.');
        if looks_like_version {
            Some(trimmed)
        } else {
            None
        }
    })
}

/// Kernel argv for a config file.
pub fn launch_arguments(kind: CoreKind, config: &Path, data_dir: &Path) -> Vec<String> {
    match kind {
        CoreKind::SingBox => vec![
            "run".into(),
            "-c".into(),
            config.display().to_string(),
            "-D".into(),
            data_dir.display().to_string(),
        ],
        CoreKind::Xray => vec!["run".into(), "-c".into(), config.display().to_string()],
        CoreKind::Mihomo => vec![
            "-d".into(),
            data_dir.display().to_string(),
            "-f".into(),
            config.display().to_string(),
        ],
    }
}

pub async fn version_line(path: &Path) -> Option<String> {
    let path = path.to_path_buf();
    let task = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(&path);
        command.arg("version");
        #[cfg(windows)]
        crate::process::hide_console(&mut command);
        command.output()
    });
    let output = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .ok()?
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_bundle_wins_over_existing_user_cores() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("user"));
        let bundled = temp.path().join("bundle");
        std::fs::create_dir_all(paths.core_bin_dir()).unwrap();
        std::fs::create_dir_all(&bundled).unwrap();
        for (kind, name) in [(CoreKind::SingBox, "sing-box"), (CoreKind::Xray, "xray")] {
            let user = paths.core_bin_dir().join(binary_name(name));
            let shipped = bundled.join(binary_name(&format!("v2rayr-core-{name}")));
            std::fs::write(&user, b"user").unwrap();
            std::fs::write(&shipped, b"bundle").unwrap();
            assert_eq!(
                CoreBinaries::discover_with_bundle(&paths, Some(&bundled)).path(kind),
                Some(shipped.as_path())
            );
            assert_eq!(
                CoreBinaries::discover(&paths).path(kind),
                Some(user.as_path())
            );
        }
    }

    #[test]
    fn version_is_extracted_from_common_banners() {
        assert_eq!(parse_version("sing-box version 1.11.3"), Some("1.11.3"));
        assert_eq!(
            parse_version("Xray 1.8.24 (Xray, Penetrates Everything.) Custom (go1.22.7)"),
            Some("1.8.24")
        );
        assert_eq!(parse_version("v1.19.0"), Some("1.19.0"));
        assert_eq!(parse_version("no version here"), None);
    }

    #[test]
    fn launch_arguments_match_each_kernel() {
        let config = Path::new("/tmp/c.json");
        let data = Path::new("/tmp/data");
        assert_eq!(
            launch_arguments(CoreKind::SingBox, config, data),
            vec!["run", "-c", "/tmp/c.json", "-D", "/tmp/data"]
        );
        assert_eq!(
            launch_arguments(CoreKind::Xray, config, data),
            vec!["run", "-c", "/tmp/c.json"]
        );
        assert_eq!(
            launch_arguments(CoreKind::Mihomo, config, data),
            vec!["-d", "/tmp/data", "-f", "/tmp/c.json"]
        );
    }

    #[test]
    fn missing_binary_is_reported_not_panicked() {
        let binaries = CoreBinaries::default();
        let error = binaries.require(CoreKind::SingBox).expect_err("missing");
        assert!(matches!(error, EngineError::MissingCore { .. }));
        assert_eq!(error.code(), v2rayr_domain::ErrorCode::CoreMissing);
    }
}
