// Every euro in and out in one list. Sales are tickets, printed or not, and
// come from the tickets themselves; recharges and expenses are typed here, one
// line each. An entry is never edited or wiped: deleting one takes it out of
// every list and total, and keeps a note of who deleted it and when that only
// the owner sees, so the books stay traceable (Ley 58/2003 art. 29.2.j).
// Sales typed here before every sale became a ticket stay as they were.

use std::collections::BTreeMap;
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
    // expense | recharge
    pub kind: String,
    pub concept: String,
    pub cents: i64,
    pub method: String,
    pub vat_bp: Option<i64>,
    // who it was bought from; on a recharge, the operator
    pub supplier: String,
    pub supplier_nif: String,
    pub invoice_ref: String,
    // the machine a recharge was made on; none is a recharge in cash
    pub machine: Option<i64>,
    // YYYY-MM-DD for something done another day; today when empty
    pub date: String,
    // made by the screen for each entry: a second Enter returns the first
    pub client_id: Option<String>,
    // a correction stays on the moment of the entry it replaces
    #[serde(skip)]
    pub keep_at: Option<String>,
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
    pub cost_cents: Option<i64>,
    pub deleted_at: Option<String>,
    pub deleted_by: Option<String>,
    pub delete_reason: Option<String>,
    pub machine: Option<i64>,
}

// a line of the day as the screen shows it: a ticket or a typed entry
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    // ticket | recharge | expense | sale (typed, from before)
    pub source: &'static str,
    pub id: i64,
    pub at: String,
    pub concept: String,
    // what it did to the takings: an expense or a return is negative
    pub cents: i64,
    pub method: String,
    pub by_name: String,
    pub supplier: String,
    pub supplier_nif: String,
    pub invoice_ref: String,
    pub vat_bp: i64,
    pub machine: Option<i64>,
    // of a recharge or a sale on a machine, what counts as an expense
    pub cost_cents: i64,
    // of a ticket: its number, whether it is a return or a full invoice, and
    // printed | unknown | failed | none
    pub number: Option<String>,
    pub refund: bool,
    pub invoice: bool,
    pub printed: Option<String>,
    // a voided ticket or a deleted entry: shown crossed out, never counted
    pub gone: bool,
    pub deleted_by: Option<String>,
    pub deleted_at: Option<String>,
    pub delete_reason: Option<String>,
}

#[derive(Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    // sales by how they were paid, net of returns, and recharges apart
    pub cash_cents: i64,
    pub card_cents: i64,
    pub bizum_cents: i64,
    pub other_cents: i64,
    pub recharge_cents: i64,
    // all of the above
    pub total_cents: i64,
    pub sale_count: i64,
    pub refund_count: i64,
    pub recharge_count: i64,
    // what was bought, what recharges and machines add to it, and all three
    pub purchase_cents: i64,
    pub purchase_count: i64,
    pub recharge_cost_cents: i64,
    pub machine_cost_cents: i64,
    pub expense_cents: i64,
    // the total less the expenses
    pub net_cents: i64,
    // cash that came in less cash that went out
    pub drawer_cents: i64,
    // what went through each machine
    pub machines: Vec<MachineSum>,
}

// sales and recharges on one machine; id 0 is cards from before there were
// machines
#[derive(Serialize, Debug, Default, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MachineSum {
    pub id: i64,
    pub sales_cents: i64,
    pub recharge_cents: i64,
    pub count: i64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Quick {
    pub concept: String,
    pub cents: i64,
    pub uses: i64,
}

