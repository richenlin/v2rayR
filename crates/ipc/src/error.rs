//! The one error shape the UI understands.

use serde::{Deserialize, Serialize};
use v2rayr_domain::{DomainError, ErrorCode};

/// A user-presentable failure.
///
/// `code` is stable and machine readable (log correlation, tests), `key` is the
/// i18n key the UI renders, `detail` is developer context that has already been
/// stripped of credentials, and `retryable` tells the UI whether an inline
/// "重试" affordance makes sense.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub code: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub retryable: bool,
}

impl Problem {
    pub fn new(code: impl Into<String>, retryable: bool) -> Self {
        let code = code.into();
        Self {
            key: format!("error.{code}"),
            code,
            detail: None,
            retryable,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal.as_str(), false).with_detail(detail)
    }

    pub fn config(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::ConfigInvalid.as_str(), false).with_detail(detail)
    }

    pub fn storage(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Storage.as_str(), true).with_detail(detail)
    }

    pub fn network(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::NetworkError.as_str(), true).with_detail(detail)
    }
}

impl From<DomainError> for Problem {
    fn from(error: DomainError) -> Self {
        Self {
            retryable: is_retryable(error.code()),
            key: error.key(),
            code: error.code().as_str().to_owned(),
            detail: error.detail().map(str::to_owned),
        }
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "{} ({detail})", self.code),
            None => f.write_str(&self.code),
        }
    }
}

impl std::error::Error for Problem {}

fn is_retryable(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::NetworkError
            | ErrorCode::Storage
            | ErrorCode::PortInUse
            | ErrorCode::CoreFailed
            | ErrorCode::TakeoverFailed
            | ErrorCode::CoreMissing
            | ErrorCode::Internal
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_errors_map_to_stable_keys() {
        let problem = Problem::from(DomainError::new(ErrorCode::InvalidPort).with_detail("70000"));
        assert_eq!(problem.code, "invalid_port");
        assert_eq!(problem.key, "error.invalid_port");
        assert!(!problem.retryable);
        assert_eq!(problem.detail.as_deref(), Some("70000"));
    }

    #[test]
    fn network_errors_are_retryable() {
        assert!(Problem::from(DomainError::new(ErrorCode::NetworkError)).retryable);
        assert!(!Problem::from(DomainError::new(ErrorCode::InvalidUrl)).retryable);
    }

    #[test]
    fn serializes_without_empty_detail() {
        let json = serde_json::to_value(Problem::new("invalid_url", false)).expect("serialize");
        assert_eq!(json["code"], "invalid_url");
        assert!(json.get("detail").is_none());
    }
}
