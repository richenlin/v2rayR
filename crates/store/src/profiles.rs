//! Node rows: a typed [`Profile`] payload plus the columns we sort and filter on.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Row, params};
use uuid::Uuid;
use v2rayr_domain::{DomainError, ErrorCode, NodeFilter, Profile};

use crate::{Database, Sealer, StoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencySample {
    pub ms: u32,
    /// `tcp`, `http`, `through-kernel`.
    pub method: String,
    pub tested_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct StoredProfile {
    pub id: Uuid,
    pub sub_id: Option<Uuid>,
    pub profile: Profile,
    pub fingerprint: String,
    pub favorite: bool,
    pub tags: Vec<String>,
    pub latency: Option<LatencySample>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl StoredProfile {
    /// Display order: favourites first, then fastest known, then name. Nodes
    /// that were never tested sort after tested ones so the list is stable.
    pub fn sort_key(&self) -> (bool, u32, String) {
        (
            !self.favorite,
            self.latency.as_ref().map(|l| l.ms).unwrap_or(u32::MAX),
            self.profile.remark.clone(),
        )
    }
}

/// What an import or a subscription refresh wants to store.
#[derive(Debug, Clone)]
pub struct ProfileUpsert {
    pub profile: Profile,
    /// Stable hash of the connection parameters, see `domain::fingerprint`.
    pub fingerprint: String,
    pub sub_id: Option<Uuid>,
}

pub(crate) fn ts_to_str(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(crate) fn ts_from_str(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

const COLUMNS: &str = "id, sub_id, payload, fingerprint, favorite, tags, latency_ms, latency_method, latency_tested_at, last_used_at, created_at, updated_at";

fn row_to_profile(row: &Row<'_>, sealer: &dyn Sealer) -> rusqlite::Result<StoredProfile> {
    let id: String = row.get("id")?;
    let sub_id: Option<String> = row.get("sub_id")?;
    let payload: String = row.get("payload")?;
    let tags: String = row.get("tags")?;
    let latency_ms: Option<i64> = row.get("latency_ms")?;
    let latency_method: Option<String> = row.get("latency_method")?;
    let latency_tested_at: Option<String> = row.get("latency_tested_at")?;
    let last_used_at: Option<String> = row.get("last_used_at")?;
    let created_at: String = row.get("created_at")?;
    let updated_at: String = row.get("updated_at")?;

    let domain_error = |detail: String| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(StoreError::Domain(
                DomainError::new(ErrorCode::Storage).with_detail(detail),
            )),
        )
    };

    let profile: Profile = serde_json::from_str(&sealer.open(&payload).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?)
    .map_err(|error| domain_error(error.to_string()))?;

    Ok(StoredProfile {
        id: Uuid::parse_str(&id).map_err(|error| domain_error(error.to_string()))?,
        sub_id: sub_id.and_then(|value| Uuid::parse_str(&value).ok()),
        profile,
        fingerprint: row.get("fingerprint")?,
        favorite: row.get::<_, i64>("favorite")? != 0,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        latency: match (latency_ms, latency_method, latency_tested_at) {
            (Some(ms), Some(method), Some(tested_at)) => {
                ts_from_str(&tested_at).map(|tested_at| LatencySample {
                    ms: ms as u32,
                    method,
                    tested_at,
                })
            }
            _ => None,
        },
        last_used_at: last_used_at.as_deref().and_then(ts_from_str),
        created_at: ts_from_str(&created_at).unwrap_or_else(Utc::now),
        updated_at: ts_from_str(&updated_at).unwrap_or_else(Utc::now),
    })
}

impl Database {
    /// Insert or refresh a node, keyed by fingerprint.
    ///
    /// The existing row keeps its `id`, `favorite`, `tags` and latency, so a
    /// subscription refresh never loses the user's annotations.
    pub fn upsert_profile(
        &self,
        upsert: &ProfileUpsert,
        sealer: &dyn Sealer,
    ) -> Result<StoredProfile, StoreError> {
        upsert.profile.validate().map_err(StoreError::Domain)?;
        let now = Utc::now();
        let payload = sealer.seal(&serde_json::to_string(&upsert.profile)?)?;
        let tags = serde_json::to_string(&upsert.profile.tags)?;

        let existing: Option<String> = self
            .conn()
            .query_row(
                "SELECT id FROM profiles WHERE fingerprint = ?1",
                params![upsert.fingerprint],
                |row| row.get(0),
            )
            .optional()?;

        let id = match existing {
            Some(id) => {
                self.conn().execute(
                    "UPDATE profiles SET sub_id = ?2, remark = ?3, address = ?4, port = ?5,
                        protocol = ?6, transport = ?7, payload = ?8, updated_at = ?9
                     WHERE id = ?1",
                    params![
                        id,
                        upsert.sub_id.map(|id| id.to_string()),
                        upsert.profile.remark,
                        upsert.profile.address,
                        upsert.profile.port,
                        upsert.profile.protocol.kind(),
                        upsert.profile.transport.kind(),
                        payload,
                        ts_to_str(now),
                    ],
                )?;
                id
            }
            None => {
                let id = upsert.profile.id.to_string();
                self.conn().execute(
                    "INSERT INTO profiles (id, sub_id, remark, address, port, protocol, transport,
                        payload, fingerprint, favorite, tags, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
                    params![
                        id,
                        upsert.sub_id.map(|id| id.to_string()),
                        upsert.profile.remark,
                        upsert.profile.address,
                        upsert.profile.port,
                        upsert.profile.protocol.kind(),
                        upsert.profile.transport.kind(),
                        payload,
                        upsert.fingerprint,
                        upsert.profile.tags.is_empty().not_favorite(),
                        tags,
                        ts_to_str(now),
                    ],
                )?;
                id
            }
        };
        self.profile(
            Uuid::parse_str(&id).map_err(|e| StoreError::Domain(e.into()))?,
            sealer,
        )?
        .ok_or_else(|| StoreError::NotFound(id))
    }

    /// Replace a node's connection payload in place, keeping its id, favourite
    /// flag, tags and latency. Used when a subscription silently changed the
    /// parameters of a node the user may have already annotated.
    pub fn replace_profile(
        &self,
        id: Uuid,
        upsert: &ProfileUpsert,
        sealer: &dyn Sealer,
    ) -> Result<StoredProfile, StoreError> {
        upsert.profile.validate().map_err(StoreError::Domain)?;
        let payload = sealer.seal(&serde_json::to_string(&upsert.profile)?)?;
        let changed = self.conn().execute(
            "UPDATE profiles SET sub_id = ?2, remark = ?3, address = ?4, port = ?5,
                protocol = ?6, transport = ?7, payload = ?8, fingerprint = ?9, updated_at = ?10
             WHERE id = ?1",
            params![
                id.to_string(),
                upsert.sub_id.map(|sub| sub.to_string()),
                upsert.profile.remark,
                upsert.profile.address,
                upsert.profile.port,
                upsert.profile.protocol.kind(),
                upsert.profile.transport.kind(),
                payload,
                upsert.fingerprint,
                ts_to_str(Utc::now()),
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.profile(id, sealer)?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub fn profile(
        &self,
        id: Uuid,
        sealer: &dyn Sealer,
    ) -> Result<Option<StoredProfile>, StoreError> {
        let sql = format!("SELECT {COLUMNS} FROM profiles WHERE id = ?1");
        let mut statement = self.conn().prepare(&sql)?;
        let mut rows = statement.query(params![id.to_string()])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_profile(row, sealer)?)),
            None => Ok(None),
        }
    }

    pub fn list_profiles(
        &self,
        sealer: &dyn Sealer,
        filter: Option<&NodeFilter>,
    ) -> Result<Vec<StoredProfile>, StoreError> {
        let sql = format!("SELECT {COLUMNS} FROM profiles");
        let mut statement = self.conn().prepare(&sql)?;
        let mut rows = statement.query([])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            let item = row_to_profile(row, sealer)?;
            if filter.is_none_or(|filter| filter.matches(&item.profile)) {
                items.push(item);
            }
        }
        items.sort_by_key(StoredProfile::sort_key);
        Ok(items)
    }

    pub fn subscription_profiles(
        &self,
        sub_id: Uuid,
        sealer: &dyn Sealer,
    ) -> Result<Vec<StoredProfile>, StoreError> {
        let sql = format!("SELECT {COLUMNS} FROM profiles WHERE sub_id = ?1");
        let mut statement = self.conn().prepare(&sql)?;
        let mut rows = statement.query(params![sub_id.to_string()])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(row_to_profile(row, sealer)?);
        }
        Ok(items)
    }

    pub fn profile_count(&self) -> Result<usize, StoreError> {
        let count: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM profiles", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn subscription_profile_count(&self, sub_id: Uuid) -> Result<usize, StoreError> {
        let count: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM profiles WHERE sub_id = ?1",
            params![sub_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    pub fn favorite_profile_count(&self) -> Result<usize, StoreError> {
        let count: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM profiles WHERE favorite <> 0",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    pub fn delete_profile(&self, id: Uuid) -> Result<bool, StoreError> {
        Ok(self.conn().execute(
            "DELETE FROM profiles WHERE id = ?1",
            params![id.to_string()],
        )? > 0)
    }

    /// Remove nodes that upstream dropped. Favourites are never removed: a
    /// starred node that disappears from a subscription is the one case where
    /// keeping stale data is better than losing an annotation.
    pub fn delete_unstarred_profiles(&self, ids: &[Uuid]) -> Result<usize, StoreError> {
        let mut removed = 0;
        for id in ids {
            removed += self.conn().execute(
                "DELETE FROM profiles WHERE id = ?1 AND favorite = 0",
                params![id.to_string()],
            )?;
        }
        Ok(removed)
    }

    pub fn set_favorite(&self, id: Uuid, favorite: bool) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE profiles SET favorite = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.to_string(), favorite as i64, ts_to_str(Utc::now())],
        )?;
        Ok(())
    }

    pub fn set_tags(&self, id: Uuid, tags: &[String]) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE profiles SET tags = ?2, updated_at = ?3 WHERE id = ?1",
            params![
                id.to_string(),
                serde_json::to_string(tags)?,
                ts_to_str(Utc::now())
            ],
        )?;
        Ok(())
    }

    pub fn set_remark(&self, id: Uuid, remark: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE profiles SET remark = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.to_string(), remark, ts_to_str(Utc::now())],
        )?;
        Ok(())
    }

    pub fn record_latency(&self, id: Uuid, ms: u32, method: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE profiles SET latency_ms = ?2, latency_method = ?3, latency_tested_at = ?4
             WHERE id = ?1",
            params![id.to_string(), ms as i64, method, ts_to_str(Utc::now())],
        )?;
        Ok(())
    }

    pub fn mark_used(&self, id: Uuid) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE profiles SET last_used_at = ?2 WHERE id = ?1",
            params![id.to_string(), ts_to_str(Utc::now())],
        )?;
        Ok(())
    }

    pub fn fingerprint_exists(&self, fingerprint: &str) -> Result<bool, StoreError> {
        let found: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM profiles WHERE fingerprint = ?1",
                params![fingerprint],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }
}

