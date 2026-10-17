use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::error::{Error, Result};

pub fn hash(pw: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error(e.to_string()))
}

pub fn verify(pw: &str, stored: &str) -> bool {
    PasswordHash::new(stored)
        .map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hash_roundtrip() {
        let h = super::hash("1234").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(super::verify("1234", &h));
        assert!(!super::verify("12345", &h));
        assert!(!super::verify("1234", "not a hash"));
    }
}
