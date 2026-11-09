//! Small key/value table for session memory: the last node, the last takeover
//! mode, cumulative counters. Losing it must never break startup.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::{Database, StoreError};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionMemory {
    pub active_profile: Option<Uuid>,
    pub last_mode: Option<&'static str>,
}

const KEY_ACTIVE_PROFILE: &str = "session.active_profile";
const KEY_LAST_MODE: &str = "session.last_mode";
const KEY_TOTAL_DOWN: &str = "traffic.total_down";
const KEY_TOTAL_UP: &str = "traffic.total_up";

impl Database {
    pub fn get_state(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT value FROM session_memory WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO session_memory (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn clear_state(&self, key: &str) -> Result<(), StoreError> {
        self.conn()
            .execute("DELETE FROM session_memory WHERE key = ?1", params![key])?;
        Ok(())
    }

    fn get_typed<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.get_state(key)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
    }

    fn set_typed<T: Serialize>(&self, key: &str, value: &T) -> Result<(), StoreError> {
        self.set_state(key, &serde_json::to_string(value)?)
    }

    pub fn active_profile(&self) -> Option<Uuid> {
        self.get_typed(KEY_ACTIVE_PROFILE)
    }

    pub fn set_active_profile(&self, id: Option<Uuid>) -> Result<(), StoreError> {
        match id {
            Some(id) => self.set_typed(KEY_ACTIVE_PROFILE, &id),
            None => self.clear_state(KEY_ACTIVE_PROFILE),
        }
    }

    pub fn last_mode(&self) -> Option<String> {
        self.get_state(KEY_LAST_MODE).ok().flatten()
    }

    pub fn set_last_mode(&self, mode: &str) -> Result<(), StoreError> {
        self.set_state(KEY_LAST_MODE, mode)
    }

    pub fn totals(&self) -> (u64, u64) {
        (
            self.get_typed::<u64>(KEY_TOTAL_DOWN).unwrap_or(0),
            self.get_typed::<u64>(KEY_TOTAL_UP).unwrap_or(0),
        )
    }

    pub fn add_totals(&self, down: u64, up: u64) -> Result<(), StoreError> {
        let (total_down, total_up) = self.totals();
        self.set_typed(KEY_TOTAL_DOWN, &total_down.saturating_add(down))?;
        self.set_typed(KEY_TOTAL_UP, &total_up.saturating_add(up))
    }

    pub fn reset_totals(&self) -> Result<(), StoreError> {
        self.clear_state(KEY_TOTAL_DOWN)?;
        self.clear_state(KEY_TOTAL_UP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_profile_round_trips() {
        let db = Database::open_in_memory().expect("db");
        let id = Uuid::new_v4();
        db.set_active_profile(Some(id)).expect("set");
        assert_eq!(db.active_profile(), Some(id));
        db.set_active_profile(None).expect("clear");
        assert_eq!(db.active_profile(), None);
    }

    #[test]
    fn totals_accumulate_and_reset() {
        let db = Database::open_in_memory().expect("db");
        db.add_totals(100, 10).expect("first");
        db.add_totals(50, 5).expect("second");
        assert_eq!(db.totals(), (150, 15));
        db.reset_totals().expect("reset");
        assert_eq!(db.totals(), (0, 0));
    }
}
