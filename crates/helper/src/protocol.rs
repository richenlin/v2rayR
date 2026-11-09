//! The JSON contract between the app and the helper.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum HelperRequest {
    /// Apply the system proxy. The helper captures the previous state itself and
    /// returns it, so a crash between the two processes cannot lose it.
    ProxyApply {
        proxy: ProxySpec,
    },
    /// Restore a captured snapshot (handed back as JSON from a previous apply).
    ProxyRestore {
        snapshot: serde_json::Value,
    },
    /// Turn the proxy off entirely, but only if it still points at `endpoint`.
    ProxyClear {
        host: String,
        port: u16,
    },
    AutostartEnable {
        program: String,
    },
    AutostartDisable,
    /// Report what the helper can do; used by the UI's diagnostics.
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySpec {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub bypass: Vec<String>,
    /// Include a SOCKS entry pointing at the same endpoint.
    #[serde(default)]
    pub socks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum HelperResponse {
    Ok {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snapshot: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl HelperResponse {
    pub fn ok() -> Self {
        Self::Ok {
            snapshot: None,
            message: None,
        }
    }

    pub fn with_snapshot(snapshot: serde_json::Value) -> Self {
        Self::Ok {
            snapshot: Some(snapshot),
            message: None,
        }
    }

    pub fn message(message: impl Into<String>) -> Self {
        Self::Ok {
            snapshot: None,
            message: Some(message.into()),
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_through_json() {
        let request = HelperRequest::ProxyApply {
            proxy: ProxySpec {
                host: "127.0.0.1".into(),
                port: 10808,
                bypass: vec!["localhost".into()],
                socks: true,
            },
        };
        let text = serde_json::to_string(&request).expect("serialise");
        assert!(text.contains("\"action\":\"proxy-apply\""));
        let parsed: HelperRequest = serde_json::from_str(&text).expect("parse");
        assert_eq!(parsed, request);
    }

    #[test]
    fn responses_are_tagged_and_omit_empty_fields() {
        let text = serde_json::to_string(&HelperResponse::ok()).expect("serialise");
        assert_eq!(text, r#"{"status":"ok"}"#);
    }

    #[test]
    fn unknown_actions_are_rejected() {
        assert!(serde_json::from_str::<HelperRequest>(r#"{"action":"format-disk"}"#).is_err());
    }
}
