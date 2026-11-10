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

/// WinINet returns `ERROR_INVALID_PARAMETER` for a bypass list that contains
/// CIDR (`/`) or IPv6 (`:`). The per-connection registry blob still stores it.
fn bypass_is_api_safe(value: &str) -> bool {
    !value.contains(['/', ':'])
}

fn blob_u32(blob: &[u8], at: usize) -> Option<u32> {
    let bytes: [u8; 4] = blob.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

/// Bypass bytes inside `DefaultConnectionSettings` / `SavedLegacySettings`.
fn connection_bypass(blob: &[u8]) -> Option<&[u8]> {
    let server_len = blob_u32(blob, 12)? as usize;
    let at = 16usize.checked_add(server_len)?;
    let len = blob_u32(blob, at)? as usize;
    let start = at.checked_add(4)?;
    blob.get(start..start.checked_add(len)?)
}

fn replace_connection_bypass(blob: &[u8], bypass: &[u8]) -> Option<Vec<u8>> {
    let server_len = blob_u32(blob, 12)? as usize;
    let at = 16usize.checked_add(server_len)?;
    let old_len = blob_u32(blob, at)? as usize;
    let after = at.checked_add(4)?.checked_add(old_len)?;
    if after > blob.len() || bypass.len() > u32::MAX as usize {
        return None;
    }
    let mut out = Vec::with_capacity(blob.len() - old_len + bypass.len());
    out.extend_from_slice(&blob[..4]);
    out.extend_from_slice(&blob_u32(blob, 4)?.wrapping_add(1).to_le_bytes());
    out.extend_from_slice(&blob[8..at]);
    out.extend_from_slice(&(bypass.len() as u32).to_le_bytes());
    out.extend_from_slice(bypass);
    out.extend_from_slice(&blob[after..]);
    Some(out)
}

#[cfg(windows)]
mod os {
    use std::{ffi::c_void, ptr};

    use super::{
        DIRECT, PAC, PROXY, bypass_is_api_safe, bypass_list, connection_bypass, endpoint,
        replace_connection_bypass, server_list,
    };
    use crate::{
        PlatformError,
        proxy::{
            ProxyEndpoint, ProxyTarget, ServiceProxyState, SystemProxyState,
            native::{Backend, NativeProxyKind, Values},
        },
    };
    use windows_sys::Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND,
            ERROR_SUCCESS,
        },
        System::Registry::{
            HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_BINARY, REG_SZ,
            RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW, RegOpenKeyExW,
            RegSetValueExW,
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
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            multi: *const u8,
            multi_len: i32,
            wide: *mut u16,
            wide_len: i32,
        ) -> i32;
        fn WideCharToMultiByte(
            code_page: u32,
            flags: u32,
            wide: *const u16,
            wide_len: i32,
            multi: *mut u8,
            multi_len: i32,
            default_char: *const u8,
            used_default: *mut i32,
        ) -> i32;
    }

    const SETTINGS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";
    const CONNECTIONS: &str =
        "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\\Connections";
    /// Not a Win32 code. Marks a proxy string that is not valid UTF-16.
    const BAD_UTF16: u32 = u32::MAX;

    fn failure(operation: &str, code: u32) -> PlatformError {
        PlatformError::InvalidState(format!(
            "Windows {operation}失败：{}",
            std::io::Error::from_raw_os_error(code as i32)
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

    fn query(option: u32, string: bool) -> Result<String, u32> {
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
            return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32);
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
        result.map_err(|_| BAD_UTF16)
    }

    fn field(operation: &str, result: Result<String, u32>) -> Result<String, PlatformError> {
        match result {
            Ok(value) => Ok(value),
            Err(BAD_UTF16) => Err(PlatformError::InvalidState(
                "Windows 系统代理地址不是有效 UTF-16".into(),
            )),
            Err(code) => Err(failure(operation, code)),
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    fn notify() -> Result<(), PlatformError> {
        for option in [SETTINGS_CHANGED, REFRESH] {
            // SAFETY: these options take no handle and no input buffer.
            if unsafe { InternetSetOptionW(ptr::null_mut(), option, ptr::null_mut(), 0) } == 0 {
                return Err(failure(
                    "刷新系统代理",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32,
                ));
            }
        }
        Ok(())
    }

    fn set_api(values: &Values) -> Result<(), PlatformError> {
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
        let mut list = list(&mut options);
        // SAFETY: the option list and all UTF-16 strings outlive the synchronous call.
        if unsafe {
            InternetSetOptionW(
                ptr::null_mut(),
                PER_CONNECTION_OPTION,
                (&mut list as *mut ConnOptions).cast(),
                list.size,
            )
        } == 0
        {
            return Err(failure(
                "写入系统代理",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32,
            ));
        }
        notify()
    }

    fn reg_bytes(subkey: &str, name: &str, flags: u32) -> Result<Option<Vec<u8>>, PlatformError> {
        let subkey = wide(subkey);
        let name = wide(name);
        let mut size = 0u32;
        let missing = |status| status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                name.as_ptr(),
                flags,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut size,
            )
        };
        if missing(status) {
            return Ok(None);
        }
        if status != ERROR_SUCCESS && status != ERROR_MORE_DATA {
            return Err(failure("读取注册表", status));
        }
        if size == 0 {
            return Ok(Some(Vec::new()));
        }
        let mut buffer = vec![0u8; size as usize];
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                name.as_ptr(),
                flags,
                ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if missing(status) {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(failure("读取注册表", status));
        }
        buffer.truncate(size as usize);
        Ok(Some(buffer))
    }

    fn reg_set(subkey: &str, name: &str, kind: u32, data: &[u8]) -> Result<(), PlatformError> {
        let subkey = wide(subkey);
        let name = wide(name);
        let mut key = ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                &mut key,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(failure("打开注册表", status));
        }
        let status = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                kind,
                data.as_ptr(),
                data.len() as u32,
            )
        };
        unsafe { RegCloseKey(key) };
        if status != ERROR_SUCCESS {
            return Err(failure("写入注册表", status));
        }
        Ok(())
    }

    fn decode_ansi(bytes: &[u8]) -> Result<String, PlatformError> {
        if bytes.is_empty() {
            return Ok(String::new());
        }
        if let Ok(text) = std::str::from_utf8(bytes) {
            return Ok(text.to_owned());
        }
        let len = unsafe {
            MultiByteToWideChar(0, 0, bytes.as_ptr(), bytes.len() as i32, ptr::null_mut(), 0)
        };
        if len <= 0 {
            return Err(PlatformError::InvalidState(
                "Windows 代理例外编码无效".into(),
            ));
        }
        let mut wide = vec![0u16; len as usize];
        let wrote = unsafe {
            MultiByteToWideChar(
                0,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                wide.as_mut_ptr(),
                len,
            )
        };
        if wrote != len {
            return Err(PlatformError::InvalidState(
                "Windows 代理例外编码无效".into(),
            ));
        }
        String::from_utf16(&wide)
            .map_err(|_| PlatformError::InvalidState("Windows 代理例外不是有效 UTF-16".into()))
    }

    fn encode_ansi(value: &str) -> Result<Vec<u8>, PlatformError> {
        if value.is_ascii() {
            return Ok(value.as_bytes().to_vec());
        }
        let wide: Vec<u16> = value.encode_utf16().collect();
        let default_char = b"?";
        let mut used = 0i32;
        let len = unsafe {
            WideCharToMultiByte(
                0,
                0,
                wide.as_ptr(),
                wide.len() as i32,
                ptr::null_mut(),
                0,
                default_char.as_ptr(),
                &mut used,
            )
        };
        if len <= 0 {
            return Err(PlatformError::InvalidState(
                "Windows 代理例外无法写入连接记录".into(),
            ));
        }
        let mut bytes = vec![0u8; len as usize];
        let wrote = unsafe {
            WideCharToMultiByte(
                0,
                0,
                wide.as_ptr(),
                wide.len() as i32,
                bytes.as_mut_ptr(),
                len,
                default_char.as_ptr(),
                &mut used,
            )
        };
        if wrote != len || used != 0 {
            return Err(PlatformError::InvalidState(
                "Windows 代理例外含当前代码页无法表示的字符".into(),
            ));
        }
        Ok(bytes)
    }

    /// The LAN connection blob is what WinINet actually applies. `ProxyOverride`
    /// is only the legacy mirror, used when that blob has never been created.
    fn stored_bypass() -> Result<String, PlatformError> {
        if let Some(blob) = reg_bytes(CONNECTIONS, "DefaultConnectionSettings", RRF_RT_REG_BINARY)?
        {
            let bytes = connection_bypass(&blob).ok_or_else(|| {
                PlatformError::InvalidState("Windows 连接代理记录无法解析".into())
            })?;
            return decode_ansi(bytes);
        }
        let Some(bytes) = reg_bytes(SETTINGS, "ProxyOverride", RRF_RT_REG_SZ)? else {
            return Ok(String::new());
        };
        if bytes.len() % 2 != 0 {
            return Err(PlatformError::InvalidState(
                "Windows 代理例外编码无效".into(),
            ));
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        let end = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        String::from_utf16(&units[..end])
            .map_err(|_| PlatformError::InvalidState("Windows 代理例外不是有效 UTF-16".into()))
    }

    fn patch_bypass(value: &str) -> Result<(), PlatformError> {
        let ansi = encode_ansi(value)?;
        let mut pending = Vec::new();
        for name in ["DefaultConnectionSettings", "SavedLegacySettings"] {
            match reg_bytes(CONNECTIONS, name, RRF_RT_REG_BINARY)? {
                Some(blob) => {
                    let next = replace_connection_bypass(&blob, &ansi).ok_or_else(|| {
                        PlatformError::InvalidState("Windows 连接代理记录无法解析".into())
                    })?;
                    pending.push((name, next));
                }
                None if name == "DefaultConnectionSettings" => {
                    return Err(PlatformError::InvalidState(
                        "Windows 没有可写回的连接代理记录".into(),
                    ));
                }
                None => {}
            }
        }
        for (name, blob) in &pending {
            reg_set(CONNECTIONS, name, REG_BINARY, blob)?;
        }
        let encoded: Vec<u8> = wide(value)
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        reg_set(SETTINGS, "ProxyOverride", REG_SZ, &encoded)?;
        notify()
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
            let flags = query(10, false).or_else(|_| query(1, false));
            let server = query(2, true);
            let bypass = query(3, true);
            let pac = query(4, true);
            Ok([
                ("flags".into(), field("读取系统代理", flags)?),
                ("server".into(), field("读取系统代理", server)?),
                (
                    "bypass".into(),
                    match bypass {
                        Ok(value) => value,
                        Err(ERROR_INVALID_PARAMETER) => stored_bypass()?,
                        Err(code) => field("读取系统代理", Err(code))?,
                    },
                ),
                ("pac".into(), field("读取系统代理", pac)?),
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
            let deferred_bypass = values
                .get("bypass")
                .filter(|value| !bypass_is_api_safe(value))
                .cloned();
            let mut api_values = values.clone();
            if deferred_bypass.is_some() {
                api_values.remove("bypass");
            }
            if !api_values.is_empty() {
                set_api(&api_values)?;
            }
            if let Some(bypass) = deferred_bypass {
                patch_bypass(&bypass)?;
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
    fn read_keeps_a_bypass_list_wininet_rejects() {
        use crate::proxy::native::Backend;
        let values = os::WinInet.read().expect("read current proxy");
        assert!(values.contains_key("flags"));
        assert!(values.contains_key("server"));
        assert!(values.contains_key("pac"));
        let bypass = values.get("bypass").expect("bypass");
        if !bypass_is_api_safe(bypass) {
            assert!(bypass.contains('/') || bypass.contains(':'));
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "rewrites the current user's proxy; set V2RAYR_TEST_WININET=1"]
    fn unsafe_bypass_round_trip_keeps_the_other_fields() {
        use crate::proxy::native::Backend;
        assert_eq!(
            std::env::var("V2RAYR_TEST_WININET").as_deref(),
            Ok("1"),
            "rewrites the current user's proxy settings"
        );
        let before = os::WinInet.read().expect("read");
        let bypass = before["bypass"].clone();
        if bypass_is_api_safe(&bypass) {
            return;
        }
        os::WinInet.write(&before).expect("write the snapshot back");
        let after = os::WinInet.read().expect("reread");
        assert_eq!(after.get("bypass"), Some(&bypass));
        assert_eq!(after.get("flags"), before.get("flags"));
        assert_eq!(after.get("server"), before.get("server"));
        assert_eq!(after.get("pac"), before.get("pac"));
    }

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
    fn cidr_and_ipv6_bypass_is_kept_outside_the_wininet_api() {
        let original = b"<local>;localhost,127.0.0.0/8,::1";
        assert!(!bypass_is_api_safe(std::str::from_utf8(original).unwrap()));
        assert!(bypass_is_api_safe("<local>;10.*;172.16.*"));
        let mut blob = Vec::new();
        blob.extend_from_slice(&70u32.to_le_bytes());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&5u32.to_le_bytes());
        blob.extend_from_slice(&(14u32).to_le_bytes());
        blob.extend_from_slice(b"127.0.0.1:1080");
        blob.extend_from_slice(&(original.len() as u32).to_le_bytes());
        blob.extend_from_slice(original);
        blob.extend_from_slice(&4u32.to_le_bytes());
        blob.extend_from_slice(b"pac!");
        blob.extend_from_slice(&[9, 9, 9, 9]);
        assert_eq!(connection_bypass(&blob), Some(original.as_slice()));
        let updated = replace_connection_bypass(&blob, b"10.*;<local>").unwrap();
        assert_eq!(blob_u32(&updated, 4), Some(2));
        assert_eq!(
            connection_bypass(&updated),
            Some(b"10.*;<local>".as_slice())
        );
        assert!(updated.windows(14).any(|part| part == b"127.0.0.1:1080"));
        assert!(updated.ends_with(&[9, 9, 9, 9]));
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
