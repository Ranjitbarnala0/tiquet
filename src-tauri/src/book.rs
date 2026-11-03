// The shop's book: every euro in and out in one list. Sales charged with a
// ticket come from the tickets themselves; sales made without a ticket and
// expenses are typed here, one line each. An entry is never edited or
// wiped: deleting one takes it out of every list and total, and keeps a note
// of who deleted it and when that only the owner sees, so the books stay
// traceable (Ley 58/2003 art. 29.2.j).

use std::fs;
use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{fail, Result};
use crate::layout::{datetime, method_label};
use crate::money::div_round;
use crate::reports::{cell, plain, Range};
use crate::settings::Settings;
use crate::state::SessionUser;
use crate::tickets::{now, MAX_PRICE};
use crate::backup;

pub const METHODS: [&str; 4] = ["cash", "card", "bizum", "transfer"];

#[derive(Deserialize, Default, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct EntryIn {
    // sale | expense
    pub kind: String,
    pub concept: String,
    pub cents: i64,
    pub method: String,
    pub vat_bp: Option<i64>,
    pub category: String,
    pub supplier: String,
    pub supplier_nif: String,
    pub invoice_ref: String,
    // YYYY-MM-DD for something done another day; today when empty
    pub date: String,
    // made by the screen for each entry: a second Enter returns the first
    pub client_id: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: i64,
    pub kind: String,
    pub at: String,
    pub concept: String,
    pub cents: i64,
    pub method: String,
    pub vat_bp: i64,
    pub category: String,
    pub supplier: String,
    pub supplier_nif: String,
    pub invoice_ref: String,
    pub by_name: String,
    pub deleted_at: Option<String>,
    pub deleted_by: Option<String>,
    pub delete_reason: Option<String>,
}

// a line of the book as the screen shows it: an entry or a ticket
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    // sale (typed, no ticket) | expense | ticket
    pub source: &'static str,
    pub id: i64,
    pub at: String,
    pub concept: String,
    // what it did to the takings: an expense or a return is negative
    pub cents: i64,
    pub method: String,
    pub by_name: String,
    pub category: String,
    // a ticket's number
    pub number: Option<String>,
    // a voided ticket or a deleted entry: shown crossed out, never counted
    pub gone: bool,
    pub deleted_by: Option<String>,
    pub deleted_at: Option<String>,
    pub delete_reason: Option<String>,
}

#[derive(Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub ticket_cents: i64,
    pub ticket_count: i64,
    pub sale_cents: i64,
    pub sale_count: i64,
    pub expense_cents: i64,
    pub expense_count: i64,
    pub result_cents: i64,
    // cash in less cash out: tickets, typed sales and expenses paid in cash
    pub cash_cents: i64,
    pub by_category: Vec<(String, i64)>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Quick {
    pub concept: String,
    pub cents: i64,
    pub uses: i64,
}

const COLS: &str = "id, kind, at, concept, cents, method, vat_bp, category, supplier, supplier_nif, invoice_ref, by_name, deleted_at, deleted_by, delete_reason";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: r.get(0)?,
        kind: r.get(1)?,
        at: r.get(2)?,
        concept: r.get(3)?,
        cents: r.get(4)?,
        method: r.get(5)?,
        vat_bp: r.get(6)?,
        category: r.get(7)?,
        supplier: r.get(8)?,
        supplier_nif: r.get(9)?,
        invoice_ref: r.get(10)?,
        by_name: r.get(11)?,
        deleted_at: r.get(12)?,
        deleted_by: r.get(13)?,
        delete_reason: r.get(14)?,
    })
}

pub fn get(conn: &Connection, id: i64) -> Result<Entry> {
    match conn.query_row(&format!("SELECT {COLS} FROM book WHERE id = ?1"), [id], row).optional()? {
        Some(e) => Ok(e),
        None => fail("E_NOT_FOUND"),
    }
}

fn text(s: &str, max: usize) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

fn event(conn: &Connection, id: i64, by: &str, kind: &str, detail: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT INTO book_events (entry_id, at, by_name, kind, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, now(), by, kind, detail],
    )?;
    Ok(())
}

