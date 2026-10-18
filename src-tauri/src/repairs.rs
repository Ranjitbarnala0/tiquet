// Repair orders. A phone left at the shop gets a number (REP2026-000001), a
// resguardo for the customer and a slip for the phone, and a status kept with
// who changed it and when. Money only ever moves as tickets: the deposit is a
// ticket issued in the same transaction as the repair, and the phone goes
// back with a sale that has the repair on one of its lines.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{fail, Result};
use crate::settings::{self, Settings};
use crate::state::SessionUser;
use crate::tickets::{self, PaymentIn, SaleLine, SaleOpts, ShopInfo, TicketRecord, TicketRef, MAX_PRICE};

// still in the shop
pub const OPEN: [&str; 4] = ["received", "in_progress", "ready", "not_repaired"];
const ALL: [&str; 6] = ["received", "in_progress", "ready", "not_repaired", "delivered", "cancelled"];
const OPEN_SQL: &str = "status IN ('received', 'in_progress', 'ready', 'not_repaired')";

// what the cashier types when the phone is left
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct RepairIn {
    pub customer_name: String,
    pub customer_phone: String,
    pub brand: String,
    pub model: String,
    pub colour: String,
    // IMEI or serial number
    pub imei: String,
    // as the customer described it
    pub fault: String,
    // for the shop only: never printed
    pub notes: String,
    // None until there is a price
    pub estimate_cents: Option<i64>,
}

fn one_line(s: &str, max: usize) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

// free text keeps its line breaks, not its runs of blank space
fn text(s: &str, max: usize) -> String {
    s.lines().map(|l| one_line(l, max)).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n").chars().take(max).collect()
}

impl RepairIn {
    fn cleaned(&self) -> RepairIn {
        RepairIn {
            customer_name: one_line(&self.customer_name, 120),
            customer_phone: one_line(&self.customer_phone, 30),
            brand: one_line(&self.brand, 40),
            model: one_line(&self.model, 60),
            colour: one_line(&self.colour, 30),
            imei: one_line(&self.imei, 40),
            fault: text(&self.fault, 500),
            notes: text(&self.notes, 1000),
            estimate_cents: self.estimate_cents,
        }
    }

    fn checked(&self) -> Result<RepairIn> {
        let r = self.cleaned();
        if r.customer_name.is_empty() {
            return fail("E_INVALID:customerName");
        }
        if r.customer_phone.is_empty() {
            return fail("E_INVALID:customerPhone");
        }
        if r.brand.is_empty() && r.model.is_empty() {
            return fail("E_INVALID:device");
        }
        if r.fault.is_empty() {
            return fail("E_INVALID:fault");
        }
        if r.estimate_cents.is_some_and(|e| !(0..=MAX_PRICE).contains(&e)) {
            return fail("E_INVALID:estimate");
        }
        Ok(r)
    }

    pub fn device(&self) -> String {
        [&self.brand, &self.model].iter().map(|x| x.trim()).filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ")
    }
}

// the name of the line that delivers it, on the ticket and in Venta
fn line_name(number: &str, device: &str) -> String {
    format!("Reparación {number}: {device}").chars().take(120).collect()
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub at: String,
    pub by_name: String,
    // created | status | edited | delivered | reopened | cancelled
    pub kind: String,
    // the status it was left in
    pub status: String,
    pub detail: Option<Value>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Repair {
    pub id: i64,
    pub number: String,
    pub created_at: String,
    pub created_by: String,
    #[serde(flatten)]
    pub data: RepairIn,
    pub status: String,
    // the deposit ticket, and what of it the shop still holds: nothing once
    // it is voided, less whatever a return gave back
    pub deposit: Option<TicketRef>,
    pub deposit_cents: i64,
    // what is left to pay when it is picked up
    pub due_cents: Option<i64>,
    // the sale that handed it back
    pub ticket: Option<TicketRef>,
    pub closed_at: Option<String>,
    pub closed_by: Option<String>,
    pub cancel_reason: Option<String>,
    pub line_name: String,
    pub events: Vec<Event>,
    // as printed on its resguardo
    pub shop: ShopInfo,
    pub terms: String,
}

impl Repair {
    pub fn open(&self) -> bool {
        OPEN.contains(&self.status.as_str())
    }
}

// one series per year, REP2026-000001..., with the same floor as tickets: a
// restored backup never hands out a number already on a resguardo
pub fn next_number(conn: &Connection, at: &str) -> Result<(String, i64, String)> {
    let series = format!("REP{}", &at[..4]);
    let seq: i64 = conn.query_row(
        "SELECT MAX(COALESCE((SELECT MAX(seq) FROM repairs WHERE series = ?1), 0),
                    COALESCE((SELECT seq FROM series_floor WHERE series = ?1), 0)) + 1",
        [&series],
        |r| r.get(0),
    )?;
    let number = format!("{series}-{seq:06}");
    Ok((series, seq, number))
}