/// `INSERT` needs an integer for `favorite`; this keeps the call site honest
/// about "a brand new node is not a favourite".
trait FavoriteFlag {
    fn not_favorite(&self) -> i64;
}

impl FavoriteFlag for bool {
    fn not_favorite(&self) -> i64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::PlaintextSealer;
    use v2rayr_domain::fingerprint::fingerprint;
    use v2rayr_domain::{Protocol, Secret};

    fn profile(remark: &str) -> Profile {
        Profile::new(
            remark,
            "example.com",
            443,
            Protocol::Trojan {
                password: Secret::new("pw"),
            },
        )
    }

    /// Distinct nodes need distinct connection parameters: the fingerprint
    /// deliberately ignores display fields, so varying only `remark` would
    /// collapse the rows.
    fn profile_at(remark: &str, port: u16) -> Profile {
        Profile::new(
            remark,
            "example.com",
            port,
            Protocol::Trojan {
                password: Secret::new("pw"),
            },
        )
    }

    fn upsert(profile: Profile) -> ProfileUpsert {
        let fp = fingerprint(&profile, b"test-salt");
        ProfileUpsert {
            profile,
            fingerprint: fp,
            sub_id: None,
        }
    }

    #[test]
    fn upsert_is_idempotent_and_keeps_the_id() {
        let db = Database::open_in_memory().expect("db");
        let sealer = PlaintextSealer;
        let first = db
            .upsert_profile(&upsert(profile("a")), &sealer)
            .expect("insert");
        let mut renamed = profile("b");
        renamed.id = Uuid::new_v4();
        let mut second = upsert(renamed);
        second.profile.id = Uuid::new_v4();
        let again = db.upsert_profile(&second, &sealer).expect("update");
        assert_eq!(first.id, again.id, "id must survive a refresh");
        assert_eq!(again.profile.remark, "b", "display fields are updated");
        assert_eq!(db.profile_count().expect("count"), 1);
    }

