// The cash drawer between opening and closing: the float it starts with,
// money taken out or put in by hand, and at the end what was counted against
// what the sales say should be there.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{fail, Result};
use crate::tickets::{now, MAX_PRICE};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: i64,
    pub opened_at: String,
    pub opened_by: String,
    pub float_cents: i64,
    pub closed_at: Option<String>,
    pub closed_by: Option<String>,
    pub expected_cents: Option<i64>,
    pub counted_cents: Option<i64>,
    pub note: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Move {
    pub at: String,
    pub by_name: String,
    pub cents: i64,
    pub reason: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Drawer {
    pub session: Option<Session>,
    pub moves: Vec<Move>,
    // cash tickets since it was opened, returns included (negative)
    pub cash_cents: i64,
    pub moves_cents: i64,
    pub expected_cents: i64,
}

const COLS: &str = "id, opened_at, opened_by, float_cents, closed_at, closed_by, expected_cents, counted_cents, note";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        opened_at: r.get(1)?,
        opened_by: r.get(2)?,
        float_cents: r.get(3)?,
        closed_at: r.get(4)?,
        closed_by: r.get(5)?,
        expected_cents: r.get(6)?,
        counted_cents: r.get(7)?,
        note: r.get(8)?,
    })
}

pub fn open_session(conn: &Connection) -> Result<Option<Session>> {
    Ok(conn
        .query_row(&format!("SELECT {COLS} FROM cash_sessions WHERE closed_at IS NULL ORDER BY id DESC LIMIT 1"), [], row)
        .optional()?)
}

pub fn get(conn: &Connection, id: i64) -> Result<Session> {
    match conn.query_row(&format!("SELECT {COLS} FROM cash_sessions WHERE id = ?1"), [id], row).optional()? {
        Some(s) => Ok(s),
        None => fail("E_NOT_FOUND"),
    }
}

fn amount(cents: i64) -> Result<i64> {
    if cents.abs() > 100 * MAX_PRICE {
        return fail("E_INVALID:amount");
    }
    Ok(cents)
}

pub fn open(conn: &Connection, by: &str, float_cents: i64) -> Result<Session> {
    if open_session(conn)?.is_some() {
        return fail("E_CASH_OPEN");
    }
    if amount(float_cents)? < 0 {
        return fail("E_INVALID:amount");
    }
    conn.execute(
        "INSERT INTO cash_sessions (opened_at, opened_by, float_cents) VALUES (?1, ?2, ?3)",
        params![now(), by, float_cents],
    )?;
    get(conn, conn.last_insert_rowid())
}

pub fn add_move(conn: &Connection, by: &str, cents: i64, reason: &str) -> Result<()> {
    let Some(s) = open_session(conn)? else {
        return fail("E_CASH_CLOSED");
    };
    let reason: String = reason.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
    if reason.is_empty() {
        return fail("E_REASON");
    }
    if amount(cents)? == 0 {
        return fail("E_INVALID:amount");
    }
    conn.execute(
        "INSERT INTO cash_moves (session_id, at, by_name, cents, reason) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![s.id, now(), by, cents, reason],
    )?;
    Ok(())
}

