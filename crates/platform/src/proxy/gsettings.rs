//! GNOME-compatible GSettings integration. KIO/KDE has a different store;
//! the presence of GNOME schemas alone does not make that desktop compatible.
use std::sync::Arc;

use super::native::{Backend, NativeProxyKind, Values};
use super::{CommandRunner, ProxyEndpoint, ProxyTarget, ServiceProxyState, SystemProxyState};
use crate::PlatformError;

const MODE: &str = "mode";
const MANUAL_GROUPS: &[&[&str]] = &[
    &["http.host", "http.port", "use-same-proxy"],
    &["https.host", "https.port"],
    &["socks.host", "socks.port"],
    &["ftp.host", "ftp.port"],
];
const KEYS: &[&str] = &[
    "mode",
    "autoconfig-url",
    "ignore-hosts",
    "use-same-proxy",
    "http.host",
    "http.port",
    "http.use-authentication",
    "https.host",
    "https.port",
    "socks.host",
    "socks.port",
    "ftp.host",
    "ftp.port",
];

pub(super) struct GSettings {
    runner: Arc<dyn CommandRunner>,
    desktop_supported: bool,
}

impl GSettings {
    pub(super) fn new(runner: Arc<dyn CommandRunner>) -> Self {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let desktop_supported = desktop
            .split(':')
            .any(|name| matches!(name, "gnome" | "ubuntu"));
        Self {
            runner,
            desktop_supported,
        }
    }

    fn query(
        &self,
        operation: &str,
        field: &str,
        value: Option<&str>,
    ) -> Result<String, PlatformError> {
        if !self.desktop_supported {
            return Err(PlatformError::Unsupported(
                "当前 Linux 桌面未验证 GSettings 系统代理，请使用本地端口模式",
            ));
        }
        if !KEYS.contains(&field) {
            return Err(PlatformError::InvalidState(
                "未知 GSettings 代理字段".into(),
            ));
        }
        let (schema, key) = field.split_once('.').map_or_else(
            || ("org.gnome.system.proxy".to_owned(), field),
            |(protocol, key)| (format!("org.gnome.system.proxy.{protocol}"), key),
        );
        let mut args = vec![operation, &schema, key];
        args.extend(value);
        let result = self
            .runner
            .run("gsettings", &args)
            .map_err(|error| match error {
                PlatformError::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound => {
                    PlatformError::Unsupported("找不到 gsettings，请使用本地端口模式")
                }
                _ => error,
            })?;
        if !result.ok() {
            return Err(PlatformError::InvalidState(format!(
                "无法操作 Linux 系统代理 {field}，请检查桌面会话或使用本地端口模式"
            )));
        }
        Ok(result.stdout.trim().to_owned())
    }
}

impl Backend for GSettings {
    fn kind(&self) -> NativeProxyKind {
        NativeProxyKind::GSettings
    }
    fn mode_key(&self) -> &'static str {
        MODE
    }
    fn endpoint_groups(&self, written: &Values) -> &'static [&'static [&'static str]] {
        if written.get(MODE).is_some_and(|mode| mode == "'auto'") {
            &[&["autoconfig-url"]]
        } else {
            MANUAL_GROUPS
        }
    }

    fn read(&self) -> Result<Values, PlatformError> {
        KEYS.iter()
            .map(|key| Ok(((*key).into(), self.query("get", key, None)?)))
            .collect()
    }

    fn target(&self, current: &Values, target: &ProxyTarget) -> Result<Values, PlatformError> {
        let mut desired = Values::new();
        if let Some(pac) = &target.pac {
            desired.insert(MODE.into(), "'auto'".into());
            desired.insert("autoconfig-url".into(), literal(pac)?);
        } else {
            desired.insert(MODE.into(), "'manual'".into());
            desired.insert("use-same-proxy".into(), "false".into());
            if current["http.use-authentication"] == "true" {
                return Err(PlatformError::Unsupported(
                    "已有认证系统代理，请改用本地端口模式",
                ));
            }
            for (protocol, endpoint) in [
                ("http", &target.http),
                ("https", &target.https),
                ("socks", &target.socks),
                ("ftp", &None),
            ] {
                desired.insert(
                    format!("{protocol}.host"),
                    literal(endpoint.as_ref().map_or("", |endpoint| &endpoint.host))?,
                );
                desired.insert(
                    format!("{protocol}.port"),
                    endpoint
                        .as_ref()
                        .map_or(0, |endpoint| endpoint.port)
                        .to_string(),
                );
            }
        }
        let bypass = target
            .bypass
            .iter()
            .map(|item| literal(item))
            .collect::<Result<Vec<_>, _>>()?;
        desired.insert(
            "ignore-hosts".into(),
            if bypass.is_empty() {
                "@as []".into()
            } else {
                format!("[{}]", bypass.join(", "))
            },
        );
        Ok(desired)
    }

    fn write(&self, values: &Values) -> Result<(), PlatformError> {
        // Check locks before changing anything; select the mode after the
        // addresses so an application never observes an empty active proxy.
        let ordered = values
            .iter()
            .filter(|(key, _)| key.as_str() != MODE)
            .chain(values.iter().filter(|(key, _)| key.as_str() == MODE));
        for (key, _) in ordered.clone() {
            if self.query("writable", key, None)? != "true" {
                return Err(PlatformError::PermissionDenied(format!(
                    "Linux 系统代理 {key} 被锁定"
                )));
            }
        }
        for (key, value) in ordered {
            self.query("set", key, Some(value))?;
        }
        Ok(())
    }

    fn state(&self, current: &Values) -> SystemProxyState {
        let manual = current[MODE] == "'manual'";
        let auto = current[MODE] == "'auto'";
        SystemProxyState {
            services: vec![ServiceProxyState {
                service: "GSettings".into(),
                enabled: manual || auto,
                http: manual.then(|| endpoint(current, "http")).flatten(),
                https: manual.then(|| endpoint(current, "https")).flatten(),
                socks: manual.then(|| endpoint(current, "socks")).flatten(),
                auto_url: auto.then(|| unquote(&current["autoconfig-url"]).to_owned()),
                auto_enabled: auto,
                bypass: vec![],
            }],
        }
    }

    fn disable_owned(&self, expected: &ProxyEndpoint, current: &Values) -> Values {
        let endpoints: Vec<_> = ["http", "https", "socks", "ftp"]
            .into_iter()
            .filter_map(|protocol| endpoint(current, protocol))
            .collect();
        if current[MODE] == "'manual'"
            && !endpoints.is_empty()
            && endpoints.iter().all(|endpoint| endpoint == expected)
        {
            return [(MODE.into(), "'none'".into())].into();
        }
        Values::new()
    }
}