const COLS: &str = "id, kind, at, concept, cents, method, vat_bp, category, supplier, supplier_nif, invoice_ref, by_name, cost_cents, deleted_at, deleted_by, delete_reason, machine_id";

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
        cost_cents: r.get(12)?,
        deleted_at: r.get(13)?,
        deleted_by: r.get(14)?,
        delete_reason: r.get(15)?,
        machine: r.get(16)?,
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
    let recharge = match e.kind.as_str() {
        "expense" => false,
        "recharge" => true,
        _ => return fail("E_INVALID:kind"),
    };
    if e.cents <= 0 || e.cents > 100 * MAX_PRICE {
        return fail("E_INVALID:amount");
    }
    // a recharge on a machine is paid the way that machine is paid
    let machine = s.machine(e.machine).filter(|m| recharge && m.recharge);
    let method = match machine {
        Some(m) => m.recharge_method(),
        None if e.method.is_empty() => "cash",
        None => e.method.as_str(),
    };
    if !METHODS.contains(&method) {
        return fail("E_INVALID:method");
    }
    let supplier = text(&e.supplier, 120);
    let concept = match text(&e.concept, 120) {
        c if !c.is_empty() => c,
        _ if !recharge => "Gasto".to_string(),
        _ if supplier.is_empty() => "Recarga".to_string(),
        _ => format!("Recarga {supplier}"),
    };
    let at = match e.date.trim() {
        _ if e.keep_at.is_some() => e.keep_at.clone().unwrap_or_default(),
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
    // A recharge always comes in as takings. Made on a machine it is money for
    // the operator too, so it counts as spent but for the shop's commission; in
    // cash it does not, unless the shop says so. Tax is the operator's.
    let spent = |bp: i64| e.cents - div_round(e.cents * bp, 10_000);
    let cost = match machine {
        Some(m) if m.recharge_expense => spent(m.commission_bp),
        None if s.recharge_cash_expense => spent(s.recharge_commission_bp),
        _ => 0,
    };
    let (kind, vat, category, cost) = if recharge {
        ("sale", 0, "", Some(cost))
    } else {
        ("expense", e.vat_bp.unwrap_or(s.default_vat_bp).clamp(0, 10_000), "Compras", None)
    };
    conn.execute(
        "INSERT INTO book (kind, at, concept, cents, method, vat_bp, category, supplier, supplier_nif, invoice_ref, by_name, training, client_id, cost_cents, machine_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            kind,
            at,
            concept,
            e.cents,
            method,
            vat,
            category,
            supplier,
            text(&e.supplier_nif, 20).to_uppercase(),
            text(&e.invoice_ref, 40),
            me.name,
            s.training,
            e.client_id.filter(|c| !c.is_empty()),
            cost,
            machine.map(|m| m.id),
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

fn mark(conn: &Connection, id: i64, by: &str, reason: &str) -> Result<()> {
    let reason = text(reason, 120);
    let reason = (!reason.is_empty()).then_some(reason.as_str());
    conn.execute("UPDATE book SET deleted_at = ?1, deleted_by = ?2, delete_reason = ?3 WHERE id = ?4", params![now(), by, reason, id])?;
    event(conn, id, by, "deleted", reason)
}

// Correcting an entry: entries never change, so the old one is marked with
// this and a new one takes its place, on the same moment unless the date was
// changed. To whoever uses it, it is simply an edit.
pub const CORRECTED: &str = "Corregido";

pub fn edit(conn: &Connection, me: &SessionUser, s: &Settings, id: i64, e: EntryIn) -> Result<Entry> {
    let old = get(conn, id)?;
    if !may_change(me, &old, s.book_staff_delete) {
        return fail("E_FORBIDDEN");
    }
    if old.deleted_at.is_some() {
        return fail("E_NOT_FOUND");
    }
    // what it is stays what it is; a sale typed before 1.8 is not touched
    let kind = match (old.kind.as_str(), old.cost_cents) {
        ("expense", _) => "expense",
        (_, Some(_)) => "recharge",
        _ => return fail("E_INVALID:kind"),
    };
    let same_day = e.date.trim().is_empty() || e.date.trim() == &old.at[..10];
    mark(conn, id, &me.name, CORRECTED)?;
    let (new, _) = add(conn, me, s, EntryIn { kind: kind.into(), client_id: None, keep_at: same_day.then(|| old.at.clone()), ..e })?;
    event(conn, new.id, &me.name, "added", Some(&format!("corrige {id}")))?;
    Ok(new)
}

pub fn delete(conn: &Connection, me: &SessionUser, staff: bool, id: i64, reason: &str) -> Result<Entry> {
    let e = get(conn, id)?;
    if !may_change(me, &e, staff) {
        return fail("E_FORBIDDEN");
    }
    if e.deleted_at.is_some() {
        return Ok(e);
    }
    mark(conn, id, &me.name, reason)?;
    get(conn, id)
}

// what was typed on a day, deleted the same way as one by one
pub fn delete_day(conn: &Connection, by: &str, date: &str, training: bool, reason: &str, what: &str) -> Result<i64> {
    let only = match what {
        "sales" => "AND kind = 'sale' AND cost_cents IS NULL",
        "recharges" => "AND cost_cents IS NOT NULL",
        "expenses" => "AND kind = 'expense'",
        _ => "",
    };
    let mut stmt = conn.prepare(&format!("SELECT id FROM book WHERE at >= ?1 AND at <= ?1 || 'T99' AND training = ?2 AND deleted_at IS NULL {only}"))?;
    let ids = stmt.query_map(params![date, training], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for id in &ids {
        mark(conn, *id, by, reason)?;
    }
    Ok(ids.len() as i64)
}

pub fn restore(conn: &Connection, me: &SessionUser, staff: bool, id: i64) -> Result<Entry> {
    let e = get(conn, id)?;
    if !may_change(me, &e, staff) {
        return fail("E_FORBIDDEN");
    }
    if e.deleted_at.is_none() {
        return Ok(e);
    }
    // the entry that corrected it is the one in force now
    if e.delete_reason.as_deref() == Some(CORRECTED) {
        return fail("E_INVALID:kind");
    }
    conn.execute("UPDATE book SET deleted_at = NULL, deleted_by = NULL, delete_reason = NULL WHERE id = ?1", [id])?;
    event(conn, id, &me.name, "restored", None)?;
    get(conn, id)
}

// Everything between two dates, newest first; what was voided or deleted only
// when asked for.
pub fn rows(conn: &Connection, r: &Range, training: bool, deleted: bool) -> Result<Vec<Row>> {
    let (from, to) = r.bounds()?;
    let mut out = Vec::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM book WHERE at >= ?1 AND at <= ?2 AND training = ?3 AND (?4 OR deleted_at IS NULL)"
    ))?;
    for e in stmt.query_map(params![from, to, training, deleted], row)? {
        let e = e?;
        let (source, sign) = match (e.kind.as_str(), e.cost_cents) {
            ("expense", _) => ("expense", -1),
            (_, Some(_)) => ("recharge", 1),
            _ => ("sale", 1),
        };
        out.push(Row {
            source,
            id: e.id,
            at: e.at,
            concept: e.concept,
            cents: sign * e.cents,
            method: e.method,
            by_name: e.by_name,
            supplier: e.supplier,
            supplier_nif: e.supplier_nif,
            invoice_ref: e.invoice_ref,
            vat_bp: e.vat_bp,
            machine: e.machine,
            cost_cents: e.cost_cents.unwrap_or(0),
            number: None,
            refund: false,
            invoice: false,
            printed: None,
            gone: e.deleted_at.is_some(),
            deleted_by: e.deleted_by,
            deleted_at: e.deleted_at,
            delete_reason: e.delete_reason,
        });
    }
    let mut stmt = conn.prepare(
        "SELECT t.id, t.issued_at, t.number, t.kind, t.total_cents, t.payment_method, t.user_name, t.voided_at, t.voided_by, t.void_reason,
                COALESCE((SELECT name FROM ticket_lines WHERE ticket_id = t.id ORDER BY pos LIMIT 1), ''),
                (SELECT COUNT(*) FROM ticket_lines WHERE ticket_id = t.id),
                t.customer_name IS NOT NULL,
                CASE WHEN EXISTS(SELECT 1 FROM ticket_prints p WHERE p.ticket_id = t.id AND p.status IN ('printed', 'sent')) THEN 'printed'
                     WHEN EXISTS(SELECT 1 FROM ticket_prints p WHERE p.ticket_id = t.id AND p.status = 'unknown') THEN 'unknown'
                     WHEN EXISTS(SELECT 1 FROM ticket_prints p WHERE p.ticket_id = t.id) THEN 'failed'
                     ELSE 'none' END,
                t.machine_id, t.machine_cost_cents
         FROM tickets t WHERE t.issued_at >= ?1 AND t.issued_at <= ?2 AND t.training = ?3 AND (?4 OR t.voided_at IS NULL)",
    )?;
    let tickets = stmt.query_map(params![from, to, training, deleted], |r| {
        let first: String = r.get(10)?;
        let lines: i64 = r.get(11)?;
        let refund = r.get::<_, String>(3)? == "refund";
        let voided_at: Option<String> = r.get(7)?;
        let mut concept = if lines > 1 { format!("{first} y {} más", lines - 1) } else { first };
        if refund {
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
            supplier: String::new(),
            supplier_nif: String::new(),
            invoice_ref: String::new(),
            vat_bp: 0,
            machine: r.get(14)?,
            cost_cents: r.get(15)?,
            number: Some(r.get(2)?),
            refund,
            invoice: r.get(12)?,
            printed: Some(r.get(13)?),
            gone: voided_at.is_some(),
            deleted_by: r.get(8)?,
            deleted_at: voided_at,
            delete_reason: r.get(9)?,
        })
    })?;
    for t in tickets {
        out.push(t?);
    }
    out.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
    Ok(out)
}

