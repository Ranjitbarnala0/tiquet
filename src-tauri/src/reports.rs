use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use chrono::NaiveDate;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::backup;
use crate::error::{fail, Result};
use crate::layout::{datetime, method_label};
use crate::tickets::VatRow;

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Range {
    pub from: String,
    pub to: String,
}

impl Range {
    fn bounds(&self) -> Result<(String, String)> {
        let ok = |d: &str| NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok();
        if !ok(&self.from) || !ok(&self.to) || self.from > self.to {
            return fail("E_INVALID:range");
        }
        Ok((self.from.clone(), format!("{}T99", self.to)))
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MethodRow {
    pub method: String,
    pub count: i64,
    pub total_cents: i64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DayRow {
    pub date: String,
    pub count: i64,
    pub total_cents: i64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TopRow {
    pub name: String,
    pub qty: i64,
    pub total_cents: i64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRow {
    pub issued_at: String,
    pub number: String,
    pub user_name: String,
    pub name: String,
    pub qty: i64,
    pub list_cents: i64,
    pub unit_cents: i64,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    // net: sales minus returns
    pub total_cents: i64,
    pub sales_cents: i64,
    pub refund_cents: i64,
    pub tickets: i64,
    pub refunds: i64,
    pub items: i64,
    pub voided: i64,
    pub first_number: Option<String>,
    pub last_number: Option<String>,
    pub by_method: Vec<MethodRow>,
    pub by_day: Vec<DayRow>,
    pub vat: Vec<VatRow>,
    pub top: Vec<TopRow>,
    // money given away at the till: discount lines, and catalogue prices
    // changed by the cashier
    pub discounts: i64,
    pub discount_cents: i64,
    pub changed: i64,
    pub changed_cents: i64,
    pub changes: Vec<ChangeRow>,
}

pub fn summary(conn: &Connection, r: &Range) -> Result<Summary> {
    let (from, to) = r.bounds()?;
    let start = NaiveDate::parse_from_str(&r.from, "%Y-%m-%d").unwrap();
    let end = NaiveDate::parse_from_str(&r.to, "%Y-%m-%d").unwrap();
    collect(conn, &from, &to, Some((start, end)))
}

// between two moments, for a till count: from opening to closing
pub fn between(conn: &Connection, from: &str, to: &str) -> Result<Summary> {
    collect(conn, from, to, None)
}

fn collect(conn: &Connection, from: &str, to: &str, fill: Option<(NaiveDate, NaiveDate)>) -> Result<Summary> {
    let mut s = Summary::default();
    let mut methods: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    let mut days: BTreeMap<String, (i64, i64)> = BTreeMap::new();

    let mut stmt = conn.prepare(
        "SELECT number, issued_at, total_cents, item_count, payment_method, voided_at IS NOT NULL, kind = 'sale'
         FROM tickets WHERE issued_at >= ?1 AND issued_at <= ?2 AND training = 0 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![from, to], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, bool>(5)?,
            r.get::<_, bool>(6)?,
        ))
    })?;
    // counts are of sales; amounts are net of returns, so cash by method is
    // what the drawer should hold
    for row in rows {
        let (number, at, total, items, method, voided, sale) = row?;
        if sale {
            if s.first_number.is_none() {
                s.first_number = Some(number.clone());
            }
            s.last_number = Some(number);
        }
        if voided {
            s.voided += 1;
            continue;
        }
        if sale {
            s.tickets += 1;
            s.sales_cents += total;
        } else {
            s.refunds += 1;
            s.refund_cents += total;
        }
        s.total_cents += total;
        s.items += items;
        let m = methods.entry(method).or_default();
        m.0 += sale as i64;
        m.1 += total;
        let d = days.entry(at[..10].to_string()).or_default();
        d.0 += sale as i64;
        d.1 += total;
    }

    // the sum of what each ticket printed, rounded per ticket
    let mut stmt = conn.prepare(
        "SELECT v.vat_bp, SUM(v.base_cents), SUM(v.vat_cents), SUM(v.total_cents)
         FROM ticket_vat v JOIN tickets t ON t.id = v.ticket_id
         WHERE t.voided_at IS NULL AND t.training = 0 AND t.issued_at >= ?1 AND t.issued_at <= ?2
         GROUP BY v.vat_bp ORDER BY v.vat_bp DESC",
    )?;
    s.vat = stmt
        .query_map(params![from, to], |r| {
            Ok(VatRow { vat_bp: r.get(0)?, base_cents: r.get(1)?, vat_cents: r.get(2)?, total_cents: r.get(3)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut stmt = conn.prepare(
        "SELECT l.name, SUM(l.qty), SUM(l.total_cents) FROM ticket_lines l JOIN tickets t ON t.id = l.ticket_id
         WHERE t.voided_at IS NULL AND t.training = 0 AND t.issued_at >= ?1 AND t.issued_at <= ?2
         GROUP BY l.name HAVING SUM(l.qty) > 0 ORDER BY 3 DESC, 2 DESC LIMIT 10",
    )?;
    s.top = stmt
        .query_map(params![from, to], |r| Ok(TopRow { name: r.get(0)?, qty: r.get(1)?, total_cents: r.get(2)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let live = "FROM ticket_lines l JOIN tickets t ON t.id = l.ticket_id
                WHERE t.kind = 'sale' AND t.voided_at IS NULL AND t.training = 0 AND t.issued_at >= ?1 AND t.issued_at <= ?2";
    (s.discounts, s.discount_cents) = conn.query_row(
        &format!("SELECT COUNT(*), COALESCE(SUM(l.total_cents), 0) {live} AND l.unit_cents < 0"),
        params![from, to],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let changed = "AND l.list_cents IS NOT NULL AND l.list_cents <> 0 AND l.unit_cents <> l.list_cents";
    (s.changed, s.changed_cents) = conn.query_row(
        &format!("SELECT COUNT(*), COALESCE(SUM((l.unit_cents - l.list_cents) * l.qty), 0) {live} {changed}"),
        params![from, to],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut stmt = conn.prepare(&format!(
        "SELECT t.issued_at, t.number, t.user_name, l.name, l.qty, l.list_cents, l.unit_cents {live} {changed}
         ORDER BY t.id DESC, l.pos LIMIT 10"
    ))?;
    s.changes = stmt
        .query_map(params![from, to], |r| {
            Ok(ChangeRow {
                issued_at: r.get(0)?,
                number: r.get(1)?,
                user_name: r.get(2)?,
                name: r.get(3)?,
                qty: r.get(4)?,
                list_cents: r.get(5)?,
                unit_cents: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for m in ["cash", "card", "bizum"] {
        if let Some((count, total)) = methods.remove(m) {
            s.by_method.push(MethodRow { method: m.into(), count, total_cents: total });
        }
    }

    // fill empty days so a chart shows the gaps; past three months a bar per
    // day is unreadable, so the days fold into months ("2026-10")
    let Some((start, end)) = fill else {
        s.by_day = days.into_iter().map(|(date, (count, total))| DayRow { date, count, total_cents: total }).collect();
        return Ok(s);
    };
    if (end - start).num_days() > 92 {
        let mut months: BTreeMap<String, (i64, i64)> = BTreeMap::new();
        for (day, (count, total)) in days {
            let m = months.entry(day[..7].to_string()).or_default();
            m.0 += count;
            m.1 += total;
        }
        for d in start.iter_days().take_while(|d| *d <= end) {
            months.entry(d.format("%Y-%m").to_string()).or_default();
        }
        days = months;
    } else {
        for d in start.iter_days().take_while(|d| *d <= end) {
            days.entry(d.format("%Y-%m-%d").to_string()).or_default();
        }
    }
    s.by_day = days
        .into_iter()
        .map(|(date, (count, total))| DayRow { date, count, total_cents: total })
        .collect();
    Ok(s)
}

fn plain(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    format!("{sign}{},{:02}", cents.unsigned_abs() / 100, cents.unsigned_abs() % 100)
}

fn cell(s: &str) -> String {
    // a product typed as =HYPERLINK(...) must open as text, not run
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    if s.contains([';', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

// semicolons and decimal commas: opens straight into Spanish Excel
pub fn csv(conn: &Connection, r: &Range) -> Result<String> {
    let (from, to) = r.bounds()?;
    let mut out =
        String::from("\u{feff}Fecha;Ticket;Tipo;Rectifica;Atendido por;Pago;Anulado;Producto;Uds;Precio;IVA %;Importe\r\n");
    let mut stmt = conn.prepare(
        "SELECT t.issued_at, t.number, t.kind, COALESCE(o.number, ''), t.user_name, t.payment_method,
                t.voided_at IS NOT NULL, l.name, l.qty, l.unit_cents, l.vat_bp, l.total_cents
         FROM tickets t JOIN ticket_lines l ON l.ticket_id = t.id LEFT JOIN tickets o ON o.id = t.refund_of
         WHERE t.issued_at >= ?1 AND t.issued_at <= ?2 AND t.training = 0 ORDER BY t.id, l.pos",
    )?;
    let rows = stmt.query_map(params![from, to], |r| {
        Ok((
            (r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?),
            (r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, bool>(6)?, r.get::<_, String>(7)?),
            (r.get::<_, i64>(8)?, r.get::<_, i64>(9)?, r.get::<_, i64>(10)?, r.get::<_, i64>(11)?),
        ))
    })?;
    for row in rows {
        let ((at, number, kind, of), (user, method, voided, name), (qty, unit, vat, total)) = row?;
        let fields = [
            datetime(&at),
            number,
            if kind == "refund" { "Devolución".into() } else { "Venta".into() },
            of,
            cell(&user),
            method_label(&method).to_string(),
            if voided { "Sí".into() } else { "No".into() },
            cell(&name),
            qty.to_string(),
            plain(unit),
            plain(vat),
            plain(total),
        ];
        out.push_str(&fields.join(";"));
        out.push_str("\r\n");
    }
    Ok(out)
}

// For the gestor: one row per ticket, in number order, with base and VAT for
// each rate as printed. Voided tickets stay in the list, marked, so the
// numbering can be checked for gaps.
pub fn vat_book(conn: &Connection, r: &Range) -> Result<String> {
    let (from, to) = r.bounds()?;
    let mut stmt = conn.prepare(
        "SELECT DISTINCT v.vat_bp FROM ticket_vat v JOIN tickets t ON t.id = v.ticket_id
         WHERE t.issued_at >= ?1 AND t.issued_at <= ?2 AND t.training = 0 ORDER BY v.vat_bp DESC",
    )?;
    let rates = stmt.query_map(params![from, to], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut head = vec!["Fecha", "Factura", "Tipo", "Rectifica a", "Anulada", "Cliente", "NIF cliente"].into_iter().map(String::from).collect::<Vec<_>>();
    for bp in &rates {
        let p = plain(*bp);
        let p = p.trim_end_matches(",00");
        head.push(format!("Base {p}%"));
        head.push(format!("Cuota {p}%"));
    }
    head.extend(["Total", "Pago"].map(String::from));
    let mut out = format!("\u{feff}{}\r\n", head.join(";"));

    let mut stmt = conn.prepare(
        "SELECT t.id, t.issued_at, t.number, t.kind, COALESCE(o.number, ''), t.voided_at IS NOT NULL, t.total_cents, t.payment_method,
                COALESCE(t.customer_name, ''), COALESCE(t.customer_nif, '')
         FROM tickets t LEFT JOIN tickets o ON o.id = t.refund_of
         WHERE t.issued_at >= ?1 AND t.issued_at <= ?2 AND t.training = 0 ORDER BY t.series, t.seq",
    )?;
    let tickets = stmt
        .query_map(params![from, to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, bool>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, String>(7)?,
                (r.get::<_, String>(8)?, r.get::<_, String>(9)?),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut rows = conn.prepare("SELECT vat_bp, base_cents, vat_cents FROM ticket_vat WHERE ticket_id = ?1")?;
    for (id, at, number, kind, of, voided, total, method, (customer, customer_nif)) in tickets {
        let split: BTreeMap<i64, (i64, i64)> = rows
            .query_map([id], |r| Ok((r.get::<_, i64>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))))?
            .collect::<rusqlite::Result<_>>()?;
        let full = !customer.is_empty();
        let doc = match (full, kind == "refund") {
            (false, false) => "Simplificada",
            (false, true) => "Rectificativa simplificada",
            (true, false) => "Completa",
            (true, true) => "Rectificativa",
        };
        let mut f = vec![
            datetime(&at),
            number,
            doc.into(),
            of,
            if voided { "Sí".into() } else { "No".into() },
            cell(&customer),
            customer_nif,
        ];
        for bp in &rates {
            let (base, vat) = split.get(bp).copied().unwrap_or_default();
            f.push(plain(base));
            f.push(plain(vat));
        }
        f.push(plain(total));
        f.push(method_label(&method).to_string());
        out.push_str(&f.join(";"));
        out.push_str("\r\n");
    }
    Ok(out)
}

pub fn export_vat_book(conn: &Connection, r: &Range) -> Result<PathBuf> {
    let out = vat_book(conn, r)?;
    let dir = backup::documents().join("Exportaciones");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("libro_iva_{}_{}.csv", r.from, r.to));
    fs::write(&path, out)?;
    Ok(path)
}

pub fn export_csv(conn: &Connection, r: &Range) -> Result<PathBuf> {
    let out = csv(conn, r)?;
    let dir = backup::documents().join("Exportaciones");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("ventas_{}_{}.csv", r.from, r.to));
    fs::write(&path, out)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::settings::Settings;
    use crate::state::SessionUser;
    use crate::tickets::{finalize, refund, PaymentIn, RefundIn, RefundLine, SaleLine};

    #[test]
    fn returns_net_out_of_totals_and_show_in_the_export() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let mut conn = db::open(&std::env::temp_dir().join(format!("tiquet-rep-{n}")).join("t.db")).unwrap();
        conn.execute("INSERT INTO users (name, role, pass_hash) VALUES ('Ana', 'admin', 'x')", []).unwrap();
        let user = SessionUser { id: 1, name: "Ana".into(), role: "admin".into() };
        let s = Settings::default();
        let line = |name: &str, qty, unit_cents| SaleLine { name: name.into(), qty, unit_cents, vat_bp: 2100, ..SaleLine::default() };
        let cash = PaymentIn { method: "cash".into(), tendered_cents: None };
        let sale = finalize(&mut conn, &user, &s, vec![line("Cable", 2, 690), line("=cmd|' /C calc'!A0", 1, 1000)], &cash).unwrap();
        finalize(&mut conn, &user, &s, vec![line("Funda", 1, 995)], &PaymentIn { method: "card".into(), tendered_cents: None }).unwrap();
        let back = RefundIn {
            id: sale.id,
            lines: vec![RefundLine { line_id: sale.lines[0].id.unwrap(), qty: 1 }],
            method: "cash".into(),
            reason: "roto".into(),
            clock_ok: false,
        };
        let r = refund(&mut conn, &user, &s, &back).unwrap();

        let today = sale.issued_at[..10].to_string();
        let range = Range { from: today.clone(), to: today };
        let sum = summary(&conn, &range).unwrap();
        assert_eq!((sum.tickets, sum.refunds), (2, 1));
        assert_eq!((sum.sales_cents, sum.refund_cents, sum.total_cents), (3375, -690, 2685));
        assert_eq!(sum.items, 3);
        assert_eq!(sum.first_number.as_deref(), Some(sale.number.as_str()));
        assert_eq!((sum.discounts, sum.changed), (0, 0));
        let cash = sum.by_method.iter().find(|m| m.method == "cash").unwrap();
        assert_eq!((cash.count, cash.total_cents), (1, 2380 - 690));
        assert_eq!(sum.vat.iter().map(|v| v.total_cents).sum::<i64>(), 2685);

        let out = csv(&conn, &range).unwrap();
        assert!(out.contains(&format!(";{};Devolución;{};", r.number, sale.number)), "{out}");
        assert!(out.contains(";-1;6,90;21,00;-6,90\r\n"), "{out}");
        assert!(out.contains(";'=cmd|' /C calc'!A0;"), "{out}");

        let book = vat_book(&conn, &range).unwrap();
        let lines: Vec<&str> = book.lines().collect();
        assert_eq!(lines[0].trim_start_matches('\u{feff}'), "Fecha;Factura;Tipo;Rectifica a;Anulada;Cliente;NIF cliente;Base 21%;Cuota 21%;Total;Pago");
        assert_eq!(lines.len(), 4);
        // refunds have their own series, so they follow the sales
        assert!(lines[3].contains(&format!("{};Rectificativa simplificada;{};No;;;-5,70;-1,20;-6,90;Efectivo", r.number, sale.number)), "{book}");
    }

    #[test]
    fn money_given_away_at_the_till_is_counted() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let mut conn = db::open(&std::env::temp_dir().join(format!("tiquet-rep-d-{n}")).join("t.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO users (name, role, pass_hash) VALUES ('Jordi', 'cashier', 'x');
             INSERT INTO products (name, price_cents) VALUES ('Cable', 690);",
        )
        .unwrap();
        let user = SessionUser { id: 1, name: "Jordi".into(), role: "cashier".into() };
        let line = |name: &str, qty, unit_cents, product_id| SaleLine { product_id, name: name.into(), qty, unit_cents, vat_bp: 2100, ..SaleLine::default() };
        let cash = PaymentIn { method: "cash".into(), tendered_cents: None };
        let t = finalize(&mut conn, &user, &Settings::default(), vec![line("Cable", 2, 500, Some(1)), line("Descuento", 1, -100, None)], &cash).unwrap();
        let day = t.issued_at[..10].to_string();
        let sum = summary(&conn, &Range { from: day.clone(), to: day }).unwrap();
        assert_eq!((sum.discounts, sum.discount_cents), (1, -100));
        assert_eq!((sum.changed, sum.changed_cents), (1, -380));
        let c = &sum.changes[0];
        assert_eq!((c.user_name.as_str(), c.list_cents, c.unit_cents, c.qty), ("Jordi", 690, 500, 2));
    }

    #[test]
    fn a_year_reports_by_month() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let conn = db::open(&std::env::temp_dir().join(format!("tiquet-rep-y-{n}")).join("t.db")).unwrap();
        let sum = summary(&conn, &Range { from: "2026-01-01".into(), to: "2026-12-31".into() }).unwrap();
        assert_eq!(sum.by_day.len(), 12);
        assert_eq!(sum.by_day[0].date, "2026-01");
    }
}