fn literal(value: &str) -> Result<String, PlatformError> {
    if value
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '\'' | '\\'))
    {
        return Err(PlatformError::InvalidState(
            "代理地址包含不受支持的字符".into(),
        ));
    }
    Ok(format!("'{value}'"))
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .unwrap_or(value)
}

fn endpoint(current: &Values, protocol: &str) -> Option<ProxyEndpoint> {
    let host = unquote(current.get(&format!("{protocol}.host"))?);
    let port: u16 = current.get(&format!("{protocol}.port"))?.parse().ok()?;
    (!host.is_empty() && port > 0).then(|| ProxyEndpoint::new(host, port))
}

#[cfg(test)]
mod tests {
    use super::super::{SystemCommandRunner, SystemProxy, native::NativeProxy};
    use super::*;

    #[test]
    #[ignore = "requires an isolated writable GNOME GSettings session"]
    fn real_gsettings_round_trip() {
        assert_eq!(
            std::env::var("V2RAYR_TEST_GSETTINGS").as_deref(),
            Ok("1"),
            "use an isolated dconf configuration"
        );
        let backend = GSettings::new(Arc::new(SystemCommandRunner));
        backend
            .write(
                &[
                    ("mode".into(), "'manual'".into()),
                    ("http.host".into(), "'old.example'".into()),
                    ("http.port".into(), "8888".into()),
                ]
                .into(),
            )
            .unwrap();
        let before = backend.read().unwrap();
        let proxy = NativeProxy(backend);
        let mut target = ProxyTarget::all_through("127.0.0.1", 10808);
        target.http = Some(ProxyEndpoint::new("127.0.0.1", 10809));
        target.https = target.http.clone();
        let journal = proxy.apply(&target).unwrap();
        assert_eq!(proxy.capture().unwrap().services[0].http, target.http);
        proxy.restore(&journal).unwrap();
        assert_eq!(proxy.0.read().unwrap(), before);
        let pac = ProxyTarget::through_pac("http://127.0.0.1:10811/proxy.pac", vec![]);
        let journal = proxy.apply(&pac).unwrap();
        assert!(proxy.capture().unwrap().services[0].auto_enabled);
        proxy.restore(&journal).unwrap();
        assert_eq!(proxy.0.read().unwrap(), before);

        let journal = proxy.apply(&target).unwrap();
        proxy
            .0
            .query("set", "http.host", Some("'external.example'"))
            .unwrap();
        proxy.restore(&journal).unwrap();
        let observed = proxy.0.read().unwrap();
        assert_eq!(observed["mode"], "'manual'");
        assert_eq!(observed["http.host"], "'external.example'");
        assert_eq!(
            observed["http.port"], "10809",
            "host and port belong together"
        );
        assert_eq!(observed["socks.port"], before["socks.port"]);
        proxy
            .0
            .write(
                &before
                    .iter()
                    .filter(|(key, value)| observed.get(*key) != Some(*value))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
            .unwrap();
        assert_eq!(proxy.0.read().unwrap(), before);
    }
}
