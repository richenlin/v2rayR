use crate::{App, settings::write_atomic};
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

impl App {
    pub(crate) fn pac_rules(&self) -> Option<Rules> {
        let path = self.paths().config_dir().join("pac-rules.json");
        if path.metadata().ok()?.len() > MAX_BYTES as u64 {
            return None;
        }
        parse_rules(&std::fs::read_to_string(path).ok()?).ok()
    }

    pub async fn update_pac_rules(&self) -> Result<usize, Problem> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Problem::network("无法创建 PAC 更新请求"))?;
        let mut response = client
            .get(SOURCE)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|_| Problem::network("无法下载 v2rayN PAC 规则"))?;
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
        self.reload_active_pac(&rules);
        Ok(rules.iter().flatten().map(Vec::len).sum())
    }
}