fn held(conn: &Connection, ticket: Option<i64>) -> Result<i64> {
    let Some(id) = ticket else { return Ok(0) };
    Ok(conn
        .query_row(
            "SELECT CASE WHEN t.voided_at IS NULL
                         THEN t.total_cents + COALESCE((SELECT SUM(r.total_cents) FROM tickets r WHERE r.refund_of = t.id AND r.voided_at IS NULL), 0)
                         ELSE 0 END
             FROM tickets t WHERE t.id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub fn load(conn: &Connection, id: i64) -> Result<Repair> {
    let row = conn
        .query_row(
            "SELECT id, number, created_at, created_by, customer_name, customer_phone, brand, model, colour, imei, fault, notes,
                    estimate_cents, status, closed_at, closed_by, cancel_reason, shop, terms, deposit_ticket, ticket_id
             FROM repairs WHERE id = ?1",
            [id],
            |r| {
                let data = RepairIn {
                    customer_name: r.get(4)?,
                    customer_phone: r.get(5)?,
                    brand: r.get(6)?,
                    model: r.get(7)?,
                    colour: r.get(8)?,
                    imei: r.get(9)?,
                    fault: r.get(10)?,
                    notes: r.get(11)?,
                    estimate_cents: r.get(12)?,
                };
                let number: String = r.get(1)?;
                Ok((
                    Repair {
                        id: r.get(0)?,
                        line_name: line_name(&number, &data.device()),
                        number,
                        created_at: r.get(2)?,
                        created_by: r.get(3)?,
                        data,
                        status: r.get(13)?,
                        deposit: None,
                        deposit_cents: 0,
                        due_cents: None,
                        ticket: None,
                        closed_at: r.get(14)?,
                        closed_by: r.get(15)?,
                        cancel_reason: r.get(16)?,
                        events: Vec::new(),
                        shop: serde_json::from_str(&r.get::<_, String>(17)?).unwrap_or_default(),
                        terms: r.get(18)?,
                    },
                    r.get::<_, Option<i64>>(19)?,
                    r.get::<_, Option<i64>>(20)?,
                ))
            },
        )
        .optional()?;
    let Some((mut rep, deposit, ticket)) = row else {
        return fail("E_NOT_FOUND");
    };
    rep.deposit = deposit.map(|t| tickets::reference(conn, t)).transpose()?.flatten();
    rep.deposit_cents = held(conn, deposit)?;
    rep.due_cents = rep.data.estimate_cents.map(|e| e - rep.deposit_cents);
    rep.ticket = ticket.map(|t| tickets::reference(conn, t)).transpose()?.flatten();
    let mut stmt = conn.prepare("SELECT at, by_name, kind, status, detail FROM repair_events WHERE repair_id = ?1 ORDER BY id")?;
    rep.events = stmt
        .query_map([id], |r| {
            Ok(Event {
                at: r.get(0)?,
                by_name: r.get(1)?,
                kind: r.get(2)?,
                status: r.get(3)?,
                detail: r.get::<_, Option<String>>(4)?.and_then(|d| serde_json::from_str(&d).ok()),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rep)
}

fn event(conn: &Connection, id: i64, by: &str, kind: &str, status: &str, detail: Option<Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO repair_events (repair_id, at, by_name, kind, status, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, tickets::now(), by, kind, status, detail.map(|d| d.to_string())],
    )?;
    Ok(())
}

#[derive(Deserialize, Default, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct NewRepair {
    #[serde(flatten)]
    pub repair: RepairIn,
    pub deposit_cents: i64,
    // how the deposit is paid; cash, exact, when left out
    pub payment: Option<PaymentIn>,
    // made by the form when it opens: a retry returns the repair already taken in
    pub request_id: Option<String>,
    // the cashier confirmed the PC clock even though it is behind the last ticket
    pub clock_ok: bool,
}

#[derive(Debug)]
pub struct Created {
    pub repair: Repair,
    pub deposit: Option<TicketRecord>,
    pub duplicate: bool,
}

// The repair and its deposit ticket are committed together or not at all.
pub fn create(conn: &mut Connection, user: &SessionUser, s: &Settings, n: &NewRepair) -> Result<Created> {
    let request = n.request_id.as_deref().map(str::trim).filter(|x| !x.is_empty());
    if request.is_some_and(|x| x.len() > 64 || !x.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) {
        return fail("E_INVALID:requestId");
    }
    let data = n.repair.checked()?;
    if !(0..=MAX_PRICE).contains(&n.deposit_cents) {
        return fail("E_INVALID:deposit");
    }
    if data.estimate_cents.is_some_and(|e| n.deposit_cents > e) {
        return fail("E_DEPOSIT_HIGH");
    }
    // practice tickets never take real money
    if n.deposit_cents > 0 && s.training {
        return fail("E_REPAIR_TRAINING");
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let done: Option<i64> =
        request.map(|x| tx.query_row("SELECT id FROM repairs WHERE client_id = ?1", [x], |r| r.get(0)).optional()).transpose()?.flatten();
    if let Some(id) = done {
        drop(tx);
        return Ok(Created { repair: load(conn, id)?, deposit: None, duplicate: true });
    }
    let at = tickets::now();
    let (series, seq, number) = next_number(&tx, &at)?;
    let shop = serde_json::to_string(&ShopInfo { logo: settings::logo_id(&tx)?, ..ShopInfo::from_settings(s) })?;
    tx.execute(
        "INSERT INTO repairs (number, series, seq, created_at, created_by, client_id, customer_name, customer_phone, brand, model,
                              colour, imei, fault, notes, estimate_cents, status, shop, terms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 'received', ?16, ?17)",
        params![
            number,
            series,
            seq,
            at,
            user.name,
            request,
            data.customer_name,
            data.customer_phone,
            data.brand,
            data.model,
            data.colour,
            data.imei,
            data.fault,
            data.notes,
            data.estimate_cents,
            shop,
            s.repair_terms
        ],
    )?;
    let id = tx.last_insert_rowid();
    event(&tx, id, &user.name, "created", "received", None)?;
    let mut deposit = None;
    if n.deposit_cents > 0 {
        let line = SaleLine {
            name: format!("Anticipo reparación {number}"),
            qty: 1,
            unit_cents: n.deposit_cents,
            vat_bp: s.default_vat_bp,
            repair_id: Some(id),
            ..SaleLine::default()
        };
        let pay = n.payment.clone().unwrap_or(PaymentIn { method: "cash".into(), tendered_cents: None });
        let sale = tickets::check(s, vec![line], &pay, &SaleOpts::default())?;
        let ticket = tickets::write_sale(&tx, user, s, sale, request, n.clock_ok)?;
        tx.execute("UPDATE repairs SET deposit_ticket = ?1 WHERE id = ?2", params![ticket, id])?;
        deposit = Some(ticket);
    }
    tx.commit()?;
    let deposit = deposit.map(|t| tickets::load(conn, t)).transpose()?;
    Ok(Created { repair: load(conn, id)?, deposit, duplicate: false })
}

fn open_one(conn: &Connection, id: i64) -> Result<Repair> {
    let r = load(conn, id)?;
    if !r.open() {
        return fail("E_REPAIR_CLOSED");
    }
    Ok(r)
}

// Corrections while the phone is still in the shop. The history says what
// changed; the estimate also how.
pub fn update(conn: &mut Connection, id: i64, by: &str, input: &RepairIn) -> Result<Repair> {
    let data = input.checked()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let cur = open_one(&tx, id)?;
    let old = &cur.data;
    let fields: Vec<&str> = [
        ("customerName", old.customer_name != data.customer_name),
        ("customerPhone", old.customer_phone != data.customer_phone),
        ("brand", old.brand != data.brand),
        ("model", old.model != data.model),
        ("colour", old.colour != data.colour),
        ("imei", old.imei != data.imei),
        ("fault", old.fault != data.fault),
        ("notes", old.notes != data.notes),
        ("estimate", old.estimate_cents != data.estimate_cents),
    ]
    .into_iter()
    .filter_map(|(f, changed)| changed.then_some(f))
    .collect();
    if fields.is_empty() {
        return Ok(cur);
    }
    tx.execute(
        "UPDATE repairs SET customer_name = ?1, customer_phone = ?2, brand = ?3, model = ?4, colour = ?5, imei = ?6, fault = ?7,
                            notes = ?8, estimate_cents = ?9
         WHERE id = ?10",
        params![
            data.customer_name,
            data.customer_phone,
            data.brand,
            data.model,
            data.colour,
            data.imei,
            data.fault,
            data.notes,
            data.estimate_cents,
            id
        ],
    )?;
    let detail = json!({ "fields": fields, "estimateFrom": old.estimate_cents, "estimateTo": data.estimate_cents });
    event(&tx, id, by, "edited", &cur.status, Some(detail))?;
    tx.commit()?;
    load(conn, id)
}

pub fn set_status(conn: &mut Connection, id: i64, by: &str, status: &str) -> Result<Repair> {
    // delivered and cancelled have their own ways in
    if !OPEN.contains(&status) {
        return fail("E_INVALID:status");
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let cur = open_one(&tx, id)?;
    if cur.status == status {
        return Ok(cur);
    }
    tx.execute("UPDATE repairs SET status = ?1 WHERE id = ?2", params![status, id])?;
    event(&tx, id, by, "status", status, None)?;
    tx.commit()?;
    load(conn, id)
}

// handed back with nothing left to pay: the deposit covered it, or it was free
pub fn deliver(conn: &mut Connection, id: i64, by: &str) -> Result<Repair> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let cur = open_one(&tx, id)?;
    match cur.due_cents {
        None => return fail("E_REPAIR_PRICE"),
        Some(d) if d > 0 => return fail("E_REPAIR_DUE"),
        Some(d) if d < 0 => return fail("E_REPAIR_DEPOSIT_HIGH"),
        _ => {}
    }
    tx.execute(
        "UPDATE repairs SET status = 'delivered', closed_at = ?1, closed_by = ?2 WHERE id = ?3",
        params![tickets::now(), by, id],
    )?;
    event(&tx, id, by, "delivered", "delivered", None)?;
    tx.commit()?;
    load(conn, id)
}

// For a repair taken in by mistake. Money already taken goes back first, with
// a void or a return of the deposit ticket.
pub fn cancel(conn: &mut Connection, id: i64, by: &str, reason: &str) -> Result<Repair> {
    let reason: String = one_line(reason, 200);
    if reason.is_empty() {
        return fail("E_REASON");
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let cur = open_one(&tx, id)?;
    if cur.deposit_cents != 0 {
        return fail("E_REPAIR_DEPOSIT");
    }
    tx.execute(
        "UPDATE repairs SET status = 'cancelled', closed_at = ?1, closed_by = ?2, cancel_reason = ?3 WHERE id = ?4",
        params![tickets::now(), by, reason, id],
    )?;
    event(&tx, id, by, "cancelled", "cancelled", Some(json!({ "reason": reason })))?;
    tx.commit()?;
    load(conn, id)
}

// A line that delivers a repair carries the repair's own name, one unit, and
// each repair goes on a ticket once.
pub fn canon(conn: &Connection, lines: &mut [SaleLine]) -> Result<()> {
    let mut seen = Vec::new();
    for l in lines.iter_mut() {
        let Some(id) = l.repair_id else { continue };
        if seen.contains(&id) {
            return fail("E_REPAIR_TWICE");
        }
        seen.push(id);
        let row: Option<(String, String, String)> = conn
            .query_row("SELECT number, brand, model FROM repairs WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        let Some((number, brand, model)) = row else {
            return fail("E_REPAIR_GONE");
        };
        let data = RepairIn { brand, model, ..RepairIn::default() };
        l.name = line_name(&number, &data.device());
        l.qty = 1;
        l.product_id = None;
    }
    Ok(())
}

// The sale just written hands back every repair on its lines.
pub fn delivered(tx: &Connection, ticket: i64, by: &str) -> Result<()> {
    let number: String = tx.query_row("SELECT number FROM tickets WHERE id = ?1", [ticket], |r| r.get(0))?;
    let mut stmt = tx.prepare("SELECT DISTINCT repair_id FROM ticket_lines WHERE ticket_id = ?1 AND repair_id IS NOT NULL")?;
    let ids = stmt.query_map([ticket], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let at = tickets::now();
    for id in ids {
        let n = tx.execute(
            &format!("UPDATE repairs SET status = 'delivered', ticket_id = ?1, closed_at = ?2, closed_by = ?3 WHERE id = ?4 AND {OPEN_SQL}"),
            params![ticket, at, by, id],
        )?;
        if n == 0 {
            return fail("E_REPAIR_CLOSED");
        }
        event(tx, id, by, "delivered", "delivered", Some(json!({ "ticket": number })))?;
    }
    Ok(())
}

// A void of the sale that handed a repair back puts it where it was before.
pub fn undelivered(tx: &Connection, ticket: i64, by: &str) -> Result<()> {
    let mut stmt = tx.prepare("SELECT r.id, t.number FROM repairs r JOIN tickets t ON t.id = r.ticket_id WHERE r.ticket_id = ?1")?;
    let rows = stmt.query_map([ticket], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, number) in rows {
        let before: Option<String> = tx
            .query_row(
                &format!("SELECT status FROM repair_events WHERE repair_id = ?1 AND {OPEN_SQL} ORDER BY id DESC LIMIT 1"),
                [id],
                |r| r.get(0),
            )
            .optional()?;
        let status = before.unwrap_or_else(|| "ready".into());
        tx.execute(
            "UPDATE repairs SET status = ?1, ticket_id = NULL, closed_at = NULL, closed_by = NULL WHERE id = ?2",
            params![status, id],
        )?;
        event(tx, id, by, "reopened", &status, Some(json!({ "ticket": number })))?;
    }
    Ok(())
}

// What a repair being typed in would print as. Nothing is checked, so a
// half-filled form still has a preview; the number is the next free one.
pub fn draft(conn: &Connection, user: &SessionUser, s: &Settings, input: &RepairIn, deposit_cents: i64) -> Result<Repair> {
    let at = tickets::now();
    let (_, _, number) = next_number(conn, &at)?;
    Ok(sketch(&number, &at, &user.name, s, input, deposit_cents))
}

// a repair already taken in, as it would print with these corrections
pub fn revised(conn: &Connection, id: i64, input: &RepairIn) -> Result<Repair> {
    let mut r = load(conn, id)?;
    r.data = input.cleaned();
    r.due_cents = r.data.estimate_cents.map(|e| e - r.deposit_cents);
    r.line_name = line_name(&r.number, &r.data.device());
    Ok(r)
}

pub fn sketch(number: &str, at: &str, by: &str, s: &Settings, input: &RepairIn, deposit_cents: i64) -> Repair {
    let data = input.cleaned();
    let deposit_cents = deposit_cents.clamp(0, MAX_PRICE);
    Repair {
        id: 0,
        number: number.into(),
        line_name: line_name(number, &data.device()),
        created_at: at.into(),
        created_by: by.into(),
        due_cents: data.estimate_cents.map(|e| e - deposit_cents),
        data,
        status: "received".into(),
        deposit: None,
        deposit_cents,
        ticket: None,
        closed_at: None,
        closed_by: None,
        cancel_reason: None,
        events: Vec::new(),
        shop: ShopInfo::from_settings(s),
        terms: s.repair_terms.clone(),
    }
}

#[derive(Deserialize, Default, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct ListQuery {
    // open (still in the shop, the default), all, or one status
    pub status: Option<String>,
    pub q: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: i64,
    pub number: String,
    pub created_at: String,
    pub customer_name: String,
    pub customer_phone: String,
    pub device: String,
    pub status: String,
    pub estimate_cents: Option<i64>,
}

// lower case and without accents, so "garcia" finds García
fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

// newest first; the search takes every word typed, in the number, the
// customer's name or phone, the IMEI or the device
pub fn list(conn: &Connection, q: &ListQuery) -> Result<Vec<Summary>> {
    let status = q.status.as_deref().unwrap_or("open");
    let filter = match status {
        "open" => OPEN_SQL.to_string(),
        "all" => "1 = 1".to_string(),
        s if ALL.contains(&s) => format!("status = '{s}'"),
        _ => return fail("E_INVALID:status"),
    };
    let needle = fold(q.q.as_deref().unwrap_or_default());
    let words: Vec<&str> = needle.split_whitespace().collect();
    let limit = q.limit.unwrap_or(500).clamp(1, 2000) as usize;
    let mut stmt = conn.prepare(&format!(
        "SELECT id, number, created_at, customer_name, customer_phone, brand, model, colour, imei, status, estimate_cents
         FROM repairs WHERE {filter} ORDER BY id DESC"
    ))?;
    let rows = stmt.query_map([], |r| {
        Ok((
            Summary {
                id: r.get(0)?,
                number: r.get(1)?,
                created_at: r.get(2)?,
                customer_name: r.get(3)?,
                customer_phone: r.get(4)?,
                device: [r.get::<_, String>(5)?, r.get(6)?, r.get(7)?].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" "),
                status: r.get(9)?,
                estimate_cents: r.get(10)?,
            },
            r.get::<_, String>(8)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (s, imei) = row?;
        if !words.is_empty() {
            let hay = fold(&format!("{} {} {} {imei} {} {} {}", s.number, s.customer_name, s.customer_phone, s.device, digits(&s.customer_phone), digits(&imei)));
            if !words.iter().all(|w| hay.contains(w)) {
                continue;
            }
        }
        out.push(s);
        if out.len() == limit {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::tickets::{Customer, RefundIn, RefundLine};
    use std::collections::BTreeMap;

    fn setup() -> (Connection, SessionUser, Settings) {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let k = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("tiquet-repairs-{}-{n}-{k}", std::process::id()));
        let conn = db::open(&dir.join("t.db")).unwrap();
        conn.execute("INSERT INTO users (name, role, pass_hash) VALUES ('Ana', 'admin', 'x')", []).unwrap();
        let user = SessionUser { id: 1, name: "Ana".into(), role: "admin".into() };
        let s = Settings { shop_name: "Mi Tienda Móvil".into(), shop_nif: "B12345674".into(), ..Settings::default() };
        (conn, user, s)
    }

    fn phone() -> RepairIn {
        RepairIn {
            customer_name: "  Joan   García ".into(),
            customer_phone: "600 123 456".into(),
            brand: "Apple".into(),
            model: "iPhone 12".into(),
            colour: "Negro".into(),
            imei: "356789012345678".into(),
            fault: "No carga.\n\n  Pantalla rota  ".into(),
            notes: "Código 1234".into(),
            estimate_cents: Some(8000),
        }
    }

    fn take(conn: &mut Connection, user: &SessionUser, s: &Settings, deposit: i64) -> Created {
        create(conn, user, s, &NewRepair { repair: phone(), deposit_cents: deposit, ..NewRepair::default() }).unwrap()
    }

    fn cash() -> PaymentIn {
        PaymentIn { method: "cash".into(), tendered_cents: None }
    }

    // what Venta sends for "Entregar y cobrar"
    fn pickup(r: &Repair, cents: i64) -> SaleLine {
        SaleLine { name: "lo que sea".into(), qty: 3, unit_cents: cents, vat_bp: 2100, repair_id: Some(r.id), ..SaleLine::default() }
    }

    #[test]
    fn numbers_run_per_year_and_never_go_back() {
        let (mut conn, user, s) = setup();
        let year = tickets::now()[..4].to_string();
        let a = take(&mut conn, &user, &s, 0).repair;
        let b = take(&mut conn, &user, &s, 0).repair;
        assert_eq!(a.number, format!("REP{year}-000001"));
        assert_eq!(b.number, format!("REP{year}-000002"));
        assert_eq!((a.data.customer_name.as_str(), a.data.fault.as_str()), ("Joan García", "No carga.\nPantalla rota"));
        assert_eq!(a.line_name, format!("Reparación REP{year}-000001: Apple iPhone 12"));

        // the numbering file says 9 were handed out before an old backup came back
        let series = format!("REP{year}");
        let missing = tickets::raise_floors(&conn, &BTreeMap::from([(series.clone(), 9)])).unwrap();
        assert_eq!(missing, vec![(series.clone(), 3, 9)]);
        assert_eq!(take(&mut conn, &user, &s, 0).repair.number, format!("{series}-000010"));
        assert_eq!(tickets::series_marks(&conn).unwrap()[&series], 10);

        // the same form sent twice is one repair
        let once = NewRepair { repair: phone(), request_id: Some("form-1".into()), ..NewRepair::default() };
        let first = create(&mut conn, &user, &s, &once).unwrap();
        let again = create(&mut conn, &user, &s, &once).unwrap();
        assert!(!first.duplicate && again.duplicate);
        assert_eq!(first.repair.id, again.repair.id);

        let mut bad = phone();
        bad.customer_phone = " ".into();
        let err = |r: RepairIn, c: &mut Connection| create(c, &user, &s, &NewRepair { repair: r, ..NewRepair::default() }).unwrap_err().0;
        assert_eq!(err(bad, &mut conn), "E_INVALID:customerPhone");
        assert_eq!(err(RepairIn { brand: String::new(), model: String::new(), ..phone() }, &mut conn), "E_INVALID:device");
        assert_eq!(err(RepairIn { fault: "\n ".into(), ..phone() }, &mut conn), "E_INVALID:fault");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM repairs", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 4);
    }

    #[test]
    fn a_deposit_is_a_ticket_the_repair_points_at() {
        let (mut conn, user, s) = setup();
        let c = take(&mut conn, &user, &s, 2000);
        let (r, t) = (c.repair, c.deposit.unwrap());
        assert_eq!(t.lines[0].name, format!("Anticipo reparación {}", r.number));
        assert_eq!((t.total_cents, t.lines[0].repair_id, t.payment.method.as_str()), (2000, Some(r.id), "cash"));
        assert_eq!(r.deposit.as_ref().unwrap().number, t.number);
        assert_eq!((r.deposit_cents, r.due_cents), (2000, Some(6000)));
        // it is a sale like any other: in the till and in the reports
        let day = t.issued_at[..10].to_string();
        let sum = crate::reports::summary(&conn, &crate::reports::Range { from: day.clone(), to: day }).unwrap();
        assert_eq!((sum.tickets, sum.total_cents), (1, 2000));
        assert_eq!(sum.top[0].name, "Reparaciones");

        // nor more than the estimate, nor a deposit while practising
        let high = NewRepair { repair: phone(), deposit_cents: 9000, ..NewRepair::default() };
        assert_eq!(create(&mut conn, &user, &s, &high).unwrap_err().0, "E_DEPOSIT_HIGH");
        let practice = Settings { training: true, ..s.clone() };
        let paid = NewRepair { repair: phone(), deposit_cents: 1000, ..NewRepair::default() };
        assert_eq!(create(&mut conn, &user, &practice, &paid).unwrap_err().0, "E_REPAIR_TRAINING");
        // a deposit is a simplified invoice for a service: 400 EUR at most
        let big = NewRepair { repair: RepairIn { estimate_cents: Some(90_000), ..phone() }, deposit_cents: 45_000, ..NewRepair::default() };
        assert_eq!(create(&mut conn, &user, &s, &big).unwrap_err().0, "E_OVER_LIMIT_SERVICE");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM repairs", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);

        // money taken cannot be left behind by a cancellation
        assert_eq!(cancel(&mut conn, r.id, "Ana", "error").unwrap_err().0, "E_REPAIR_DEPOSIT");
        // a return of the deposit gives it back
        let back = RefundIn { id: t.id, lines: vec![RefundLine { line_id: t.lines[0].id.unwrap(), qty: 1 }], method: "cash".into(), reason: String::new(), clock_ok: false };
        let refund = tickets::refund(&mut conn, &user, &s, &back).unwrap();
        assert_eq!(refund.lines[0].repair_id, Some(r.id));
        let r = load(&conn, r.id).unwrap();
        assert_eq!((r.deposit_cents, r.due_cents), (0, Some(8000)));
        let r = cancel(&mut conn, r.id, "Ana", "  cliente se lo pensó ").unwrap();
        assert_eq!((r.status.as_str(), r.cancel_reason.as_deref()), ("cancelled", Some("cliente se lo pensó")));

        // a void of the deposit ticket does the same
        let c = take(&mut conn, &user, &s, 1500);
        tickets::void(&mut conn, c.deposit.unwrap().id, "Ana", "error").unwrap();
        assert_eq!(load(&conn, c.repair.id).unwrap().deposit_cents, 0);
    }

    #[test]
    fn status_changes_are_kept_with_who_and_when() {
        let (mut conn, user, s) = setup();
        let r = take(&mut conn, &user, &s, 0).repair;
        assert_eq!(r.status, "received");
        set_status(&mut conn, r.id, "Jordi", "in_progress").unwrap();
        set_status(&mut conn, r.id, "Jordi", "in_progress").unwrap();
        let r = set_status(&mut conn, r.id, "Ana", "ready").unwrap();
        let seen: Vec<(&str, &str, &str)> = r.events.iter().map(|e| (e.kind.as_str(), e.status.as_str(), e.by_name.as_str())).collect();
        assert_eq!(seen, vec![("created", "received", "Ana"), ("status", "in_progress", "Jordi"), ("status", "ready", "Ana")]);
        assert!(r.events.iter().all(|e| e.at.len() == 19));
        assert_eq!(set_status(&mut conn, r.id, "Ana", "delivered").unwrap_err().0, "E_INVALID:status");

        let r = update(&mut conn, r.id, "Jordi", &RepairIn { customer_phone: "611 000 111".into(), estimate_cents: Some(9500), ..phone() }).unwrap();
        let e = r.events.last().unwrap();
        assert_eq!((e.kind.as_str(), e.status.as_str()), ("edited", "ready"));
        assert_eq!(e.detail.as_ref().unwrap(), &json!({ "fields": ["customerPhone", "estimate"], "estimateFrom": 8000, "estimateTo": 9500 }));
        // the same data again changes nothing
        assert_eq!(update(&mut conn, r.id, "Jordi", &RepairIn { customer_phone: "611 000 111".into(), estimate_cents: Some(9500), ..phone() }).unwrap().events.len(), 4);
        assert!(conn.execute("UPDATE repair_events SET by_name = 'x'", []).is_err());
        assert!(conn.execute("DELETE FROM repair_events", []).is_err());
    }

    #[test]
    fn a_sale_with_the_repair_on_it_delivers_it_and_a_void_brings_it_back() {
        let (mut conn, user, s) = setup();
        let c = take(&mut conn, &user, &s, 2000);
        let r = set_status(&mut conn, c.repair.id, "Ana", "ready").unwrap();
        let cable = SaleLine { name: "Cable".into(), qty: 1, unit_cents: 690, vat_bp: 2100, ..SaleLine::default() };
        let (t, _) = tickets::sell(&mut conn, &user, &s, vec![pickup(&r, 6000), cable], &cash(), &SaleOpts::default()).unwrap();
        // the core names the line and sells it once, whatever the till sent
        assert_eq!((t.lines[0].name.as_str(), t.lines[0].qty, t.lines[0].repair_id), (r.line_name.as_str(), 1, Some(r.id)));
        assert_eq!(t.total_cents, 6690);
        let r = load(&conn, r.id).unwrap();
        assert_eq!((r.status.as_str(), r.ticket.as_ref().unwrap().number.as_str()), ("delivered", t.number.as_str()));
        assert_eq!(r.events.last().unwrap().detail, Some(json!({ "ticket": t.number })));
        assert!(r.closed_at.is_some());

        // handed back once only, and a delivered repair is frozen
        let again = tickets::sell(&mut conn, &user, &s, vec![pickup(&r, 6000)], &cash(), &SaleOpts::default());
        assert_eq!(again.unwrap_err().0, "E_REPAIR_CLOSED");
        assert_eq!(set_status(&mut conn, r.id, "Ana", "ready").unwrap_err().0, "E_REPAIR_CLOSED");
        assert!(conn.execute("UPDATE repairs SET notes = 'x' WHERE id = ?1", [r.id]).is_err());
        assert!(conn.execute("UPDATE repairs SET status = 'ready', ticket_id = NULL WHERE id = ?1", [r.id]).is_err());
        assert!(conn.execute("DELETE FROM repairs", []).is_err());

        // charged by mistake: the void puts the phone back where it was
        tickets::void(&mut conn, t.id, "Ana", "cobrado de más").unwrap();
        let r = load(&conn, r.id).unwrap();
        assert_eq!((r.status.as_str(), r.ticket.is_none(), r.closed_at.is_none()), ("ready", true, true));
        assert_eq!(r.events.last().unwrap().kind, "reopened");
        let (t2, _) = tickets::sell(&mut conn, &user, &s, vec![pickup(&r, 6000)], &cash(), &SaleOpts::default()).unwrap();
        assert_eq!(load(&conn, r.id).unwrap().ticket.unwrap().id, t2.id);
    }

    #[test]
    fn a_repair_goes_on_a_ticket_once_and_never_in_practice() {
        let (mut conn, user, s) = setup();
        let r = take(&mut conn, &user, &s, 0).repair;
        let twice = vec![pickup(&r, 4000), pickup(&r, 4000)];
        assert_eq!(tickets::sell(&mut conn, &user, &s, twice, &cash(), &SaleOpts::default()).unwrap_err().0, "E_REPAIR_TWICE");
        let practice = Settings { training: true, ..s.clone() };
        let err = tickets::sell(&mut conn, &user, &practice, vec![pickup(&r, 4000)], &cash(), &SaleOpts::default()).unwrap_err().0;
        assert_eq!(err, "E_REPAIR_TRAINING");
        let gone = SaleLine { repair_id: Some(999), ..pickup(&r, 4000) };
        assert_eq!(tickets::sell(&mut conn, &user, &s, vec![gone], &cash(), &SaleOpts::default()).unwrap_err().0, "E_REPAIR_GONE");

        // a service over 400 EUR needs the customer's details
        let dear = vec![pickup(&r, 45_000)];
        assert_eq!(tickets::sell(&mut conn, &user, &s, dear.clone(), &cash(), &SaleOpts::default()).unwrap_err().0, "E_OVER_LIMIT_SERVICE");
        let invoice = SaleOpts { customer: Some(Customer { name: "Joan García".into(), nif: "B12345674".into(), address: String::new() }), ..SaleOpts::default() };
        let (t, _) = tickets::sell(&mut conn, &user, &s, dear, &cash(), &invoice).unwrap();
        assert!(t.number.starts_with("TF"));
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn nothing_left_to_pay_delivers_without_a_sale() {
        let (mut conn, user, s) = setup();
        let r = take(&mut conn, &user, &s, 2000).repair;
        assert_eq!(deliver(&mut conn, r.id, "Ana").unwrap_err().0, "E_REPAIR_DUE");
        let r = update(&mut conn, r.id, "Ana", &RepairIn { estimate_cents: Some(1000), ..phone() }).unwrap();
        assert_eq!(deliver(&mut conn, r.id, "Ana").unwrap_err().0, "E_REPAIR_DEPOSIT_HIGH");
        let r = update(&mut conn, r.id, "Ana", &RepairIn { estimate_cents: None, ..phone() }).unwrap();
        assert_eq!(deliver(&mut conn, r.id, "Ana").unwrap_err().0, "E_REPAIR_PRICE");
        update(&mut conn, r.id, "Ana", &RepairIn { estimate_cents: Some(2000), ..phone() }).unwrap();
        let r = deliver(&mut conn, r.id, "Jordi").unwrap();
        assert_eq!((r.status.as_str(), r.closed_by.as_deref(), r.ticket.is_none()), ("delivered", Some("Jordi"), true));
        assert_eq!(cancel(&mut conn, r.id, "Ana", "error").unwrap_err().0, "E_REPAIR_CLOSED");
    }

    #[test]
    fn search_finds_by_number_name_phone_and_imei() {
        let (mut conn, user, s) = setup();
        let a = take(&mut conn, &user, &s, 0).repair;
        let other = RepairIn { customer_name: "Marta Puig".into(), customer_phone: "699 999 999".into(), imei: "SN-ABC123".into(), model: "Galaxy S21".into(), brand: "Samsung".into(), ..phone() };
        let b = create(&mut conn, &user, &s, &NewRepair { repair: other, ..NewRepair::default() }).unwrap().repair;
        set_status(&mut conn, b.id, "Ana", "ready").unwrap();
        let find = |q: &str, status: Option<&str>| -> Vec<i64> {
            list(&conn, &ListQuery { q: Some(q.into()), status: status.map(Into::into), limit: None }).unwrap().iter().map(|r| r.id).collect()
        };
        assert_eq!(find("", None), vec![b.id, a.id]);
        assert_eq!(find("garcia", None), vec![a.id]);
        assert_eq!(find("600123456", None), vec![a.id]);
        assert_eq!(find("699 999", None), vec![b.id]);
        assert_eq!(find("abc123", None), vec![b.id]);
        assert_eq!(find("356789", None), vec![a.id]);
        assert_eq!(find(&a.number, None), vec![a.id]);
        assert_eq!(find("galaxy", None), vec![b.id]);
        assert_eq!(find("", Some("ready")), vec![b.id]);
        assert_eq!(find("", Some("delivered")), Vec::<i64>::new());
        assert!(list(&conn, &ListQuery { status: Some("x'; DROP".into()), ..ListQuery::default() }).is_err());
    }
}
