use std::collections::BTreeMap;

use chrono::Local;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::{fail, Result};
use crate::money::split_vat;
use crate::settings::Settings;
use crate::state::SessionUser;

pub const MAX_PRICE: i64 = 9_999_999;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SaleLine {
    #[serde(default)]
    pub product_id: Option<i64>,
    pub name: String,
    pub qty: i64,
    pub unit_cents: i64,
    pub vat_bp: i64,
}

impl SaleLine {
    pub fn total(&self) -> i64 {
        self.qty * self.unit_cents
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VatRow {
    pub vat_bp: i64,
    pub base_cents: i64,
    pub vat_cents: i64,
    pub total_cents: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub total_cents: i64,
    pub item_count: i64,
    pub vat: Vec<VatRow>,
}

pub fn compute(lines: &[SaleLine]) -> Totals {
    let mut by_rate: BTreeMap<i64, i64> = BTreeMap::new();
    let (mut total, mut items) = (0, 0);
    for l in lines {
        total += l.total();
        items += l.qty;
        *by_rate.entry(l.vat_bp).or_default() += l.total();
    }
    let vat = by_rate
        .into_iter()
        .rev()
        .map(|(bp, gross)| {
            let (base, vat) = split_vat(gross, bp);
            VatRow { vat_bp: bp, base_cents: base, vat_cents: vat, total_cents: gross }
        })
        .collect();
    Totals { total_cents: total, item_count: items, vat }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PaymentIn {
    pub method: String,
    #[serde(default)]
    pub tendered_cents: Option<i64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Payment {
    pub method: String,
    pub tendered_cents: i64,
    pub change_cents: i64,
}

pub fn resolve_payment(p: &PaymentIn, total: i64) -> Result<Payment> {
    match p.method.as_str() {
        "cash" => {
            let tendered = p.tendered_cents.unwrap_or(total);
            if tendered < total || tendered > 100 * MAX_PRICE {
                return fail("E_TENDERED");
            }
            Ok(Payment { method: "cash".into(), tendered_cents: tendered, change_cents: tendered - total })
        }
        "card" | "bizum" => Ok(Payment { method: p.method.clone(), tendered_cents: total, change_cents: 0 }),
        _ => fail("E_INVALID:method"),
    }
}

// what the header and footer looked like when the ticket was issued, so a
// reprint next year still matches the original
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ShopInfo {
    pub name: String,
    pub address: String,
    pub phone: String,
    pub nif: String,
    pub footer: String,
}

impl ShopInfo {
    pub fn from_settings(s: &Settings) -> ShopInfo {
        ShopInfo {
            name: s.shop_name.clone(),
            address: s.shop_address.clone(),
            phone: s.shop_phone.clone(),
            nif: s.shop_nif.clone(),
            footer: s.footer.clone(),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TicketRecord {
    pub id: i64,
    pub number: String,
    pub issued_at: String,
    pub user_name: String,
    pub total_cents: i64,
    pub item_count: i64,
    pub payment: Payment,
    pub voided_at: Option<String>,
    pub voided_by: Option<String>,
    pub void_reason: Option<String>,
    pub lines: Vec<SaleLine>,
    pub shop: ShopInfo,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TicketSummary {
    pub id: i64,
    pub number: String,
    pub issued_at: String,
    pub user_name: String,
    pub total_cents: i64,
    pub item_count: i64,
    pub payment_method: String,
    pub voided: bool,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct ListQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub q: Option<String>,
    pub limit: Option<i64>,
}

pub fn now() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

// one gapless series per prefix and year: T2026-000001, T2026-000002, ...
pub fn next_number(conn: &Connection, prefix: &str, issued_at: &str) -> Result<(String, i64, String)> {
    let series = format!("{prefix}{}", &issued_at[..4]);
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM tickets WHERE series = ?1",
        [&series],
        |r| r.get(0),
    )?;
    let number = format!("{series}-{seq:06}");
    Ok((series, seq, number))
}

pub fn clamp(lines: Vec<SaleLine>) -> Vec<SaleLine> {
    lines
        .into_iter()
        .take(500)
        .map(|mut l| {
            l.qty = l.qty.clamp(1, 999);
            l.unit_cents = l.unit_cents.clamp(-MAX_PRICE, MAX_PRICE);
            l.vat_bp = l.vat_bp.clamp(0, 10_000);
            l
        })
        .collect()
}

fn validate(lines: Vec<SaleLine>) -> Result<Vec<SaleLine>> {
    if lines.is_empty() {
        return fail("E_EMPTY");
    }
    if lines.len() > 500 {
        return fail("E_INVALID:lines");
    }
    let mut out = Vec::with_capacity(lines.len());
    for mut l in lines {
        l.name = l.name.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.name.is_empty() || l.name.chars().count() > 120 {
            return fail("E_INVALID:name");
        }
        if !(1..=999).contains(&l.qty) {
            return fail("E_INVALID:qty");
        }
        if l.unit_cents.abs() > MAX_PRICE {
            return fail("E_INVALID:price");
        }
        if !(0..=10_000).contains(&l.vat_bp) {
            return fail("E_INVALID:vat");
        }
        out.push(l);
    }
    if out.iter().map(SaleLine::total).sum::<i64>() < 0 {
        return fail("E_NEGATIVE");
    }
    Ok(out)
}

pub fn finalize(
    conn: &mut Connection,
    user: &SessionUser,
    s: &Settings,
    lines: Vec<SaleLine>,
    pay: &PaymentIn,
) -> Result<TicketRecord> {
    let lines = validate(lines)?;
    let totals = compute(&lines);
    let payment = resolve_payment(pay, totals.total_cents)?;
    let shop = serde_json::to_string(&ShopInfo::from_settings(s))?;

    // IMMEDIATE takes the write lock before reading MAX(seq), so two
    // instances can never hand out the same number
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let issued_at = now();
    let (series, seq, number) = next_number(&tx, &s.ticket_prefix, &issued_at)?;
    tx.execute(
        "INSERT INTO tickets (number, series, seq, issued_at, user_id, user_name, total_cents, item_count,
                              payment_method, tendered_cents, change_cents, shop)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            number,
            series,
            seq,
            issued_at,
            user.id,
            user.name,
            totals.total_cents,
            totals.item_count,
            payment.method,
            payment.tendered_cents,
            payment.change_cents,
            shop
        ],
    )?;
    let id = tx.last_insert_rowid();
    for (pos, l) in lines.iter().enumerate() {
        tx.execute(
            "INSERT INTO ticket_lines (ticket_id, pos, product_id, name, qty, unit_cents, vat_bp, total_cents)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, pos as i64, l.product_id, l.name, l.qty, l.unit_cents, l.vat_bp, l.total()],
        )?;
        if let Some(pid) = l.product_id {
            tx.execute("UPDATE products SET sold_qty = sold_qty + ?1 WHERE id = ?2", params![l.qty, pid])?;
        }
    }
    tx.commit()?;
    load(conn, id)
}

pub fn load(conn: &Connection, id: i64) -> Result<TicketRecord> {
    let rec = conn
        .query_row(
            "SELECT id, number, issued_at, user_name, total_cents, item_count, payment_method,
                    tendered_cents, change_cents, voided_at, voided_by, void_reason, shop
             FROM tickets WHERE id = ?1",
            [id],
            |r| {
                Ok(TicketRecord {
                    id: r.get(0)?,
                    number: r.get(1)?,
                    issued_at: r.get(2)?,
                    user_name: r.get(3)?,
                    total_cents: r.get(4)?,
                    item_count: r.get(5)?,
                    payment: Payment { method: r.get(6)?, tendered_cents: r.get(7)?, change_cents: r.get(8)? },
                    voided_at: r.get(9)?,
                    voided_by: r.get(10)?,
                    void_reason: r.get(11)?,
                    shop: serde_json::from_str(&r.get::<_, String>(12)?).unwrap_or_default(),
                    lines: Vec::new(),
                })
            },
        )
        .optional()?;
    let Some(mut rec) = rec else {
        return fail("E_NOT_FOUND");
    };
    let mut stmt = conn.prepare(
        "SELECT product_id, name, qty, unit_cents, vat_bp FROM ticket_lines WHERE ticket_id = ?1 ORDER BY pos",
    )?;
    rec.lines = stmt
        .query_map([id], |r| {
            Ok(SaleLine {
                product_id: r.get(0)?,
                name: r.get(1)?,
                qty: r.get(2)?,
                unit_cents: r.get(3)?,
                vat_bp: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rec)
}

pub fn list(conn: &Connection, q: &ListQuery) -> Result<Vec<TicketSummary>> {
    let from = q.from.clone().unwrap_or_default();
    let to = q.to.as_ref().map(|t| format!("{t}T99")).unwrap_or_else(|| "9999".into());
    let needle = q.q.clone().unwrap_or_default().trim().to_uppercase();
    let amount = needle.replace(',', ".");
    let limit = q.limit.unwrap_or(300).clamp(1, 2000);
    let mut stmt = conn.prepare(
        "SELECT id, number, issued_at, user_name, total_cents, item_count, payment_method, voided_at IS NOT NULL
         FROM tickets
         WHERE issued_at >= ?1 AND issued_at <= ?2
           AND (?3 = '' OR number LIKE '%' || ?3 || '%' OR printf('%.2f', total_cents / 100.0) LIKE '%' || ?4 || '%')
         ORDER BY id DESC LIMIT ?5",
    )?;
    let rows = stmt
        .query_map(params![from, to, needle, amount, limit], |r| {
            Ok(TicketSummary {
                id: r.get(0)?,
                number: r.get(1)?,
                issued_at: r.get(2)?,
                user_name: r.get(3)?,
                total_cents: r.get(4)?,
                item_count: r.get(5)?,
                payment_method: r.get(6)?,
                voided: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn void(conn: &mut Connection, id: i64, by: &str, reason: &str) -> Result<()> {
    let reason: String = reason.trim().chars().take(200).collect();
    let tx = conn.transaction()?;
    let n = tx.execute(
        "UPDATE tickets SET voided_at = ?1, voided_by = ?2, void_reason = ?3 WHERE id = ?4 AND voided_at IS NULL",
        params![now(), by, reason, id],
    )?;
    if n == 0 {
        return fail("E_NOT_FOUND");
    }
    tx.execute(
        "UPDATE products SET sold_qty = MAX(0, sold_qty - (
             SELECT COALESCE(SUM(qty), 0) FROM ticket_lines WHERE ticket_id = ?1 AND product_id = products.id))
         WHERE id IN (SELECT product_id FROM ticket_lines WHERE ticket_id = ?1)",
        [id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn sample_lines(vat_bp: i64) -> Vec<SaleLine> {
    vec![
        SaleLine { product_id: None, name: "Cable USB-C 1 m".into(), qty: 2, unit_cents: 690, vat_bp },
        SaleLine {
            product_id: None,
            name: "Protector de pantalla cristal templado".into(),
            qty: 1,
            unit_cents: 795,
            vat_bp,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn setup() -> (Connection, SessionUser, Settings) {
        let dir = std::env::temp_dir().join(format!("tiquet-test-{}-{}", std::process::id(), rand_suffix()));
        let conn = db::open(&dir.join("t.db")).unwrap();
        conn.execute("INSERT INTO users (name, role, pass_hash) VALUES ('Ana', 'admin', 'x')", []).unwrap();
        conn.execute("INSERT INTO products (name, price_cents) VALUES ('Cable', 690)", []).unwrap();
        let user = SessionUser { id: 1, name: "Ana".into(), role: "admin".into() };
        (conn, user, Settings::default())
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    fn line(name: &str, qty: i64, unit: i64) -> SaleLine {
        SaleLine { product_id: None, name: name.into(), qty, unit_cents: unit, vat_bp: 2100 }
    }

    fn cash(t: Option<i64>) -> PaymentIn {
        PaymentIn { method: "cash".into(), tendered_cents: t }
    }

    #[test]
    fn totals_group_vat_by_rate() {
        let mut lines = vec![line("a", 2, 690), line("b", 1, 795)];
        lines.push(SaleLine { vat_bp: 1000, ..line("c", 1, 1100) });
        let t = compute(&lines);
        assert_eq!(t.total_cents, 3275);
        assert_eq!(t.item_count, 4);
        assert_eq!(t.vat[0], VatRow { vat_bp: 2100, base_cents: 1798, vat_cents: 377, total_cents: 2175 });
        assert_eq!(t.vat[1], VatRow { vat_bp: 1000, base_cents: 1000, vat_cents: 100, total_cents: 1100 });
    }

    #[test]
    fn numbers_are_sequential_and_records_frozen() {
        let (mut conn, user, s) = setup();
        let a = finalize(&mut conn, &user, &s, vec![line("Cable", 2, 690)], &cash(Some(2000))).unwrap();
        let b = finalize(&mut conn, &user, &s, vec![line("Funda", 1, 995)], &cash(None)).unwrap();
        let year = &a.issued_at[..4];
        assert_eq!(a.number, format!("T{year}-000001"));
        assert_eq!(b.number, format!("T{year}-000002"));
        assert_eq!(a.payment.change_cents, 620);
        assert_eq!(b.payment.tendered_cents, 995);

        assert!(conn.execute("DELETE FROM tickets WHERE id = ?1", [a.id]).is_err());
        assert!(conn.execute("UPDATE tickets SET total_cents = 1 WHERE id = ?1", [a.id]).is_err());
        assert!(conn.execute("UPDATE ticket_lines SET qty = 9", []).is_err());
        assert!(conn.execute("DELETE FROM ticket_lines", []).is_err());

        void(&mut conn, a.id, "Ana", "error").unwrap();
        assert!(load(&conn, a.id).unwrap().voided_at.is_some());
        assert!(void(&mut conn, a.id, "Ana", "again").is_err());
    }

    #[test]
    fn rejects_bad_sales() {
        let (mut conn, user, s) = setup();
        let mut err = |lines: Vec<SaleLine>, pay: PaymentIn| finalize(&mut conn, &user, &s, lines, &pay).unwrap_err().0;
        assert_eq!(err(vec![], cash(None)), "E_EMPTY");
        assert_eq!(err(vec![line("x", 1, 500)], cash(Some(400))), "E_TENDERED");
        assert_eq!(err(vec![line("x", 0, 500)], cash(None)), "E_INVALID:qty");
        assert_eq!(err(vec![line("  ", 1, 500)], cash(None)), "E_INVALID:name");
        assert_eq!(err(vec![line("x", 1, -500)], cash(None)), "E_NEGATIVE");
        assert_eq!(err(vec![line("x", 1, MAX_PRICE + 1)], cash(None)), "E_INVALID:price");
        assert_eq!(err(vec![line("x", 1, 500)], PaymentIn { method: "gold".into(), tendered_cents: None }), "E_INVALID:method");
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn product_sales_counted_and_unwound_on_void() {
        let (mut conn, user, s) = setup();
        let l = SaleLine { product_id: Some(1), ..line("Cable", 3, 690) };
        let t = finalize(&mut conn, &user, &s, vec![l], &PaymentIn { method: "card".into(), tendered_cents: None }).unwrap();
        let sold = |c: &Connection| c.query_row("SELECT sold_qty FROM products WHERE id = 1", [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(sold(&conn), 3);
        void(&mut conn, t.id, "Ana", "").unwrap();
        assert_eq!(sold(&conn), 0);
    }
}
