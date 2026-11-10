//! Per-user WinINet proxy for the default LAN connection. WinHTTP, browser
//! overrides and named RAS/VPN connections have independent settings.
use super::{ProxyEndpoint, ProxyTarget};
use crate::PlatformError;

#[cfg(windows)]
const DIRECT: u32 = 1;
#[cfg(windows)]
const PROXY: u32 = 2;
#[cfg(windows)]
const PAC: u32 = 4;

fn server_list(target: &ProxyTarget) -> Result<String, PlatformError> {
    let mut slots = Vec::new();
    for (scheme, endpoint) in [
        ("http", &target.http),
        ("https", &target.https),
        ("socks", &target.socks),
    ] {
        if let Some(endpoint) = endpoint {
            let host = &endpoint.host;
            if host.is_empty()
                || host
                    .chars()
                    .any(|c| !(c.is_ascii_alphanumeric() || ".-:[]".contains(c)))
            {
                return Err(PlatformError::InvalidState("Windows 代理主机名无效".into()));
            }
            let host = if host.contains(':') && !host.starts_with('[') {
                format!("[{host}]")
            } else {
                host.clone()
            };
            slots.push(format!("{scheme}={host}:{}", endpoint.port));
        }
    }
    Ok(slots.join(";"))
}

fn endpoint(value: &str, scheme: &str) -> Option<ProxyEndpoint> {
    let slot = if value.contains('=') {
        value
            .split(';')
            .filter_map(|slot| slot.split_once('='))
            .find(|(prefix, _)| prefix.eq_ignore_ascii_case(scheme))?
            .1
    } else {
        value
    };
    let (host, port) = slot.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    (!host.is_empty()).then_some(ProxyEndpoint::new(host, port.parse().ok()?))
}

fn bypass_list(items: &[String]) -> Result<String, PlatformError> {
    let invalid =
        || PlatformError::InvalidState("Windows 代理例外须为域名、通配符、IP 或 IPv4 网段".into());
    let mut patterns = Vec::new();
    for item in items {
        if item.contains(';') || item.chars().any(char::is_control) {
            return Err(invalid());
        }
        if let Some((address, prefix)) = item.split_once('/') {
            let address: std::net::Ipv4Addr = address.parse().map_err(|_| invalid())?;
            let prefix: u8 = prefix
                .parse()
                .ok()
                .filter(|prefix| *prefix <= 32)
                .ok_or_else(invalid)?;
            if prefix == 32 {
                patterns.push(address.to_string());
            } else if prefix == 0 {
                patterns.push("*".into());
            } else {
                // WinINet takes wildcard patterns, not CIDR. Expand only the
                // partial octet (at most 128 alternatives).
                let bytes = address.octets();
                let index = usize::from(prefix / 8);
                let partial = prefix % 8;
                let leading: Vec<_> = bytes[..index].iter().map(u8::to_string).collect();
                if partial == 0 {
                    patterns.push(format!("{}.*", leading.join(".")));
                    continue;
                }
                let width = 1u16 << (8 - partial);
                let first = (u16::from(bytes[index]) / width) * width;
                for byte in first..first + width {
                    let mut path = leading.clone();
                    path.push(byte.to_string());
                    if index < 3 {
                        path.push("*".into());
                    }
                    patterns.push(path.join("."));
                }
            }
        } else {
            patterns.push(if item == "localhost" {
                "<local>".into()
            } else {
                item.clone()
            });
        }
    }
    Ok(patterns.join(";"))
}

#[cfg(windows)]
mod os {
    use std::{ffi::c_void, ptr};

    use super::{DIRECT, PAC, PROXY, bypass_list, endpoint, server_list};
    use crate::{
        PlatformError,
        proxy::{
            ProxyEndpoint, ProxyTarget, ServiceProxyState, SystemProxyState,
            native::{Backend, NativeProxyKind, Values},
        },
    };

    const PER_CONNECTION_OPTION: u32 = 75;
    const SETTINGS_CHANGED: u32 = 39;
    const REFRESH: u32 = 37;

    // INTERNET_PER_CONN_OPTIONW contains a union of DWORD, PWSTR, FILETIME.
    #[repr(C)]
    union OptionValue {
        number: u32,
        string: *mut u16,
        file_time: [u32; 2],
    }
    #[repr(C)]
    struct ConnOption {
        option: u32,
        value: OptionValue,
    }
    #[repr(C)]
    struct ConnOptions {
        size: u32,
        connection: *mut u16,
        count: u32,
        error: u32,
        options: *mut ConnOption,
    }

