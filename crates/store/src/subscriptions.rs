//! Subscription rows and their per-subscription state.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{OptionalExtension, Row, params};
use uuid::Uuid;

use crate::profiles::{ts_from_str, ts_to_str};
use crate::{Database, StoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSubscription {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub auto_update: bool,
    pub interval_hours: u32,
    pub last_updated_at: Option<DateTime<Utc>>,
    pub next_update_at: Option<DateTime<Utc>>,
    /// Serialised `Problem`; kept as JSON so the store does not depend on the
    /// wire crate.
    pub last_error: Option<String>,
    /// Serialised quota object (upload/download/total/expire).
    pub quota: Option<String>,
    pub format: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl StoredSubscription {
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.enabled
            && self.auto_update
            && self.next_update_at.map(|next| next <= now).unwrap_or(true)
    }

    pub fn next_due(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        now + Duration::hours(self.interval_hours.max(1) as i64)
    }
}

pub struct SubscriptionUpsert {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub auto_update: bool,
    pub interval_hours: u32,
}

fn row_to_subscription(row: &Row<'_>) -> rusqlite::Result<StoredSubscription> {
    let id: String = row.get("id")?;
    let last_updated_at: Option<String> = row.get("last_updated_at")?;
    let next_update_at: Option<String> = row.get("next_update_at")?;
    let created_at: String = row.get("created_at")?;
    let domain_error = |detail: String| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(StoreError::Domain(
                v2rayr_domain::DomainError::new(v2rayr_domain::ErrorCode::Storage)
                    .with_detail(detail),
            )),
        )
    };
    Ok(StoredSubscription {
        id: Uuid::parse_str(&id).map_err(|error| domain_error(error.to_string()))?,
        name: row.get("name")?,
        url: row.get("url")?,
        enabled: row.get::<_, i64>("enabled")? != 0,
        auto_update: row.get::<_, i64>("auto_update")? != 0,
        interval_hours: row.get::<_, i64>("interval_hours")? as u32,
        last_updated_at: last_updated_at.as_deref().and_then(ts_from_str),
        next_update_at: next_update_at.as_deref().and_then(ts_from_str),
        last_error: row.get("last_error")?,
        quota: row.get("quota")?,
        format: row.get("format")?,
        created_at: ts_from_str(&created_at).unwrap_or_else(Utc::now),
    })
}

const COLUMNS: &str = "id, name, url, enabled, auto_update, interval_hours, last_updated_at, next_update_at, last_error, quota, format, created_at";