// Returns the entry and whether it was already there (the same client id).
pub fn add(conn: &Connection, me: &SessionUser, s: &Settings, e: EntryIn) -> Result<(Entry, bool)> {
    if let Some(cid) = e.client_id.as_deref().filter(|c| !c.is_empty()) {
        let found: Option<i64> = conn.query_row("SELECT id FROM book WHERE client_id = ?1", [cid], |r| r.get(0)).optional()?;
        if let Some(id) = found {
            return Ok((get(conn, id)?, true));
        }
    }
    let kind = e.kind.as_str();
    if !["sale", "expense"].contains(&kind) {
        return fail("E_INVALID:kind");
    }
    if e.cents <= 0 || e.cents > 100 * MAX_PRICE {
        return fail("E_INVALID:amount");
    }
    let method = if e.method.is_empty() { "cash" } else { e.method.as_str() };
    if !METHODS.contains(&method) {
        return fail("E_INVALID:method");
    }
    let concept = match text(&e.concept, 120) {
        c if c.is_empty() && kind == "sale" => "Venta".to_string(),
        c if c.is_empty() => "Gasto".to_string(),
        c => c,
    };
    let category = match (kind, text(&e.category, 60)) {
        ("expense", c) if c.is_empty() => "Otros".to_string(),
        ("expense", c) => c,
        _ => String::new(),
    };
    let vat = e.vat_bp.unwrap_or(s.default_vat_bp).clamp(0, 10_000);
    let at = match e.date.trim() {
        "" => now(),
        d => {
            let Ok(day) = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d") else {
                return fail("E_INVALID:date");
            };
            let today = now()[..10].to_string();
            match d.cmp(today.as_str()) {
                std::cmp::Ordering::Greater => return fail("E_INVALID:date"),
                std::cmp::Ordering::Equal => now(),
                std::cmp::Ordering::Less => format!("{}T12:00:00", day.format("%Y-%m-%d")),
            }
        }
    };
    conn.execute(
        "INSERT INTO book (kind, at, concept, cents, method, vat_bp, category, supplier, supplier_nif, invoice_ref, by_name, training, client_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            kind,
            at,
            concept,
            e.cents,
            method,
            vat,
            category,
            text(&e.supplier, 120),
            text(&e.supplier_nif, 20).to_uppercase(),
            text(&e.invoice_ref, 40),
            me.name,
            s.training,
            e.client_id.filter(|c| !c.is_empty()),
        ],
    )?;
    let id = conn.last_insert_rowid();
    event(conn, id, &me.name, "added", None)?;
    Ok((get(conn, id)?, false))
}

// The owner may delete anything; anyone else only what they typed today,
// when Ajustes allows it.
fn may_change(me: &SessionUser, e: &Entry, staff: bool) -> bool {
    me.role == "admin" || (staff && e.by_name == me.name && e.at[..10] == now()[..10])
}

pub fn delete(conn: &Connection, me: &SessionUser, staff: bool, id: i64, reason: &str) -> Result<Entry> {
    let e = get(conn, id)?;
    if !may_change(me, &e, staff) {
        return fail("E_FORBIDDEN");
    }
    if e.deleted_at.is_some() {
        return Ok(e);
    }
    let reason = text(reason, 120);
    conn.execute(
        "UPDATE book SET deleted_at = ?1, deleted_by = ?2, delete_reason = ?3 WHERE id = ?4",
        params![now(), me.name, (!reason.is_empty()).then_some(reason.as_str()), id],
    )?;
    event(conn, id, &me.name, "deleted", (!reason.is_empty()).then_some(reason.as_str()))?;
    get(conn, id)
}

pub fn restore(conn: &Connection, me: &SessionUser, staff: bool, id: i64) -> Result<Entry> {
    let e = get(conn, id)?;
    if !may_change(me, &e, staff) {
        return fail("E_FORBIDDEN");
    }
    if e.deleted_at.is_none() {
        return Ok(e);
    }
    conn.execute("UPDATE book SET deleted_at = NULL, deleted_by = NULL, delete_reason = NULL WHERE id = ?1", [id])?;
    event(conn, id, &me.name, "restored", None)?;
    get(conn, id)
}

