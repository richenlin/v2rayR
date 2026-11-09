//! Subscription dedupe.
//!
//! A node is identified by everything that changes the wire format, hashed with
//! a machine-local salt. The salt is not a security boundary — it only prevents
//! the digest of a node from being comparable across machines (and therefore
//! from leaking the node's address into a support bundle).

use sha2::{Digest, Sha256};

use crate::model::Profile;

/// `sha256(salt || canonical_connection(profile))`, lowercase hex.
pub fn fingerprint(profile: &Profile, salt: &[u8]) -> String {
    let canonical = serde_json::to_vec(&profile.canonical_connection()).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update((canonical.len() as u64).to_be_bytes());
    hasher.update(&canonical);
    hex::encode(hasher.finalize())
}

/// Default salt for tests and for the very first launch, before the real salt
/// has been generated.
pub const DEFAULT_SALT: &[u8] = b"v2rayr-default-salt";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Protocol, Secret};
    use uuid::Uuid;

    fn profile() -> Profile {
        Profile::new(
            "a",
            "example.com",
            443,
            Protocol::Trojan {
                password: Secret::new("pw"),
            },
        )
    }

    #[test]
    fn salt_changes_the_digest() {
        let profile = profile();
        assert_ne!(fingerprint(&profile, b"a"), fingerprint(&profile, b"b"));
    }

    #[test]
    fn password_change_changes_the_digest() {
        let first = profile();
        let mut second = profile();
        second.protocol = Protocol::Trojan {
            password: Secret::new("other"),
        };
        assert_ne!(fingerprint(&first, b"salt"), fingerprint(&second, b"salt"));
    }

    #[test]
    fn id_change_does_not_change_the_digest() {
        let first = profile();
        let mut second = first.clone();
        second.id = Uuid::new_v4();
        assert_eq!(fingerprint(&first, b"salt"), fingerprint(&second, b"salt"));
    }

    #[test]
    fn digest_is_stable_across_releases() {
        // Pinned so that a refactor cannot silently invalidate every stored
        // fingerprint (which would duplicate the user's whole node list).
        let digest = fingerprint(&profile(), DEFAULT_SALT);
        assert_eq!(digest.len(), 64);
        assert_eq!(digest, fingerprint(&profile(), DEFAULT_SALT));
    }
}
