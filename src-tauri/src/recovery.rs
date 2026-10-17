// A one-time code shown when the shop is set up. It lets an administrator
// set a new password after forgetting the old one, with no server involved.
// Only its Argon2 hash is stored, and every use replaces it.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::error::Result;
use crate::{auth, settings, tickets};

// no 0/O or 1/I, so it can be read back from paper
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn generate() -> String {
    let mut b = [0u8; 16];
    OsRng.fill_bytes(&mut b);
    // 256 is a multiple of 32, so every symbol is equally likely
    let s: Vec<char> = b.iter().map(|x| ALPHABET[*x as usize % 32] as char).collect();
    s.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

pub fn normalize(code: &str) -> String {
    code.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).collect()
}

pub fn issue(conn: &Connection) -> Result<String> {
    let code = generate();
    settings::put(conn, "recoveryHash", &auth::hash(&normalize(&code))?)?;
    settings::put(conn, "recoveryAt", &tickets::now())?;
    Ok(code)
}

pub fn exists(conn: &Connection) -> Result<bool> {
    Ok(settings::get(conn, "recoveryHash")?.is_some())
}

pub fn status(conn: &Connection) -> Result<Value> {
    Ok(json!({ "set": exists(conn)?, "at": settings::get(conn, "recoveryAt")? }))
}

pub fn matches(conn: &Connection, code: &str) -> Result<bool> {
    let code = normalize(code);
    Ok(code.len() == 16 && settings::get(conn, "recoveryHash")?.is_some_and(|h| auth::verify(&code, &h)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_readable_and_forgiving_to_type() {
        let c = generate();
        assert_eq!(c.len(), 19);
        assert!(c.split('-').all(|g| g.len() == 4 && g.bytes().all(|b| ALPHABET.contains(&b))), "{c}");
        assert_ne!(c, generate());
        assert_eq!(normalize(" ab2c-dE3f ghjk-LMNP\n"), "AB2CDE3FGHJKLMNP");
    }
}
