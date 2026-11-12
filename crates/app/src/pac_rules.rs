use crate::{App, session::LocalHttpProxy, settings::write_atomic};
use std::io::Read;
use v2rayr_ipc::Problem;

const SOURCE: &str =
    "https://raw.githubusercontent.com/2dust/v2rayN/master/v2rayN/ServiceLib/Sample/pac";
const MAX_BYTES: usize = 2 * 1024 * 1024;
pub(crate) type Rules = Vec<Vec<Vec<String>>>;

fn parse_rules(json: &str) -> Result<Rules, Problem> {
    let mut rules: Rules =
        serde_json::from_str(json).map_err(|_| Problem::config("PAC 规则格式无效"))?;
    let mut count = 0;
    for group in &mut rules {
        if group.len() != 2 {
            return Err(Problem::config("PAC 规则分组格式无效"));
        }
        for domain in group.iter_mut().flatten() {
            *domain = domain.trim_end_matches('.').to_ascii_lowercase();
            if domain.is_empty()
                || domain.len() > 253
                || !domain.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
                        })
                })
            {
                return Err(Problem::config("PAC 包含无法识别的域名"));
            }
            count += 1;
        }
    }
    if count == 0 || count > 50_000 {
        return Err(Problem::config("PAC 域名数量超出范围"));
    }
    Ok(rules)
}

fn read_cached_rules(reader: impl Read) -> Result<Rules, Problem> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(crate::error::internal)?;
    if bytes.len() > MAX_BYTES {
        return Err(Problem::config("PAC 缓存超过大小限制"));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| Problem::config("PAC 缓存不是 UTF-8"))?;
    parse_rules(text)
}

impl App {
    pub(crate) fn pac_rules(&self) -> Result<Option<Rules>, Problem> {
        let path = self.paths().config_dir().join("pac-rules.json");
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(crate::error::internal(error)),
        };
        read_cached_rules(file).map(Some)
    }

    pub async fn update_pac_rules(&self) -> Result<usize, Problem> {
        let port = match self.session.lock().expect("session").local_http_proxy() {
            LocalHttpProxy::Ready(port) => port,
            LocalHttpProxy::NotConnected => {
                return Err(Problem::network(
                    "请先连接节点。PAC 规则需要经过本地代理下载",
                ));
            }
            LocalHttpProxy::Direct => {
                return Err(Problem::network(
                    "当前是直连分流，PAC 规则下载会绕过代理节点。请改用规则或全局模式后再更新",
                ));
            }
            LocalHttpProxy::MissingPort => {
                return Err(Problem::network("本地代理端口不可用，无法下载 PAC 规则"));
            }
        };
        // Explicit proxy, not the system PAC/WinINet setting. reqwest does not
        // evaluate a PAC script, and without the system-proxy feature it also
        // ignores the proxy this app writes into the OS.
        let proxy = reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))
            .map_err(|_| Problem::network("无法创建 PAC 更新请求"))?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .connect_timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .proxy(proxy)
            .build()
            .map_err(|_| Problem::network("无法创建 PAC 更新请求"))?;
        let mut response = client
            .get(SOURCE)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| {
                Problem::network(format!("无法经本地代理下载 v2rayN PAC 规则：{error}"))
            })?;
        if !response.status().is_success() {
            return Err(Problem::network("PAC 来源返回意外状态"));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Problem::network("PAC 下载中断"))?
        {
            if body.len() + chunk.len() > MAX_BYTES {
                return Err(Problem::config("PAC 文件超出大小限制"));
            }
            body.extend_from_slice(&chunk);
        }
        let text = std::str::from_utf8(&body).map_err(|_| Problem::config("PAC 文件不是 UTF-8"))?;
        let json = text
            .split_once("var rules =")
            .and_then(|(_, tail)| tail.split_once(';'))
            .map(|(json, _)| json)
            .ok_or_else(|| Problem::config("PAC 来源未提供域名规则表"))?;
        let rules = parse_rules(json)?;
        let encoded = serde_json::to_vec(&rules).map_err(crate::error::internal)?;
        let _guard = self.op_lock.lock().await;
        write_atomic(&self.paths().config_dir().join("pac-rules.json"), &encoded)
            .map_err(crate::error::internal)?;
        self.logs.info("routing", "PAC 规则已保存，下次连接生效");
        Ok(rules.iter().flatten().map(Vec::len).sum())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use v2rayr_ipc::RoutingPolicy;

    use crate::test_support::temp_app;

    #[test]
    fn cache_reader_is_bounded_even_without_file_metadata() {
        let mut reader = std::io::repeat(b' ');
        assert!(
            super::read_cached_rules(&mut reader)
                .unwrap_err()
                .detail
                .unwrap()
                .contains("大小限制")
        );
        assert!(
            super::read_cached_rules(&b"\xff"[..])
                .unwrap_err()
                .detail
                .unwrap()
                .contains("UTF-8")
        );
    }
    #[test]
    fn cache_missing_corrupt_and_oversized_are_distinct() {
        let app = temp_app();
        assert!(app.pac_rules().unwrap().is_none());
        let path = app.paths().config_dir().join("pac-rules.json");
        std::fs::write(&path, "broken").unwrap();
        assert!(app.pac_rules().is_err());
        std::fs::write(&path, " ".repeat(super::MAX_BYTES + 1)).unwrap();
        assert!(
            app.pac_rules()
                .unwrap_err()
                .detail
                .unwrap()
                .contains("大小限制")
        );
        std::fs::write(&path, r#"[[["Example.COM."],["proxy.net"]]]"#).unwrap();
        assert_eq!(app.pac_rules().unwrap().unwrap()[0][0][0], "example.com");
    }
    #[tokio::test]
    async fn download_is_refused_until_a_node_is_connected() {
        let error = temp_app()
            .update_pac_rules()
            .await
            .expect_err("not connected");
        assert!(
            error.detail.as_deref().unwrap_or("").contains("请先连接"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn direct_policy_does_not_pretend_the_download_is_proxied() {
        let app = temp_app();
        app.session
            .lock()
            .expect("session")
            .set_download_proxy_for_test(Some(9), None, RoutingPolicy::Direct);
        let error = app.update_pac_rules().await.expect_err("direct");
        assert!(
            error.detail.as_deref().unwrap_or("").contains("直连"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn download_connects_through_the_http_inbound_not_the_socks_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let http_port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(String::new()));
        let captured = Arc::clone(&seen);
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = [0u8; 2048];
            let size = socket.read(&mut buffer).await.expect("read proxy request");
            *captured.lock().expect("seen") = String::from_utf8_lossy(&buffer[..size]).into_owned();
            let _ = socket
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        });

        let app = temp_app();
        app.session
            .lock()
            .expect("session")
            .set_download_proxy_for_test(Some(http_port), Some(1), RoutingPolicy::Rule);
        let error = app.update_pac_rules().await.expect_err("upstream rejected");
        let request = seen.lock().expect("seen").clone();
        assert!(
            request.starts_with("CONNECT raw.githubusercontent.com:443 "),
            "request did not enter the local HTTP proxy: {request:?} ({error})"
        );
        assert!(
            error.detail.as_deref().unwrap_or("").contains("本地代理"),
            "{error}"
        );
    }
}