// Everything between two dates, newest first: typed entries (the deleted
// ones only when asked) and tickets (voided ones crossed out).
pub fn rows(conn: &Connection, r: &Range, training: bool, deleted: bool) -> Result<Vec<Row>> {
    let (from, to) = r.bounds()?;
    let mut out = Vec::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM book WHERE at >= ?1 AND at <= ?2 AND training = ?3 AND (?4 OR deleted_at IS NULL)"
    ))?;
    for e in stmt.query_map(params![from, to, training, deleted], row)? {
        let e = e?;
        let sign = if e.kind == "expense" { -1 } else { 1 };
        out.push(Row {
            source: if e.kind == "expense" { "expense" } else { "sale" },
            id: e.id,
            at: e.at,
            concept: e.concept,
            cents: sign * e.cents,
            method: e.method,
            by_name: e.by_name,
            category: e.category,
            number: None,
            gone: e.deleted_at.is_some(),
            deleted_by: e.deleted_by,
            deleted_at: e.deleted_at,
            delete_reason: e.delete_reason,
        });
    }
    let mut stmt = conn.prepare(
        "SELECT t.id, t.issued_at, t.number, t.kind, t.total_cents, t.payment_method, t.user_name, t.voided_at IS NOT NULL,
                COALESCE((SELECT name FROM ticket_lines WHERE ticket_id = t.id ORDER BY pos LIMIT 1), ''),
                (SELECT COUNT(*) FROM ticket_lines WHERE ticket_id = t.id)
         FROM tickets t WHERE t.issued_at >= ?1 AND t.issued_at <= ?2 AND t.training = ?3",
    )?;
    let tickets = stmt.query_map(params![from, to, training], |r| {
        let first: String = r.get(8)?;
        let lines: i64 = r.get(9)?;
        let kind: String = r.get(3)?;
        let mut concept = if lines > 1 { format!("{first} y {} más", lines - 1) } else { first };
        if kind == "refund" {
            concept = format!("Devolución: {concept}");
        }
        Ok(Row {
            source: "ticket",
            id: r.get(0)?,
            at: r.get(1)?,
            concept,
            cents: r.get(4)?,
            method: r.get(5)?,
            by_name: r.get(6)?,
            category: String::new(),
            number: Some(r.get(2)?),
            gone: r.get(7)?,
            deleted_by: None,
            deleted_at: None,
            delete_reason: None,
        })
    })?;
    for t in tickets {
        out.push(t?);
    }
    out.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
    Ok(out)
}

pub fn summary(rows: &[Row]) -> Summary {
    let mut s = Summary::default();
    let mut cats: std::collections::BTreeMap<String, i64> = Default::default();
    for r in rows.iter().filter(|r| !r.gone) {
        match r.source {
            "ticket" => {
                s.ticket_cents += r.cents;
                s.ticket_count += 1;
            }
            "sale" => {
                s.sale_cents += r.cents;
                s.sale_count += 1;
            }
            _ => {
                s.expense_cents -= r.cents;
                s.expense_count += 1;
                *cats.entry(r.category.clone()).or_default() -= r.cents;
            }
        }
        if r.method == "cash" {
            s.cash_cents += r.cents;
        }
    }
    s.result_cents = s.ticket_cents + s.sale_cents - s.expense_cents;
    let mut by: Vec<(String, i64)> = cats.into_iter().collect();
    by.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    s.by_category = by;
    s
}

// what the shop types most, for one-tap buttons: the last 90 days' sales
pub fn quick(conn: &Connection, training: bool) -> Result<Vec<Quick>> {
    let since = (chrono::Local::now() - chrono::Duration::days(90)).format("%Y-%m-%d").to_string();
    let mut stmt = conn.prepare(
        "SELECT concept, cents, n FROM (
           SELECT concept, cents, COUNT(*) OVER (PARTITION BY lower(concept)) AS n,
                  ROW_NUMBER() OVER (PARTITION BY lower(concept) ORDER BY id DESC) AS k
           FROM book WHERE kind = 'sale' AND deleted_at IS NULL AND training = ?1 AND at >= ?2 AND concept <> 'Venta')
         WHERE k = 1 ORDER BY n DESC, concept LIMIT 8",
    )?;
    let rows = stmt
        .query_map(params![training, since], |r| Ok(Quick { concept: r.get(0)?, cents: r.get(1)?, uses: r.get(2)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// what the cash drawer gained from typed entries since it was opened
pub fn cash_between(conn: &Connection, from: &str, to: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(CASE kind WHEN 'sale' THEN cents ELSE -cents END), 0) FROM book
         WHERE method = 'cash' AND deleted_at IS NULL AND training = 0 AND at >= ?1 AND at <= ?2",
        params![from, to],
        |r| r.get(0),
    )?)
}