fn count(machines: &mut BTreeMap<i64, MachineSum>, id: i64, sales: i64, recharges: i64, n: i64) {
    let m = machines.entry(id).or_insert_with(|| MachineSum { id, ..MachineSum::default() });
    m.sales_cents += sales;
    m.recharge_cents += recharges;
    m.count += n;
}

pub fn summary(rows: &[Row]) -> Summary {
    let mut s = Summary::default();
    let mut machines = BTreeMap::new();
    for r in rows.iter().filter(|r| !r.gone) {
        match r.source {
            "recharge" => {
                s.recharge_cents += r.cents;
                s.recharge_count += 1;
                s.recharge_cost_cents += r.cost_cents;
            }
            "expense" => {
                s.purchase_cents -= r.cents;
                s.purchase_count += 1;
            }
            _ => {
                match r.method.as_str() {
                    "cash" => s.cash_cents += r.cents,
                    "card" => s.card_cents += r.cents,
                    "bizum" => s.bizum_cents += r.cents,
                    _ => s.other_cents += r.cents,
                }
                s.machine_cost_cents += r.cost_cents;
                if r.refund {
                    s.refund_count += 1;
                } else {
                    s.sale_count += 1;
                }
            }
        }
        if r.method == "cash" {
            s.drawer_cents += r.cents;
        }
        if r.source != "expense" && (r.machine.is_some() || r.method == "card") {
            let (sales, recharges) = if r.source == "recharge" { (0, r.cents) } else { (r.cents, 0) };
            count(&mut machines, r.machine.unwrap_or(0), sales, recharges, 1);
        }
    }
    s.total_cents = s.cash_cents + s.card_cents + s.bizum_cents + s.other_cents + s.recharge_cents;
    s.expense_cents = s.purchase_cents + s.recharge_cost_cents + s.machine_cost_cents;
    s.net_cents = s.total_cents - s.expense_cents;
    s.machines = machines.into_values().collect();
    s
}

