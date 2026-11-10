//! Where v2rayR keeps its files.
//!
//! macOS convention (`~/Library/Application Support/v2rayR`), with an override
//! for tests and portable installs. Everything the app writes lives under one
//! root so "reset data" is a single directory operation.

use std::path::{Path, PathBuf};

use crate::PlatformError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub root: PathBuf,
}

impl AppPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `V2RAYR_HOME` wins (tests, portable mode), then the platform default.
    pub fn discover() -> Result<Self, PlatformError> {
        if let Ok(root) = std::env::var("V2RAYR_HOME") {
            return Ok(Self::new(root));
        }
        #[cfg(unix)]
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(PlatformError::InvalidState("HOME is not set".to_owned()))?;
        #[cfg(target_os = "macos")]
        let root = home.join("Library/Application Support/v2rayR");
        #[cfg(all(unix, not(target_os = "macos")))]
        let root = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("v2rayR");
        #[cfg(windows)]
        let root = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .map(|home| PathBuf::from(home).join("AppData/Roaming"))
            })
            .ok_or_else(|| {
                PlatformError::InvalidState("APPDATA and USERPROFILE are not set".into())
            })?
            .join("v2rayR");
        Ok(Self::new(root))
    }

    pub fn ensure(&self) -> Result<(), PlatformError> {
        for dir in [
            self.root.clone(),
            self.config_dir(),
            self.data_dir(),
            self.logs_dir(),
            self.cache_dir(),
        ] {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir().join("v2rayr.db")
    }

    pub fn settings(&self) -> PathBuf {
        self.config_dir().join("settings.json")
    }

    pub fn secrets_dir(&self) -> PathBuf {
        self.config_dir().join("secrets")
    }

    pub fn takeover_snapshot(&self) -> PathBuf {
        self.runtime_dir().join("takeover.json")
    }

    pub fn core_config(&self, core: &str) -> PathBuf {
        self.runtime_dir().join(format!("{core}-config.json"))
    }

    pub fn core_log(&self, core: &str) -> PathBuf {
        self.logs_dir().join(format!("{core}.log"))
    }

    pub fn app_log(&self) -> PathBuf {
        self.logs_dir().join("app.log")
    }

    /// Rule sets shipped with the app or downloaded into the data directory.
    pub fn rule_sets_dir(&self) -> PathBuf {
        self.data_dir().join("rules")
    }

    pub fn core_bin_dir(&self) -> PathBuf {
        self.data_dir().join("cores")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_derived_from_the_root() {
        let paths = AppPaths::new("/tmp/v2rayr-test");
        assert_eq!(
            paths.database(),
            Path::new("/tmp/v2rayr-test/data/v2rayr.db")
        );
        assert_eq!(
            paths.settings(),
            Path::new("/tmp/v2rayr-test/config/settings.json")
        );
        assert_eq!(
            paths.core_config("sing-box"),
            Path::new("/tmp/v2rayr-test/run/sing-box-config.json")
        );
    }

    #[test]
    fn ensure_creates_every_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::new(dir.path().join("root"));
        paths.ensure().expect("ensure");
        for path in [
            paths.config_dir(),
            paths.data_dir(),
            paths.logs_dir(),
            paths.cache_dir(),
        ] {
            assert!(path.is_dir(), "{} should exist", path.display());
        }
    }
}
