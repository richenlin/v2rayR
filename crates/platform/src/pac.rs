//! PAC (proxy auto-config) support.
//!
//! The design document asks for a PAC takeover that "同时关闭系统手动代理，避免双重
//! 生效". macOS' `networksetup` accepts any URL, including `file://`, but Safari and
//! Chromium refuse file-based PAC scripts, so we serve the script over loopback —
//! this is also the only way a PAC keeps working while the app is running without
//! touching the filesystem permissions of other users.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;

use crate::PlatformError;

/// Build the PAC script.
///
/// Local and private destinations always go direct (a proxy client that sends
/// `127.0.0.1` through itself is a loop), everything else uses the inbound.
pub fn render_pac(proxy_host: &str, proxy_port: u16, bypass: &[String]) -> String {
    render_pac_with_rules(proxy_host, proxy_port, bypass, None)
}

#[derive(Debug, Clone)]
pub enum PacMatch {
    Domain(String),
    DomainSuffix(String),
    Ip(std::net::IpAddr, u8),
}

#[derive(Debug, Clone)]
pub struct PacOverride {
    pub target: PacMatch,
    /// Block uses the local proxy as well, where the kernel rejects the request.
    pub direct: bool,
}

pub fn render_pac_with_rules(
    proxy_host: &str,
    proxy_port: u16,
    bypass: &[String],
    rules: Option<&[Vec<Vec<String>>]>,
) -> String {
    render_pac_with_overrides(proxy_host, proxy_port, bypass, rules, &[])
}

pub fn render_pac_with_overrides(
    proxy_host: &str,
    proxy_port: u16,
    bypass: &[String],
    rules: Option<&[Vec<Vec<String>>]>,
    overrides: &[PacOverride],
) -> String {
    let mut conditions = vec![
        "  if (host === \"localhost\" || host === \"127.0.0.1\" || host === \"::1\" || host === \"[::1]\") return DIRECT;".to_owned(),
    ];
    if bypass.iter().any(|item| item == "localhost") {
        conditions.push("  if (isPlainHostName(host)) return DIRECT;".to_owned());
    }
    let quote = |value: &str| serde_json::to_string(value).expect("string serialization");
    for entry in bypass {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if let Some((network, bits)) = entry.split_once('/') {
            if let Some(mask) = cidr_to_mask(network, bits) {
                conditions.push(format!(
                    "  if (isInNet(host, {}, {})) return DIRECT;",
                    quote(network),
                    quote(&mask)
                ));
            }
        } else if let Some(suffix) = entry.strip_prefix("*.") {
            conditions.push(format!(
                "  if (dnsDomainIs(host, {})) return DIRECT;",
                quote(&format!(".{suffix}"))
            ));
        } else if entry.contains('*') || entry.contains('?') {
            conditions.push(format!(
                "  if (shExpMatch(host, {})) return DIRECT;",
                quote(entry)
            ));
        } else {
            conditions.push(format!(
                "  if (host === {} || dnsDomainIs(host, {})) return DIRECT;",
                quote(entry),
                quote(&format!(".{entry}"))
            ));
        }
    }
    let destination = quote(&format!("PROXY {proxy_host}:{proxy_port}"));
    let custom = overrides.iter().map(|rule| {
        let condition = match &rule.target {
            PacMatch::Domain(domain) => format!("host === {}", quote(domain)),
            PacMatch::DomainSuffix(domain) => format!(
                "host === {} || dnsDomainIs(host, {})", quote(domain), quote(&format!(".{domain}")),
            ),
            PacMatch::Ip(std::net::IpAddr::V4(ip), bits) => {
                let ip = ip.to_string();
                let Some(mask) = cidr_to_mask(&ip, &bits.to_string()) else { return String::new() };
                format!("/^([0-9]{{1,3}}\\.){{3}}[0-9]{{1,3}}$/.test(host) && isInNet(host, {}, {})", quote(&ip), quote(&mask))
            }
            PacMatch::Ip(std::net::IpAddr::V6(ip), bits) => format!(
                "ipv6InNet(host, {}, {bits})",
                serde_json::to_string(&ip.segments()).expect("segments serialization"),
            ),
        };
        let action = if rule.direct { "DIRECT" } else { &destination };
        format!("  if ({condition}) return {action};\n")
    }).collect::<String>();
    let ipv6 = if overrides
        .iter()
        .any(|rule| matches!(rule.target, PacMatch::Ip(std::net::IpAddr::V6(_), _)))
    {
        IPV6_MATCHER
    } else {
        ""
    };
    let decision = match rules {
        Some(rules) => format!(
            "  var rules = {};\n\
             for (var i = 0; i < rules.length; i++) {{\n\
               for (var j = 0; j < rules[i].length; j++) {{\n\
                 for (var k = 0; k < rules[i][j].length; k++) {{\n\
                   var domain = rules[i][j][k];\n\
                   if (host === domain || dnsDomainIs(host, '.' + domain)) return j % 2 === 0 ? DIRECT : {destination};\n\
                 }}\n\
               }}\n\
             }}\n\
             return DIRECT;",
            serde_json::to_string(rules).expect("domain serialization")
        ),
        None => format!("  return {destination};"),
    };
    format!(
        "// Generated by v2rayR. Do not edit; it is rewritten on every connect.\n\
{ipv6}function FindProxyForURL(url, host) {{\n  var DIRECT = 'DIRECT';\n  host = host.toLowerCase().replace(/\\.$/, '');\n{}\n{custom}{decision}\n}}\n",
        conditions.join("\n")
    )
}