// what went through each machine between two moments, for the closing report
pub fn machines_between(conn: &Connection, from: &str, to: &str) -> Result<Vec<MachineSum>> {
    let mut machines = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT COALESCE(machine_id, 0), SUM(total_cents), COUNT(*) FROM tickets
         WHERE payment_method = 'card' AND voided_at IS NULL AND training = 0 AND issued_at >= ?1 AND issued_at <= ?2 GROUP BY 1",
    )?;
    for r in stmt.query_map(params![from, to], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
        let (id, cents, n): (i64, i64, i64) = r?;
        count(&mut machines, id, cents, 0, n);
    }
    let mut stmt = conn.prepare(
        "SELECT COALESCE(machine_id, 0), SUM(cents), COUNT(*) FROM book
         WHERE cost_cents IS NOT NULL AND (machine_id IS NOT NULL OR method = 'card')
           AND deleted_at IS NULL AND training = 0 AND at >= ?1 AND at <= ?2 GROUP BY 1",
    )?;
    for r in stmt.query_map(params![from, to], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
        let (id, cents, n): (i64, i64, i64) = r?;
        count(&mut machines, id, 0, cents, n);
    }
    Ok(machines.into_values().collect())
}

// what the shop buys most, for one-tap buttons: the last 90 days' expenses
pub fn quick(conn: &Connection, training: bool) -> Result<Vec<Quick>> {
    let since = (chrono::Local::now() - chrono::Duration::days(90)).format("%Y-%m-%d").to_string();
    let mut stmt = conn.prepare(
        "SELECT concept, cents, n FROM (
           SELECT concept, cents, COUNT(*) OVER (PARTITION BY lower(concept)) AS n,
                  ROW_NUMBER() OVER (PARTITION BY lower(concept) ORDER BY id DESC) AS k
           FROM book WHERE kind = 'expense' AND deleted_at IS NULL AND training = ?1 AND at >= ?2 AND concept <> 'Gasto')
         WHERE k = 1 ORDER BY n DESC, concept LIMIT 8",
    )?;
    let rows = stmt
        .query_map(params![training, since], |r| Ok(Quick { concept: r.get(0)?, cents: r.get(1)?, uses: r.get(2)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// what typed entries did to the cash drawer since it was opened: the money of
// recharges came in, expenses went out. What a recharge costs is paid to the
// operator from elsewhere, so the drawer keeps the cash.
pub fn cash_between(conn: &Connection, from: &str, to: &str) -> Result<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(CASE kind WHEN 'sale' THEN cents ELSE 0 END), 0), COALESCE(SUM(CASE kind WHEN 'expense' THEN cents ELSE 0 END), 0)
         FROM book WHERE method = 'cash' AND deleted_at IS NULL AND training = 0 AND at >= ?1 AND at <= ?2",
        params![from, to],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

// The period for the gestor: income and expenses with VAT split out, the
// tickets as one line each, recharges with what they cost. Deleted entries
// and voided tickets are not in the books, so not here either.
pub fn export(conn: &Connection, s: &Settings, r: &Range) -> Result<PathBuf> {
    let (from, to) = r.bounds()?;
    let machine = |id: Option<i64>| id.map(|m| cell(s.machine_name(m))).unwrap_or_default();
    let mut out = String::from(
        "\u{feff}Fecha;Origen;Tipo;Concepto;Pago;Máquina;Base;IVA %;Cuota IVA;Total;Coste;Proveedor;NIF proveedor;Nº factura;Usuario\r\n",
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
            match (e.kind.as_str(), e.cost_cents) {
                ("expense", _) => "Gasto".to_string(),
                (_, Some(_)) => "Recarga".to_string(),
                _ => "Venta".to_string(),
            },
            cell(&e.concept),
            method_label(&e.method).to_string(),
            machine(e.machine),
            plain(sign * base),
            plain(e.vat_bp),
            plain(sign * (e.cents - base)),
            plain(sign * e.cents),
            e.cost_cents.map(plain).unwrap_or_default(),
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
                (SELECT CASE WHEN COUNT(*) = 1 THEN MAX(vat_bp) END FROM ticket_vat WHERE ticket_id = t.id),
                t.machine_id, t.machine_cost_cents
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
            r.get::<_, Option<i64>>(9)?,
            r.get::<_, i64>(10)?,
        ))
    })?;
    for t in tickets {
        let (at, number, kind, total, method, user, base, vat, rate, on, cost) = t?;
        let fields = [
            datetime(&at),
            format!("Ticket {number}"),
            if kind == "refund" { "Devolución".into() } else { "Venta".into() },
            String::new(),
            method_label(&method).to_string(),
            machine(on),
            plain(base),
            rate.map(plain).unwrap_or_else(|| "varios".into()),
            plain(vat),
            plain(total),
            if cost != 0 { plain(cost) } else { String::new() },
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
    let path = dir.join(format!("ventas_{}_{}.csv", r.from, r.to));
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
    use crate::settings::Machine;

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

    fn rows_today(c: &Connection) -> Vec<Row> {
        rows(c, &today(), false, false).unwrap()
    }

    // a card terminal that also does recharges, and a machine that only does those
    fn two_machines() -> Settings {
        Settings {
            machines: vec![
                Machine { id: 1, name: "BBVA".into(), commission_bp: 300, ..Machine::default() },
                Machine { id: 2, name: "Recargas".into(), card: false, commission_bp: 500, ..Machine::default() },
            ],
            ..Settings::default()
        }
    }

    #[test]
    fn a_recharge_is_an_expense_on_a_machine_and_not_in_cash() {
        let c = conn();
        let s = two_machines();
        let ana = me("Ana", "cashier");
        // in cash it comes in and that is all
        let (r, _) = add(&c, &ana, &s, EntryIn { supplier: "Lycamobile".into(), ..entry("recharge", "", 1000, "cash") }).unwrap();
        assert_eq!((r.concept.as_str(), r.vat_bp, r.cost_cents, r.machine), ("Recarga Lycamobile", 0, Some(0), None));
        // on a machine that takes cards it is paid by card, and spent but for the commission
        let (r, _) = add(&c, &ana, &s, EntryIn { machine: Some(1), ..entry("recharge", "", 1000, "") }).unwrap();
        assert_eq!((r.method.as_str(), r.cost_cents, r.machine), ("card", Some(970), Some(1)));
        // on one that only does recharges the customer paid at the counter
        let (r, _) = add(&c, &ana, &s, EntryIn { machine: Some(2), ..entry("recharge", "", 2000, "card") }).unwrap();
        assert_eq!((r.method.as_str(), r.cost_cents, r.machine), ("cash", Some(1900), Some(2)));
        add(&c, &ana, &s, entry("expense", "  pantalla   iphone 11 ", 4500, "cash")).unwrap();
        let list = rows_today(&c);
        let sum = summary(&list);
        assert_eq!((sum.recharge_cents, sum.recharge_count, sum.recharge_cost_cents), (4000, 3, 2870));
        assert_eq!((sum.purchase_cents, sum.expense_cents), (4500, 7370));
        assert_eq!((sum.total_cents, sum.net_cents), (4000, -3370));
        // the money of a recharge in hand stays in the drawer; a purchase comes out of it
        assert_eq!(sum.drawer_cents, 1000 + 2000 - 4500);
        let (from, to) = (today().from, format!("{}T99", today().to));
        assert_eq!(cash_between(&c, &from, &to).unwrap(), (3000, 4500));
        // each machine apart, the same on the screen and on the closing report
        let machines = vec![
            MachineSum { id: 1, sales_cents: 0, recharge_cents: 1000, count: 1 },
            MachineSum { id: 2, sales_cents: 0, recharge_cents: 2000, count: 1 },
        ];
        assert_eq!(sum.machines, machines);
        assert_eq!(machines_between(&c, &from, &to).unwrap(), machines);
        assert!(list.iter().any(|r| r.concept == "pantalla iphone 11" && r.source == "expense"));
        // no other kind of entry is typed: a sale is a ticket
        assert_eq!(add(&c, &ana, &s, entry("sale", "x", 100, "cash")).unwrap_err().0, "E_INVALID:kind");
    }

    #[test]
    fn what_counts_as_an_expense_is_up_to_the_shop() {
        let c = conn();
        let mut s = two_machines();
        let ana = me("Ana", "cashier");
        // this machine's recharges are not an expense, recharges in cash are
        s.machines[0].recharge_expense = false;
        s.recharge_cash_expense = true;
        s.recharge_commission_bp = 200;
        let cost = |s: &Settings, machine, cents| add(&c, &ana, s, EntryIn { machine, ..entry("recharge", "", cents, "cash") }).unwrap().0.cost_cents;
        assert_eq!(cost(&s, Some(1), 1000), Some(0));
        assert_eq!(cost(&s, None, 1000), Some(980));
        // paid in cash on a machine that also takes cards: the drawer has it
        s.machines[0].recharge_cash = true;
        let (r, _) = add(&c, &ana, &s, EntryIn { machine: Some(1), ..entry("recharge", "", 500, "") }).unwrap();
        assert_eq!(r.method, "cash");
        // a machine that does no recharges makes none, and an unknown one neither
        s.machines[0].recharge = false;
        let (r, _) = add(&c, &ana, &s, EntryIn { machine: Some(1), ..entry("recharge", "", 500, "cash") }).unwrap();
        assert_eq!(r.machine, None);
        let (r, _) = add(&c, &ana, &s, EntryIn { machine: Some(9), ..entry("recharge", "", 500, "cash") }).unwrap();
        assert_eq!(r.machine, None);
        // a change in Ajustes is for what comes next: what was saved stays as it was
        let before = summary(&rows_today(&c)).recharge_cost_cents;
        s.recharge_cash_expense = false;
        assert_eq!(cost(&s, None, 1000), Some(0));
        assert_eq!(summary(&rows_today(&c)).recharge_cost_cents, before);
        // only a recharge is made on a machine
        let (e, _) = add(&c, &ana, &s, EntryIn { machine: Some(2), ..entry("expense", "Fundas", 900, "card") }).unwrap();
        assert_eq!((e.machine, e.method.as_str()), (None, "card"));
        // a machine that was taken away keeps its name on what it did
        let mut s = two_machines();
        s.machines[1].removed = true;
        let (r, _) = add(&c, &ana, &s, EntryIn { machine: Some(2), ..entry("recharge", "", 500, "cash") }).unwrap();
        assert_eq!(s.machine_name(r.machine.unwrap()), "Recargas");
        // it never changes behind the owner's back
        assert!(c.execute("UPDATE book SET machine_id = 1 WHERE id = ?1", [r.id]).is_err());
    }

    #[test]
    fn deleting_leaves_a_trace_and_an_undo() {
        let c = conn();
        let s = Settings::default();
        let ana = me("Ana", "cashier");
        let (a, _) = add(&c, &ana, &s, entry("recharge", "", 1000, "cash")).unwrap();
        add(&c, &ana, &s, entry("expense", "Fundas", 1250, "card")).unwrap();
        // deleted: out of every list and total, but kept with who did it
        delete(&c, &ana, true, a.id, "").unwrap();
        assert_eq!(summary(&rows_today(&c)).recharge_cents, 0);
        let kept = rows(&c, &today(), false, true).unwrap();
        let gone = kept.iter().find(|r| r.id == a.id && r.source == "recharge").unwrap();
        assert!(gone.gone && gone.deleted_by.as_deref() == Some("Ana"));
        assert!(c.execute("DELETE FROM book WHERE id = ?1", [a.id]).is_err());
        assert!(c.execute("UPDATE book SET cents = 1 WHERE id = ?1", [a.id]).is_err());
        assert!(c.execute("UPDATE book SET cost_cents = 1 WHERE id = ?1", [a.id]).is_err());
        // undone: back in the totals, and the history says both
        restore(&c, &ana, true, a.id).unwrap();
        assert_eq!(summary(&rows_today(&c)).recharge_cents, 1000);
        let n: i64 = c.query_row("SELECT COUNT(*) FROM book_events WHERE entry_id = ?1", [a.id], |r| r.get(0)).unwrap();
        assert_eq!(n, 3);
        // someone else's entry is the owner's to delete
        assert_eq!(delete(&c, &me("Eva", "cashier"), true, a.id, "").unwrap_err().0, "E_FORBIDDEN");
        assert!(delete(&c, &me("Jefe", "admin"), false, a.id, "error").is_ok());
        assert_eq!(delete(&c, &ana, false, a.id, "").unwrap_err().0, "E_FORBIDDEN");
    }

    #[test]
    fn an_edit_replaces_the_entry_and_keeps_the_old_one_on_record() {
        let c = conn();
        let s = Settings::default();
        let ana = me("Ana", "cashier");
        let (a, _) = add(&c, &ana, &s, entry("expense", "Fundas", 1000, "cash")).unwrap();
        let (r, _) = add(&c, &ana, &s, entry("recharge", "", 1000, "cash")).unwrap();
        // the wrong way of paying and the wrong amount, put right
        let b = edit(&c, &ana, &s, a.id, entry("expense", "Fundas iPhone", 1500, "card")).unwrap();
        assert_eq!((b.concept.as_str(), b.cents, b.method.as_str(), b.at.as_str()), ("Fundas iPhone", 1500, "card", a.at.as_str()));
        let e = edit(&c, &ana, &s, r.id, EntryIn { supplier: "Lebara".into(), ..entry("recharge", "", 2000, "card") }).unwrap();
        assert_eq!((e.cost_cents, e.supplier.as_str(), e.concept.as_str()), (Some(0), "Lebara", "Recarga Lebara"));
        let sum = summary(&rows_today(&c));
        assert_eq!((sum.purchase_cents, sum.recharge_cents, sum.drawer_cents), (1500, 2000, 0));
        // moved to a machine it becomes an expense, and back to cash it stops being one
        let e = edit(&c, &ana, &s, e.id, EntryIn { machine: Some(1), ..entry("recharge", "", 2000, "") }).unwrap();
        assert_eq!((e.cost_cents, e.machine, e.method.as_str()), (Some(2000), Some(1), "card"));
        let e = edit(&c, &ana, &s, e.id, entry("recharge", "", 2000, "cash")).unwrap();
        assert_eq!((e.cost_cents, e.machine, e.method.as_str()), (Some(0), None, "cash"));
        // the old one is still there, marked, and cannot come back over the new one
        let kept = rows(&c, &today(), false, true).unwrap();
        assert!(kept.iter().any(|x| x.id == a.id && x.gone && x.delete_reason.as_deref() == Some(CORRECTED)));
        assert_eq!(restore(&c, &ana, true, a.id).unwrap_err().0, "E_INVALID:kind");
        assert_eq!(edit(&c, &ana, &s, a.id, entry("expense", "x", 100, "cash")).unwrap_err().0, "E_NOT_FOUND");
        // someone else's is the owner's to correct, and a new date moves it
        assert_eq!(edit(&c, &me("Eva", "cashier"), &s, b.id, entry("expense", "x", 100, "cash")).unwrap_err().0, "E_FORBIDDEN");
        let moved = edit(&c, &me("Jefe", "admin"), &s, b.id, EntryIn { date: "2026-01-02".into(), ..entry("expense", "Fundas", 1500, "card") }).unwrap();
        assert_eq!(moved.at, "2026-01-02T12:00:00");
    }

    #[test]
    fn sales_typed_before_every_sale_was_a_ticket_still_count() {
        let c = conn();
        c.execute(
            "INSERT INTO book (kind, at, concept, cents, method, vat_bp, category, supplier, supplier_nif, invoice_ref, by_name)
             VALUES ('sale', ?1, 'funda', 900, 'bizum', 2100, '', '', '', '', 'Ana')",
            [now()],
        )
        .unwrap();
        let list = rows_today(&c);
        assert_eq!(list[0].source, "sale");
        let sum = summary(&list);
        assert_eq!((sum.bizum_cents, sum.sale_count, sum.total_cents, sum.net_cents), (900, 1, 900, 900));
    }

    #[test]
    fn a_second_enter_bad_amounts_and_future_dates_are_refused() {
        let c = conn();
        let s = Settings::default();
        let ana = me("Ana", "cashier");
        let once = EntryIn { client_id: Some("x1".into()), ..entry("expense", "Fundas", 900, "cash") };
        let (a, dup) = add(&c, &ana, &s, once).unwrap();
        assert!(!dup);
        let (b, dup) = add(&c, &ana, &s, EntryIn { client_id: Some("x1".into()), ..entry("expense", "Fundas", 900, "cash") }).unwrap();
        assert!(dup && a.id == b.id);
        assert_eq!(add(&c, &ana, &s, entry("expense", "x", 0, "cash")).unwrap_err().0, "E_INVALID:amount");
        assert_eq!(add(&c, &ana, &s, entry("expense", "x", 100, "crypto")).unwrap_err().0, "E_INVALID:method");
        let future = EntryIn { date: "2999-01-01".into(), ..entry("expense", "x", 100, "cash") };
        assert_eq!(add(&c, &ana, &s, future).unwrap_err().0, "E_INVALID:date");
        let past = EntryIn { date: "2026-01-02".into(), ..entry("expense", "Pantallas", 50000, "transfer") };
        let (p, _) = add(&c, &ana, &s, past).unwrap();
        assert_eq!((p.at.as_str(), p.category.as_str()), ("2026-01-02T12:00:00", "Compras"));
        // what is bought most comes back as one-tap buttons, at its last price
        add(&c, &ana, &s, entry("expense", "fundas", 950, "cash")).unwrap();
        let q = quick(&c, false).unwrap();
        assert_eq!((q[0].concept.as_str(), q[0].cents, q[0].uses), ("fundas", 950, 2));
    }
}