// The book for the gestor: income and expenses with VAT split out, the
// tickets as one line each. Deleted entries and voided tickets are not in
// the books, so not here either.
pub fn export(conn: &Connection, r: &Range) -> Result<PathBuf> {
    let (from, to) = r.bounds()?;
    let mut out = String::from(
        "\u{feff}Fecha;Origen;Tipo;Concepto;Categoría;Pago;Base;IVA %;Cuota IVA;Total;Proveedor;NIF proveedor;Nº factura;Usuario\r\n",
    );
    let mut lines: Vec<(String, String)> = Vec::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM book WHERE at >= ?1 AND at <= ?2 AND training = 0 AND deleted_at IS NULL"
    ))?;
    for e in stmt.query_map(params![from, to], row)? {
        let e = e?;
        let base = div_round(e.cents * 10_000, 10_000 + e.vat_bp);
        let sign = if e.kind == "expense" { -1 } else { 1 };
        let fields = [
            datetime(&e.at),
            "Manual".to_string(),
            if e.kind == "expense" { "Gasto".into() } else { "Venta sin ticket".into() },
            cell(&e.concept),
            cell(&e.category),
            method_label(&e.method).to_string(),
            plain(sign * base),
            plain(e.vat_bp),
            plain(sign * (e.cents - base)),
            plain(sign * e.cents),
            cell(&e.supplier),
            cell(&e.supplier_nif),
            cell(&e.invoice_ref),
            cell(&e.by_name),
        ];
        lines.push((e.at.clone(), fields.join(";")));
    }
    let mut stmt = conn.prepare(
        "SELECT t.issued_at, t.number, t.kind, t.total_cents, t.payment_method, t.user_name,
                COALESCE((SELECT SUM(base_cents) FROM ticket_vat WHERE ticket_id = t.id), t.total_cents),
                COALESCE((SELECT SUM(vat_cents) FROM ticket_vat WHERE ticket_id = t.id), 0),
                (SELECT CASE WHEN COUNT(*) = 1 THEN MAX(vat_bp) END FROM ticket_vat WHERE ticket_id = t.id)
         FROM tickets t WHERE t.issued_at >= ?1 AND t.issued_at <= ?2 AND t.training = 0 AND t.voided_at IS NULL",
    )?;
    let tickets = stmt.query_map(params![from, to], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, i64>(7)?,
            r.get::<_, Option<i64>>(8)?,
        ))
    })?;
    for t in tickets {
        let (at, number, kind, total, method, user, base, vat, rate) = t?;
        let fields = [
            datetime(&at),
            format!("Ticket {number}"),
            if kind == "refund" { "Devolución".into() } else { "Venta con ticket".into() },
            String::new(),
            String::new(),
            method_label(&method).to_string(),
            plain(base),
            rate.map(plain).unwrap_or_else(|| "varios".into()),
            plain(vat),
            plain(total),
            String::new(),
            String::new(),
            String::new(),
            cell(&user),
        ];
        lines.push((at, fields.join(";")));
    }
    lines.sort();
    for (_, l) in lines {
        out.push_str(&l);
        out.push_str("\r\n");
    }
    let dir = backup::documents().join("Exportaciones");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("libro_{}_{}.csv", r.from, r.to));
    fs::write(&path, out)?;
    Ok(path)
}

