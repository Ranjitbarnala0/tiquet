use std::collections::BTreeMap;

use chrono::{Local, NaiveDateTime, TimeDelta};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::{fail, Result};
use crate::money::split_vat;
use crate::settings::{self, Settings};
use crate::state::SessionUser;
use crate::verifactu::{self, TicketAeat};
use crate::{log, repairs};

pub const MAX_PRICE: i64 = 9_999_999;
pub const MAX_CHANGE: i64 = 50_000;
// a simplified invoice may not go over 3.000 EUR (retail, RD 1619/2012 art. 4);
// above that the customer's details make it a full invoice
pub const SIMPLIFIED_LIMIT: i64 = 300_000;
// a repair is a service, not a retail sale: its limit is the general 400 EUR
pub const SERVICE_LIMIT: i64 = 40_000;

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
    // the repair this line delivers, or the one it is the deposit for
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair_id: Option<i64>,
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
    // the machine a card went through, when the shop has more than one
    #[serde(default)]
    pub machine: Option<i64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Payment {
    pub method: String,
    pub tendered_cents: i64,
    pub change_cents: i64,
    pub machine: Option<i64>,
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
        return Ok(Payment { method: m, tendered_cents: total, change_cents: 0, machine: p.machine });
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
    Ok(Payment { method: m, tendered_cents: tendered, change_cents: tendered - total, machine: None })
}

// How much a ticket or a bill says about VAT. The amounts are the same in
// all three; only the paper changes.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VatView {
    // base, rate and VAT for each rate, and each price without VAT
    Full,
    // only the words "IVA incluido" and the rate
    Note,
    // nothing: items, prices and the total
    None,
}

impl VatView {
    // what Ajustes > Ticket says for the receipts of Venta
    pub fn of(s: &Settings) -> VatView {
        match (s.show_vat, s.vat_note) {
            (true, _) => VatView::Full,
            (false, true) => VatView::Note,
            (false, false) => VatView::None,
        }
    }

    // a bill made in Facturas shows it all or nothing
    pub fn bill(shown: bool) -> VatView {
        if shown {
            VatView::Full
        } else {
            VatView::None
        }
    }
}

// what the ticket looked like when it was issued, so a reprint next year
// still matches the original
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct ShopInfo {
    pub name: String,
    pub tagline: String,
    pub address: String,
    pub phone: String,
    pub email: String,
    pub website: String,
    pub nif: String,
    pub footer: String,
    pub legal_name: String,
    pub header_note: String,
    // id in `logos`, 0 for no logo, -1 for tickets from before logos were kept
    pub logo: i64,
    // None for tickets from before this was kept
    pub show_vat: Option<bool>,
    // since 1.13, all of it; None for tickets from before, which follow
    // show_vat and always show the VAT of an invoice with a customer
    pub vat: Option<VatView>,
    // what the ticket shows of the shop; None (tickets before 1.6) shows it
    pub show_nif: Option<bool>,
    pub show_address: Option<bool>,
    pub show_phone: Option<bool>,
    pub show_cashier: Option<bool>,
    pub show_legal: Option<bool>,
    pub show_tagline: Option<bool>,
    pub show_email: Option<bool>,
    pub show_website: Option<bool>,
    pub legal_bottom: Option<bool>,
}

impl Default for ShopInfo {
    fn default() -> Self {
        ShopInfo {
            name: String::new(),
            tagline: String::new(),
            address: String::new(),
            phone: String::new(),
            email: String::new(),
            website: String::new(),
            nif: String::new(),
            footer: String::new(),
            legal_name: String::new(),
            header_note: String::new(),
            logo: -1,
            show_vat: None,
            vat: None,
            show_nif: None,
            show_address: None,
            show_phone: None,
            show_cashier: None,
            show_legal: None,
            show_tagline: None,
            show_email: None,
            show_website: None,
            legal_bottom: None,
        }
    }
}

impl ShopInfo {
    pub fn from_settings(s: &Settings) -> ShopInfo {
        Self::from_settings_for(s, false)
    }

    // Legal bills can carry a separate billing company identity when configured,
    // show both legal entity + storefront trade name, or stick to the shop's own details.
    pub fn from_settings_for(s: &Settings, for_bill: bool) -> ShopInfo {
        let has_bill = for_bill
            && s.bill_identity_mode != "shop"
            && [&s.bill_company_name, &s.bill_legal_name, &s.bill_nif, &s.bill_address, &s.bill_phone, &s.bill_email, &s.bill_website]
                .iter()
                .any(|x| !x.trim().is_empty());
        let pick = |bill: &str, shop: &str| if has_bill && !bill.trim().is_empty() { bill.trim().to_string() } else { shop.to_string() };
        let name = if has_bill && !s.bill_company_name.trim().is_empty() {
            s.bill_company_name.trim().to_string()
        } else if has_bill && s.bill_identity_mode == "separate" && !s.bill_legal_name.trim().is_empty() {
            s.bill_legal_name.trim().to_string()
        } else {
            s.shop_name.clone()
        };
        let legal_name = if !has_bill {
            s.legal_name.clone()
        } else if s.bill_identity_mode == "both" {
            let bill_legal = if !s.bill_legal_name.trim().is_empty() {
                s.bill_legal_name.trim()
            } else {
                s.bill_company_name.trim()
            };
            let shop = s.shop_name.trim();
            match (
                bill_legal.is_empty() || bill_legal.eq_ignore_ascii_case(&name),
                shop.is_empty() || shop.eq_ignore_ascii_case(&name),
            ) {
                (true, true) => s.legal_name.clone(),
                (false, true) => bill_legal.to_string(),
                (true, false) => format!("Establecimiento: {shop}"),
                (false, false) => format!("{bill_legal} · {shop}"),
            }
        } else if !s.bill_legal_name.trim().is_empty() && !s.bill_legal_name.trim().eq_ignore_ascii_case(&name) {
            s.bill_legal_name.trim().to_string()
        } else if !s.bill_company_name.trim().is_empty() {
            String::new()
        } else {
            s.legal_name.clone()
        };
        ShopInfo {
            name,
            tagline: s.shop_tagline.clone(),
            address: pick(&s.bill_address, &s.shop_address),
            phone: pick(&s.bill_phone, &s.shop_phone),
            email: pick(&s.bill_email, &s.shop_email),
            website: pick(&s.bill_website, &s.shop_website),
            nif: pick(&s.bill_nif, &s.shop_nif),
            footer: s.footer.clone(),
            legal_name,
            header_note: s.ticket_header_note.clone(),
            logo: -1,
            show_vat: Some(s.show_vat),
            vat: Some(VatView::of(s)),
            show_nif: Some(s.ticket_nif),
            show_address: Some(s.ticket_address),
            show_phone: Some(s.ticket_phone),
            show_cashier: Some(s.ticket_cashier),
            show_legal: Some(s.ticket_legal_name),
            show_tagline: Some(s.ticket_tagline),
            show_email: Some(s.ticket_email),
            show_website: Some(s.ticket_website),
            legal_bottom: Some(s.ticket_legal_bottom),
        }
    }
}

