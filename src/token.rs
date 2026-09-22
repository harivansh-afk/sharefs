use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

const VERSION: u8 = 1;
const DOMAIN: &[u8] = b"sharefs/read-file\0";
const TOKEN_BYTES: usize = 1 + 8 + 32;

pub struct SigningKey(Hmac<Sha256>);

impl SigningKey {
    pub fn new(key: &[u8; 32]) -> Self {
        Self(Hmac::new_from_slice(key).expect("HMAC accepts a 32-byte key"))
    }

    pub fn sign(&self, path: &str, expires_at: u64) -> String {
        let mut token = [0; TOKEN_BYTES];
        token[0] = VERSION;
        token[1..9].copy_from_slice(&expires_at.to_be_bytes());

        let signature = self.mac(path, &token[..9]).finalize().into_bytes();
        token[9..].copy_from_slice(&signature);

        URL_SAFE_NO_PAD.encode(token)
    }

    /// Verification uses a fixed-size buffer and constant-time MAC comparison.
    pub fn verify(&self, path: &str, encoded: &str, now: u64) -> bool {
        let mut token = [0; TOKEN_BYTES];
        if URL_SAFE_NO_PAD.decode_slice(encoded, &mut token) != Ok(TOKEN_BYTES)
            || token[0] != VERSION
        {
            return false;
        }

        let expires_at = u64::from_be_bytes(token[1..9].try_into().unwrap());
        if expires_at <= now {
            return false;
        }

        self.mac(path, &token[..9])
            .verify_slice(&token[9..])
            .is_ok()
    }

    fn mac(&self, path: &str, claims: &[u8]) -> Hmac<Sha256> {
        let mut mac = self.0.clone();
        mac.update(DOMAIN);
        mac.update(claims);
        mac.update(path.as_bytes());
        mac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_path_key_and_expiry() {
        let key = SigningKey::new(&[7; 32]);
        let token = key.sign("/Documents/notes.md", 100);

        assert!(key.verify("/Documents/notes.md", &token, 99));
        assert!(!key.verify("/Documents/other.md", &token, 99));
        assert!(!key.verify("/Documents/notes.md", &token, 100));
        assert!(!SigningKey::new(&[8; 32]).verify("/Documents/notes.md", &token, 99));

        let mut changed = URL_SAFE_NO_PAD.decode(&token).unwrap();
        changed[8] = 200;
        assert!(!key.verify("/Documents/notes.md", &URL_SAFE_NO_PAD.encode(changed), 99));
    }

    #[test]
    fn rejects_malformed_tokens_without_panicking() {
        let key = SigningKey::new(&[7; 32]);
        for token in ["", "!", "1", "ab", &"x".repeat(100_000)] {
            assert!(!key.verify("/file", token, 0));
        }

        let valid = key.sign("/file", 100);
        for length in 0..valid.len() {
            assert!(!key.verify("/file", &valid[..length], 0));
        }
    }
}
