//! Lenient base64 decoding for share links.
//!
//! Publishers disagree on padding, on `-_` vs `+/`, and on line wrapping. A
//! strict decoder rejects perfectly usable subscriptions, so we normalise first
//! and only then hand the bytes to a strict decoder.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};

/// Decodes standard or URL-safe base64, with or without padding, ignoring
/// ASCII whitespace (links pasted from chat apps are often wrapped).
pub fn decode(input: &str) -> Option<Vec<u8>> {
    let cleaned: String = input.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if cleaned.is_empty() {
        return None;
    }
    let normalised = cleaned.replace('-', "+").replace('_', "/");

    // Fast path: the payload is already canonical.
    if let Ok(bytes) = STANDARD.decode(&normalised) {
        return Some(bytes);
    }
    if let Ok(bytes) = STANDARD_NO_PAD.decode(&normalised) {
        return Some(bytes);
    }

    // Slow path: fix up padding for inputs that were truncated or re-padded.
    let trimmed = normalised.trim_end_matches('=');
    let mut padded = trimmed.to_owned();
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    STANDARD
        .decode(&padded)
        .ok()
        .or_else(|| URL_SAFE.decode(&padded).ok())
        .or_else(|| URL_SAFE_NO_PAD.decode(trimmed).ok())
}

/// Decodes to UTF-8, returning `None` when the payload is not text.
pub fn decode_str(input: &str) -> Option<String> {
    let bytes = decode(input)?;
    String::from_utf8(bytes).ok()
}

/// Encodes with the standard alphabet and padding (what most panels expect from
/// a regenerated link).
pub fn encode(input: &[u8]) -> String {
    STANDARD.encode(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_all_four_alphabets() {
        let raw = b"methods: aes-256-gcm";
        for encoded in [
            STANDARD.encode(raw),
            STANDARD_NO_PAD.encode(raw),
            URL_SAFE.encode(raw),
            URL_SAFE_NO_PAD.encode(raw),
        ] {
            assert_eq!(
                decode_str(&encoded).as_deref(),
                Some("methods: aes-256-gcm")
            );
        }
    }

    #[test]
    fn ignores_padding_and_line_wrapping() {
        assert_eq!(decode_str("5L2g5aW9\n").as_deref(), Some("你好"));
        assert_eq!(decode_str("5L2g5aW9==").as_deref(), Some("你好"));
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_str("").is_none());
        assert!(decode_str("!!!!").is_none());
    }
}
