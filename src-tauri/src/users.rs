use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth;
use crate::error::{fail, Result};
use crate::state::SessionUser;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UserRow {
    pub id: i64,
    pub name: String,
    pub role: String,
    pub active: bool,
    pub created_at: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UserIn {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    pub role: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "yes")]
    pub active: bool,
}

fn yes() -> bool {
    true
}

pub fn list(conn: &Connection, only_active: bool) -> Result<Vec<UserRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, role, active, created_at FROM users
         WHERE active = 1 OR ?1 = 0
         ORDER BY role, name COLLATE NOCASE",
    )?;
    let rows = stmt
        .query_map([only_active], |r| {
            Ok(UserRow {
                id: r.get(0)?,
                name: r.get(1)?,
                role: r.get(2)?,
                active: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?)
}

pub fn credentials(conn: &Connection, id: i64) -> Result<Option<(SessionUser, String, bool)>> {
    Ok(conn
        .query_row(
            "SELECT id, name, role, pass_hash, active FROM users WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    SessionUser { id: r.get(0)?, name: r.get(1)?, role: r.get(2)? },
                    r.get(3)?,
                    r.get(4)?,
                ))
            },
        )
        .optional()?)
}

// a PIN is fine for a cashier; whoever can void, refund and manage users
// needs at least six characters
fn strong_enough(role: &str, password: &str) -> Result<()> {
    let n = password.chars().count();
    if role == "admin" && n < 6 {
        return fail("E_INVALID:adminPassword");
    }
    if n < 4 {
        return fail("E_INVALID:password");
    }
    Ok(())
}

pub fn set_password(conn: &Connection, id: i64, password: &str) -> Result<()> {
    let role: String = conn.query_row("SELECT role FROM users WHERE id = ?1", [id], |r| r.get(0))?;
    strong_enough(&role, password)?;
    conn.execute("UPDATE users SET pass_hash = ?1 WHERE id = ?2", params![auth::hash(password)?, id])?;
    Ok(())
}

pub fn save(conn: &Connection, u: UserIn) -> Result<UserRow> {
    let name = u.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > 40 {
        return fail("E_INVALID:name");
    }
    if u.role != "admin" && u.role != "cashier" {
        return fail("E_INVALID:role");
    }
    let new_pw = !u.password.is_empty();
    if u.id.is_none() || new_pw {
        strong_enough(&u.role, &u.password)?;
    }
    let taken: Option<i64> = conn
        .query_row(
            "SELECT id FROM users WHERE name = ?1 COLLATE NOCASE AND id <> ?2",
            params![name, u.id.unwrap_or(0)],
            |r| r.get(0),
        )
        .optional()?;
    if taken.is_some() {
        return fail("E_USER_EXISTS");
    }

    let id = match u.id {
        None => {
            conn.execute(
                "INSERT INTO users (name, role, pass_hash, active) VALUES (?1, ?2, ?3, ?4)",
                params![name, u.role, auth::hash(&u.password)?, u.active],
            )?;
            conn.last_insert_rowid()
        }
        Some(id) => {
            let was_admin: Option<bool> = conn
                .query_row("SELECT role = 'admin' AND active = 1 FROM users WHERE id = ?1", [id], |r| r.get(0))
                .optional()?;
            let Some(was_admin) = was_admin else {
                return fail("E_NOT_FOUND");
            };
            if was_admin && (u.role != "admin" || !u.active) {
                let others: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM users WHERE role = 'admin' AND active = 1 AND id <> ?1",
                    [id],
                    |r| r.get(0),
                )?;
                if others == 0 {
                    return fail("E_LAST_ADMIN");
                }
            }
            conn.execute(
                "UPDATE users SET name = ?1, role = ?2, active = ?3 WHERE id = ?4",
                params![name, u.role, u.active, id],
            )?;
            if new_pw {
                conn.execute(
                    "UPDATE users SET pass_hash = ?1 WHERE id = ?2",
                    params![auth::hash(&u.password)?, id],
                )?;
            }
            id
        }
    };

    Ok(conn.query_row(
        "SELECT id, name, role, active, created_at FROM users WHERE id = ?1",
        [id],
        |r| {
            Ok(UserRow {
                id: r.get(0)?,
                name: r.get(1)?,
                role: r.get(2)?,
                active: r.get(3)?,
                created_at: r.get(4)?,
            })
        },
    )?)
}