    #[link(name = "wininet")]
    unsafe extern "system" {
        fn InternetQueryOptionW(
            handle: *mut c_void,
            option: u32,
            buffer: *mut c_void,
            size: *mut u32,
        ) -> i32;
        fn InternetSetOptionW(
            handle: *mut c_void,
            option: u32,
            buffer: *mut c_void,
            size: u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalFree(memory: *mut c_void) -> *mut c_void;
    }

    fn failure(operation: &str) -> PlatformError {
        PlatformError::InvalidState(format!(
            "Windows {operation}失败：{}",
            std::io::Error::last_os_error()
        ))
    }

    fn list(options: &mut [ConnOption]) -> ConnOptions {
        ConnOptions {
            size: std::mem::size_of::<ConnOptions>() as u32,
            connection: ptr::null_mut(), // Default LAN.
            count: options.len() as u32,
            error: 0,
            options: options.as_mut_ptr(),
        }
    }

    fn query(option: u32, string: bool) -> Result<String, PlatformError> {
        let mut options = [ConnOption {
            option,
            value: OptionValue {
                string: ptr::null_mut(),
            },
        }];
        let mut values = list(&mut options);
        let mut size = values.size;
        // SAFETY: the list and its one option are live and correctly laid out.
        if unsafe {
            InternetQueryOptionW(
                ptr::null_mut(),
                PER_CONNECTION_OPTION,
                (&mut values as *mut ConnOptions).cast(),
                &mut size,
            )
        } == 0
        {
            return Err(failure("读取系统代理"));
        }
        if !string {
            // SAFETY: the queried option is documented as a DWORD.
            return Ok(unsafe { options[0].value.number }.to_string());
        }
        // SAFETY: WinINet owns this allocation until we free it with GlobalFree.
        let data = unsafe { options[0].value.string };
        if data.is_null() {
            return Ok(String::new());
        }
        let result = unsafe {
            let mut len = 0;
            while *data.add(len) != 0 {
                len += 1;
            }
            let result = String::from_utf16(std::slice::from_raw_parts(data, len));
            GlobalFree(data.cast());
            result
        };
        result
            .map_err(|_| PlatformError::InvalidState("Windows 系统代理地址不是有效 UTF-16".into()))
    }

    pub(in super::super) struct WinInet;

    impl Backend for WinInet {
        fn kind(&self) -> NativeProxyKind {
            NativeProxyKind::WinInet
        }
        fn mode_key(&self) -> &'static str {
            "flags"
        }
        fn endpoint_groups(&self, written: &Values) -> &'static [&'static [&'static str]] {
            let flags = written
                .get("flags")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            if flags & PAC != 0 {
                &[&["pac"]]
            } else {
                &[&["server"]]
            }
        }

        fn read(&self) -> Result<Values, PlatformError> {
            Ok([
                (
                    "flags".into(),
                    query(10, false).or_else(|_| query(1, false))?,
                ),
                ("server".into(), query(2, true)?),
                ("bypass".into(), query(3, true)?),
                ("pac".into(), query(4, true)?),
            ]
            .into())
        }

        fn target(&self, current: &Values, target: &ProxyTarget) -> Result<Values, PlatformError> {
            let mut values = current.clone();
            if let Some(pac) = &target.pac {
                values.insert("flags".into(), (DIRECT | PAC).to_string());
                values.insert("pac".into(), pac.clone());
            } else {
                values.insert("flags".into(), (DIRECT | PROXY).to_string());
                values.insert("server".into(), server_list(target)?);
                let bypass = bypass_list(&target.bypass)?;
                values.insert("bypass".into(), bypass);
            }
            Ok(values)
        }

        fn write(&self, values: &Values) -> Result<(), PlatformError> {
            if values.is_empty() {
                return Ok(());
            }
            let mut strings = Vec::new();
            let mut options = Vec::new();
            for (key, value) in values {
                let option = match key.as_str() {
                    "flags" => 1,
                    "server" => 2,
                    "bypass" => 3,
                    "pac" => 4,
                    _ => {
                        return Err(PlatformError::InvalidState(
                            "未知 Windows 系统代理字段".into(),
                        ));
                    }
                };
                if option == 1 {
                    let number = value
                        .parse()
                        .map_err(|_| PlatformError::InvalidState("Windows 代理标志无效".into()))?;
                    options.push(ConnOption {
                        option,
                        value: OptionValue { number },
                    });
                } else {
                    if value.contains('\0') {
                        return Err(PlatformError::InvalidState(
                            "Windows 代理地址含空字符".into(),
                        ));
                    }
                    let mut encoded: Vec<u16> = value.encode_utf16().chain([0]).collect();
                    options.push(ConnOption {
                        option,
                        value: OptionValue {
                            string: encoded.as_mut_ptr(),
                        },
                    });
                    strings.push(encoded);
                }
            }
            let mut values = list(&mut options);
            // SAFETY: the option list and all UTF-16 strings outlive the synchronous call.
            if unsafe {
                InternetSetOptionW(
                    ptr::null_mut(),
                    PER_CONNECTION_OPTION,
                    (&mut values as *mut ConnOptions).cast(),
                    values.size,
                )
            } == 0
            {
                return Err(failure("写入系统代理"));
            }
            for option in [SETTINGS_CHANGED, REFRESH] {
                // SAFETY: these options require no handle and no input buffer.
                if unsafe { InternetSetOptionW(ptr::null_mut(), option, ptr::null_mut(), 0) } == 0 {
                    return Err(failure("刷新系统代理"));
                }
            }
            Ok(())
        }

        fn state(&self, current: &Values) -> SystemProxyState {
            let flags = current["flags"].parse::<u32>().unwrap_or(0);
            let manual = flags & PROXY != 0;
            let auto = flags & PAC != 0;
            SystemProxyState {
                services: vec![ServiceProxyState {
                    service: "WinINet / LAN".into(),
                    enabled: manual || auto,
                    http: manual
                        .then(|| endpoint(&current["server"], "http"))
                        .flatten(),
                    https: manual
                        .then(|| endpoint(&current["server"], "https"))
                        .flatten(),
                    socks: manual
                        .then(|| endpoint(&current["server"], "socks"))
                        .flatten(),
                    auto_url: auto.then(|| current["pac"].clone()),
                    auto_enabled: auto,
                    bypass: current["bypass"]
                        .split(';')
                        .filter(|item| !item.is_empty())
                        .map(str::to_owned)
                        .collect(),
                }],
            }
        }

        fn disable_owned(&self, expected: &ProxyEndpoint, current: &Values) -> Values {
            let flags = current["flags"].parse::<u32>().unwrap_or(0);
            let slots: Vec<_> = ["http", "https", "socks"]
                .into_iter()
                .filter_map(|scheme| endpoint(&current["server"], scheme))
                .collect();
            if flags & PROXY != 0 && !slots.is_empty() && slots.iter().all(|slot| slot == expected)
            {
                return [("flags".into(), (flags & !PROXY).to_string())].into();
            }
            Values::new()
        }
    }
}

