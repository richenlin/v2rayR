//! The system operations the helper performs.
//!
//! Everything goes through the `platform` traits, so the logic is testable with
//! fakes and the same code path is exercised on every OS.

use serde_json::Value;
use thiserror::Error;
use v2rayr_platform::proxy::{ProxyTarget, SystemProxy, TakeoverSnapshot, default_bypass};
use v2rayr_platform::{Autostart, PlatformError};

use crate::protocol::{HelperRequest, HelperResponse, ProxySpec};

#[derive(Debug, Error)]
pub enum HelperError {
    #[error("platform: {0}")]
    Platform(#[from] PlatformError),
    #[error("snapshot: {0}")]
    Snapshot(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub fn execute(
    request: &HelperRequest,
    proxy: &dyn SystemProxy,
    autostart: &dyn Autostart,
) -> HelperResponse {
    match run(request, proxy, autostart) {
        Ok(response) => response,
        Err(error) => HelperResponse::error(error_code(&error), error.to_string()),
    }
}

fn run(
    request: &HelperRequest,
    proxy: &dyn SystemProxy,
    autostart: &dyn Autostart,
) -> Result<HelperResponse, HelperError> {
    match request {
        HelperRequest::ProxyApply { proxy: spec } => {
            let target = target_from(spec);
            let snapshot = proxy.apply(&target)?;
            let value = serde_json::to_value(&snapshot)
                .map_err(|error| HelperError::Snapshot(error.to_string()))?;
            Ok(HelperResponse::with_snapshot(value))
        }
        HelperRequest::ProxyRestore { snapshot } => {
            let snapshot: TakeoverSnapshot = serde_json::from_value(snapshot.clone())
                .map_err(|error| HelperError::Snapshot(error.to_string()))?;
            proxy.restore(&snapshot)?;
            Ok(HelperResponse::ok())
        }
        HelperRequest::ProxyClear { host, port } => {
            proxy.clear(&v2rayr_platform::proxy::ProxyEndpoint::new(
                host.clone(),
                *port,
            ))?;
            Ok(HelperResponse::ok())
        }
        HelperRequest::AutostartEnable { program } => {
            autostart.set_enabled(true, std::path::Path::new(program))?;
            Ok(HelperResponse::message(format!(
                "autostart enabled for {program}"
            )))
        }
        HelperRequest::AutostartDisable => {
            autostart.set_enabled(false, std::path::Path::new(""))?;
            Ok(HelperResponse::message("autostart disabled"))
        }
        HelperRequest::Status => Ok(HelperResponse::message(status_text(proxy, autostart))),
    }
}

fn target_from(spec: &ProxySpec) -> ProxyTarget {
    let mut target = ProxyTarget::all_through(spec.host.clone(), spec.port);
    target.bypass = if spec.bypass.is_empty() {
        default_bypass()
    } else {
        spec.bypass.clone()
    };
    if !spec.socks {
        target.socks = None;
    }
    target
}

fn status_text(proxy: &dyn SystemProxy, autostart: &dyn Autostart) -> String {
    let capture = match proxy.capture() {
        Ok(state) => format!("{} service(s) with proxy state", state.services.len()),
        Err(error) => format!("capture failed: {error}"),
    };
    let autostart_state = if autostart.is_enabled() {
        "enabled"
    } else {
        "disabled"
    };
    format!("proxy: {capture}; autostart: {autostart_state}")
}

fn error_code(error: &HelperError) -> &'static str {
    match error {
        HelperError::Platform(PlatformError::PermissionDenied(_)) => "permission_denied",
        HelperError::Platform(_) => "takeover_failed",
        HelperError::Snapshot(_) => "invalid_value",
        HelperError::Unsupported(_) => "unsupported_protocol",
    }
}

/// Exposed for the app's diagnostics: a snapshot the app can persist before
/// asking the helper to change anything.
pub fn snapshot_value(snapshot: &TakeoverSnapshot) -> Option<Value> {
    serde_json::to_value(snapshot).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use v2rayr_platform::proxy::{ProxyEndpoint, SystemProxyState};

    #[derive(Default)]
    struct FakeProxy {
        applied: Mutex<Vec<ProxyTarget>>,
        restored: Mutex<usize>,
        fail: bool,
    }

    impl SystemProxy for FakeProxy {
        fn capture(&self) -> Result<SystemProxyState, PlatformError> {
            Ok(SystemProxyState::default())
        }

        fn apply(&self, target: &ProxyTarget) -> Result<TakeoverSnapshot, PlatformError> {
            if self.fail {
                return Err(PlatformError::PermissionDenied("test".into()));
            }
            self.applied.lock().expect("lock").push(target.clone());
            Ok(TakeoverSnapshot {
                services: Vec::new(),
                applied_endpoint: target
                    .http
                    .clone()
                    .unwrap_or_else(|| ProxyEndpoint::new("127.0.0.1", 10808)),
                applied_at_ms: 1,
            })
        }

        fn restore(&self, _snapshot: &TakeoverSnapshot) -> Result<(), PlatformError> {
            *self.restored.lock().expect("lock") += 1;
            Ok(())
        }

        fn clear(&self, _endpoint: &ProxyEndpoint) -> Result<(), PlatformError> {
            Ok(())
        }
    }

    struct FakeAutostart {
        enabled: Mutex<bool>,
    }

    impl Autostart for FakeAutostart {
        fn is_enabled(&self) -> bool {
            *self.enabled.lock().expect("lock")
        }

        fn set_enabled(
            &self,
            enabled: bool,
            _program: &std::path::Path,
        ) -> Result<(), PlatformError> {
            *self.enabled.lock().expect("lock") = enabled;
            Ok(())
        }
    }

    fn request() -> HelperRequest {
        HelperRequest::ProxyApply {
            proxy: ProxySpec {
                host: "127.0.0.1".into(),
                port: 10814,
                bypass: Vec::new(),
                socks: true,
            },
        }
    }

    #[test]
    fn apply_returns_the_snapshot_it_captured() {
        let proxy = FakeProxy::default();
        let autostart = FakeAutostart {
            enabled: Mutex::new(false),
        };
        let response = execute(&request(), &proxy, &autostart);
        match response {
            HelperResponse::Ok { snapshot, .. } => {
                assert!(snapshot.is_some());
            }
            HelperResponse::Error { .. } => panic!("apply failed"),
        }
        let applied = proxy.applied.lock().expect("lock");
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].http.as_ref().expect("http").port, 10814);
    }

