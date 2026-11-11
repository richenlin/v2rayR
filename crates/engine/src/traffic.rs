//! Cumulative counters from the running node kernel.
//!
//! One probe lives for the whole session and is polled every second, so it
//! keeps a single loopback HTTP client (one kept-alive connection) and decodes
//! only the fields it needs instead of materialising the whole JSON document.
use std::{collections::HashMap, fmt, time::Duration};

use serde::{Deserialize, Deserializer, de};

use crate::EngineError;

#[derive(Debug, Default, Clone, Copy)]
pub struct Counters {
    pub down: u64,
    pub up: u64,
    pub connections: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// sing-box / mihomo `GET /connections`.
    Clash,
    /// Xray `metrics.listen` expvar endpoint, `GET /debug/vars`.
    XrayMetrics,
}

#[derive(Debug)]
pub struct TrafficProbe {
    http: reqwest::Client,
    url: String,
    secret: String,
    source: Source,
}

impl TrafficProbe {
    pub fn clash(port: u16, secret: impl Into<String>) -> Self {
        Self::new(format!("http://127.0.0.1:{port}/connections"), secret.into(), Source::Clash)
    }

    pub fn xray(port: u16) -> Self {
        Self::new(format!("http://127.0.0.1:{port}/debug/vars"), String::new(), Source::XrayMetrics)
    }

    fn new(url: String, secret: String, source: Source) -> Self {
        Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .pool_max_idle_per_host(1)
                .build()
                .expect("valid loopback HTTP client"),
            url,
            secret,
            source,
        }
    }

    pub async fn totals(&self) -> Result<Counters, EngineError> {
        let mut request = self.http.get(&self.url);
        if !self.secret.is_empty() {
            request = request.bearer_auth(&self.secret);
        }
        let body = request
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| EngineError::ControlApi(error.to_string()))?
            .bytes()
            .await
            .map_err(|error| EngineError::ControlApi(error.to_string()))?;
        match self.source {
            Source::Clash => parse_clash(&body),
            Source::XrayMetrics => parse_xray_metrics(&body),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClashConnections {
    download_total: u64,
    upload_total: u64,
    #[serde(default)]
    connections: SeqLen,
}

/// Length of a JSON array (or `null`) without allocating its elements.
#[derive(Default)]
struct SeqLen(u64);

impl<'de> Deserialize<'de> for SeqLen {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = SeqLen;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an array or null")
            }
            fn visit_unit<E: de::Error>(self) -> Result<SeqLen, E> {
                Ok(SeqLen(0))
            }
            fn visit_none<E: de::Error>(self) -> Result<SeqLen, E> {
                Ok(SeqLen(0))
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<SeqLen, A::Error> {
                let mut len = 0;
                while seq.next_element::<de::IgnoredAny>()?.is_some() {
                    len += 1;
                }
                Ok(SeqLen(len))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

fn parse_clash(body: &[u8]) -> Result<Counters, EngineError> {
    let parsed: ClashConnections = serde_json::from_slice(body)
        .map_err(|error| EngineError::ControlApi(format!("traffic counters: {error}")))?;
    Ok(Counters {
        down: parsed.download_total,
        up: parsed.upload_total,
        connections: parsed.connections.0,
    })
}

#[derive(Deserialize)]
struct XrayVars {
    #[serde(default)]
    stats: XrayStats,
}

#[derive(Default, Deserialize)]
struct XrayStats {
    #[serde(default)]
    inbound: HashMap<String, XrayLink>,
}

#[derive(Deserialize)]
struct XrayLink {
    #[serde(default)]
    uplink: u64,
    #[serde(default)]
    downlink: u64,
}

/// Inbounds that carry user traffic; the metrics listener itself is excluded.
const XRAY_USER_INBOUNDS: [&str; 5] = ["socks-in", "http-in", "lan-socks", "lan-http", "dokodemo-in"];

fn parse_xray_metrics(body: &[u8]) -> Result<Counters, EngineError> {
    let parsed: XrayVars = serde_json::from_slice(body)
        .map_err(|error| EngineError::ControlApi(format!("Xray metrics: {error}")))?;
    let mut counters = Counters::default();
    for (tag, link) in &parsed.stats.inbound {
        if XRAY_USER_INBOUNDS.contains(&tag.as_str()) {
            counters.up = counters.up.saturating_add(link.uplink);
            counters.down = counters.down.saturating_add(link.downlink);
        }
    }
    Ok(counters)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clash_counts_connections_without_reading_them() {
        let body = br#"{"downloadTotal":10,"uploadTotal":4,"connections":[{"id":"a","metadata":{}},{"id":"b"}],"memory":1}"#;
        let counters = parse_clash(body).expect("parse");
        assert_eq!((counters.down, counters.up, counters.connections), (10, 4, 2));
    }

    #[test]
    fn clash_accepts_null_connections() {
        let counters = parse_clash(br#"{"downloadTotal":1,"uploadTotal":2,"connections":null}"#)
            .expect("parse");
        assert_eq!(counters.connections, 0);
    }

    #[test]
    fn clash_requires_totals() {
        assert!(parse_clash(br#"{"connections":[]}"#).is_err());
    }

    #[test]
    fn xray_metrics_sum_user_inbounds_only() {
        let body = br#"{"cmdline":["xray"],"memstats":{"Alloc":1,"BySize":[{"Size":0}]},
            "stats":{"inbound":{"socks-in":{"downlink":945,"uplink":88},"http-in":{"downlink":5,"uplink":2},
            "api":{"downlink":999,"uplink":999}},"outbound":{"proxy":{"downlink":1,"uplink":1}},"user":{}}}"#;
        let counters = parse_xray_metrics(body).expect("parse");
        assert_eq!((counters.down, counters.up), (950, 90));
    }

    #[test]
    fn xray_metrics_without_stats_reads_zero() {
        let counters = parse_xray_metrics(br#"{"memstats":{}}"#).expect("parse");
        assert_eq!((counters.down, counters.up), (0, 0));
    }
}
