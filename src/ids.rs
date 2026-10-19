//! Clip ids and uploader tokens: random letters and digits.

use rand::distr::{Alphanumeric, SampleString};
use sha2::{Digest, Sha256};

/// Length of a clip id: 62^10 ≈ 8 × 10^17 of them, too many to guess one.
const ID_LEN: usize = 10;

/// A new clip id, like `aB3xY9k2Qp`.
pub fn clip_id() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), ID_LEN)
}

/// Whether `s` could be a clip id (checked before touching the database).
pub fn is_clip_id(s: &str) -> bool {
    s.len() == ID_LEN && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A new uploader token: `hct_` and 43 letters and digits (256 bits).
pub fn token() -> String {
    format!("hct_{}", Alphanumeric.sample_string(&mut rand::rng(), 43))
}

/// What we keep of a token: its SHA-256. A token is random enough that a
/// plain hash can't be reversed, so it needs no salt or slow hash.
pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_look_right() {
        let id = clip_id();
        assert!(is_clip_id(&id), "{id}");
        assert_ne!(clip_id(), id);
        assert!(!is_clip_id("short"));
        assert!(!is_clip_id("aB3xY9k2Q-"));
        assert!(!is_clip_id("healthz"));
    }

    #[test]
    fn tokens_hash_the_same_every_time() {
        let t = token();
        assert!(t.starts_with("hct_") && t.len() == 47);
        assert_eq!(token_hash(&t), token_hash(&t));
        assert_ne!(token_hash(&t), token_hash(&token()));
    }
}
