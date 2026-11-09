//! Subscription refresh diffing.
//!
//! Refreshing a subscription must not destroy local state: favourites and
//! manual remarks survive, node ids stay stable (so "the node I was connected
//! to" keeps its identity), and a failed refresh keeps the old list untouched.

use std::collections::HashMap;

use uuid::Uuid;

use crate::fingerprint::fingerprint;
use crate::model::Profile;

/// The part of a stored node the merge needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingNode {
    pub id: Uuid,
    pub fingerprint: String,
    pub favorite: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MergePlan {
    /// New nodes, already stamped with `sub_id`.
    pub added: Vec<Profile>,
    /// Nodes that exist locally and changed; the id is preserved.
    pub updated: Vec<(Uuid, Profile)>,
    /// Nodes of this subscription that disappeared upstream.
    pub removed: Vec<Uuid>,
    /// Nodes kept only because the user favourited them.
    pub kept: Vec<Uuid>,
}

impl MergePlan {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.removed.is_empty()
    }
}

/// Computes what a refresh would change.
pub fn plan_merge(existing: &[ExistingNode], incoming: &[Profile], salt: &[u8]) -> MergePlan {
    let mut by_fingerprint: HashMap<&str, &ExistingNode> = HashMap::with_capacity(existing.len());
    for node in existing {
        by_fingerprint.insert(node.fingerprint.as_str(), node);
    }

    let mut plan = MergePlan::default();
    let mut seen = std::collections::HashSet::with_capacity(incoming.len());

    for profile in incoming {
        let digest = fingerprint(profile, salt);
        // Two identical nodes inside one payload: keep the first.
        if !seen.insert(digest.clone()) {
            continue;
        }
        match by_fingerprint.get(digest.as_str()) {
            Some(node) => {
                // Keep the id, refresh everything else from upstream.
                let mut refreshed = profile.clone();
                refreshed.id = node.id;
                plan.updated.push((node.id, refreshed));
            }
            None => plan.added.push(profile.clone()),
        }
    }

    for node in existing {
        let still_present = seen.contains(&node.fingerprint);
        if still_present {
            continue;
        }
        if node.favorite {
            plan.kept.push(node.id);
        } else {
            plan.removed.push(node.id);
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Protocol, Secret};

    fn node(remark: &str) -> Profile {
        Profile::new(
            remark,
            "example.com",
            443,
            Protocol::Trojan {
                password: Secret::new("pw"),
            },
        )
    }

    fn existing(profile: &Profile, favorite: bool) -> ExistingNode {
        ExistingNode {
            id: profile.id,
            fingerprint: fingerprint(profile, b"salt"),
            favorite,
        }
    }

    #[test]
    fn unchanged_nodes_are_kept_and_updated() {
        let first = node("a");
        let plan = plan_merge(
            &[existing(&first, false)],
            std::slice::from_ref(&first),
            b"salt",
        );
        assert!(plan.added.is_empty());
        assert_eq!(plan.updated.len(), 1);
        assert_eq!(plan.updated[0].0, first.id);
    }

    #[test]
    fn renamed_node_keeps_its_id() {
        let first = node("a");
        let mut renamed = first.clone();
        renamed.remark = "b".into();
        let plan = plan_merge(&[existing(&first, false)], &[renamed], b"salt");
        assert_eq!(plan.updated.len(), 1);
        assert_eq!(plan.updated[0].0, first.id);
    }

    #[test]
    fn vanished_nodes_are_removed_unless_favourited() {
        let dropped = node("dropped");
        let favourite = node("favourite");
        let plan = plan_merge(
            &[existing(&dropped, false), existing(&favourite, true)],
            &[],
            b"salt",
        );
        assert_eq!(plan.removed, vec![dropped.id]);
        assert_eq!(plan.kept, vec![favourite.id]);
    }

    #[test]
    fn added_nodes_carry_the_subscription() {
        let sub_id = Uuid::new_v4();
        let mut profile = node("new");
        profile.sub_id = Some(sub_id);
        let plan = plan_merge(&[], &[profile], b"salt");
        assert_eq!(plan.added.len(), 1);
        assert_eq!(plan.added[0].sub_id, Some(sub_id));
    }

    #[test]
    fn duplicates_inside_the_payload_are_collapsed() {
        let profile = node("dup");
        let plan = plan_merge(&[], &[profile.clone(), profile], b"salt");
        assert_eq!(plan.added.len(), 1);
    }
}
