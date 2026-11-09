//! Domain errors.
//!
//! Every error carries a stable [`ErrorCode`]; the UI translates it via
//! `error.<code>` keys in the locale files, so users never see a Rust `Debug`
//! string. Anything that is only useful to a bug report goes into `detail` and
//! is redacted before it reaches the UI.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The scheme is known to belong to another product (ssh, ssr, ...).
    UnsupportedScheme,
    /// The link is a subscription URL rather than a profile.
    NotAProfileLink,
    InvalidUrl,
    InvalidBase64,
    InvalidEntry,
    MissingField,
    InvalidUuid,
    InvalidPort,
    InvalidValue,
    RealityWithoutPublicKey,
    UnsupportedTransport,
    UnsupportedProtocol,
    UnsupportedFormat,
    EmptyInput,
    DuplicateNode,
    NotFound,
    Storage,
    ConfigInvalid,
    CoreMissing,
    CoreFailed,
    PortInUse,
    TakeoverFailed,
    PermissionDenied,
    NotConnected,
    NetworkError,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedScheme => "unsupported_scheme",
            Self::NotAProfileLink => "not_a_profile_link",
            Self::InvalidUrl => "invalid_url",
            Self::InvalidBase64 => "invalid_base64",
            Self::InvalidEntry => "invalid_entry",
            Self::MissingField => "missing_field",
            Self::InvalidUuid => "invalid_uuid",
            Self::InvalidPort => "invalid_port",
            Self::InvalidValue => "invalid_value",
            Self::RealityWithoutPublicKey => "reality_without_public_key",
            Self::UnsupportedTransport => "unsupported_transport",
            Self::UnsupportedProtocol => "unsupported_protocol",
            Self::UnsupportedFormat => "unsupported_format",
            Self::EmptyInput => "empty_input",
            Self::DuplicateNode => "duplicate_node",
            Self::NotFound => "not_found",
            Self::Storage => "storage",
            Self::ConfigInvalid => "config_invalid",
            Self::CoreMissing => "core_missing",
            Self::CoreFailed => "core_failed",
            Self::PortInUse => "port_in_use",
            Self::TakeoverFailed => "takeover_failed",
            Self::PermissionDenied => "permission_denied",
            Self::NotConnected => "not_connected",
            Self::NetworkError => "network_error",
            Self::Internal => "internal",
        }
    }

    /// i18n key for this error.
    pub fn key(self) -> String {
        format!("error.{}", self.as_str())
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainError {
    code: ErrorCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

impl DomainError {
    pub fn new(code: ErrorCode) -> Self {
        Self { code, detail: None }
    }

    /// `detail` is developer-facing context (a field name, a scheme, an offset).
    /// It must never contain a credential.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    pub fn key(&self) -> String {
        self.code.key()
    }
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "{}: {detail}", self.code),
            None => write!(f, "{}", self.code),
        }
    }
}

impl std::error::Error for DomainError {}

impl From<serde_json::Error> for DomainError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(ErrorCode::InvalidValue).with_detail(format!("json: {error}"))
    }
}

impl From<uuid::Error> for DomainError {
    fn from(_: uuid::Error) -> Self {
        Self::new(ErrorCode::InvalidUuid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable_snake_case_keys() {
        assert_eq!(
            ErrorCode::RealityWithoutPublicKey.as_str(),
            "reality_without_public_key"
        );
        assert_eq!(ErrorCode::TakeoverFailed.key(), "error.takeover_failed");
    }

    #[test]
    fn error_serializes_as_code_and_detail() {
        let error = DomainError::new(ErrorCode::InvalidPort).with_detail("70000");
        let json = serde_json::to_value(&error).expect("serialize");
        assert_eq!(json["code"], "invalid_port");
        assert_eq!(json["detail"], "70000");
    }
}