// what the drawer should hold for a session, up to `until` (now while open)
fn figures(conn: &Connection, s: &Session, until: &str) -> Result<(Vec<Move>, i64)> {
    let cash: i64 = conn.query_row(
        "SELECT COALESCE(SUM(total_cents), 0) FROM tickets
         WHERE payment_method = 'cash' AND voided_at IS NULL AND training = 0 AND issued_at >= ?1 AND issued_at <= ?2",
        params![s.opened_at, until],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare("SELECT at, by_name, cents, reason FROM cash_moves WHERE session_id = ?1 ORDER BY id")?;
    let moves = stmt
        .query_map([s.id], |r| Ok(Move { at: r.get(0)?, by_name: r.get(1)?, cents: r.get(2)?, reason: r.get(3)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((moves, cash))
}

pub fn drawer(conn: &Connection, s: Option<Session>) -> Result<Drawer> {
    let Some(s) = s else {
        return Ok(Drawer { session: None, moves: Vec::new(), cash_cents: 0, moves_cents: 0, expected_cents: 0 });
    };
    let until = s.closed_at.clone().unwrap_or_else(now);
    let (moves, cash) = figures(conn, &s, &until)?;
    let moves_cents = moves.iter().map(|m| m.cents).sum::<i64>();
    let expected = s.expected_cents.unwrap_or(s.float_cents + cash + moves_cents);
    Ok(Drawer { session: Some(s), moves, cash_cents: cash, moves_cents, expected_cents: expected })
}

pub fn close(conn: &mut Connection, by: &str, counted_cents: i64, note: &str) -> Result<Session> {
    let tx = conn.transaction()?;
    let Some(s) = open_session(&tx)? else {
        return fail("E_CASH_CLOSED");
    };
    if amount(counted_cents)? < 0 {
        return fail("E_INVALID:amount");
    }
    let at = now();
    let d = drawer(&tx, Some(s.clone()))?;
    let note: String = note.trim().chars().take(200).collect();
    tx.execute(
        "UPDATE cash_sessions SET closed_at = ?1, closed_by = ?2, expected_cents = ?3, counted_cents = ?4, note = ?5 WHERE id = ?6",
        params![at, by, d.expected_cents, counted_cents, (!note.is_empty()).then_some(note), s.id],
    )?;
    let closed = get(&tx, s.id)?;
    tx.commit()?;
    Ok(closed)
}

pub fn history(conn: &Connection, limit: i64) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM cash_sessions ORDER BY id DESC LIMIT ?1"))?;
    let rows = stmt.query_map([limit], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::state::SessionUser;
    use crate::tickets::{finalize, PaymentIn, SaleLine};

    #[test]
    fn the_count_is_checked_against_float_sales_and_moves() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let mut conn = crate::db::open(&std::env::temp_dir().join(format!("tiquet-cash-{n}")).join("t.db")).unwrap();
        conn.execute("INSERT INTO users (name, role, pass_hash) VALUES ('Ana', 'admin', 'x')", []).unwrap();
        assert_eq!(add_move(&conn, "Ana", 500, "cambio").unwrap_err().0, "E_CASH_CLOSED");
        open(&conn, "Ana", 10_000).unwrap();
        assert_eq!(open(&conn, "Ana", 0).unwrap_err().0, "E_CASH_OPEN");
        let user = SessionUser { id: 1, name: "Ana".into(), role: "admin".into() };
        let line = SaleLine { name: "Cable".into(), qty: 1, unit_cents: 690, vat_bp: 2100, ..SaleLine::default() };
        let pay = |m: &str| PaymentIn { method: m.into(), tendered_cents: None };
        finalize(&mut conn, &user, &Settings::default(), vec![line.clone()], &pay("cash")).unwrap();
        finalize(&mut conn, &user, &Settings::default(), vec![line.clone()], &pay("card")).unwrap();
        finalize(&mut conn, &user, &Settings { training: true, ..Settings::default() }, vec![line], &pay("cash")).unwrap();
        add_move(&conn, "Ana", -2_000, "pago proveedor").unwrap();
        assert_eq!(add_move(&conn, "Ana", 100, " ").unwrap_err().0, "E_REASON");
        let d = drawer(&conn, open_session(&conn).unwrap()).unwrap();
        // float 100,00 + cash sale 6,90 - 20,00 out; the card sale and the practice one do not count
        assert_eq!((d.cash_cents, d.moves_cents, d.expected_cents), (690, -2_000, 8_690));
        let closed = close(&mut conn, "Ana", 8_600, "faltan 90 céntimos").unwrap();
        assert_eq!((closed.expected_cents, closed.counted_cents), (Some(8_690), Some(8_600)));
        assert!(conn.execute("UPDATE cash_sessions SET counted_cents = 8690", []).is_err());
        assert!(open_session(&conn).unwrap().is_none());
        assert_eq!(history(&conn, 10).unwrap().len(), 1);
    }
}