impl ShopInfo {
    // How this ticket shows its VAT on a receipt. `table` is what the PC's
    // settings say for tickets from before 1.13, which kept less.
    pub fn vat_view(&self, customer: bool, table: bool) -> VatView {
        match self.vat {
            Some(v) => v,
            None if customer || self.show_vat.unwrap_or(table) => VatView::Full,
            None => VatView::Note,
        }
    }
}

// who an invoice or ticket with customer details is made out to; below the
// simplified limit any field is optional, while a full fiscal invoice has both
// name and NIF
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Customer {
    pub name: String,
    pub nif: String,
    pub address: String,
    pub phone: String,
    pub email: String,
}

impl Customer {
    pub fn is_empty(&self) -> bool {
        self.name.trim().is_empty()
            && self.nif.trim().is_empty()
            && self.address.trim().is_empty()
            && self.phone.trim().is_empty()
            && self.email.trim().is_empty()
    }

    pub fn is_fiscal(&self) -> bool {
        !self.name.trim().is_empty() && !self.nif.trim().is_empty()
    }

    pub fn cleaned(&self) -> Customer {
        Customer {
            name: self.name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect(),
            nif: if self.nif.trim().is_empty() { String::new() } else { crate::nif::normalize(&self.nif) },
            address: self.address.trim().chars().take(300).collect(),
            phone: self.phone.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(30).collect(),
            email: self.email.trim().chars().take(120).collect(),
        }
    }

