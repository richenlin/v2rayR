//! Traffic samples behind the connection page's chart.
//!
//! Samples are written once per second while connected and pruned to a bounded
//! window, so the table stays small enough to keep the whole thing vacuum-free.

use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::Database;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrafficSample {
    pub ts: DateTime<Utc>,
    pub down_bytes: u64,
    pub up_bytes: u64,
}

/// Keep an hour of history: enough to redraw the chart after a restart without
/// letting the table grow without bound.
/// How long samples are kept. One hour at 1 Hz is enough for the sparkline and
/// keeps the table small.
#[cfg(test)]
pub const RETENTION: chrono::Duration = chrono::Duration::hours(1);

impl Database {
    pub fn record_traffic(
        &self,
        ts: DateTime<Utc>,
        down_bytes: u64,
        up_bytes: u64,
    ) -> Result<(), crate::StoreError> {
        self.conn().execute(
            "INSERT OR REPLACE INTO traffic_samples (ts, down_bytes, up_bytes) VALUES (?1, ?2, ?3)",
            params![ts.timestamp_millis(), down_bytes as i64, up_bytes as i64],
        )?;
        Ok(())
    }

    pub fn recent_traffic(
        &self,
        since: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<TrafficSample>, crate::StoreError> {
        let mut statement = self.conn().prepare(
            "SELECT ts, down_bytes, up_bytes FROM traffic_samples WHERE ts >= ?1
             ORDER BY ts DESC LIMIT ?2",
        )?;
        let mut rows = statement.query(params![since.timestamp_millis(), limit as i64])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            let ts: i64 = row.get(0)?;
            let down: i64 = row.get(1)?;
            let up: i64 = row.get(2)?;
            items.push(TrafficSample {
                ts: DateTime::from_timestamp_millis(ts).unwrap_or_else(Utc::now),
                down_bytes: down.max(0) as u64,
                up_bytes: up.max(0) as u64,
            });
        }
        items.reverse();
        Ok(items)
    }

    pub fn prune_traffic(&self, before: DateTime<Utc>) -> Result<usize, crate::StoreError> {
        Ok(self.conn().execute(
            "DELETE FROM traffic_samples WHERE ts < ?1",
            params![before.timestamp_millis()],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn samples_come_back_in_time_order() {
        let db = Database::open_in_memory().expect("db");
        let now = Utc::now();
        for offset in 0..5 {
            db.record_traffic(now - Duration::seconds(4 - offset), offset as u64, 0)
                .expect("record");
        }
        let samples = db
            .recent_traffic(now - Duration::minutes(1), 10)
            .expect("recent");
        assert_eq!(samples.len(), 5);
        assert_eq!(samples[0].down_bytes, 0);
        assert_eq!(samples[4].down_bytes, 4);
    }

    #[test]
    fn pruning_drops_old_samples_only() {
        let db = Database::open_in_memory().expect("db");
        let now = Utc::now();
        db.record_traffic(now - Duration::hours(2), 1, 1)
            .expect("old");
        db.record_traffic(now, 2, 2).expect("new");
        let removed = db.prune_traffic(now - RETENTION).expect("prune");
        assert_eq!(removed, 1);
        assert_eq!(
            db.recent_traffic(now - Duration::days(1), 10)
                .expect("recent")
                .len(),
            1
        );
    }
}
