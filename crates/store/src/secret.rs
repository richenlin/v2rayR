//! Payload sealing.
//!
//! The database holds proxy credentials, so the file is created `0600` and the
//! payload column is written through a [`Sealer`]. The default sealer is
//! deliberately honest about what it is: a self-describing *plaintext* envelope.
//! Copying the database therefore still leaks credentials, which is why the
//! envelope carries a scheme tag — `sealed-payloads` builds swap in AES-256-GCM
//! without touching a single call site.
//!
//! Threat model (documented in `docs/安全性.md`): protects against accidental
//! exposure in logs and bug reports, not against an attacker who can already
//! read files as this user.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use v2rayr_domain::base64 as domain_base64;

use crate::StoreError;

const PLAINTEXT_PREFIX: &str = "v2rayr:plain:";

pub trait Sealer: Send + Sync {
    fn seal(&self, plaintext: &str) -> Result<String, StoreError>;
    fn open(&self, sealed: &str) -> Result<String, StoreError>;
    /// Shown in the diagnostics page so users know what they are getting.
    fn describe(&self) -> &'static str;
}

/// Stores the payload as-is, base64 encoded so that credentials never appear as
/// printable text in the file.
#[derive(Debug, Default)]
pub struct PlaintextSealer;

impl Sealer for PlaintextSealer {
    fn seal(&self, plaintext: &str) -> Result<String, StoreError> {
        Ok(format!(
            "{PLAINTEXT_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(plaintext.as_bytes())
        ))
    }

    fn open(&self, sealed: &str) -> Result<String, StoreError> {
        let body = sealed.strip_prefix(PLAINTEXT_PREFIX).ok_or_else(|| {
            StoreError::Domain(v2rayr_domain::DomainError::new(
                v2rayr_domain::ErrorCode::Storage,
            ))
        })?;
        let bytes = domain_base64::decode(body).ok_or_else(|| {
            StoreError::Domain(v2rayr_domain::DomainError::new(
                v2rayr_domain::ErrorCode::Storage,
            ))
        })?;
        String::from_utf8(bytes).map_err(|_| {
            StoreError::Domain(v2rayr_domain::DomainError::new(
                v2rayr_domain::ErrorCode::Storage,
            ))
        })
    }

    fn describe(&self) -> &'static str {
        "plaintext"
    }
}

/// Pick the strongest sealer this build supports.
///
/// `dir` is where a key file would live; the plaintext sealer ignores it.
pub fn open_sealer(dir: &Path) -> Result<Box<dyn Sealer>, StoreError> {
    std::fs::create_dir_all(dir)?;
    #[cfg(feature = "sealed-payloads")]
    {
        return Ok(Box::new(sealed::AesGcmSealer::load_or_create(
            &dir.join("secret.key"),
        )?));
    }
    #[cfg(not(feature = "sealed-payloads"))]
    {
        let _ = dir;
        Ok(Box::new(PlaintextSealer))
    }
}

#[cfg(feature = "sealed-payloads")]
mod sealed {
    use std::path::Path;

    use ring::aead::{AAD, AES_256_GCM, LessSafeKey, Nonce, UnboundKey};
    use ring::rand::{SecureRandom, SystemRandom};

    use super::*;

    const PREFIX: &str = "v2rayr:gcm:";

    pub struct AesGcmSealer {
        key: LessSafeKey,
    }

    impl AesGcmSealer {
        pub fn load_or_create(path: &Path) -> Result<Self, StoreError> {
            let bytes = match std::fs::read(path) {
                Ok(bytes) if bytes.len() == 32 => bytes,
                _ => {
                    let mut bytes = vec![0u8; 32];
                    SystemRandom::new()
                        .fill(&mut bytes)
                        .map_err(|_| StoreError::Io(std::io::Error::other("rng failed")))?;
                    std::fs::write(path, &bytes)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
                    }
                    bytes
                }
            };
            let key = UnboundKey::new(&AES_256_GCM, &bytes)
                .map_err(|_| StoreError::Io(std::io::Error::other("bad key")))?;
            Ok(Self {
                key: LessSafeKey::new(key),
            })
        }
    }

    impl Sealer for AesGcmSealer {
        fn seal(&self, plaintext: &str) -> Result<String, StoreError> {
            let mut nonce_bytes = [0u8; 12];
            SystemRandom::new()
                .fill(&mut nonce_bytes)
                .map_err(|_| StoreError::Io(std::io::Error::other("rng failed")))?;
            let mut buffer = plaintext.as_bytes().to_vec();
            self.key
                .seal_in_place_append_tag(
                    Nonce::assume_unique_for_key(nonce_bytes),
                    AAD::empty(),
                    &mut buffer,
                )
                .map_err(|_| StoreError::Io(std::io::Error::other("seal failed")))?;
            let mut out = nonce_bytes.to_vec();
            out.extend_from_slice(&buffer);
            Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(out)))
        }

        fn open(&self, sealed: &str) -> Result<String, StoreError> {
            let body = sealed
                .strip_prefix(PREFIX)
                .ok_or_else(|| StoreError::Io(std::io::Error::other("unknown sealing scheme")))?;
            let bytes = URL_SAFE_NO_PAD
                .decode(body)
                .map_err(|_| StoreError::Io(std::io::Error::other("bad sealed payload")))?;
            if bytes.len() < 12 {
                return Err(StoreError::Io(std::io::Error::other("truncated payload")));
            }
            let (nonce, ciphertext) = bytes.split_at(12);
            let nonce_bytes: [u8; 12] = nonce.try_into().expect("12 bytes");
            let mut buffer = ciphertext.to_vec();
            let plaintext = self
                .key
                .open_in_place(
                    Nonce::assume_unique_for_key(nonce_bytes),
                    AAD::empty(),
                    &mut buffer,
                )
                .map_err(|_| StoreError::Io(std::io::Error::other("unseal failed")))?;
            String::from_utf8(plaintext.to_vec())
                .map_err(|_| StoreError::Io(std::io::Error::other("payload is not utf-8")))
        }

        fn describe(&self) -> &'static str {
            "aes-256-gcm"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_sealer_round_trips() {
        let sealer = PlaintextSealer;
        let sealed = sealer.seal(r#"{"password":"hunter2"}"#).expect("seal");
        assert!(!sealed.contains("hunter2"), "payload must not be printable");
        assert_eq!(
            sealer.open(&sealed).expect("open"),
            r#"{"password":"hunter2"}"#
        );
    }

    #[test]
    fn unknown_scheme_is_rejected() {
        assert!(PlaintextSealer.open("v2rayr:gcm:AAAA").is_err());
    }
}