    fn checked_for(&self, require_fiscal: bool) -> Result<Option<Customer>> {
        let c = self.cleaned();
        if c.is_empty() && !require_fiscal {
            return Ok(None);
        }
        if require_fiscal && c.name.is_empty() {
            return fail("E_INVALID:customerName");
        }
        if (!c.nif.is_empty() || require_fiscal) && !crate::nif::customer(&c.nif) {
            return fail("E_INVALID:customerNif");
        }
        Ok(Some(c))
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

// What another PC of the shop could not print: the ticket's latest print is
// what really happened there. A print that may be partly on paper stays unknown.
pub fn print_failed(conn: &Connection, number: &str, detail: &str) -> Result<()> {
    let status = if detail == "E_PRINT:timeout" || detail.starts_with("E_PRINT:partial") { "unknown" } else { "failed" };
    conn.execute(
        "UPDATE ticket_prints SET status = ?2, detail = ?3
         WHERE id = (SELECT MAX(p.id) FROM ticket_prints p JOIN tickets t ON t.id = p.ticket_id WHERE t.number = ?1)",
        params![number, status, detail],
    )?;
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
    shop_logo(conn, &t.shop)
}

pub fn shop_logo(conn: &Connection, shop: &ShopInfo) -> Result<Option<settings::Logo>> {
    match shop.logo {
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
    pub payment_changes: Vec<PaymentChange>,
    pub shop: ShopInfo,
    pub customer: Option<Customer>,
    pub training: bool,
    // the AEAT's QR printed on it, and how its billing record went
    pub qr: Option<String>,
    pub aeat: Option<TicketAeat>,
    // made in Facturas
    pub bill: bool,
    // a correction: the number it took the place of, or the one that took
    // its place (this one is then voided)
    pub replaces: Option<String>,
    pub replaced_by: Option<String>,
    // free words printed on a bill
    pub note: Option<String>,
}

// how a ticket's payment was put right
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PaymentChange {
    pub at: String,
    pub by_name: String,
    pub from: String,
    pub to: String,
    pub from_machine: Option<i64>,
    pub to_machine: Option<i64>,
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
    // the ticket this sale corrects: it is voided in the same step
    pub replaces: Option<i64>,
    // made in Facturas: it is listed there, customer or not
    pub bill: bool,
    // chosen on the bill: its paper shows the VAT in full, or not at all.
    // Left out, the receipts of Venta follow Ajustes and an invoice with a
    // customer shows it.
    pub show_vat: Option<bool>,
    // free words printed on the bill
    pub note: Option<String>,
    // the corrected ticket may be from another day: only the owner, and only
    // while VERI*FACTU is off; never taken from the request
    #[serde(skip)]
    pub any_day: bool,
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

// highest number used per series, tickets and repairs, in the database or as a floor
pub fn series_marks(conn: &Connection) -> Result<BTreeMap<String, i64>> {
    let mut stmt = conn.prepare(
        "SELECT series, MAX(seq) FROM (SELECT series, seq FROM tickets UNION ALL SELECT series, seq FROM repairs
                                       UNION ALL SELECT series, seq FROM series_floor)
         GROUP BY series",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
}

// Takes numbers recorded outside the database into account. Returns the
// ranges this database has no ticket or repair for, e.g. after restoring an
// old copy.
pub fn raise_floors(conn: &Connection, marks: &BTreeMap<String, i64>) -> Result<Vec<(String, i64, i64)>> {
    let mut missing = Vec::new();
    for (series, seq) in marks {
        let have: i64 = conn.query_row(
            "SELECT MAX(COALESCE((SELECT MAX(seq) FROM tickets WHERE series = ?1), 0),
                        COALESCE((SELECT MAX(seq) FROM repairs WHERE series = ?1), 0))",
            [series],
            |r| r.get(0),
        )?;
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
    bill: bool,
    replaces: Option<i64>,
    at: Option<&'a str>,
    // None: an invoice with a customer shows its VAT, a receipt follows Ajustes
    vat: Option<VatView>,
    note: Option<&'a str>,
}

// The ticket row goes in open (sealed = 0); its lines and VAT rows follow and
// then it is sealed. The triggers refuse lines for a sealed ticket, so an
// issued ticket cannot gain lines later.
fn insert(tx: &Connection, user: &SessionUser, s: &Settings, t: &NewTicket) -> Result<i64> {
    let fiscal = t.customer.is_some_and(Customer::is_fiscal);
    let is_bill = t.bill || fiscal;
    let mut shop = ShopInfo { logo: settings::logo_id(tx)?, ..ShopInfo::from_settings_for(s, is_bill) };
    let view = t.vat.unwrap_or(if fiscal { VatView::Full } else { VatView::of(s) });
    shop.vat = Some(view);
    shop.show_vat = Some(view == VatView::Full);
    let shop = serde_json::to_string(&shop)?;
    // one instant for the ticket and its billing record
    let at = Local::now();
    let issued_at = t.at.map_or_else(|| stamp(&at), str::to_string);
    if !t.training && t.at.is_none() {
        check_clock(tx, &issued_at, t.clock_ok)?;
    }
    let tag = series_tag(t.kind, fiscal, t.training);
    let (series, seq, number) = next_number(tx, &s.ticket_prefix, &tag, &issued_at)?;
    let (totals, payment) = (t.totals, t.payment);
    let c = t.customer;
    let machine = s.paid_on(&payment.method, payment.machine);
    tx.execute(
        "INSERT INTO tickets (number, series, seq, issued_at, user_id, user_name, total_cents, item_count,
                              payment_method, tendered_cents, change_cents, shop, kind, refund_of, refund_reason,
                              client_id, sealed, customer_name, customer_nif, customer_address, customer_phone, customer_email, training,
                              machine_id, machine_cost_cents, bill, replaces, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 0, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27)",
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
            c.map(|c| c.phone.as_str()),
            c.map(|c| c.email.as_str()),
            t.training,
            machine.map(|m| m.id),
            machine.map_or(0, |m| m.sale_cost(totals.total_cents)),
            t.bill,
            t.replaces,
            t.note,
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
            "INSERT INTO ticket_lines (ticket_id, pos, product_id, name, qty, unit_cents, vat_bp, total_cents, refund_of_line, list_cents, repair_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![id, pos as i64, l.product_id, l.name, l.qty, l.unit_cents, l.vat_bp, l.total(), of_line, l.list_cents, l.repair_id],
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

// a sale that passed every check and knows its totals, not yet written
pub struct Checked {
    lines: Vec<SaleLine>,
    totals: Totals,
    payment: Payment,
    customer: Option<Customer>,
    bill: bool,
    replaces: Option<i64>,
    // a correction issued as of the sale it corrects; None is now
    at: Option<String>,
    vat: Option<VatView>,
    note: Option<String>,
}

// free text keeps its line breaks, not its runs of blank space: at most 6
// lines and 400 letters
pub fn note_of(s: Option<&str>) -> Option<String> {
    let lines: Vec<String> = s.unwrap_or_default().lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).take(6).collect();
    let text: String = lines.join("\n").chars().take(400).collect();
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

pub fn check(s: &Settings, lines: Vec<SaleLine>, pay: &PaymentIn, opts: &SaleOpts) -> Result<Checked> {
    let lines = validate(lines)?;
    // the AEAT refuses any other rate, and a refused record means an invoice
    // that has to be issued again
    if verifactu::Env::of(s).is_some() && !s.training && lines.iter().any(|l| !verifactu::rate_allowed(l.vat_bp)) {
        return fail("E_VAT_AEAT");
    }
    let totals = compute(&lines);
    if opts.expected_cents.is_some_and(|e| e != totals.total_cents) {
        return fail("E_TOTAL_CHANGED");
    }
    let over_retail = totals.total_cents > SIMPLIFIED_LIMIT;
    let over_service = totals.total_cents > SERVICE_LIMIT && lines.iter().any(|l| l.repair_id.is_some());
    let customer = opts.customer.as_ref().map(|c| c.checked_for(over_retail || over_service)).transpose()?.flatten();
    let fiscal = customer.as_ref().is_some_and(Customer::is_fiscal);
    let is_bill = opts.bill || fiscal;
    // a full fiscal invoice made out to a customer names the issuer's NIF
    if fiscal && ShopInfo::from_settings_for(s, is_bill).nif.trim().is_empty() {
        return fail("E_SHOP_NIF");
    }
    if !fiscal && over_retail {
        return fail("E_OVER_LIMIT");
    }
    if !fiscal && over_service {
        return fail("E_OVER_LIMIT_SERVICE");
    }
    let payment = resolve_payment(pay, totals.total_cents)?;
    let (vat, note) = (opts.show_vat.map(VatView::bill), note_of(opts.note.as_deref()));
    Ok(Checked { lines, totals, payment, customer, bill: opts.bill, replaces: opts.replaces, at: None, vat, note })
}

// Writes a checked sale. The caller holds an IMMEDIATE transaction, which
// takes the write lock before MAX(seq) is read, so two instances can never
// hand out the same number.
pub fn write_sale(tx: &Connection, user: &SessionUser, s: &Settings, c: Checked, sale_id: Option<&str>, clock_ok: bool) -> Result<i64> {
    let Checked { mut lines, totals, payment, customer, bill, replaces, at, vat, note } = c;
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
        clock_ok,
        customer: customer.as_ref(),
        training: s.training,
        bill,
        replaces,
        at: at.as_deref(),
        vat,
        note: note.as_deref(),
    };
    insert(tx, user, s, &t)
}

// the bool is true when this cart had already been sold and nothing new was
// issued. A line with a repair on it delivers that repair.
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
    let mut lines = lines;
    repairs::canon(conn, &mut lines)?;
    let mut c = check(s, lines, pay, opts)?;
    let delivers = c.lines.iter().any(|l| l.repair_id.is_some());
    // a practice sale must not hand a real phone back
    if delivers && s.training {
        return fail("E_REPAIR_TRAINING");
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(done) = sale_id.map(|x| by_sale_id(&tx, x)).transpose()?.flatten() {
        drop(tx);
        let was = load(conn, done)?;
        // The same cart sent twice (a retry after a lost answer) gets its
        // ticket back. Another cart under that id, or the same one after its
        // ticket was deleted, is another sale whose screen kept the id of
        // one it never heard was saved: it is told which, takes a new id and
        // is charged, instead of vanishing as "already charged".
        let same = was.voided_at.is_none()
            && was.lines.len() == c.lines.len()
            && was.lines.iter().zip(&c.lines).all(|(a, b)| a.name == b.name && a.qty == b.qty && a.unit_cents == b.unit_cents && a.vat_bp == b.vat_bp);
        if !same {
            return fail(format!("E_SALE_REUSED:{}", was.number));
        }
        return Ok((was, true));
    }
    if let Some(old) = opts.replaces {
        fixable(&tx, old, s)?;
    }
    c.at = correction_time(&tx, s, opts.replaces)?;
    let id = write_sale(&tx, user, s, c, sale_id, opts.clock_ok)?;
    if delivers {
        repairs::delivered(&tx, id, &user.name)?;
    }
    if let Some(old) = opts.replaces {
        let number: String = tx.query_row("SELECT number FROM tickets WHERE id = ?1", [id], |r| r.get(0))?;
        void_in(&tx, old, &user.name, &format!("Corregida en {number}"), opts.any_day)?;
    }
    tx.commit()?;
    Ok((load(conn, id)?, false))
}

// What a correction may replace: a live sale of the same mode (practice or
// real), of this year, with no repair handed back on it. A return, a sale
// that delivered a repair, or one of a closed year is put right with a
// return in Hoy instead. (void_in then checks the day and live returns.)
fn fixable(conn: &Connection, id: i64, s: &Settings) -> Result<()> {
    let row: Option<(String, bool, String, i64)> = conn
        .query_row(
            "SELECT kind, training, issued_at,
                    (SELECT COUNT(*) FROM ticket_lines l WHERE l.ticket_id = tickets.id AND l.repair_id IS NOT NULL)
             FROM tickets WHERE id = ?1 AND voided_at IS NULL",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((kind, training, issued, repairs)) = row else {
        return fail("E_NOT_FOUND");
    };
    if training != s.training {
        return fail("E_FIX_TRAINING");
    }
    if kind != "sale" || repairs > 0 {
        return fail("E_FIX_KIND");
    }
    if issued.get(..4) != now().get(..4) {
        return fail("E_FIX_YEAR");
    }
    Ok(())
}

// A correction made the same day is issued now, like any sale. One of an
// earlier day (only the owner, only without VERI*FACTU) keeps the day and
// time of the sale it replaces: that is when it was sold and paid, so
// neither day's cash moves, and the customer's paper shows the day they
// bought. With VERI*FACTU every correction is of today, as its chain needs.
pub fn correction_time(conn: &Connection, s: &Settings, replaces: Option<i64>) -> Result<Option<String>> {
    let Some(old) = replaces.filter(|_| verifactu::Env::of(s).is_none()) else { return Ok(None) };
    let Some(at) = conn.query_row("SELECT issued_at FROM tickets WHERE id = ?1 AND voided_at IS NULL", [old], |r| r.get::<_, String>(0)).optional()? else {
        return Ok(None);
    };
    if at.get(..10) != now().get(..10) {
        return Ok(Some(at));
    }
    // The same day, but the till was opened or counted since: the sale stays
    // in the count it was part of, or the afternoon would be short by a sale
    // of the morning.
    let counted: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM cash_sessions WHERE opened_at > ?1 OR closed_at > ?1)", [&at], |r| r.get(0))?;
    Ok(counted.then_some(at))
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
            repair_id: src.repair_id,
            ..SaleLine::default()
        };
        lines.push((back, line_id));
    }
    if !wanted.is_empty() {
        return fail("E_REFUND_INVALID");
    }
    // What stays on the sale must still be worth something at each rate:
    // taking an item back and leaving its discount there would pay out more
    // than was paid.
    let mut stays: BTreeMap<i64, i64> = BTreeMap::new();
    for src in &original.lines {
        let now_back: i64 = lines.iter().filter(|(_, of)| Some(*of) == src.id).map(|(l, _)| -l.qty).sum();
        *stays.entry(src.vat_bp).or_default() += (src.qty - src.refunded - now_back) * src.unit_cents;
    }
    if stays.values().any(|v| *v < 0) {
        return fail("E_REFUND_DISCOUNT");
    }
    let totals = compute(&lines.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>());
    if totals.total_cents >= 0 {
        return fail("E_REFUND_INVALID");
    }
    // a card goes back through the machine that charged it
    let machine = original.payment.machine.filter(|_| a.method == "card");
    let payment = Payment { method: method(&a.method)?, tendered_cents: totals.total_cents, change_cents: 0, machine };
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
        bill: plan.original.bill,
        replaces: None,
        at: None,
        // as the sale it returns shows it, also one from before 1.13 (the
        // preview says the same)
        vat: Some(plan.original.shop.vat_view(plan.original.customer.as_ref().is_some_and(Customer::is_fiscal), s.show_vat)),
        note: None,
    };
    let id = insert(&tx, user, s, &t)?;
    tx.commit()?;
    load(conn, id)
}