    #[test]
    fn a_permission_failure_maps_to_its_code() {
        let proxy = FakeProxy {
            fail: true,
            ..FakeProxy::default()
        };
        let autostart = FakeAutostart {
            enabled: Mutex::new(false),
        };
        let response = execute(&request(), &proxy, &autostart);
        match response {
            HelperResponse::Error { code, .. } => assert_eq!(code, "permission_denied"),
            HelperResponse::Ok { .. } => panic!("expected failure"),
        }
    }

    #[test]
    fn restore_and_autostart_are_idempotent() {
        let proxy = FakeProxy::default();
        let autostart = FakeAutostart {
            enabled: Mutex::new(false),
        };
        let snapshot = serde_json::to_value(TakeoverSnapshot {
            services: Vec::new(),
            applied_endpoint: ProxyEndpoint::new("127.0.0.1", 10808),
            applied_at_ms: 0,
        })
        .expect("snapshot");
        let response = execute(
            &HelperRequest::ProxyRestore { snapshot },
            &proxy,
            &autostart,
        );
        assert!(matches!(response, HelperResponse::Ok { .. }));
        assert_eq!(*proxy.restored.lock().expect("lock"), 1);

        for _ in 0..2 {
            let response = execute(
                &HelperRequest::AutostartEnable {
                    program: "/bin/true".into(),
                },
                &proxy,
                &autostart,
            );
            assert!(matches!(response, HelperResponse::Ok { .. }));
            assert!(*autostart.enabled.lock().expect("lock"));
        }
        execute(&HelperRequest::AutostartDisable, &proxy, &autostart);
        assert!(!*autostart.enabled.lock().expect("lock"));
    }
}