// PAC engines may omit isInNetEx, so compare IPv6 literals locally.
const IPV6_MATCHER: &str = r#"function ipv6InNet(host, network, bits) {
  host = host.replace(/^\[|\]$/g, '');
  if (host.indexOf(':') < 0 || !/^[0-9a-f:]+$/.test(host)) return false;
  var halves = host.split('::');
  if (halves.length > 2) return false;
  var left = halves[0] ? halves[0].split(':') : [];
  var right = halves.length === 2 && halves[1] ? halves[1].split(':') : [];
  var zeros = 8 - left.length - right.length;
  if (halves.length === 1 ? zeros !== 0 : zeros < 1) return false;
  while (zeros-- > 0) left.push('0');
  var segments = left.concat(right);
  for (var i = 0; i < 8; i++) {
    if (!/^[0-9a-f]{1,4}$/.test(segments[i])) return false;
    if (bits > 0) {
      var shift = 16 - Math.min(bits, 16);
      if ((parseInt(segments[i], 16) >>> shift) !== (network[i] >>> shift)) return false;
      bits -= 16;
    }
  }
  return true;
}
"#;

/// Convert `10.0.0.0/8` into `255.0.0.0`; returns `None` for malformed input.
fn cidr_to_mask(network: &str, bits: &str) -> Option<String> {
    let bits: u32 = bits.trim().parse().ok()?;
    if bits > 32 || network.parse::<std::net::Ipv4Addr>().is_err() {
        return None;
    }
    let mask = if bits == 0 {
        0
    } else {
        u32::MAX << (32 - bits)
    };
    Some(format!(
        "{}.{}.{}.{}",
        mask >> 24 & 0xff,
        mask >> 16 & 0xff,
        mask >> 8 & 0xff,
        mask & 0xff
    ))
}

/// A tiny loopback HTTP server that serves exactly one PAC document.
///
/// It is deliberately dumb: one file, no keep-alive, no request parsing beyond the
/// request line. That is all a system proxy setting needs, and it keeps the
/// process free of an async HTTP stack on this path.
#[derive(Debug)]
pub struct PacServer {
    port: u16,
    body: Arc<RwLock<Arc<str>>>,
    stop: Arc<AtomicBool>,
    #[allow(dead_code)]
    handle: Option<JoinHandle<()>>,
}