#[cfg(windows)]
pub(super) use os::WinInet;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an isolated Windows test account and V2RAYR_TEST_WININET=1"]
    fn real_wininet_round_trip() {
        use crate::proxy::{
            SystemProxy, TakeoverSnapshot,
            native::{Backend, NativeProxy},
        };
        assert_eq!(
            std::env::var("V2RAYR_TEST_WININET").as_deref(),
            Ok("1"),
            "use an isolated Windows test account"
        );
        struct Restore<'a>(&'a dyn SystemProxy, TakeoverSnapshot);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                if let Err(error) = self.0.restore(&self.1) {
                    eprintln!("WinINet fixture cleanup failed: {error}");
                }
            }
        }
        let proxy = NativeProxy(WinInet);
        let before = proxy.0.read().unwrap();
        for target in [
            ProxyTarget::all_through("127.0.0.1", 10808),
            ProxyTarget::through_pac("http://127.0.0.1:10811/proxy.pac", vec![]),
        ] {
            let guard = Restore(&proxy, proxy.apply(&target).unwrap());
            assert!(proxy.capture().unwrap().is_enabled());
            drop(guard);
            assert_eq!(proxy.0.read().unwrap(), before);
        }
    }

    #[test]
    fn cidr_bypass_uses_only_wininet_patterns() {
        let bypass = bypass_list(&[
            "10.0.0.0/8".into(),
            "172.16.0.0/12".into(),
            "192.0.2.11/31".into(),
        ])
        .unwrap();
        let patterns: Vec<_> = bypass.split(';').collect();
        assert_eq!(patterns.len(), 19);
        assert_eq!(patterns[0], "10.*");
        assert_eq!(patterns[1], "172.16.*");
        assert_eq!(patterns[16], "172.31.*");
        assert_eq!(&patterns[17..], &["192.0.2.10", "192.0.2.11"]);
    }

    #[test]
    fn distinct_http_and_socks_endpoints_round_trip() {
        let mut target = ProxyTarget::all_through("127.0.0.1", 10808);
        target.http = Some(ProxyEndpoint::new("127.0.0.1", 10809));
        target.https = Some(ProxyEndpoint::new("::1", 10810));
        let value = server_list(&target).unwrap();
        assert_eq!(endpoint(&value, "http"), target.http);
        assert_eq!(endpoint(&value, "https"), target.https);
        assert_eq!(endpoint(&value, "socks"), target.socks);
    }
}
