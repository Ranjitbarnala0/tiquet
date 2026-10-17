use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use chrono::{Local, NaiveDateTime, TimeDelta};
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{fail, Error, Result};

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

// Five misses in a row lock that key (a user id, or 0 for the recovery code)
// for `base` seconds, doubling with every further miss up to 15 minutes. It
// is kept in the database, so restarting the app does not reset it.
const MAX_LOCK: i64 = 900;

fn now() -> NaiveDateTime {
    Local::now().naive_local()
}

pub fn check_lock(conn: &Connection, key: i64) -> Result<()> {
    let until: Option<Option<String>> =
        conn.query_row("SELECT locked_until FROM attempts WHERE key = ?1", [key], |r| r.get(0)).optional()?;
    let until = until.flatten().and_then(|u| NaiveDateTime::parse_from_str(&u, "%Y-%m-%dT%H:%M:%S").ok());
    if let Some(u) = until {
        let wait = (u - now()).num_seconds();
        if wait > 0 {
            return fail(format!("E_LOCKED:{wait}"));
        }
    }
    Ok(())
}

pub fn missed(conn: &Connection, key: i64, base: i64) -> Result<i64> {
    let n: i64 = conn.query_row("SELECT COALESCE((SELECT failures FROM attempts WHERE key = ?1), 0) + 1", [key], |r| r.get(0))?;
    let until = (n >= 5).then(|| {
        let secs = (base << (n - 5).min(10)).min(MAX_LOCK);
        (now() + TimeDelta::seconds(secs)).format("%Y-%m-%dT%H:%M:%S").to_string()
    });
    conn.execute(
        "INSERT INTO attempts (key, failures, locked_until) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET failures = excluded.failures, locked_until = excluded.locked_until",
        params![key, n, until],
    )?;
    Ok(n)
}

pub fn cleared(conn: &Connection, key: i64) -> Result<()> {
    conn.execute("DELETE FROM attempts WHERE key = ?1", [key])?;
    Ok(())
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
