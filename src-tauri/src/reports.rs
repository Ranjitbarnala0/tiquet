use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use chrono::NaiveDate;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::backup;
use crate::error::{fail, Result};
use crate::layout::{datetime, method_label};
use crate::money::split_vat;
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

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub total_cents: i64,
    pub tickets: i64,
    pub items: i64,
    pub voided: i64,
    pub first_number: Option<String>,
    pub last_number: Option<String>,
    pub by_method: Vec<MethodRow>,
    pub by_day: Vec<DayRow>,
    pub vat: Vec<VatRow>,
    pub top: Vec<TopRow>,
}

pub fn summary(conn: &Connection, r: &Range) -> Result<Summary> {
    let (from, to) = r.bounds()?;
    let mut s = Summary::default();
    let mut methods: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    let mut days: BTreeMap<String, (i64, i64)> = BTreeMap::new();

    let mut stmt = conn.prepare(
        "SELECT number, issued_at, total_cents, item_count, payment_method, voided_at IS NOT NULL
         FROM tickets WHERE issued_at >= ?1 AND issued_at <= ?2 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![from, to], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, bool>(5)?,
        ))
    })?;
    for row in rows {
        let (number, at, total, items, method, voided) = row?;
        if s.first_number.is_none() {
            s.first_number = Some(number.clone());
        }
        s.last_number = Some(number);
        if voided {
            s.voided += 1;
            continue;
        }
        s.tickets += 1;
        s.total_cents += total;
        s.items += items;
        let m = methods.entry(method).or_default();
        m.0 += 1;
        m.1 += total;
        let d = days.entry(at[..10].to_string()).or_default();
        d.0 += 1;
        d.1 += total;
    }

    // VAT is rounded per ticket, like on the paper, then summed
    let mut stmt = conn.prepare(
        "SELECT l.vat_bp, SUM(l.total_cents) FROM ticket_lines l JOIN tickets t ON t.id = l.ticket_id
         WHERE t.voided_at IS NULL AND t.issued_at >= ?1 AND t.issued_at <= ?2
         GROUP BY l.ticket_id, l.vat_bp",
    )?;
    let mut vat: BTreeMap<i64, (i64, i64, i64)> = BTreeMap::new();
    for row in stmt.query_map(params![from, to], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
        let (bp, gross) = row?;
        let (base, v) = split_vat(gross, bp);
        let e = vat.entry(bp).or_default();
        e.0 += base;
        e.1 += v;
        e.2 += gross;
    }
    s.vat = vat
        .into_iter()
        .rev()
        .map(|(bp, (b, v, g))| VatRow { vat_bp: bp, base_cents: b, vat_cents: v, total_cents: g })
        .collect();

    let mut stmt = conn.prepare(
        "SELECT l.name, SUM(l.qty), SUM(l.total_cents) FROM ticket_lines l JOIN tickets t ON t.id = l.ticket_id
         WHERE t.voided_at IS NULL AND t.issued_at >= ?1 AND t.issued_at <= ?2
         GROUP BY l.name ORDER BY 3 DESC, 2 DESC LIMIT 10",
    )?;
    s.top = stmt
        .query_map(params![from, to], |r| Ok(TopRow { name: r.get(0)?, qty: r.get(1)?, total_cents: r.get(2)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for m in ["cash", "card", "bizum"] {
        if let Some((count, total)) = methods.remove(m) {
            s.by_method.push(MethodRow { method: m.into(), count, total_cents: total });
        }
    }

    // fill empty days so a chart shows the gaps
    let start = NaiveDate::parse_from_str(&r.from, "%Y-%m-%d").unwrap();
    let end = NaiveDate::parse_from_str(&r.to, "%Y-%m-%d").unwrap();
    if (end - start).num_days() <= 92 {
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
    if s.contains([';', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// semicolons and decimal commas: opens straight into Spanish Excel
pub fn export_csv(conn: &Connection, r: &Range) -> Result<PathBuf> {
    let (from, to) = r.bounds()?;
    let dir = backup::documents().join("Exportaciones");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("ventas_{}_{}.csv", r.from, r.to));
    let mut out = String::from("\u{feff}Fecha;Ticket;Atendido por;Pago;Anulado;Producto;Uds;Precio;IVA %;Importe\r\n");
    let mut stmt = conn.prepare(
        "SELECT t.issued_at, t.number, t.user_name, t.payment_method, t.voided_at IS NOT NULL,
                l.name, l.qty, l.unit_cents, l.vat_bp, l.total_cents
         FROM tickets t JOIN ticket_lines l ON l.ticket_id = t.id
         WHERE t.issued_at >= ?1 AND t.issued_at <= ?2 ORDER BY t.id, l.pos",
    )?;
    let rows = stmt.query_map(params![from, to], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, bool>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, i64>(7)?,
            r.get::<_, i64>(8)?,
            r.get::<_, i64>(9)?,
        ))
    })?;
    for row in rows {
        let (at, number, user, method, voided, name, qty, unit, vat, total) = row?;
        let fields = [
            datetime(&at),
            number,
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
    fs::write(&path, out)?;
    Ok(path)
}