impl PacServer {
    /// Bind `port` (0 picks a free one) and start serving `body`.
    pub fn start(port: u16, body: String) -> Result<Self, PlatformError> {
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|error| {
            PlatformError::Io(std::io::Error::other(format!(
                "PAC 服务无法监听 127.0.0.1:{port}: {error}"
            )))
        })?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let body = Arc::new(RwLock::new(Arc::<str>::from(body)));
        let current = body.clone();
        let handle = std::thread::Builder::new()
            .name("v2rayr-pac".to_owned())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let body = current.read().expect("PAC body").clone();
                            serve(stream, &body);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(40));
                        }
                        Err(_) => std::thread::sleep(std::time::Duration::from_millis(80)),
                    }
                }
            })
            .map_err(|error| {
                PlatformError::Io(std::io::Error::other(format!("PAC 线程启动失败: {error}")))
            })?;
        Ok(Self {
            port,
            body,
            stop,
            handle: Some(handle),
        })
    }

    pub fn replace_body(&self, body: String) {
        *self.body.write().expect("PAC body") = body.into();
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/proxy.pac", self.port)
    }

    /// Stop accepting and wake the loop; idempotent because `Drop` calls it too.
    pub fn stop(&mut self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        // A connect unblocks `accept` immediately instead of waiting for the next
        // poll interval.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PacServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(mut stream: TcpStream, body: &str) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
    let mut buffer = [0u8; 1024];
    let _ = stream.read(&mut buffer);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-ns-proxy-autoconfig\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_decisions_precede_downloaded_rules() {
        let downloaded = vec![vec![
            vec!["prefer.example".into()],
            vec!["block.example".into()],
        ]];
        let overrides = [
            PacOverride {
                target: PacMatch::Domain("prefer.example".into()),
                direct: false,
            },
            PacOverride {
                target: PacMatch::DomainSuffix("block.example".into()),
                direct: true,
            },
            PacOverride {
                target: PacMatch::Domain("reject.example".into()),
                direct: false,
            },
            PacOverride {
                target: PacMatch::Ip("203.0.113.0".parse().unwrap(), 24),
                direct: false,
            },
            PacOverride {
                target: PacMatch::Ip("2001:db8::".parse().unwrap(), 32),
                direct: false,
            },
        ];
        let script =
            render_pac_with_overrides("127.0.0.1", 10808, &[], Some(&downloaded), &overrides);
        let pac_list = script.find("var rules").unwrap();
        for condition in [
            "if (host === \"prefer.example\") return \"PROXY 127.0.0.1:10808\";",
            "if (host === \"block.example\" || dnsDomainIs(host, \".block.example\")) return DIRECT;",
            "if (host === \"reject.example\") return \"PROXY 127.0.0.1:10808\";",
        ] {
            assert!(script.find(condition).unwrap() < pac_list);
        }
        if let Some(path) = std::env::var_os("V2RAYR_TEST_PAC_SCRIPT") {
            std::fs::write(path, script).unwrap();
        }
    }

    #[test]
    fn script_covers_bypass_forms() {
        let pac = render_pac(
            "127.0.0.1",
            10808,
            &[
                "127.0.0.1".to_owned(),
                "localhost".to_owned(),
                "*.local".to_owned(),
                "10.0.0.0/8".to_owned(),
                "*.corp.example.com".to_owned(),
            ],
        );
        assert!(pac.starts_with("// Generated by v2rayR"));
        assert!(pac.contains("return \"PROXY 127.0.0.1:10808\";"));
        assert!(pac.contains("isPlainHostName(host)"));
        assert!(pac.contains("dnsDomainIs(host, \".local\")"));
        assert!(pac.contains("isInNet(host, \"10.0.0.0\", \"255.0.0.0\")"));
        assert!(pac.contains("dnsDomainIs(host, \".corp.example.com\")"));
    }

    #[test]
    fn malformed_cidr_is_ignored() {
        let pac = render_pac("127.0.0.1", 10808, &["10.0.0.0/99".to_owned()]);
        assert!(!pac.contains("isInNet"));
    }

    #[test]
    fn the_loopback_server_answers_with_the_script() {
        let body = render_pac("127.0.0.1", 10808, &[]);
        let server = PacServer::start(0, body.clone()).expect("start");
        let url = server.url();
        assert!(url.ends_with("/proxy.pac"));
        let address = url
            .trim_start_matches("http://")
            .trim_end_matches("/proxy.pac");
        let mut stream = TcpStream::connect(address).expect("connect");
        write!(stream, "GET /proxy.pac HTTP/1.1\r\nHost: x\r\n\r\n").expect("write");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read");
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("application/x-ns-proxy-autoconfig"));
        assert!(response.contains("FindProxyForURL"));
        assert_eq!(
            &server.port().to_string(),
            address.split(':').nth(1).unwrap()
        );
        let updated = render_pac_with_rules(
            "127.0.0.1",
            10808,
            &[],
            Some(&[vec![
                vec!["direct.example".into()],
                vec!["proxy.example".into()],
            ]]),
        );
        server.replace_body(updated.clone());
        let mut stream = TcpStream::connect(address).expect("reconnect to same PAC address");
        write!(stream, "GET /proxy.pac HTTP/1.1\r\nHost: x\r\n\r\n").expect("write");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read updated rules");
        assert_eq!(response.split_once("\r\n\r\n").unwrap().1, updated);
    }
}
