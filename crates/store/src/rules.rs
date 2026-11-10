//! Custom routing rules.
//!
//! A rule is intentionally tiny: a matcher kind, a value and an action. Geo
//! databases and rule sets cover the long tail; these are the handful of
//! exceptions a user actually types.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use v2rayr_domain::RuleApplication;

use crate::profiles::{ts_from_str, ts_to_str};
use crate::{Database, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleKind {
    Domain,
    DomainSuffix,
    IpCidr,
    Process,
    GeoIp,
    GeoSite,
}

impl RuleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Domain => "domain",
            Self::DomainSuffix => "domain-suffix",
            Self::IpCidr => "ip-cidr",
            Self::Process => "process",
            Self::GeoIp => "geo-ip",
            Self::GeoSite => "geo-site",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "domain" => Self::Domain,
            "domain-suffix" => Self::DomainSuffix,
            "ip-cidr" => Self::IpCidr,
            "process" => Self::Process,
            "geo-ip" => Self::GeoIp,
            "geo-site" => Self::GeoSite,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleAction {
    Proxy,
    Direct,
    Block,
}

impl RuleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Direct => "direct",
            Self::Block => "block",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "proxy" => Self::Proxy,
            "direct" => Self::Direct,
            "block" => Self::Block,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleRow {
    pub id: Uuid,
    pub kind: RuleKind,
    pub value: String,
    #[serde(default)]
    pub applications: Vec<RuleApplication>,
    pub action: RuleAction,
    pub enabled: bool,
    pub position: i32,
    pub created_at: DateTime<Utc>,
}

fn row_to_rule(row: &Row<'_>) -> rusqlite::Result<RuleRow> {
    let id: String = row.get("id")?;
    let kind: String = row.get("kind")?;
    let action: String = row.get("action")?;
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
    Ok(RuleRow {
        id: Uuid::parse_str(&id).map_err(|error| domain_error(error.to_string()))?,
        kind: RuleKind::parse(&kind).ok_or_else(|| domain_error(format!("kind {kind}")))?,
        value: row.get("value")?,
        applications: serde_json::from_str(&row.get::<_, String>("applications")?)
            .map_err(|error| domain_error(error.to_string()))?,
        action: RuleAction::parse(&action)
            .ok_or_else(|| domain_error(format!("action {action}")))?,
        enabled: row.get::<_, i64>("enabled")? != 0,
        position: row.get("position")?,
        created_at: ts_from_str(&created_at).unwrap_or_else(Utc::now),
    })
}

impl Database {
    pub fn list_rules(&self) -> Result<Vec<RuleRow>, StoreError> {
        let mut statement = self
            .conn()
            .prepare("SELECT id, kind, value, action, enabled, position, created_at, applications FROM rules ORDER BY position, created_at")?;
        let mut rows = statement.query([])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(row_to_rule(row)?);
        }
        Ok(items)
    }

    pub fn enabled_rules(&self) -> Result<Vec<RuleRow>, StoreError> {
        Ok(self
            .list_rules()?
            .into_iter()
            .filter(|rule| rule.enabled)
            .collect())
    }

    /// Import portable rules without replacing any existing rule, retaining
    /// order within the imported set and the local enabled value on duplicates.
    pub fn merge_backup_rules(
        &self,
        rules: &[(RuleKind, String, RuleAction, bool, Vec<RuleApplication>)],
        before_commit: impl FnOnce() -> Result<(), StoreError>,
    ) -> Result<usize, StoreError> {
        let transaction = self.conn().unchecked_transaction()?;
        let mut position: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM rules",
            [],
            |row| row.get(0),
        )?;
        let mut added = 0;
        for (kind, value, action, enabled, applications) in rules {
            let applications = serde_json::to_string(applications)?;
            let inserted = transaction.execute(
                "INSERT INTO rules (id, kind, value, action, enabled, position, created_at, applications)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
                 WHERE NOT EXISTS (SELECT 1 FROM rules WHERE kind = ?2 AND value = ?3 AND action = ?4 AND applications = ?8)",
                params![Uuid::new_v4().to_string(), kind.as_str(), value, action.as_str(), *enabled, position, ts_to_str(Utc::now()), applications],
            )?;
            if inserted != 0 {
                position += 1;
                added += 1;
            }
        }
        before_commit()?;
        transaction.commit()?;
        Ok(added)
    }

    pub fn add_rule(
        &self,
        kind: RuleKind,
        value: &str,
        action: RuleAction,
    ) -> Result<RuleRow, StoreError> {
        self.add_rule_for_applications(kind, value, action, &[])
    }

    pub fn add_rule_for_applications(
        &self,
        kind: RuleKind,
        value: &str,
        action: RuleAction,
        applications: &[RuleApplication],
    ) -> Result<RuleRow, StoreError> {
        let position: i64 = self.conn().query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM rules",
            [],
            |row| row.get(0),
        )?;
        let id = Uuid::new_v4();
        self.conn().execute(
            "INSERT INTO rules (id, kind, value, action, enabled, position, created_at, applications)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7)",
            params![
                id.to_string(),
                kind.as_str(),
                value,
                action.as_str(),
                position,
                ts_to_str(Utc::now()),
                serde_json::to_string(applications)?
            ],
        )?;
        Ok(RuleRow {
            id,
            kind,
            value: value.to_owned(),
            applications: applications.to_vec(),
            action,
            enabled: true,
            position: position as i32,
            created_at: Utc::now(),
        })
    }

    pub fn update_rule(
        &self,
        id: Uuid,
        kind: RuleKind,
        value: &str,
        action: RuleAction,
        applications: &[RuleApplication],
    ) -> Result<bool, StoreError> {
        Ok(self.conn().execute(
            "UPDATE rules SET kind = ?2, value = ?3, action = ?4, applications = ?5 WHERE id = ?1",
            params![id.to_string(), kind.as_str(), value, action.as_str(), serde_json::to_string(applications)?],
        )? > 0)
    }

    pub fn set_rule_enabled(&self, id: Uuid, enabled: bool) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE rules SET enabled = ?2 WHERE id = ?1",
            params![id.to_string(), enabled as i64],
        )?;
        Ok(())
    }

    pub fn delete_rule(&self, id: Uuid) -> Result<bool, StoreError> {
        Ok(self
            .conn()
            .execute("DELETE FROM rules WHERE id = ?1", params![id.to_string()])?
            > 0)
    }

    /// Persist an explicit order (drag and drop in the rules list).
    pub fn reorder_rules(&self, ordered: &[Uuid]) -> Result<(), StoreError> {
        let tx = self.conn().unchecked_transaction()?;
        for (index, id) in ordered.iter().enumerate() {
            tx.execute(
                "UPDATE rules SET position = ?2 WHERE id = ?1",
                params![id.to_string(), index as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_keep_insertion_order() {
        let db = Database::open_in_memory().expect("db");
        db.add_rule(RuleKind::DomainSuffix, "google.com", RuleAction::Proxy)
            .expect("a");
        db.add_rule(RuleKind::IpCidr, "10.0.0.0/8", RuleAction::Direct)
            .expect("b");
        let rules = db.list_rules().expect("list");
        assert_eq!(rules[0].value, "google.com");
        assert_eq!(rules[1].position, 1);
    }

    #[test]
    fn disabled_rules_are_skipped_by_the_emitter() {
        let db = Database::open_in_memory().expect("db");
        let rule = db
            .add_rule(RuleKind::Domain, "ads.example", RuleAction::Block)
            .expect("add");
        db.set_rule_enabled(rule.id, false).expect("disable");
        assert!(db.enabled_rules().expect("enabled").is_empty());
        assert_eq!(db.list_rules().expect("all").len(), 1);
    }

    #[test]
    fn reordering_rewrites_positions() {
        let db = Database::open_in_memory().expect("db");
        let a = db
            .add_rule(RuleKind::Domain, "a.com", RuleAction::Proxy)
            .expect("a");
        let b = db
            .add_rule(RuleKind::Domain, "b.com", RuleAction::Block)
            .expect("b");
        db.reorder_rules(&[b.id, a.id]).expect("reorder");
        let rules = db.list_rules().expect("list");
        assert_eq!(rules[0].value, "b.com");
        assert_eq!(rules[1].value, "a.com");
    }
}