// practice entries go with the practice tickets
pub fn wipe_training(conn: &Connection) -> Result<usize> {
    conn.execute("DELETE FROM book_events WHERE entry_id IN (SELECT id FROM book WHERE training = 1)", [])?;
    Ok(conn.execute("DELETE FROM book WHERE training = 1", [])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        crate::db::open(&std::env::temp_dir().join(format!("tiquet-book-{n}")).join("t.db")).unwrap()
    }

    fn me(name: &str, role: &str) -> SessionUser {
        SessionUser { id: 1, name: name.into(), role: role.into() }
    }

    fn entry(kind: &str, concept: &str, cents: i64, method: &str) -> EntryIn {
        EntryIn { kind: kind.into(), concept: concept.into(), cents, method: method.into(), ..EntryIn::default() }
    }

    fn today() -> Range {
        let d = now()[..10].to_string();
        Range { from: d.clone(), to: d }
    }

    #[test]
    fn sales_and_expenses_add_up_and_deleting_leaves_a_trace() {
        let c = conn();
        let s = Settings::default();
        let ana = me("Ana", "cashier");
        let (a, _) = add(&c, &ana, &s, entry("sale", "Recarga", 1000, "cash")).unwrap();
        add(&c, &ana, &s, entry("sale", "  funda   iphone ", 1250, "card")).unwrap();
        add(&c, &ana, &s, EntryIn { category: "Luz y agua".into(), ..entry("expense", "Luz", 4500, "cash") }).unwrap();
        let list = rows_today(&c);
        let sum = summary(&list);
        assert_eq!((sum.sale_cents, sum.expense_cents, sum.result_cents, sum.cash_cents), (2250, 4500, -2250, -3500));
        assert_eq!(sum.by_category, vec![("Luz y agua".to_string(), 4500)]);
        assert!(list.iter().any(|r| r.concept == "funda iphone"));
        // deleted: out of every list and total, but kept with who did it
        delete(&c, &ana, true, a.id, "").unwrap();
        let after = summary(&rows_today(&c));
        assert_eq!(after.sale_cents, 1250);
        let kept = rows(&c, &today(), false, true).unwrap();
        let gone = kept.iter().find(|r| r.id == a.id && r.source == "sale").unwrap();
        assert!(gone.gone && gone.deleted_by.as_deref() == Some("Ana"));
        assert!(c.execute("DELETE FROM book WHERE id = ?1", [a.id]).is_err());
        assert!(c.execute("UPDATE book SET cents = 1 WHERE id = ?1", [a.id]).is_err());
        // undone: back in the totals, and the history says both
        restore(&c, &ana, true, a.id).unwrap();
        assert_eq!(summary(&rows_today(&c)).sale_cents, 2250);
        let n: i64 = c.query_row("SELECT COUNT(*) FROM book_events WHERE entry_id = ?1", [a.id], |r| r.get(0)).unwrap();
        assert_eq!(n, 3);
        // someone else's entry is the owner's to delete
        assert_eq!(delete(&c, &me("Eva", "cashier"), true, a.id, "").unwrap_err().0, "E_FORBIDDEN");
        assert!(delete(&c, &me("Jefe", "admin"), false, a.id, "error").is_ok());
        assert_eq!(delete(&c, &ana, false, a.id, "").unwrap_err().0, "E_FORBIDDEN");
    }

    fn rows_today(c: &Connection) -> Vec<Row> {
        rows(c, &today(), false, false).unwrap()
    }

    #[test]
    fn a_second_enter_bad_amounts_and_future_dates_are_refused() {
        let c = conn();
        let s = Settings::default();
        let ana = me("Ana", "cashier");
        let once = EntryIn { client_id: Some("x1".into()), ..entry("sale", "Funda", 900, "cash") };
        let (a, dup) = add(&c, &ana, &s, once).unwrap();
        assert!(!dup);
        let (b, dup) = add(&c, &ana, &s, EntryIn { client_id: Some("x1".into()), ..entry("sale", "Funda", 900, "cash") }).unwrap();
        assert!(dup && a.id == b.id);
        assert_eq!(add(&c, &ana, &s, entry("sale", "x", 0, "cash")).unwrap_err().0, "E_INVALID:amount");
        assert_eq!(add(&c, &ana, &s, entry("sale", "x", 100, "crypto")).unwrap_err().0, "E_INVALID:method");
        let future = EntryIn { date: "2999-01-01".into(), ..entry("expense", "x", 100, "cash") };
        assert_eq!(add(&c, &ana, &s, future).unwrap_err().0, "E_INVALID:date");
        let past = EntryIn { date: "2026-01-02".into(), ..entry("expense", "Alquiler", 50000, "transfer") };
        let (p, _) = add(&c, &ana, &s, past).unwrap();
        assert_eq!((p.at.as_str(), p.category.as_str()), ("2026-01-02T12:00:00", "Otros"));
        // the most typed sales come back as one-tap buttons, at their last price
        add(&c, &ana, &s, entry("sale", "funda", 950, "cash")).unwrap();
        let q = quick(&c, false).unwrap();
        assert_eq!((q[0].concept.as_str(), q[0].cents, q[0].uses), ("funda", 950, 2));
    }
}
