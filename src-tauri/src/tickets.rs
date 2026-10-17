use std::collections::BTreeMap;

use chrono::{Local, NaiveDateTime, TimeDelta};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::{fail, Result};
use crate::money::split_vat;
use crate::settings::{self, Settings};
use crate::state::SessionUser;
use crate::verifactu::{self, TicketAeat};
use crate::log;

pub const MAX_PRICE: i64 = 9_999_999;
pub const MAX_CHANGE: i64 = 50_000;
// a simplified invoice may not go over 3.000 EUR (retail, RD 1619/2012 art. 4);
// above that the customer's details make it a full invoice
pub const SIMPLIFIED_LIMIT: i64 = 300_000;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SaleLine {
    #[serde(default)]
    pub product_id: Option<i64>,
    pub name: String,
    pub qty: i64,
    pub unit_cents: i64,
    pub vat_bp: i64,
    // filled in for stored tickets only: the line's id and the units of it
    // already returned by refunds that are still valid
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub refunded: i64,
    // the catalogue price when it was sold; set by the core, never the till
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_cents: Option<i64>,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl SaleLine {
    pub fn total(&self) -> i64 {
        self.qty * self.unit_cents
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
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
        // a discount line is not an article
        if l.unit_cents >= 0 {
            items += l.qty;
        }
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

fn method(m: &str) -> Result<String> {
    match m {
        "cash" | "card" | "bizum" => Ok(m.to_string()),
        _ => fail("E_INVALID:method"),
    }
}

pub fn resolve_payment(p: &PaymentIn, total: i64) -> Result<Payment> {
    let m = method(&p.method)?;
    if m != "cash" {
        return Ok(Payment { method: m, tendered_cents: total, change_cents: 0 });
    }
    let tendered = p.tendered_cents.unwrap_or(total);
    if tendered < total || tendered > 100 * MAX_PRICE {
        return fail("E_TENDERED");
    }
    // more than 500 EUR of change is a barcode scanned into the cash field,
    // not a customer
    if tendered - total > MAX_CHANGE {
        return fail("E_TENDERED_HIGH");
    }
    Ok(Payment { method: m, tendered_cents: tendered, change_cents: tendered - total })
}

// what the ticket looked like when it was issued, so a reprint next year
// still matches the original
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct ShopInfo {
    pub name: String,
    pub address: String,
    pub phone: String,
    pub nif: String,
    pub footer: String,
    pub legal_name: String,
    // id in `logos`, 0 for no logo, -1 for tickets from before logos were kept
    pub logo: i64,
    // None for tickets from before this was kept
    pub show_vat: Option<bool>,
}

impl Default for ShopInfo {
    fn default() -> Self {
        ShopInfo {
            name: String::new(),
            address: String::new(),
            phone: String::new(),
            nif: String::new(),
            footer: String::new(),
            legal_name: String::new(),
            logo: -1,
            show_vat: None,
        }
    }
}

impl ShopInfo {
    pub fn from_settings(s: &Settings) -> ShopInfo {
        ShopInfo {
            name: s.shop_name.clone(),
            address: s.shop_address.clone(),
            phone: s.shop_phone.clone(),
            nif: s.shop_nif.clone(),
            footer: s.footer.clone(),
            legal_name: s.legal_name.clone(),
            logo: -1,
            show_vat: Some(s.show_vat),
        }
    }
}

// who a full invoice is made out to
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Customer {
    pub name: String,
    pub nif: String,
    pub address: String,
}

impl Customer {
    fn checked(&self) -> Result<Customer> {
        let name = self.name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() || name.chars().count() > 120 {
            return fail("E_INVALID:customerName");
        }
        if !crate::nif::customer(&self.nif) {
            return fail("E_INVALID:customerNif");
        }
        let address: String = self.address.trim().chars().take(300).collect();
        Ok(Customer { name, nif: crate::nif::normalize(&self.nif), address })
    }
}

// the letters between the prefix and the year: P practice, F full invoice,
// R rectifying. T2026, TR2026, TF2026, TFR2026, TP2026...
pub fn series_tag(kind: &str, invoice: bool, training: bool) -> String {
    format!(
        "{}{}{}",
        if training { "P" } else { "" },
        if invoice { "F" } else { "" },
        if kind == "refund" { "R" } else { "" }
    )
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrintRow {
    pub at: String,
    pub copy: bool,
    pub status: String,
    pub detail: Option<String>,
}

// Was any earlier print of this ticket possibly on paper? Then the next one
// is a copy. Looked up and recorded under the same lock as the new attempt,
// so two prints at once cannot both come out as the original.
pub fn begin_print(conn: &Connection, ticket: i64, copy: bool) -> Result<(i64, bool)> {
    let out: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM ticket_prints WHERE ticket_id = ?1 AND status IN ('sent', 'printed', 'unknown'))",
        [ticket],
        |r| r.get(0),
    )?;
    let copy = copy || out;
    conn.execute(
        "INSERT INTO ticket_prints (ticket_id, at, copy, status) VALUES (?1, ?2, ?3, 'sent')",
        params![ticket, now(), copy],
    )?;
    Ok((conn.last_insert_rowid(), copy))
}

pub fn end_print(conn: &Connection, attempt: i64, status: &str, detail: Option<&str>) -> Result<()> {
    conn.execute("UPDATE ticket_prints SET status = ?1, detail = ?2 WHERE id = ?3", params![status, detail, attempt])?;
    Ok(())
}

// the latest attempt for any ticket: what the printer pill reports between sales
pub fn last_print(conn: &Connection) -> Result<Option<PrintRow>> {
    Ok(conn
        .query_row(
            "SELECT p.at, p.copy, p.status, p.detail FROM ticket_prints p ORDER BY p.id DESC LIMIT 1",
            [],
            |r| Ok(PrintRow { at: r.get(0)?, copy: r.get(1)?, status: r.get(2)?, detail: r.get(3)? }),
        )
        .optional()?)
}

// the logo a ticket was printed with
pub fn logo_for(conn: &Connection, t: &TicketRecord) -> Result<Option<settings::Logo>> {
    match t.shop.logo {
        -1 => settings::logo(conn),
        0 => Ok(None),
        id => settings::logo_by_id(conn, id),
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TicketRef {
    pub id: i64,
    pub number: String,
    pub issued_at: String,
    pub total_cents: i64,
    pub voided: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TicketRecord {
    pub id: i64,
    pub kind: String,
    pub number: String,
    pub issued_at: String,
    pub user_name: String,
    pub total_cents: i64,
    pub item_count: i64,
    pub payment: Payment,
    pub voided_at: Option<String>,
    pub voided_by: Option<String>,
    pub void_reason: Option<String>,
    pub refund_of: Option<TicketRef>,
    pub refund_reason: Option<String>,
    pub refunds: Vec<TicketRef>,
    pub lines: Vec<SaleLine>,
    // as stored when the ticket was issued
    pub vat: Vec<VatRow>,
    pub prints: Vec<PrintRow>,
    pub shop: ShopInfo,
    pub customer: Option<Customer>,
    pub training: bool,
    // the AEAT's QR printed on it, and how its billing record went
    pub qr: Option<String>,
    pub aeat: Option<TicketAeat>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TicketSummary {
    pub id: i64,
    pub kind: String,
    pub number: String,
    pub issued_at: String,
    pub user_name: String,
    pub total_cents: i64,
    pub item_count: i64,
    pub payment_method: String,
    pub voided: bool,
    // printed | unknown | failed | none
    pub printed: String,
    pub invoice: bool,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct ListQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub q: Option<String>,
    pub limit: Option<i64>,
    // only tickets that never reached the paper
    pub unprinted: bool,
    // practice tickets instead of real ones
    pub training: bool,
}

#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct RefundLine {
    pub line_id: i64,
    pub qty: i64,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RefundIn {
    pub id: i64,
    pub lines: Vec<RefundLine>,
    pub method: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub clock_ok: bool,
}

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default, rename_all = "camelCase")]
pub struct SaleOpts {
    // made by the till when a cart starts; a second finalize of the same cart
    // (a retry after a crash) returns the first ticket instead of a new one
    pub sale_id: Option<String>,
    // the cashier confirmed the PC clock even though it is behind the last ticket
    pub clock_ok: bool,
    // the total the cashier saw; a cart that changed underneath is refused
    pub expected_cents: Option<i64>,
    // makes it a full invoice
    pub customer: Option<Customer>,
}

pub struct RefundPlan {
    pub original: TicketRecord,
    // refund line (negative quantity) and the sale line it returns
    pub lines: Vec<(SaleLine, i64)>,
    pub totals: Totals,
    pub payment: Payment,
    pub reason: String,
}

impl RefundPlan {
    pub fn sale_lines(&self) -> Vec<SaleLine> {
        self.lines.iter().map(|(l, _)| l.clone()).collect()
    }
}

pub fn now() -> String {
    stamp(&Local::now())
}

fn stamp(at: &chrono::DateTime<Local>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S").to_string()
}

// one series per prefix, kind and year: sales T2026-000001..., refunds
// (rectifying invoices) TR2026-000001... A floor raised after restoring a
// backup keeps numbers already printed from being handed out again.
pub fn next_number(conn: &Connection, prefix: &str, tag: &str, issued_at: &str) -> Result<(String, i64, String)> {
    let series = format!("{prefix}{tag}{}", &issued_at[..4]);
    let seq: i64 = conn.query_row(
        "SELECT MAX(COALESCE((SELECT MAX(seq) FROM tickets WHERE series = ?1), 0),
                    COALESCE((SELECT seq FROM series_floor WHERE series = ?1), 0)) + 1",
        [&series],
        |r| r.get(0),
    )?;
    let number = format!("{series}-{seq:06}");
    Ok((series, seq, number))
}

// highest number used per series, in the database or as a floor
pub fn series_marks(conn: &Connection) -> Result<BTreeMap<String, i64>> {
    let mut stmt = conn.prepare(
        "SELECT series, MAX(seq) FROM (SELECT series, seq FROM tickets UNION ALL SELECT series, seq FROM series_floor)
         GROUP BY series",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
}

// Takes numbers recorded outside the database into account. Returns the
// ranges this database has no ticket for, e.g. after restoring an old copy.
pub fn raise_floors(conn: &Connection, marks: &BTreeMap<String, i64>) -> Result<Vec<(String, i64, i64)>> {
    let mut missing = Vec::new();
    for (series, seq) in marks {
        let have: i64 = conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM tickets WHERE series = ?1", [series], |r| r.get(0))?;
        conn.execute(
            "INSERT INTO series_floor (series, seq) VALUES (?1, ?2)
             ON CONFLICT(series) DO UPDATE SET seq = MAX(seq, excluded.seq)",
            params![series, seq],
        )?;
        if *seq > have {
            missing.push((series.clone(), have + 1, *seq));
        }
    }
    Ok(missing)
}

fn parse_ts(ts: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S").ok()
}

// A PC clock that went backwards (flat CMOS battery, wrong manual change)
// would date tickets before ones already issued. An hour of slack covers the
// autumn clock change. Once someone confirms the clock is right, that last
// ticket no longer blocks.
fn check_clock(conn: &Connection, issued_at: &str, confirmed: bool) -> Result<()> {
    let last: Option<String> = conn.query_row("SELECT MAX(issued_at) FROM tickets", [], |r| r.get(0))?;
    let Some(last) = last else { return Ok(()) };
    if settings::get(conn, "clockAccepted")?.is_some_and(|a| last <= a) {
        return Ok(());
    }
    let behind = match (parse_ts(issued_at), parse_ts(&last)) {
        (Some(now), Some(then)) => now < then - TimeDelta::hours(1),
        _ => false,
    };
    if !behind {
        return Ok(());
    }
    if !confirmed {
        return fail(format!("E_CLOCK:{last}"));
    }
    log::warn(format!("clock at {issued_at} is behind the last ticket ({last}); confirmed by the cashier"));
    settings::put(conn, "clockAccepted", &last)
}

pub fn clamp(lines: Vec<SaleLine>) -> Vec<SaleLine> {
    lines
        .into_iter()
        .take(500)
        .map(|l| SaleLine {
            qty: l.qty.clamp(1, 999),
            unit_cents: l.unit_cents.clamp(-MAX_PRICE, MAX_PRICE),
            vat_bp: l.vat_bp.clamp(0, 10_000),
            id: None,
            refunded: 0,
            list_cents: None,
            ..l
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
        // an open-price product whose price was never typed
        if l.unit_cents == 0 {
            return fail("E_ZERO_PRICE");
        }
        if !(0..=10_000).contains(&l.vat_bp) {
            return fail("E_INVALID:vat");
        }
        l.id = None;
        l.refunded = 0;
        l.list_cents = None;
        out.push(l);
    }
    let totals = compute(&out);
    if totals.total_cents < 0 {
        return fail("E_NEGATIVE");
    }
    // a discount bigger than everything sold at its rate would print a
    // negative taxable base for that rate
    if totals.vat.iter().any(|v| v.total_cents < 0) {
        return fail("E_NEGATIVE_VAT");
    }
    Ok(out)
}

struct NewTicket<'a> {
    kind: &'a str,
    lines: &'a [(SaleLine, Option<i64>)],
    totals: &'a Totals,
    payment: &'a Payment,
    refund_of: Option<i64>,
    reason: Option<&'a str>,
    client_id: Option<&'a str>,
    clock_ok: bool,
    customer: Option<&'a Customer>,
    training: bool,
}

// The ticket row goes in open (sealed = 0); its lines and VAT rows follow and
// then it is sealed. The triggers refuse lines for a sealed ticket, so an
// issued ticket cannot gain lines later.
fn insert(tx: &Connection, user: &SessionUser, s: &Settings, t: &NewTicket) -> Result<i64> {
    let shop = ShopInfo { logo: settings::logo_id(tx)?, ..ShopInfo::from_settings(s) };
    let shop = serde_json::to_string(&shop)?;
    // one instant for the ticket and its billing record
    let at = Local::now();
    let issued_at = stamp(&at);
    if !t.training {
        check_clock(tx, &issued_at, t.clock_ok)?;
    }
    let tag = series_tag(t.kind, t.customer.is_some(), t.training);
    let (series, seq, number) = next_number(tx, &s.ticket_prefix, &tag, &issued_at)?;
    let (totals, payment) = (t.totals, t.payment);
    let c = t.customer;
    tx.execute(
        "INSERT INTO tickets (number, series, seq, issued_at, user_id, user_name, total_cents, item_count,
                              payment_method, tendered_cents, change_cents, shop, kind, refund_of, refund_reason,
                              client_id, sealed, customer_name, customer_nif, customer_address, training)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 0, ?17, ?18, ?19, ?20)",
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
            shop,
            t.kind,
            t.refund_of,
            t.reason,
            t.client_id,
            c.map(|c| c.name.as_str()),
            c.map(|c| c.nif.as_str()),
            c.map(|c| c.address.as_str()),
            t.training
        ],
    )?;
    let id = tx.last_insert_rowid();
    for v in &totals.vat {
        tx.execute(
            "INSERT INTO ticket_vat (ticket_id, vat_bp, base_cents, vat_cents, total_cents) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, v.vat_bp, v.base_cents, v.vat_cents, v.total_cents],
        )?;
    }
    for (pos, (l, of_line)) in t.lines.iter().enumerate() {
        tx.execute(
            "INSERT INTO ticket_lines (ticket_id, pos, product_id, name, qty, unit_cents, vat_bp, total_cents, refund_of_line, list_cents)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![id, pos as i64, l.product_id, l.name, l.qty, l.unit_cents, l.vat_bp, l.total(), of_line, l.list_cents],
        )?;
        if let (Some(pid), false) = (l.product_id, t.training) {
            tx.execute(
                "UPDATE products SET sold_qty = MAX(0, sold_qty + ?1) WHERE id = ?2",
                params![l.qty, pid],
            )?;
        }
    }
    tx.execute("UPDATE tickets SET sealed = 1 WHERE id = ?1", [id])?;
    if !t.training {
        verifactu::alta(tx, id, s, &at)?;
    }
    Ok(id)
}

pub fn finalize(
    conn: &mut Connection,
    user: &SessionUser,
    s: &Settings,
    lines: Vec<SaleLine>,
    pay: &PaymentIn,
) -> Result<TicketRecord> {
    sell(conn, user, s, lines, pay, &SaleOpts::default()).map(|(t, _)| t)
}

pub fn by_sale_id(conn: &Connection, sale_id: &str) -> Result<Option<i64>> {
    Ok(conn.query_row("SELECT id FROM tickets WHERE client_id = ?1", [sale_id], |r| r.get(0)).optional()?)
}

// the bool is true when this cart had already been sold and nothing new was issued
pub fn sell(
    conn: &mut Connection,
    user: &SessionUser,
    s: &Settings,
    lines: Vec<SaleLine>,
    pay: &PaymentIn,
    opts: &SaleOpts,
) -> Result<(TicketRecord, bool)> {
    let sale_id = opts.sale_id.as_deref().map(str::trim).filter(|x| !x.is_empty());
    if sale_id.is_some_and(|x| x.len() > 64 || !x.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) {
        return fail("E_INVALID:saleId");
    }
    let mut lines = validate(lines)?;
    // the AEAT refuses any other rate, and a refused record means an invoice
    // that has to be issued again
    if verifactu::Env::of(s).is_some() && !s.training && lines.iter().any(|l| !verifactu::rate_allowed(l.vat_bp)) {
        return fail("E_VAT_AEAT");
    }
    let totals = compute(&lines);
    if opts.expected_cents.is_some_and(|e| e != totals.total_cents) {
        return fail("E_TOTAL_CHANGED");
    }
    let customer = opts.customer.as_ref().map(Customer::checked).transpose()?;
    if customer.is_none() && totals.total_cents > SIMPLIFIED_LIMIT {
        return fail("E_OVER_LIMIT");
    }
    let payment = resolve_payment(pay, totals.total_cents)?;

    // IMMEDIATE takes the write lock before reading MAX(seq), so two
    // instances can never hand out the same number
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(done) = sale_id.map(|x| by_sale_id(&tx, x)).transpose()?.flatten() {
        drop(tx);
        return Ok((load(conn, done)?, true));
    }
    // a cart can outlive its products (restored backup, another database);
    // such lines are sold as free items rather than failing the sale. Known
    // ones note the catalogue price, so a price changed at the till shows.
    for l in lines.iter_mut() {
        if let Some(pid) = l.product_id {
            let price: Option<i64> =
                tx.query_row("SELECT price_cents FROM products WHERE id = ?1", [pid], |r| r.get(0)).optional()?;
            l.list_cents = price;
            if price.is_none() {
                l.product_id = None;
            }
        }
    }
    let rows: Vec<(SaleLine, Option<i64>)> = lines.into_iter().map(|l| (l, None)).collect();
    let t = NewTicket {
        kind: "sale",
        lines: &rows,
        totals: &totals,
        payment: &payment,
        refund_of: None,
        reason: None,
        client_id: sale_id,
        clock_ok: opts.clock_ok,
        customer: customer.as_ref(),
        training: s.training,
    };
    let id = insert(&tx, user, s, &t)?;
    tx.commit()?;
    Ok((load(conn, id)?, false))
}

// Shared by the preview and the real refund so both always agree. Only units
// still on the ticket can be returned: what other refunds already took back
// is subtracted, unless that refund was voided.
pub fn plan_refund(conn: &Connection, a: &RefundIn) -> Result<RefundPlan> {
    let original = load(conn, a.id)?;
    if original.kind != "sale" || original.voided_at.is_some() {
        return fail("E_REFUND_INVALID");
    }
    let mut wanted: BTreeMap<i64, i64> = BTreeMap::new();
    for l in &a.lines {
        if l.qty < 0 {
            return fail("E_REFUND_QTY");
        }
        if l.qty > 0 {
            *wanted.entry(l.line_id).or_default() += l.qty;
        }
    }
    if wanted.is_empty() {
        return fail("E_REFUND_EMPTY");
    }
    let mut lines = Vec::new();
    for src in &original.lines {
        let Some(line_id) = src.id else { continue };
        let Some(qty) = wanted.remove(&line_id) else { continue };
        if qty > src.qty - src.refunded {
            return fail("E_REFUND_QTY");
        }
        let back = SaleLine {
            product_id: src.product_id,
            name: src.name.clone(),
            qty: -qty,
            unit_cents: src.unit_cents,
            vat_bp: src.vat_bp,
            ..SaleLine::default()
        };
        lines.push((back, line_id));
    }
    if !wanted.is_empty() {
        return fail("E_REFUND_INVALID");
    }
    let totals = compute(&lines.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>());
    if totals.total_cents >= 0 {
        return fail("E_REFUND_INVALID");
    }
    let payment = Payment { method: method(&a.method)?, tendered_cents: totals.total_cents, change_cents: 0 };
    let reason = a.reason.trim().chars().take(200).collect();
    Ok(RefundPlan { original, lines, totals, payment, reason })
}

pub fn refund(conn: &mut Connection, user: &SessionUser, s: &Settings, a: &RefundIn) -> Result<TicketRecord> {
    // planned inside the write lock: two cashiers cannot return the same unit
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let plan = plan_refund(&tx, a)?;
    // practice and real money never mix
    if plan.original.training != s.training {
        return fail("E_TRAINING");
    }
    let rows: Vec<(SaleLine, Option<i64>)> = plan.lines.iter().map(|(l, of)| (l.clone(), Some(*of))).collect();
    let t = NewTicket {
        kind: "refund",
        lines: &rows,
        totals: &plan.totals,
        payment: &plan.payment,
        refund_of: Some(plan.original.id),
        reason: (!plan.reason.is_empty()).then_some(plan.reason.as_str()),
        client_id: None,
        clock_ok: a.clock_ok,
        customer: plan.original.customer.as_ref(),
        training: plan.original.training,
    };
    let id = insert(&tx, user, s, &t)?;
    tx.commit()?;
    load(conn, id)
}

fn reference(conn: &Connection, id: i64) -> Result<Option<TicketRef>> {
    Ok(conn
        .query_row(
            "SELECT id, number, issued_at, total_cents, voided_at IS NOT NULL FROM tickets WHERE id = ?1",
            [id],
            |r| Ok(TicketRef { id: r.get(0)?, number: r.get(1)?, issued_at: r.get(2)?, total_cents: r.get(3)?, voided: r.get(4)? }),
        )
        .optional()?)
}

pub fn load(conn: &Connection, id: i64) -> Result<TicketRecord> {
    let row = conn
        .query_row(
            "SELECT id, number, issued_at, user_name, total_cents, item_count, payment_method,
                    tendered_cents, change_cents, voided_at, voided_by, void_reason, shop, kind, refund_of, refund_reason,
                    customer_name, customer_nif, customer_address, training
             FROM tickets WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    TicketRecord {
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
                        kind: r.get(13)?,
                        refund_of: None,
                        refund_reason: r.get(15)?,
                        refunds: Vec::new(),
                        lines: Vec::new(),
                        vat: Vec::new(),
                        prints: Vec::new(),
                        customer: r.get::<_, Option<String>>(16)?.map(|name| Customer {
                            name,
                            nif: r.get::<_, Option<String>>(17).ok().flatten().unwrap_or_default(),
                            address: r.get::<_, Option<String>>(18).ok().flatten().unwrap_or_default(),
                        }),
                        training: r.get(19)?,
                        qr: None,
                        aeat: None,
                    },
                    r.get::<_, Option<i64>>(14)?,
                ))
            },
        )
        .optional()?;
    let Some((mut rec, refund_of)) = row else {
        return fail("E_NOT_FOUND");
    };
    if let Some(of) = refund_of {
        rec.refund_of = reference(conn, of)?;
    }
    let mut stmt = conn.prepare(
        "SELECT id, number, issued_at, total_cents, voided_at IS NOT NULL FROM tickets WHERE refund_of = ?1 ORDER BY id",
    )?;
    rec.refunds = stmt
        .query_map([id], |r| {
            Ok(TicketRef { id: r.get(0)?, number: r.get(1)?, issued_at: r.get(2)?, total_cents: r.get(3)?, voided: r.get(4)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = conn.prepare(
        "SELECT l.id, l.product_id, l.name, l.qty, l.unit_cents, l.vat_bp,
                (SELECT COALESCE(-SUM(b.qty), 0) FROM ticket_lines b JOIN tickets t ON t.id = b.ticket_id
                  WHERE b.refund_of_line = l.id AND t.voided_at IS NULL),
                l.list_cents
         FROM ticket_lines l WHERE l.ticket_id = ?1 ORDER BY l.pos",
    )?;
    rec.lines = stmt
        .query_map([id], |r| {
            Ok(SaleLine {
                id: Some(r.get(0)?),
                product_id: r.get(1)?,
                name: r.get(2)?,
                qty: r.get(3)?,
                unit_cents: r.get(4)?,
                vat_bp: r.get(5)?,
                refunded: r.get(6)?,
                list_cents: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = conn.prepare(
        "SELECT vat_bp, base_cents, vat_cents, total_cents FROM ticket_vat WHERE ticket_id = ?1 ORDER BY vat_bp DESC",
    )?;
    rec.vat = stmt
        .query_map([id], |r| Ok(VatRow { vat_bp: r.get(0)?, base_cents: r.get(1)?, vat_cents: r.get(2)?, total_cents: r.get(3)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = conn.prepare("SELECT at, copy, status, detail FROM ticket_prints WHERE ticket_id = ?1 ORDER BY id")?;
    rec.prints = stmt
        .query_map([id], |r| Ok(PrintRow { at: r.get(0)?, copy: r.get(1)?, status: r.get(2)?, detail: r.get(3)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rec.qr = verifactu::qr_of(conn, id)?;
    rec.aeat = verifactu::ticket_state(conn, id)?;
    Ok(rec)
}

pub fn list(conn: &Connection, q: &ListQuery) -> Result<Vec<TicketSummary>> {
    let from = q.from.clone().unwrap_or_default();
    let to = q.to.as_ref().map(|t| format!("{t}T99")).unwrap_or_else(|| "9999".into());
    let needle = q.q.clone().unwrap_or_default().trim().to_uppercase();
    let amount = needle.replace(',', ".");
    let limit = q.limit.unwrap_or(300).clamp(1, 2000);
    let mut stmt = conn.prepare(
        "SELECT * FROM (
           SELECT t.id, t.kind, t.number, t.issued_at, t.user_name, t.total_cents, t.item_count, t.payment_method,
                  t.voided_at IS NOT NULL,
                  CASE WHEN EXISTS(SELECT 1 FROM ticket_prints p WHERE p.ticket_id = t.id AND p.status IN ('printed', 'sent')) THEN 'printed'
                       WHEN EXISTS(SELECT 1 FROM ticket_prints p WHERE p.ticket_id = t.id AND p.status = 'unknown') THEN 'unknown'
                       WHEN EXISTS(SELECT 1 FROM ticket_prints p WHERE p.ticket_id = t.id) THEN 'failed'
                       ELSE 'none' END AS printed,
                  t.customer_name IS NOT NULL
           FROM tickets t
           WHERE t.issued_at >= ?1 AND t.issued_at <= ?2 AND t.training = ?7
             AND (?3 = '' OR t.number LIKE '%' || ?3 || '%' OR printf('%.2f', abs(t.total_cents) / 100.0) LIKE '%' || ?4 || '%'))
         WHERE ?6 = 0 OR printed IN ('failed', 'none')
         ORDER BY id DESC LIMIT ?5",
    )?;
    let rows = stmt
        .query_map(params![from, to, needle, amount, limit, q.unprinted, q.training], |r| {
            Ok(TicketSummary {
                id: r.get(0)?,
                kind: r.get(1)?,
                number: r.get(2)?,
                issued_at: r.get(3)?,
                user_name: r.get(4)?,
                total_cents: r.get(5)?,
                item_count: r.get(6)?,
                payment_method: r.get(7)?,
                voided: r.get(8)?,
                printed: r.get(9)?,
                invoice: r.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// For a ticket issued by mistake, for a sale that never happened: a real
// sale is corrected with a return. With VERI*FACTU it leaves an anulación.
pub fn void(conn: &mut Connection, id: i64, by: &str, reason: &str) -> Result<()> {
    let reason: String = reason.trim().chars().take(200).collect();
    if reason.is_empty() {
        return fail("E_REASON");
    }
    // IMMEDIATE: the anulación reads the end of the chain it joins
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let issued: Option<String> = tx
        .query_row("SELECT issued_at FROM tickets WHERE id = ?1 AND voided_at IS NULL", [id], |r| r.get(0))
        .optional()?;
    let Some(issued) = issued else {
        return fail("E_NOT_FOUND");
    };
    // an earlier day may already be declared; it is corrected with a return
    if issued.get(..10) != now().get(..10) {
        return fail("E_VOID_OLD");
    }
    // a sale with live returns would leave those returns pointing at nothing
    let open_refunds: i64 = tx.query_row(
        "SELECT COUNT(*) FROM tickets WHERE refund_of = ?1 AND voided_at IS NULL",
        [id],
        |r| r.get(0),
    )?;
    if open_refunds > 0 {
        return fail("E_HAS_REFUNDS");
    }
    let n = tx.execute(
        "UPDATE tickets SET voided_at = ?1, voided_by = ?2, void_reason = ?3 WHERE id = ?4 AND voided_at IS NULL",
        params![now(), by, reason, id],
    )?;
    if n == 0 {
        return fail("E_NOT_FOUND");
    }
    verifactu::anulacion(&tx, id, &settings::load(&tx)?, &Local::now())?;
    tx.execute(
        "UPDATE products SET sold_qty = MAX(0, sold_qty - (
             SELECT COALESCE(SUM(qty), 0) FROM ticket_lines WHERE ticket_id = ?1 AND product_id = products.id))
         WHERE id IN (SELECT product_id FROM ticket_lines WHERE ticket_id = ?1)",
        [id],
    )?;
    tx.commit()?;
    Ok(())
}

// Training mode's "start over": practice tickets are the only ones the
// database lets go of.
pub fn wipe_training(conn: &mut Connection) -> Result<usize> {
    let tx = conn.transaction()?;
    let practice = "SELECT id FROM tickets WHERE training = 1";
    tx.execute(&format!("DELETE FROM ticket_prints WHERE ticket_id IN ({practice})"), [])?;
    tx.execute(&format!("DELETE FROM ticket_vat WHERE ticket_id IN ({practice})"), [])?;
    tx.execute(&format!("DELETE FROM ticket_lines WHERE ticket_id IN ({practice})"), [])?;
    let n = tx.execute("DELETE FROM tickets WHERE training = 1", [])?;
    tx.commit()?;
    Ok(n)
}

pub fn sample_lines(vat_bp: i64) -> Vec<SaleLine> {
    vec![
        SaleLine { name: "Cable USB-C 1 m".into(), qty: 2, unit_cents: 690, vat_bp, ..SaleLine::default() },
        SaleLine {
            name: "Protector de pantalla cristal templado".into(),
            qty: 1,
            unit_cents: 795,
            vat_bp,
            ..SaleLine::default()
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

    fn rand_suffix() -> String {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let k = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        format!("{n}-{k}")
    }

    fn line(name: &str, qty: i64, unit: i64) -> SaleLine {
        SaleLine { name: name.into(), qty, unit_cents: unit, vat_bp: 2100, ..SaleLine::default() }
    }

    fn cash(t: Option<i64>) -> PaymentIn {
        PaymentIn { method: "cash".into(), tendered_cents: t }
    }

    fn back(id: i64, lines: &[(i64, i64)]) -> RefundIn {
        RefundIn {
            id,
            lines: lines.iter().map(|&(line_id, qty)| RefundLine { line_id, qty }).collect(),
            method: "cash".into(),
            reason: "devuelve".into(),
            clock_ok: false,
        }
    }

    // a ticket from another day, written the way the app writes them
    fn old_ticket(conn: &Connection, issued_at: &str) -> i64 {
        conn.execute(
            "INSERT INTO tickets (number, series, seq, issued_at, user_id, user_name, total_cents, item_count,
                                  payment_method, tendered_cents, change_cents, shop, sealed)
             VALUES ('T' || substr(?1, 1, 4) || '-000001', 'T' || substr(?1, 1, 4), 1, ?1, 1, 'Ana', 1380, 2,
                     'card', 1380, 0, '{}', 0)",
            [issued_at],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO ticket_lines (ticket_id, pos, name, qty, unit_cents, vat_bp, total_cents)
             VALUES (?1, 0, 'Cable', 2, 690, 2100, 1380)",
            [id],
        )
        .unwrap();
        conn.execute("UPDATE tickets SET sealed = 1 WHERE id = ?1", [id]).unwrap();
        id
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
        lines.push(line("Descuento", 1, -300));
        assert_eq!(compute(&lines).item_count, 4);
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
        assert!(conn.execute("UPDATE tickets SET kind = 'refund' WHERE id = ?1", [a.id]).is_err());
        assert!(conn.execute("UPDATE ticket_lines SET qty = 9", []).is_err());
        assert!(conn.execute("DELETE FROM ticket_lines", []).is_err());

        assert_eq!(void(&mut conn, a.id, "Ana", "  ").unwrap_err().0, "E_REASON");
        void(&mut conn, a.id, "Ana", "error").unwrap();
        assert!(load(&conn, a.id).unwrap().voided_at.is_some());
        assert_eq!(void(&mut conn, a.id, "Ana", "again").unwrap_err().0, "E_NOT_FOUND");
    }

    #[test]
    fn earlier_days_are_corrected_with_returns_not_voids() {
        let (mut conn, user, s) = setup();
        old_ticket(&conn, "2025-12-30T10:00:00");
        assert_eq!(void(&mut conn, 1, "Ana", "error").unwrap_err().0, "E_VOID_OLD");
        let line = load(&conn, 1).unwrap().lines[0].id.unwrap();
        let r = refund(&mut conn, &user, &s, &RefundIn { method: "card".into(), ..back(1, &[(line, 2)]) }).unwrap();
        assert_eq!(r.total_cents, -1380);
        assert_eq!(r.payment.method, "card");
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
        assert_eq!(err(vec![line("x", 1, 0)], cash(None)), "E_ZERO_PRICE");
        // an EAN typed into the cash field as euros
        assert_eq!(err(vec![line("x", 1, 500)], cash(Some(4_006_381))), "E_TENDERED_HIGH");
        let mixed = vec![SaleLine { vat_bp: 1000, ..line("Pila", 1, 1000) }, line("Funda", 1, 500), line("Descuento", 1, -600)];
        assert_eq!(err(mixed, cash(None)), "E_NEGATIVE_VAT");
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn the_catalogue_price_is_kept_and_cannot_be_sent_by_the_till() {
        let (mut conn, user, s) = setup();
        let l = SaleLine { product_id: Some(1), list_cents: Some(1), ..line("Cable", 2, 500) };
        let t = finalize(&mut conn, &user, &s, vec![l], &cash(None)).unwrap();
        assert_eq!(t.lines[0].list_cents, Some(690));
        assert_eq!(t.lines[0].unit_cents, 500);
    }

    #[test]
    fn unknown_product_ids_sell_as_free_items() {
        let (mut conn, user, s) = setup();
        let l = SaleLine { product_id: Some(999), ..line("Funda vieja", 1, 1250) };
        let t = finalize(&mut conn, &user, &s, vec![l], &cash(None)).unwrap();
        assert_eq!(t.lines[0].product_id, None);
        assert_eq!(t.total_cents, 1250);
    }

    #[test]
    fn product_sales_counted_and_unwound_on_void() {
        let (mut conn, user, s) = setup();
        let l = SaleLine { product_id: Some(1), ..line("Cable", 3, 690) };
        let t = finalize(&mut conn, &user, &s, vec![l], &PaymentIn { method: "card".into(), tendered_cents: None }).unwrap();
        let sold = |c: &Connection| c.query_row("SELECT sold_qty FROM products WHERE id = 1", [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(sold(&conn), 3);
        void(&mut conn, t.id, "Ana", "error").unwrap();
        assert_eq!(sold(&conn), 0);
    }

    #[test]
    fn partial_returns_never_exceed_what_was_sold() {
        let (mut conn, user, s) = setup();
        let sale = finalize(
            &mut conn,
            &user,
            &s,
            vec![SaleLine { product_id: Some(1), ..line("Cable", 3, 690) }, line("Funda", 1, 995)],
            &cash(None),
        )
        .unwrap();
        let (cable, funda) = (sale.lines[0].id.unwrap(), sale.lines[1].id.unwrap());

        let r1 = refund(&mut conn, &user, &s, &back(sale.id, &[(cable, 2)])).unwrap();
        let year = &r1.issued_at[..4];
        assert_eq!(r1.number, format!("TR{year}-000001"));
        assert_eq!(r1.kind, "refund");
        assert_eq!(r1.total_cents, -1380);
        assert_eq!(r1.lines[0].qty, -2);
        assert_eq!(r1.refund_of.as_ref().unwrap().number, sale.number);
        assert_eq!(r1.refund_reason.as_deref(), Some("devuelve"));

        let after = load(&conn, sale.id).unwrap();
        assert_eq!(after.lines[0].refunded, 2);
        assert_eq!(after.refunds.len(), 1);

        let err = |c: &mut Connection, r: RefundIn| refund(c, &user, &s, &r).unwrap_err().0;
        assert_eq!(err(&mut conn, back(sale.id, &[(cable, 2)])), "E_REFUND_QTY");
        assert_eq!(err(&mut conn, back(sale.id, &[])), "E_REFUND_EMPTY");
        assert_eq!(err(&mut conn, back(sale.id, &[(9999, 1)])), "E_REFUND_INVALID");
        assert_eq!(err(&mut conn, back(r1.id, &[(cable, 1)])), "E_REFUND_INVALID");

        // the last cable and the case can still go back
        refund(&mut conn, &user, &s, &back(sale.id, &[(cable, 1), (funda, 1)])).unwrap();
        assert_eq!(err(&mut conn, back(sale.id, &[(funda, 1)])), "E_REFUND_QTY");
        let sold: i64 = conn.query_row("SELECT sold_qty FROM products WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(sold, 0);

        // a sale with live returns cannot be voided; voiding the return frees the units
        assert_eq!(void(&mut conn, sale.id, "Ana", "error").unwrap_err().0, "E_HAS_REFUNDS");
        void(&mut conn, r1.id, "Ana", "error").unwrap();
        assert_eq!(load(&conn, sale.id).unwrap().lines[0].refunded, 1);
        let sold: i64 = conn.query_row("SELECT sold_qty FROM products WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(sold, 2);
    }

    #[test]
    fn issued_tickets_cannot_grow_lines_or_be_unvoided() {
        let (mut conn, user, s) = setup();
        let t = finalize(&mut conn, &user, &s, vec![line("Cable", 2, 690)], &cash(None)).unwrap();
        assert_eq!(t.vat, vec![VatRow { vat_bp: 2100, base_cents: 1140, vat_cents: 240, total_cents: 1380 }]);
        let add = "INSERT INTO ticket_lines (ticket_id, pos, name, qty, unit_cents, vat_bp, total_cents) VALUES (?1, 1, 'x', 1, 100, 0, 100)";
        assert!(conn.execute(add, [t.id]).is_err());
        assert!(conn.execute("UPDATE tickets SET sealed = 0 WHERE id = ?1", [t.id]).is_err());
        assert!(conn.execute("UPDATE tickets SET id = 99 WHERE id = ?1", [t.id]).is_err());
        assert!(conn.execute("DELETE FROM ticket_vat", []).is_err());
        void(&mut conn, t.id, "Ana", "error").unwrap();
        assert!(conn.execute("UPDATE tickets SET voided_at = NULL WHERE id = ?1", [t.id]).is_err());
    }

    #[test]
    fn the_same_cart_is_never_sold_twice() {
        let (mut conn, user, s) = setup();
        let opts = SaleOpts { sale_id: Some("c0ffee-01".into()), ..SaleOpts::default() };
        let (a, dup) = sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &opts).unwrap();
        assert!(!dup);
        let (b, dup) = sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &opts).unwrap();
        assert!(dup);
        assert_eq!(a.id, b.id);
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let bad = SaleOpts { sale_id: Some("x'; DROP".into()), ..SaleOpts::default() };
        let stale = SaleOpts { expected_cents: Some(1000), ..SaleOpts::default() };
        assert_eq!(sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &stale).unwrap_err().0, "E_TOTAL_CHANGED");
        assert_eq!(sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &bad).unwrap_err().0, "E_INVALID:saleId");
    }

    #[test]
    fn numbers_survive_a_restored_backup_and_a_clock_gone_back() {
        let (mut conn, user, s) = setup();
        for _ in 0..2 {
            finalize(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None)).unwrap();
        }
        let year = now()[..4].to_string();
        let series = format!("T{year}");
        // the file outside the database says 7 were printed before the restore
        let missing = raise_floors(&conn, &BTreeMap::from([(series.clone(), 7)])).unwrap();
        assert_eq!(missing, vec![(series.clone(), 3, 7)]);
        let t = finalize(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None)).unwrap();
        assert_eq!(t.number, format!("{series}-000008"));
        assert_eq!(series_marks(&conn).unwrap()[&series], 8);
        assert!(conn.execute("UPDATE series_floor SET seq = 1", []).is_err());

        // a ticket dated next year means this clock is behind
        let next = format!("{}-01-02T09:00:00", year.parse::<i32>().unwrap() + 1);
        old_ticket(&conn, &next);
        let err = finalize(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None)).unwrap_err().0;
        assert_eq!(err, format!("E_CLOCK:{next}"));
        let ok = SaleOpts { clock_ok: true, ..SaleOpts::default() };
        sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &ok).unwrap();
        // confirmed once, the next sale goes through without asking again
        finalize(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None)).unwrap();
    }

    #[test]
    fn only_the_first_print_that_may_have_reached_paper_is_the_original() {
        let (mut conn, user, s) = setup();
        let t = finalize(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None)).unwrap();
        // the printer was unplugged: nothing came out, the retry is still the original
        let (a, copy) = begin_print(&conn, t.id, false).unwrap();
        assert!(!copy);
        end_print(&conn, a, "failed", Some("E_PRINT:offline")).unwrap();
        let (b, copy) = begin_print(&conn, t.id, false).unwrap();
        assert!(!copy);
        // while that one is out, a second request can only be a copy
        let (_, copy) = begin_print(&conn, t.id, false).unwrap();
        assert!(copy);
        end_print(&conn, b, "printed", None).unwrap();
        assert!(begin_print(&conn, t.id, false).unwrap().1);
        assert_eq!(load(&conn, t.id).unwrap().prints.len(), 4);
        let unprinted = ListQuery { unprinted: true, ..ListQuery::default() };
        assert!(list(&conn, &unprinted).unwrap().is_empty());
        let other = finalize(&mut conn, &user, &s, vec![line("Funda", 1, 995)], &cash(None)).unwrap();
        assert_eq!(list(&conn, &unprinted).unwrap()[0].id, other.id);
    }

    #[test]
    fn a_customer_makes_it_a_full_invoice_in_its_own_series() {
        let (mut conn, user, s) = setup();
        let year = now()[..4].to_string();
        let big = vec![line("Portátil", 1, 350_000)];
        assert_eq!(finalize(&mut conn, &user, &s, big.clone(), &cash(None)).unwrap_err().0, "E_OVER_LIMIT");
        let bad = SaleOpts { customer: Some(Customer { name: "Acme SL".into(), nif: "B12345678".into(), address: String::new() }), ..SaleOpts::default() };
        assert_eq!(sell(&mut conn, &user, &s, big.clone(), &cash(None), &bad).unwrap_err().0, "E_INVALID:customerNif");
        let acme = Customer { name: "  Acme   SL ".into(), nif: "b-12345674".into(), address: "Calle 1, Girona".into() };
        let opts = SaleOpts { customer: Some(acme), ..SaleOpts::default() };
        let (t, _) = sell(&mut conn, &user, &s, big, &cash(None), &opts).unwrap();
        assert_eq!(t.number, format!("TF{year}-000001"));
        let c = t.customer.as_ref().unwrap();
        assert_eq!((c.name.as_str(), c.nif.as_str()), ("Acme SL", "B12345674"));
        // its return is a full rectifying invoice, made out to the same customer
        let r = refund(&mut conn, &user, &s, &back(t.id, &[(t.lines[0].id.unwrap(), 1)])).unwrap();
        assert_eq!(r.number, format!("TFR{year}-000001"));
        assert_eq!(r.customer, t.customer);
    }

    #[test]
    fn practice_sales_never_count_and_can_be_wiped() {
        let (mut conn, user, s) = setup();
        let real = finalize(&mut conn, &user, &s, vec![SaleLine { product_id: Some(1), ..line("Cable", 1, 690) }], &cash(None)).unwrap();
        let practice = Settings { training: true, ..s.clone() };
        let t = finalize(&mut conn, &user, &practice, vec![SaleLine { product_id: Some(1), ..line("Cable", 2, 690) }], &cash(None)).unwrap();
        assert!(t.training && t.number.starts_with("TP"));
        let sold: i64 = conn.query_row("SELECT sold_qty FROM products WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(sold, 1);
        let mine = |training| ListQuery { training, ..ListQuery::default() };
        assert_eq!(list(&conn, &mine(false)).unwrap().len(), 1);
        assert_eq!(list(&conn, &mine(true)).unwrap().len(), 1);
        // a real sale cannot be refunded while practising, nor the other way round
        assert_eq!(refund(&mut conn, &user, &practice, &back(real.id, &[(real.lines[0].id.unwrap(), 1)])).unwrap_err().0, "E_TRAINING");
        refund(&mut conn, &user, &practice, &back(t.id, &[(t.lines[0].id.unwrap(), 1)])).unwrap();
        assert_eq!(wipe_training(&mut conn).unwrap(), 2);
        assert!(conn.execute("DELETE FROM tickets WHERE id = ?1", [real.id]).is_err());
        assert_eq!(list(&conn, &mine(false)).unwrap().len(), 1);
    }

    #[test]
    fn voided_sales_cannot_be_returned() {
        let (mut conn, user, s) = setup();
        let sale = finalize(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None)).unwrap();
        void(&mut conn, sale.id, "Ana", "error").unwrap();
        let r = back(sale.id, &[(sale.lines[0].id.unwrap(), 1)]);
        assert_eq!(refund(&mut conn, &user, &s, &r).unwrap_err().0, "E_REFUND_INVALID");
    }
}