    #[test]
    fn favorite_and_tags_survive_a_refresh() {
        let db = Database::open_in_memory().expect("db");
        let sealer = PlaintextSealer;
        let stored = db
            .upsert_profile(&upsert(profile("a")), &sealer)
            .expect("insert");
        db.set_favorite(stored.id, true).expect("favorite");
        db.set_tags(stored.id, &["stream".to_owned()])
            .expect("tags");

        let mut refreshed = upsert(profile("a"));
        refreshed.fingerprint = stored.fingerprint.clone();
        let again = db.upsert_profile(&refreshed, &sealer).expect("update");
        assert!(again.favorite);
        assert_eq!(again.tags, vec!["stream".to_owned()]);
    }

    #[test]
    fn filter_runs_on_the_domain_level() {
        let db = Database::open_in_memory().expect("db");
        let sealer = PlaintextSealer;
        db.upsert_profile(&upsert(profile_at("Tokyo 01", 443)), &sealer)
            .expect("a");
        db.upsert_profile(&upsert(profile_at("Osaka 01", 8443)), &sealer)
            .expect("b");
        let all = db.list_profiles(&sealer, None).expect("list");
        assert_eq!(all.len(), 2);
        let filtered = db
            .list_profiles(&sealer, Some(&NodeFilter::parse("tokyo")))
            .expect("filtered");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].profile.remark, "Tokyo 01");
    }

    #[test]
    fn favourites_sort_first() {
        let db = Database::open_in_memory().expect("db");
        let sealer = PlaintextSealer;
        let za = db
            .upsert_profile(&upsert(profile_at("Zeta", 443)), &sealer)
            .expect("a");
        db.upsert_profile(&upsert(profile_at("Alpha", 8443)), &sealer)
            .expect("b");
        db.set_favorite(za.id, true).expect("favorite");
        let items = db.list_profiles(&sealer, None).expect("list");
        assert_eq!(items[0].profile.remark, "Zeta");
    }

    #[test]
    fn unstarred_nodes_can_be_pruned_but_favourites_stay() {
        let db = Database::open_in_memory().expect("db");
        let sealer = PlaintextSealer;
        let keep = db
            .upsert_profile(&upsert(profile_at("keep", 443)), &sealer)
            .expect("a");
        let drop = db
            .upsert_profile(&upsert(profile_at("drop", 8443)), &sealer)
            .expect("b");
        db.set_favorite(keep.id, true).expect("favorite");
        let removed = db
            .delete_unstarred_profiles(&[keep.id, drop.id])
            .expect("prune");
        assert_eq!(removed, 1);
        assert!(db.profile(keep.id, &sealer).expect("get").is_some());
        assert!(db.profile(drop.id, &sealer).expect("get").is_none());
    }
}
