//! Small formatting helpers shared by the CLI and the desktop shell.

/// `1536` → `1.5 KB/s`. Kept in the app layer so both front ends agree.
pub fn rate(bytes_per_second: u64) -> String {
    const UNITS: [&str; 5] = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s"];
    let mut value = bytes_per_second as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes_per_second, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `1536` → `1.5 KB` (totals, not rates).
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Strip credentials from a URL before it reaches a log line.
pub fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    match rest.split_once('@') {
        Some((_, host)) => format!("{scheme}://***@{host}"),
        None => url.to_owned(),
    }
}

pub fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_use_binary_units() {
        assert_eq!(rate(0), "0 B/s");
        assert_eq!(rate(1023), "1023 B/s");
        assert_eq!(rate(1536), "1.5 KB/s");
        assert_eq!(rate(1024 * 1024 * 3), "3.0 MB/s");
    }

    #[test]
    fn sizes_do_not_lie_about_bytes() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(2048), "2.0 KB");
    }

    #[test]
    fn urls_lose_their_credentials() {
        assert_eq!(
            redact_url("https://user:pass@example.com/sub"),
            "https://***@example.com/sub"
        );
        assert_eq!(
            redact_url("https://example.com/sub"),
            "https://example.com/sub"
        );
    }

    #[test]
    fn truncation_keeps_char_boundaries() {
        assert_eq!(truncate("中文节点名称", 3), "中文…");
        assert_eq!(truncate("短", 5), "短");
    }
}
