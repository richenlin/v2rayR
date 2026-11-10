//! Request tokens.
//!
//! The token lives in the app runtime directory (mode 0600). The helper refuses
//! to act without it, which prevents another local account from driving a
//! root-capable binary. Comparison is constant time over the raw bytes.

use std::io::Write;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TokenError {
    #[error("token file is missing: {0}")]
    Missing(String),
    #[error("token file is not readable by its owner only: {0}")]
    Permissions(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct Token(String);

impl Token {
    /// Create a new token, replacing any existing file with mode 0600.
    pub fn create(path: &Path) -> Result<Self, TokenError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(token.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|failure| failure.error)?;
        Ok(Self(token))
    }

    pub fn load(path: &Path) -> Result<Self, TokenError> {
        let metadata =
            std::fs::metadata(path).map_err(|_| TokenError::Missing(path.display().to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(TokenError::Permissions(path.display().to_string()));
            }
        }
        let _ = metadata;
        let token = std::fs::read_to_string(path)?;
        Ok(Self(token.trim().to_owned()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Constant-time comparison; length differences are not secret here because
    /// the length is fixed by construction.
    pub fn matches(&self, candidate: &str) -> bool {
        let expected = self.0.as_bytes();
        let candidate = candidate.as_bytes();
        if expected.len() != candidate.len() {
            return false;
        }
        let mut diff = 0u8;
        for (a, b) in expected.iter().zip(candidate.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// The default location inside the app runtime directory.
pub fn default_token_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("helper.token")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_round_trips_and_matches_only_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("helper.token");
        let token = Token::create(&path).expect("create");
        let loaded = Token::load(&path).expect("load");
        assert!(loaded.matches(token.expose()));
        assert!(!loaded.matches("something-else"));
        assert!(!loaded.matches(""));
    }

    #[test]
    fn the_token_file_is_owner_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("helper.token");
        Token::create(&path).expect("create");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn a_world_readable_token_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("helper.token");
        Token::create(&path).expect("create");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
            assert!(matches!(
                Token::load(&path),
                Err(TokenError::Permissions(_))
            ));
        }
    }

    #[test]
    fn a_missing_token_is_reported_clearly() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            Token::load(&dir.path().join("nope")),
            Err(TokenError::Missing(_))
        ));
    }
}