impl Database {
    pub fn add_subscription(
        &self,
        upsert: &SubscriptionUpsert,
    ) -> Result<StoredSubscription, StoreError> {
        let existing: Option<String> = self
            .conn()
            .query_row(
                "SELECT id FROM subscriptions WHERE url = ?1",
                params![upsert.url],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Err(StoreError::Domain(
                v2rayr_domain::DomainError::new(v2rayr_domain::ErrorCode::DuplicateNode)
                    .with_detail(id),
            ));
        }
        let id = Uuid::new_v4();
        self.conn().execute(
            "INSERT INTO subscriptions (id, name, url, enabled, auto_update, interval_hours, next_update_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id.to_string(),
                upsert.name,
                upsert.url,
                upsert.enabled as i64,
                upsert.auto_update as i64,
                upsert.interval_hours as i64,
                ts_to_str(Utc::now()),
                ts_to_str(Utc::now()),
            ],
        )?;
        self.subscription(id)?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub fn subscription(&self, id: Uuid) -> Result<Option<StoredSubscription>, StoreError> {
        let sql = format!("SELECT {COLUMNS} FROM subscriptions WHERE id = ?1");
        let mut statement = self.conn().prepare(&sql)?;
        let mut rows = statement.query(params![id.to_string()])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_subscription(row)?)),
            None => Ok(None),
        }
    }

    pub fn subscription_by_url(&self, url: &str) -> Result<Option<StoredSubscription>, StoreError> {
        let sql = format!("SELECT {COLUMNS} FROM subscriptions WHERE url = ?1");
        let mut statement = self.conn().prepare(&sql)?;
        let mut rows = statement.query(params![url])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_subscription(row)?)),
            None => Ok(None),
        }
    }

    pub fn list_subscriptions(&self) -> Result<Vec<StoredSubscription>, StoreError> {
        let sql = format!("SELECT {COLUMNS} FROM subscriptions ORDER BY created_at, name");
        let mut statement = self.conn().prepare(&sql)?;
        let mut rows = statement.query([])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(row_to_subscription(row)?);
        }
        Ok(items)
    }

    pub fn due_subscriptions(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<StoredSubscription>, StoreError> {
        Ok(self
            .list_subscriptions()?
            .into_iter()
            .filter(|item| item.is_due(now))
            .collect())
    }

    pub fn set_subscription_enabled(&self, id: Uuid, enabled: bool) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE subscriptions SET enabled = ?2 WHERE id = ?1",
            params![id.to_string(), enabled as i64],
        )?;
        Ok(())
    }

    /// Change the feed URL of an existing subscription (the user fixed a typo, or
    /// the provider moved). Nodes are left alone until the next refresh.
    pub fn set_subscription_url(&self, id: Uuid, url: &str) -> Result<(), StoreError> {
        let changed = self.conn().execute(
            "UPDATE subscriptions SET url = ?2 WHERE id = ?1",
            params![id.to_string(), url],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn set_subscription_auto_update(
        &self,
        id: Uuid,
        auto_update: bool,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE subscriptions SET auto_update = ?2 WHERE id = ?1",
            params![id.to_string(), auto_update as i64],
        )?;
        Ok(())
    }

    pub fn rename_subscription(&self, id: Uuid, name: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE subscriptions SET name = ?2 WHERE id = ?1",
            params![id.to_string(), name],
        )?;
        Ok(())
    }

    /// Record a successful refresh: bump timestamps and clear the last error.
    pub fn mark_subscription_updated(
        &self,
        id: Uuid,
        format: Option<&str>,
        quota: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let interval: i64 = self.conn().query_row(
            "SELECT interval_hours FROM subscriptions WHERE id = ?1",
            params![id.to_string()],
            |row| row.get(0),
        )?;
        self.conn().execute(
            "UPDATE subscriptions SET last_updated_at = ?2, next_update_at = ?3, last_error = NULL,
                format = COALESCE(?4, format), quota = COALESCE(?5, quota)
             WHERE id = ?1",
            params![
                id.to_string(),
                ts_to_str(now),
                ts_to_str(now + Duration::hours(interval.max(1))),
                format,
                quota
            ],
        )?;
        Ok(())
    }

    /// Record a failed refresh. `next_update_at` still moves forward so a broken
    /// subscription does not hammer the server every tick, but the old nodes are
    /// kept (the plan requires that).
    pub fn mark_subscription_failed(
        &self,
        id: Uuid,
        error_json: &str,
        retry_in: Duration,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE subscriptions SET last_error = ?2, next_update_at = ?3 WHERE id = ?1",
            params![id.to_string(), error_json, ts_to_str(now + retry_in)],
        )?;
        Ok(())
    }

    pub fn delete_subscription(&self, id: Uuid) -> Result<bool, StoreError> {
        Ok(self.conn().execute(
            "DELETE FROM subscriptions WHERE id = ?1",
            params![id.to_string()],
        )? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upsert(url: &str) -> SubscriptionUpsert {
        SubscriptionUpsert {
            name: "sub".into(),
            url: url.into(),
            enabled: true,
            auto_update: true,
            interval_hours: 12,
        }
    }

    #[test]
    fn adding_the_same_url_twice_is_rejected() {
        let db = Database::open_in_memory().expect("db");
        db.add_subscription(&upsert("https://example.com/a"))
            .expect("add");
        let error = db
            .add_subscription(&upsert("https://example.com/a"))
            .expect_err("dup");
        assert!(matches!(error, StoreError::Domain(_)));
        assert_eq!(db.list_subscriptions().expect("list").len(), 1);
    }

    #[test]
    fn successful_refresh_schedules_the_next_run() {
        let db = Database::open_in_memory().expect("db");
        let sub = db
            .add_subscription(&upsert("https://example.com/a"))
            .expect("add");
        let now = Utc::now();
        db.mark_subscription_updated(sub.id, Some("links"), None, now)
            .expect("mark");
        let refreshed = db.subscription(sub.id).expect("get").expect("exists");
        // Timestamps are stored with millisecond precision.
        assert_eq!(
            refreshed.last_updated_at.map(|ts| ts.timestamp_millis()),
            Some(now.timestamp_millis())
        );
        assert_eq!(
            refreshed.next_update_at.map(|ts| ts.timestamp_millis()),
            Some((now + Duration::hours(12)).timestamp_millis())
        );
        assert!(refreshed.last_error.is_none());
        assert!(!refreshed.is_due(now));
    }

    #[test]
    fn failed_refresh_keeps_the_old_timestamp_and_stores_the_error() {
        let db = Database::open_in_memory().expect("db");
        let sub = db
            .add_subscription(&upsert("https://example.com/a"))
            .expect("add");
        let now = Utc::now();
        db.mark_subscription_failed(
            sub.id,
            r#"{"code":"network_error"}"#,
            Duration::hours(1),
            now,
        )
        .expect("mark");
        let refreshed = db.subscription(sub.id).expect("get").expect("exists");
        assert!(refreshed.last_updated_at.is_none(), "nodes stay valid");
        assert!(refreshed.last_error.is_some());
        assert!(refreshed.next_update_at.expect("next") > now);
    }

    #[test]
    fn due_subscriptions_skip_disabled_ones() {
        let db = Database::open_in_memory().expect("db");
        let sub = db
            .add_subscription(&upsert("https://example.com/a"))
            .expect("add");
        db.set_subscription_enabled(sub.id, false).expect("disable");
        assert!(
            db.due_subscriptions(Utc::now() + Duration::hours(48))
                .expect("due")
                .is_empty()
        );
    }

    #[test]
    fn deleting_a_subscription_cascades_to_its_nodes() {
        use crate::profiles::ProfileUpsert;
        use crate::secret::PlaintextSealer;
        use v2rayr_domain::fingerprint::fingerprint;
        use v2rayr_domain::{Profile, Protocol, Secret};

        let db = Database::open_in_memory().expect("db");
        let sealer = PlaintextSealer;
        let sub = db
            .add_subscription(&upsert("https://example.com/a"))
            .expect("add");
        let profile = Profile::new(
            "node",
            "a.com",
            443,
            Protocol::Trojan {
                password: Secret::new("pw"),
            },
        );
        let fp = fingerprint(&profile, b"salt");
        db.upsert_profile(
            &ProfileUpsert {
                profile,
                fingerprint: fp,
                sub_id: Some(sub.id),
            },
            &sealer,
        )
        .expect("node");
        assert_eq!(db.profile_count().expect("count"), 1);
        db.delete_subscription(sub.id).expect("delete");
        assert_eq!(
            db.profile_count().expect("count"),
            0,
            "foreign key cascades"
        );
    }
}