pub fn reference(conn: &Connection, id: i64) -> Result<Option<TicketRef>> {
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
                    customer_name, customer_nif, customer_address, training, machine_id, bill,
                    (SELECT o.number FROM tickets o WHERE o.id = tickets.replaces),
                    (SELECT n.number FROM tickets n WHERE n.replaces = tickets.id ORDER BY n.id DESC LIMIT 1),
                    note, customer_phone, customer_email
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
                        payment: Payment { method: r.get(6)?, tendered_cents: r.get(7)?, change_cents: r.get(8)?, machine: r.get(20)? },
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
                        payment_changes: Vec::new(),
                        customer: r.get::<_, Option<String>>(16)?.map(|name| Customer {
                            name,
                            nif: r.get::<_, Option<String>>(17).ok().flatten().unwrap_or_default(),
                            address: r.get::<_, Option<String>>(18).ok().flatten().unwrap_or_default(),
                            phone: r.get::<_, Option<String>>(25).ok().flatten().unwrap_or_default(),
                            email: r.get::<_, Option<String>>(26).ok().flatten().unwrap_or_default(),
                        }),
                        training: r.get(19)?,
                        qr: None,
                        aeat: None,
                        bill: r.get(21)?,
                        replaces: r.get(22)?,
                        replaced_by: r.get(23)?,
                        note: r.get(24)?,
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
                l.list_cents, l.repair_id
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
                repair_id: r.get(8)?,
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
    let mut stmt = conn.prepare("SELECT at, by_name, from_method, to_method, from_machine, to_machine FROM ticket_payments WHERE ticket_id = ?1 ORDER BY id")?;
    rec.payment_changes = stmt
        .query_map([id], |r| Ok(PaymentChange { at: r.get(0)?, by_name: r.get(1)?, from: r.get(2)?, to: r.get(3)?, from_machine: r.get(4)?, to_machine: r.get(5)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rec.qr = verifactu::qr_of(conn, id)?;
    rec.aeat = verifactu::ticket_state(conn, id)?;
    Ok(rec)
}

#[derive(Deserialize, Default, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct InvoiceQuery {
    pub q: Option<String>,
    pub limit: Option<i64>,
    // practice bills in practice mode, real ones otherwise; set by the core
    #[serde(skip)]
    pub training: bool,
}

// a customer of an earlier invoice, to fill the next one in one tap
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnownCustomer {
    pub name: String,
    pub nif: String,
    pub address: String,
    pub phone: String,
    pub email: String,
    pub last: String,
}

// The customers of earlier invoices that fit what is typed (name, NIF or phone),
// the most recent first, each once.
pub fn customers(conn: &Connection, q: &str) -> Result<Vec<KnownCustomer>> {
    let words: Vec<String> = q.to_lowercase().split_whitespace().map(str::to_string).collect();
    let mut stmt = conn.prepare(
        "SELECT COALESCE(customer_name, ''), COALESCE(customer_nif, ''), COALESCE(customer_address, ''),
                COALESCE(customer_phone, ''), COALESCE(customer_email, ''), MAX(issued_at)
         FROM tickets WHERE (COALESCE(customer_name, '') <> '' OR COALESCE(customer_nif, '') <> '' OR COALESCE(customer_phone, '') <> '') AND training = 0
         GROUP BY UPPER(COALESCE(customer_nif, '')), COALESCE(customer_name, ''), COALESCE(customer_phone, '') ORDER BY MAX(issued_at) DESC LIMIT 500",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(KnownCustomer { name: r.get(0)?, nif: r.get(1)?, address: r.get(2)?, phone: r.get(3)?, email: r.get(4)?, last: r.get(5)? })
    })?;
    let mut out = Vec::new();
    for c in rows {
        let c = c?;
        let hay = format!("{} {} {} {}", c.name, c.nif, c.phone, c.email).to_lowercase();
        if words.iter().all(|w| hay.contains(w.as_str())) {
            out.push(c);
            if out.len() == 12 {
                break;
            }
        }
    }
    Ok(out)
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceRow {
    pub id: i64,
    pub kind: String,
    pub number: String,
    pub issued_at: String,
    pub user_name: String,
    pub total_cents: i64,
    pub payment_method: String,
    pub voided: bool,
    pub customer_name: String,
    pub customer_nif: String,
    // "Corregida en TF2026-000014", "Borrada por Ana"...
    pub void_reason: Option<String>,
    // the one it took the place of
    pub replaces: Option<String>,
}

// The full invoices, refunds of them included, newest first. Every word typed
// must be in the number, the customer, the NIF or the amount.
pub fn invoices(conn: &Connection, q: &InvoiceQuery) -> Result<Vec<InvoiceRow>> {
    let limit = q.limit.unwrap_or(500).clamp(1, 5000) as usize;
    let words: Vec<String> = q.q.as_deref().unwrap_or_default().to_lowercase().split_whitespace().map(|w| w.replace(',', ".")).collect();
    let mut stmt = conn.prepare(
        "SELECT id, kind, number, issued_at, user_name, total_cents, payment_method, voided_at IS NOT NULL,
                COALESCE(customer_name, ''), COALESCE(customer_nif, ''), void_reason,
                (SELECT o.number FROM tickets o WHERE o.id = tickets.replaces)
         FROM tickets WHERE (customer_name IS NOT NULL OR bill = 1) AND training = ?1 ORDER BY id DESC",
    )?;
    let rows = stmt.query_map([q.training], |r| {
        Ok(InvoiceRow {
            id: r.get(0)?,
            kind: r.get(1)?,
            number: r.get(2)?,
            issued_at: r.get(3)?,
            user_name: r.get(4)?,
            total_cents: r.get(5)?,
            payment_method: r.get(6)?,
            voided: r.get(7)?,
            customer_name: r.get(8)?,
            customer_nif: r.get(9)?,
            void_reason: r.get(10)?,
            replaces: r.get(11)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        let row = row?;
        let hay = format!("{} {} {} {:.2}", row.number, row.customer_name, row.customer_nif, row.total_cents.abs() as f64 / 100.0).to_lowercase();
        if words.iter().all(|w| hay.contains(w.as_str())) {
            out.push(row);
            if out.len() == limit {
                break;
            }
        }
    }
    Ok(out)
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

// A sale charged the wrong way: how it was paid is in no billing record, so it
// is put right on the ticket itself, with who changed it and when.
pub fn set_payment(conn: &mut Connection, s: &Settings, id: i64, to: &str, asked: Option<i64>, by: &str) -> Result<()> {
    let to = method(to)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let row: Option<(String, Option<i64>, i64)> = tx
        .query_row("SELECT payment_method, machine_id, total_cents FROM tickets WHERE id = ?1 AND voided_at IS NULL", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let Some((from, from_machine, total)) = row else {
        return fail("E_NOT_FOUND");
    };
    let machine = s.paid_on(&to, asked);
    let to_machine = machine.map(|m| m.id);
    if from == to && from_machine == to_machine {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO ticket_payments (ticket_id, at, by_name, from_method, to_method, from_machine, to_machine) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, now(), by, from, to, from_machine, to_machine],
    )?;
    tx.execute(
        "UPDATE tickets SET payment_method = ?1, tendered_cents = ?2, change_cents = 0, machine_id = ?3, machine_cost_cents = ?4 WHERE id = ?5",
        params![to, total, to_machine, machine.map_or(0, |m| m.sale_cost(total)), id],
    )?;
    tx.commit()?;
    Ok(())
}

// For a ticket issued by mistake, for a sale that never happened: a real
// sale is corrected with a return. With VERI*FACTU it leaves an anulación.
pub fn void(conn: &mut Connection, id: i64, by: &str, reason: &str) -> Result<()> {
    void_on(conn, id, by, reason, false)
}

// `any_day`: an earlier day too, for the owner while nothing goes to the AEAT
pub fn void_on(conn: &mut Connection, id: i64, by: &str, reason: &str, any_day: bool) -> Result<()> {
    // IMMEDIATE: the anulación reads the end of the chain it joins
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    void_in(&tx, id, by, reason, any_day)?;
    tx.commit()?;
    Ok(())
}

#[derive(Serialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DayGone {
    pub tickets: i64,
    pub entries: i64,
    // sales with a return made another day stay, or that return would point
    // at nothing
    pub kept: i64,
}

// A day, or `what` of it (all | sales | recharges | expenses), taken out of
// every list and total: each ticket voided and each typed entry marked, all
// or nothing, newest first so a return goes before the sale it returns.
// Nothing is erased: who, when and why stay on record.
pub fn void_day(conn: &mut Connection, by: &str, date: &str, training: bool, reason: &str, any_day: bool, what: &str) -> Result<DayGone> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut gone = DayGone::default();
    if matches!(what, "all" | "sales") {
        if !any_day && date != &now()[..10] {
            return fail("E_VOID_OLD");
        }
        let mut stmt = tx.prepare(
            "SELECT id FROM tickets WHERE issued_at >= ?1 AND issued_at <= ?1 || 'T99' AND training = ?2 AND voided_at IS NULL ORDER BY id DESC",
        )?;
        let ids = stmt.query_map(params![date, training], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for id in ids {
            match void_in(&tx, id, by, reason, any_day) {
                Ok(()) => gone.tickets += 1,
                Err(e) if e.0 == "E_HAS_REFUNDS" => gone.kept += 1,
                Err(e) => return Err(e),
            }
        }
    }
    gone.entries = crate::book::delete_day(&tx, by, date, training, reason, what)?;
    tx.commit()?;
    Ok(gone)
}

fn void_in(tx: &Connection, id: i64, by: &str, reason: &str, any_day: bool) -> Result<()> {
    let reason: String = reason.trim().chars().take(200).collect();
    if reason.is_empty() {
        return fail("E_REASON");
    }
    let issued: Option<String> = tx
        .query_row("SELECT issued_at FROM tickets WHERE id = ?1 AND voided_at IS NULL", [id], |r| r.get(0))
        .optional()?;
    let Some(issued) = issued else {
        return fail("E_NOT_FOUND");
    };
    // an earlier day may already be declared; it is corrected with a return
    if !any_day && issued.get(..10) != now().get(..10) {
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
    verifactu::anulacion(tx, id, &settings::load(tx)?, &Local::now())?;
    // a phone this ticket handed back is waiting in the shop again
    repairs::undelivered(tx, id, by)?;
    tx.execute(
        "UPDATE products SET sold_qty = MAX(0, sold_qty - (
             SELECT COALESCE(SUM(qty), 0) FROM ticket_lines WHERE ticket_id = ?1 AND product_id = products.id))
         WHERE id IN (SELECT product_id FROM ticket_lines WHERE ticket_id = ?1)",
        [id],
    )?;
    Ok(())
}

// Training mode's "start over": practice tickets are the only ones the
// database lets go of.
pub fn wipe_training(conn: &mut Connection) -> Result<usize> {
    let tx = conn.transaction()?;
    let practice = "SELECT id FROM tickets WHERE training = 1";
    tx.execute(&format!("DELETE FROM ticket_prints WHERE ticket_id IN ({practice})"), [])?;
    tx.execute(&format!("DELETE FROM ticket_payments WHERE ticket_id IN ({practice})"), [])?;
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
        PaymentIn { method: "cash".into(), tendered_cents: t, machine: None }
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
             SELECT 'T' || substr(?1, 1, 4) || '-' || printf('%06d', n), 'T' || substr(?1, 1, 4), n, ?1, 1, 'Ana', 1380, 2,
                    'card', 1380, 0, '{}', 0
             FROM (SELECT COALESCE(MAX(seq), 0) + 1 AS n FROM tickets WHERE series = 'T' || substr(?1, 1, 4))",
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
    fn what_the_audit_of_1_12_found_stays_fixed() {
        let (mut conn, user, s) = setup();
        // an id used again for another cart is another sale, never "already charged"
        let first = SaleOpts { sale_id: Some("cart-1".into()), ..SaleOpts::default() };
        let (a, dup) = sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &first).unwrap();
        assert!(!dup);
        let (again, dup) = sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &first).unwrap();
        assert!(dup && again.id == a.id);
        assert_eq!(sell(&mut conn, &user, &s, vec![line("Funda", 1, 2500)], &cash(None), &first).unwrap_err().0, format!("E_SALE_REUSED:{}", a.number));
        // another item at the same price is another sale too
        assert!(sell(&mut conn, &user, &s, vec![line("Funda", 1, 690)], &cash(None), &first).unwrap_err().0.starts_with("E_SALE_REUSED"));
        // and so is the same cart once its ticket was deleted
        void(&mut conn, a.id, &user.name, "error").unwrap();
        assert!(sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &first).unwrap_err().0.starts_with("E_SALE_REUSED"));

        // a return never leaves the discount behind: with it, or not at all
        let cart = vec![line("Funda", 1, 2000), line("Descuento 10%", 1, -200)];
        let (t, _) = sell(&mut conn, &user, &s, cart, &cash(None), &SaleOpts::default()).unwrap();
        let (funda, discount) = (t.lines[0].id.unwrap(), t.lines[1].id.unwrap());
        assert_eq!(refund(&mut conn, &user, &s, &back(t.id, &[(funda, 1)])).unwrap_err().0, "E_REFUND_DISCOUNT");
        let ret = refund(&mut conn, &user, &s, &back(t.id, &[(funda, 1), (discount, 1)])).unwrap();
        assert_eq!(ret.total_cents, -1800);

        // corrected the same day after the till was counted and opened again:
        // the sale stays in the count it was part of
        let (morning, _) = sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &SaleOpts::default()).unwrap();
        assert_eq!(correction_time(&conn, &s, Some(morning.id)).unwrap(), None);
        conn.execute("INSERT INTO cash_sessions (opened_at, opened_by, float_cents) VALUES ('2999-01-01T00:00:00', 'Ana', 0)", []).unwrap();
        assert_eq!(correction_time(&conn, &s, Some(morning.id)).unwrap(), Some(morning.issued_at.clone()));
    }

    #[test]
    fn a_bill_shows_its_vat_or_says_nothing_of_it() {
        let (mut conn, user, s) = setup();
        let s = Settings { shop_nif: "B12345674".into(), ..s };
        let has = |r: &TicketRecord, s: &Settings, word: &str| crate::layout::plain(&crate::layout::record(r, s, false, false), 48).contains(word);
        let page = |r: &TicketRecord| -> String {
            let sheets = crate::invoice::invoice(r, None, &crate::invoice::InvoiceOpts::default());
            let words = sheets.iter().flat_map(|p| &p.items).filter_map(|m| if let crate::invoice::Mark::Text { text, .. } = m { Some(text.clone()) } else { None });
            words.collect::<Vec<_>>().join("|")
        };
        // a simple bill: items, prices and the total, nothing about VAT, on
        // the receipt and on the page; its note is kept tidy
        let plain = SaleOpts { bill: true, show_vat: Some(false), note: Some("  Garantía 1 año \n\n Batería   91% ".into()), ..SaleOpts::default() };
        let (t, _) = sell(&mut conn, &user, &s, vec![line("Funda", 2, 900)], &cash(None), &plain).unwrap();
        assert_eq!((t.shop.vat, t.note.as_deref()), (Some(VatView::None), Some("Garantía 1 año\nBatería 91%")));
        assert!(!has(&t, &s, "IVA") && has(&t, &s, "Garantía 1 año") && has(&t, &s, "18,00"));
        let a4 = page(&t);
        assert!(!a4.contains("IVA") && !a4.contains("Base") && a4.contains("Observaciones") && a4.contains("Batería 91%") && a4.contains("Importe"), "{a4}");
        // the VAT stays in the books all the same
        assert_eq!(t.vat.iter().map(|v| (v.vat_bp, v.total_cents)).collect::<Vec<_>>(), [(2100, 1800)]);
        // with the box ticked: base, rate and VAT, customer or not
        let shown = SaleOpts { bill: true, show_vat: Some(true), ..SaleOpts::default() };
        let (t, _) = sell(&mut conn, &user, &s, vec![line("Funda", 2, 900)], &cash(None), &shown).unwrap();
        assert_eq!((t.shop.vat, t.note.as_deref()), (Some(VatView::Full), None));
        assert!(has(&t, &s, "BASE") && page(&t).contains("Base imponible"));
        // a receipt of Venta follows Ajustes: the table, the words, or nothing
        for (show_vat, vat_note, view, word) in [(true, true, VatView::Full, "BASE"), (false, true, VatView::Note, "IVA 21% incluido"), (false, false, VatView::None, "")] {
            let s = Settings { show_vat, vat_note, ..s.clone() };
            let (t, _) = sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &SaleOpts::default()).unwrap();
            assert_eq!(t.shop.vat, Some(view));
            assert!(if word.is_empty() { !has(&t, &s, "IVA") } else { has(&t, &s, word) }, "{view:?}");
        }
        // an invoice with a customer made in Venta shows its VAT whatever
        // the receipts do, and so does its return
        let acme = Customer { name: "Acme Telecom SL".into(), nif: "B12345674".into(), ..Customer::default() };
        let quiet = Settings { show_vat: false, vat_note: false, ..s.clone() };
        let (inv, _) = sell(&mut conn, &user, &quiet, vec![line("iPhone", 1, 69_900)], &cash(None), &SaleOpts { customer: Some(acme), ..SaleOpts::default() }).unwrap();
        assert_eq!(inv.shop.vat, Some(VatView::Full));
        let ret = refund(&mut conn, &user, &quiet, &back(inv.id, &[(inv.lines[0].id.unwrap(), 1)])).unwrap();
        assert_eq!(ret.shop.vat, Some(VatView::Full));
        // a ticket from before 1.13 prints as it always did
        let old = TicketRecord { shop: ShopInfo { vat: None, show_vat: Some(false), ..t.shop.clone() }, ..t.clone() };
        assert!(has(&old, &s, "IVA 21% incluido") && !has(&old, &s, "BASE") && page(&old).contains("IVA incluido"));
        let old = TicketRecord { shop: ShopInfo { vat: None, show_vat: Some(false), ..inv.shop.clone() }, ..inv.clone() };
        assert!(has(&old, &s, "BASE") && page(&old).contains("Base imponible"));
    }

    #[test]
    fn a_bill_is_corrected_with_a_clean_new_one_and_the_old_keeps_why() {
        let (mut conn, user, s) = setup();
        let acme = Customer { name: "Acme Telecom SL".into(), nif: "B12345674".into(), ..Customer::default() };
        let s = Settings { shop_nif: "B12345674".into(), ..s };
        let opts = SaleOpts { customer: Some(acme.clone()), bill: true, ..SaleOpts::default() };
        let (wrong, _) = sell(&mut conn, &user, &s, vec![line("iPhone 14", 1, 69_900)], &cash(None), &opts).unwrap();
        // the right model, in its place
        let fix = SaleOpts { replaces: Some(wrong.id), ..opts.clone() };
        let (right, _) = sell(&mut conn, &user, &s, vec![line("iPhone 15", 1, 79_900)], &cash(None), &fix).unwrap();
        assert_eq!((right.bill, right.replaces.as_deref(), right.voided_at.is_none()), (true, Some(wrong.number.as_str()), true));
        let old = load(&conn, wrong.id).unwrap();
        assert_eq!((old.void_reason, old.replaced_by), (Some(format!("Corregida en {}", right.number)), Some(right.number.clone())));
        // nothing on the new paper says it was corrected
        let page = crate::invoice::invoice(&right, None, &crate::invoice::InvoiceOpts::default());
        let words: Vec<String> = page.iter().flat_map(|p| &p.items).filter_map(|m| if let crate::invoice::Mark::Text { text, .. } = m { Some(text.to_uppercase()) } else { None }).collect();
        assert!(!words.iter().any(|w| w.contains("CORREG") || w.contains("RECTIFIC") || w.contains("COPIA") || w.contains("ANULAD")), "{words:?}");
        // Facturas lists both, the old one saying where it went
        let list = invoices(&conn, &InvoiceQuery::default()).unwrap();
        assert_eq!(list.iter().map(|r| (r.number.as_str(), r.voided)).collect::<Vec<_>>(), [(right.number.as_str(), false), (wrong.number.as_str(), true)]);
        assert_eq!(list[0].replaces.as_deref(), Some(wrong.number.as_str()));
        // a simple bill with no customer is in Facturas too; a sale from Venta is not
        sell(&mut conn, &user, &s, vec![line("Funda", 1, 900)], &cash(None), &SaleOpts { bill: true, ..SaleOpts::default() }).unwrap();
        sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &SaleOpts::default()).unwrap();
        assert_eq!(invoices(&conn, &InvoiceQuery::default()).unwrap().len(), 3);
        // the customer comes back for the next invoice
        let known = customers(&conn, "acme").unwrap();
        assert_eq!(known.iter().map(|c| (c.name.as_str(), c.nif.as_str())).collect::<Vec<_>>(), [("Acme Telecom SL", "B12345674")]);
        assert!(customers(&conn, "nadie").unwrap().is_empty());

        // corrected the same day, it is issued now
        assert!(right.issued_at >= wrong.issued_at);

        // a bill from an earlier day: only when the caller may (the owner,
        // VERI*FACTU off), never by default
        let year = &now()[..4];
        let day = if now()[5..10] == *"01-01" { "12-31" } else { "01-01" };
        let yesterday = old_ticket(&conn, &format!("{year}-{day}T10:00:00"));
        let late = SaleOpts { replaces: Some(yesterday), ..SaleOpts::default() };
        assert_eq!(sell(&mut conn, &user, &s, vec![line("Cable", 2, 690)], &cash(None), &late).unwrap_err().0, "E_VOID_OLD");
        let owner = SaleOpts { any_day: true, ..late };
        let (fixed, _) = sell(&mut conn, &user, &s, vec![line("Cable", 2, 690)], &cash(None), &owner).unwrap();
        assert!(load(&conn, yesterday).unwrap().voided_at.is_some());
        // it stays on the day it was sold: that day's cash and today's are untouched
        assert_eq!(fixed.issued_at, load(&conn, yesterday).unwrap().issued_at);
        assert!(fixed.number.starts_with(&format!("T{year}")), "{}", fixed.number);

        // never one of a closed year, a return, a sale that handed a repair
        // back, or across practice and real
        let closed = old_ticket(&conn, "2020-03-01T10:00:00");
        let fix = |id: i64| SaleOpts { replaces: Some(id), any_day: true, ..SaleOpts::default() };
        assert_eq!(sell(&mut conn, &user, &s, vec![line("Cable", 1, 690)], &cash(None), &fix(closed)).unwrap_err().0, "E_FIX_YEAR");
        let (sold, _) = sell(&mut conn, &user, &s, vec![line("Funda", 2, 900)], &cash(None), &SaleOpts::default()).unwrap();
        let back = refund(&mut conn, &user, &s, &back(sold.id, &[(sold.lines[0].id.unwrap(), 1)])).unwrap();
        assert_eq!(sell(&mut conn, &user, &s, vec![line("Funda", 1, 900)], &cash(None), &fix(back.id)).unwrap_err().0, "E_FIX_KIND");
        let practice = Settings { training: true, ..s.clone() };
        assert_eq!(sell(&mut conn, &user, &practice, vec![line("Funda", 1, 900)], &cash(None), &fix(fixed.id)).unwrap_err().0, "E_FIX_TRAINING");
        // and the list is of the mode the till is in
        let real = invoices(&conn, &InvoiceQuery::default()).unwrap().len();
        sell(&mut conn, &user, &practice, vec![line("Funda", 1, 900)], &cash(None), &SaleOpts { bill: true, ..SaleOpts::default() }).unwrap();
        assert_eq!(invoices(&conn, &InvoiceQuery::default()).unwrap().len(), real);
        assert_eq!(invoices(&conn, &InvoiceQuery { training: true, ..InvoiceQuery::default() }).unwrap().len(), 1);
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
        assert_eq!(err(vec![line("x", 1, 500)], PaymentIn { method: "gold".into(), tendered_cents: None, machine: None }), "E_INVALID:method");
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
        let t = finalize(&mut conn, &user, &s, vec![l], &PaymentIn { method: "card".into(), tendered_cents: None, machine: None }).unwrap();
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
        // an invoice names the shop's NIF: without one there is none
        let bad = SaleOpts { customer: Some(Customer { name: "Acme SL".into(), nif: "B12345674".into(), ..Customer::default() }), ..SaleOpts::default() };
        let no_nif = Settings { shop_nif: String::new(), ..s.clone() };
        assert_eq!(sell(&mut conn, &user, &no_nif, vec![line("Funda", 1, 995)], &cash(None), &bad).unwrap_err().0, "E_SHOP_NIF");
        let s = Settings { shop_nif: "B12345674".into(), ..s };
        let year = now()[..4].to_string();
        let big = vec![line("Portátil", 1, 350_000)];
        assert_eq!(finalize(&mut conn, &user, &s, big.clone(), &cash(None)).unwrap_err().0, "E_OVER_LIMIT");
        let bad = SaleOpts { customer: Some(Customer { name: "Acme SL".into(), nif: "B12345678".into(), ..Customer::default() }), ..SaleOpts::default() };
        assert_eq!(sell(&mut conn, &user, &s, big.clone(), &cash(None), &bad).unwrap_err().0, "E_INVALID:customerNif");
        let acme = Customer { name: "  Acme   SL ".into(), nif: "b-12345674".into(), address: "Calle 1, Girona".into(), ..Customer::default() };
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
