//! Transaction shared by GNOME GSettings and WinINet. Values are kept in the
//! backend's canonical form so a crash recovery can compare exact ownership.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[cfg(any(target_os = "linux", windows, test))]
use super::{ProxyEndpoint, ProxyTarget, SystemProxy, SystemProxyState, TakeoverSnapshot};
#[cfg(any(target_os = "linux", windows, test))]
use crate::PlatformError;

pub type Values = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeProxyKind {
    GSettings,
    WinInet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeProxyJournal {
    pub kind: NativeProxyKind,
    pub before: Values,
    pub written: Values,
}

#[cfg(any(target_os = "linux", windows, test))]
pub(super) trait Backend: Send + Sync {
    fn kind(&self) -> NativeProxyKind;
    fn read(&self) -> Result<Values, PlatformError>;
    fn target(&self, current: &Values, target: &ProxyTarget) -> Result<Values, PlatformError>;
    fn write(&self, values: &Values) -> Result<(), PlatformError>;
    fn state(&self, current: &Values) -> SystemProxyState;
    fn mode_key(&self) -> &'static str;
    fn endpoint_groups(&self, written: &Values) -> &'static [&'static [&'static str]];
    fn disable_owned(&self, endpoint: &ProxyEndpoint, current: &Values) -> Values;
}

#[cfg(any(target_os = "linux", windows, test))]
pub(super) struct NativeProxy<B>(pub B);

#[cfg(any(target_os = "linux", windows, test))]
impl<B: Backend> NativeProxy<B> {
    fn verify(&self, values: &Values) -> Result<(), PlatformError> {
        if values.is_empty() {
            return Ok(());
        }
        let observed = self.0.read()?;
        if values.iter().all(|(k, v)| observed.get(k) == Some(v)) {
            Ok(())
        } else {
            Err(PlatformError::InvalidState(
                "系统代理设置未通过读回验证".into(),
            ))
        }
    }
}

#[cfg(any(target_os = "linux", windows, test))]
impl<B: Backend> SystemProxy for NativeProxy<B> {
    fn capture(&self) -> Result<SystemProxyState, PlatformError> {
        Ok(self.0.state(&self.0.read()?))
    }

    fn apply(&self, target: &ProxyTarget) -> Result<TakeoverSnapshot, PlatformError> {
        let mut prepared = None;
        match self.apply_journaled(target, &mut |snapshot| {
            prepared = Some(snapshot.clone());
            Ok(())
        }) {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => {
                if let Some(snapshot) = prepared {
                    self.restore(&snapshot)?;
                }
                Err(error)
            }
        }
    }

    fn apply_journaled(
        &self,
        target: &ProxyTarget,
        journal: &mut dyn FnMut(&TakeoverSnapshot) -> Result<(), PlatformError>,
    ) -> Result<TakeoverSnapshot, PlatformError> {
        if target.is_empty() {
            return Err(PlatformError::InvalidState("缺少代理地址".into()));
        }
        let current = self.0.read()?;
        let desired = self.0.target(&current, target)?;
        if desired.keys().any(|k| !current.contains_key(k)) {
            return Err(PlatformError::InvalidState(
                "系统代理设置缺少预期字段".into(),
            ));
        }
        let snapshot = TakeoverSnapshot {
            services: self.0.state(&current).services,
            applied_endpoint: target.http.clone().or_else(|| target.socks.clone()),
            applied_pac: target.pac.clone(),
            applied_at_ms: chrono::Utc::now().timestamp_millis(),
            target: Some(target.clone()),
            native: Some(NativeProxyJournal {
                kind: self.0.kind(),
                before: desired
                    .keys()
                    .map(|key| (key.clone(), current[key].clone()))
                    .collect(),
                written: desired.clone(),
            }),
        };
        journal(&snapshot)?;
        let changes = desired
            .iter()
            .filter(|(key, value)| current.get(*key) != Some(*value))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        self.0.write(&changes)?;
        self.verify(&desired)?;
        Ok(snapshot)
    }

    fn restore(&self, snapshot: &TakeoverSnapshot) -> Result<(), PlatformError> {
        let Some(journal) = snapshot.native.as_ref().filter(|j| j.kind == self.0.kind()) else {
            return Err(PlatformError::InvalidState(
                "系统代理恢复记录与当前平台不匹配".into(),
            ));
        };
        let current = self.0.read()?;
        if journal.before.keys().ne(journal.written.keys())
            || journal.before.keys().any(|key| !current.contains_key(key))
        {
            return Err(PlatformError::InvalidState(
                "系统代理恢复记录字段无效".into(),
            ));
        }
        let mode = self.0.mode_key();
        // If another app changed an address, do not disable the mode it is
        // using merely because the mode string still matches ours.
        let changed_groups: Vec<_> = self
            .0
            .endpoint_groups(&journal.written)
            .iter()
            .filter(|group| {
                group.iter().any(|key| {
                    journal.written.get(*key).is_some_and(|written| {
                        current.get(*key) != Some(written)
                            && current.get(*key) != journal.before.get(*key)
                    })
                })
            })
            .collect();
        let changes: Values = journal
            .before
            .iter()
            .filter(|(key, before)| {
                current.get(*key) == journal.written.get(*key)
                    && current.get(*key) != Some(*before)
                    && (key.as_str() != mode || changed_groups.is_empty())
                    && !changed_groups
                        .iter()
                        .any(|group| group.contains(&key.as_str()))
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        self.0.write(&changes)?;
        self.verify(&changes)
    }

    fn clear(&self, endpoint: &ProxyEndpoint) -> Result<(), PlatformError> {
        let current = self.0.read()?;
        let changes = self.0.disable_owned(endpoint, &current);
        self.0.write(&changes)?;
        self.verify(&changes)
    }
}
