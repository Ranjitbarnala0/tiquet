// RD 1007/2023 (Reglamento de los sistemas informáticos de facturación) and
// Orden HAC/1177/2024: every invoice, refund and void Tiquet issues leaves a
// billing record (registro de facturación) that names the record before it by
// its SHA-256 fingerprint (huella), in the exact format the AEAT publishes.
// Tiquet only works as VERI*FACTU: the records go to the AEAT as they are
// made (aeat.rs), so there is no signature and no event log to keep.

use std::fmt::Write as _;
use std::path::Path;

use chrono::{DateTime, Local};
use qrcodegen::{QrCode, QrCodeEcc, QrSegment, Version};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{fail, Result};
use crate::settings::{self, Settings};
use crate::tickets::{self, Customer, TicketRecord};
use crate::{log, nif};

// Who makes Tiquet, as every record and the declaración responsable must say.
// To be filled in before VERI*FACTU is used for real; while the name or NIF
// is missing Ajustes refuses to switch it on. A build may also set them with
// TIQUET_PRODUCER_NAME, TIQUET_PRODUCER_NIF and TIQUET_PRODUCER_ADDRESS.
const PRODUCER_NAME: &str = "";
const PRODUCER_NIF: &str = "";
const PRODUCER_ADDRESS: &str = "";
// where and when the producer signs the declaración responsable of this version
const DECLARED_ON: &str = "";
const DECLARED_IN: &str = "";

pub const SYSTEM_NAME: &str = "Tiquet";
// two characters, A-Z or 0-9, that tell this product apart from the producer's others
pub const SYSTEM_ID: &str = "TQ";
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Producer {
    pub name: &'static str,
    pub nif: &'static str,
    pub address: &'static str,
}

pub fn producer() -> Producer {
    Producer {
        name: option_env!("TIQUET_PRODUCER_NAME").unwrap_or(PRODUCER_NAME),
        nif: option_env!("TIQUET_PRODUCER_NIF").unwrap_or(PRODUCER_NIF),
        address: option_env!("TIQUET_PRODUCER_ADDRESS").unwrap_or(PRODUCER_ADDRESS),
    }
}

pub fn producer_ready() -> bool {
    let p = producer();
    !p.name.trim().is_empty() && nif::spanish(p.nif)
}

// The AEAT keeps a test copy of everything (preproducción) apart from the
// real one, so each has its own chain and its own QR address.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Env {
    Test,
    Prod,
}

impl Env {
    pub fn of(s: &Settings) -> Option<Env> {
        Env::parse(&s.verifactu)
    }

    pub fn parse(code: &str) -> Option<Env> {
        match code {
            "test" => Some(Env::Test),
            "prod" => Some(Env::Prod),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Env::Test => "test",
            Env::Prod => "prod",
        }
    }

    fn qr_base(self) -> &'static str {
        match self {
            Env::Test => "https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR",
            Env::Prod => "https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR",
        }
    }

    // where the records are sent, with a certificate (not a seal)
    pub fn host(self) -> &'static str {
        match self {
            Env::Test => "prewww1.aeat.es",
            Env::Prod => "www1.agenciatributaria.gob.es",
        }
    }
}

pub const SOAP_PATH: &str = "/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP";

// The AEAT's huella: SHA-256 of the field=value pairs joined by '&', every
// value trimmed and an absent one left empty, in upper-case hex.
fn huella(pairs: &[(&str, &str)]) -> String {
    let text = pairs.iter().map(|(k, v)| format!("{k}={}", v.trim())).collect::<Vec<_>>().join("&");
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02X}")).collect()
}

// what an alta record's huella covers
pub struct AltaKey<'a> {
    pub issuer_nif: &'a str,
    pub number: &'a str,
    // dd-mm-yyyy
    pub date: &'a str,
    pub invoice_type: &'a str,
    pub vat_total: &'a str,
    pub total: &'a str,
    // huella of the record before; empty for the first one
    pub prev: &'a str,
    pub generated_at: &'a str,
}

pub fn huella_alta(k: &AltaKey) -> String {
    huella(&[
        ("IDEmisorFactura", k.issuer_nif),
        ("NumSerieFactura", k.number),
        ("FechaExpedicionFactura", k.date),
        ("TipoFactura", k.invoice_type),
        ("CuotaTotal", k.vat_total),
        ("ImporteTotal", k.total),
        ("Huella", k.prev),
        ("FechaHoraHusoGenRegistro", k.generated_at),
    ])
}

// and an anulación's: the invoice it cancels
pub struct AnulacionKey<'a> {
    pub issuer_nif: &'a str,
    pub number: &'a str,
    pub date: &'a str,
    pub prev: &'a str,
    pub generated_at: &'a str,
}

pub fn huella_anulacion(k: &AnulacionKey) -> String {
    huella(&[
        ("IDEmisorFacturaAnulada", k.issuer_nif),
        ("NumSerieFacturaAnulada", k.number),
        ("FechaExpedicionFacturaAnulada", k.date),
        ("Huella", k.prev),
        ("FechaHoraHusoGenRegistro", k.generated_at),
    ])
}

// Amounts as the record carries them, and so as they are hashed: a dot and
// always two decimals. "12.3" and "12.30" would give different huellas.
pub fn amount(cents: i64) -> String {
    let a = cents.unsigned_abs();
    format!("{}{}.{:02}", if cents < 0 { "-" } else { "" }, a / 100, a % 100)
}

// FechaExpedicionFactura, dd-mm-yyyy, from a ticket's yyyy-mm-ddThh:mm:ss
pub fn date(issued_at: &str) -> String {
    match (issued_at.get(..4), issued_at.get(5..7), issued_at.get(8..10)) {
        (Some(y), Some(m), Some(d)) => format!("{d}-{m}-{y}"),
        _ => issued_at.to_string(),
    }
}

// FechaHoraHusoGenRegistro: the local time with its offset from UTC, so the
// autumn hour that repeats stays two different instants
pub fn stamp(at: &DateTime<Local>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

// the VAT rates the AEAT accepts today (2 %, 5 % and 7,5 % only applied for
// some months of 2022 to 2024)
pub fn rate_allowed(bp: i64) -> bool {
    [0, 400, 1000, 2100].contains(&bp)
}

fn rate(bp: i64) -> String {
    format!("{}.{:02}", bp / 100, bp % 100)
}

// java.net.URLEncoder, which the QR specification's own example uses
fn form_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'*' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

// the address a customer's phone opens to check the invoice with the AEAT
pub fn qr_url(env: Env, nif: &str, number: &str, date: &str, total: &str) -> String {
    format!(
        "{}?nif={}&numserie={}&fecha={}&importe={}",
        env.qr_base(),
        form_encode(nif),
        form_encode(number),
        form_encode(date),
        form_encode(total)
    )
}

// what a ticket not issued yet will carry, for the previews
pub fn preview_qr(s: &Settings, number: &str, issued_at: &str, total_cents: i64) -> Option<String> {
    let env = Env::of(s).filter(|_| !s.training)?;
    Some(qr_url(env, &nif::normalize(&s.shop_nif), number, &date(issued_at), &amount(total_cents)))
}

// The QR as rows of modules, '1' dark. Error correction M as the AEAT asks:
// the encoder is not allowed to raise it, which would change the symbol.
pub fn qr_rows(text: &str) -> Option<Vec<String>> {
    let segs = QrSegment::make_segments(text);
    let qr = QrCode::encode_segments_advanced(&segs, QrCodeEcc::Medium, Version::MIN, Version::MAX, None, false).ok()?;
    let n = qr.size();
    Some((0..n).map(|y| (0..n).map(|x| if qr.get_module(x, y) { '1' } else { '0' }).collect()).collect())
}

// Printer dots per module, so the symbol comes out between 30 and 40 mm.
// The heads are 8 dots per mm (576 dots on 72 mm, 384 on 48 mm); 272 dots is
// 34 mm there and still under 40 mm on a 7 dots per mm head.
pub fn module_dots(modules: usize) -> usize {
    ((272 + modules / 2) / modules.max(1)).max(1)
}

// XML 1.0 text: the five entities, and nothing the parser would refuse
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(' '),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

fn cut(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

// elements of SuministroInformacion.xsd, under the prefix the envelope declares
struct Xml(String);

impl Xml {
    fn open(&mut self, tag: &str) -> &mut Self {
        let _ = write!(self.0, "<sum1:{tag}>");
        self
    }

    fn close(&mut self, tag: &str) -> &mut Self {
        let _ = write!(self.0, "</sum1:{tag}>");
        self
    }

    fn el(&mut self, tag: &str, value: &str) -> &mut Self {
        let _ = write!(self.0, "<sum1:{tag}>{}</sum1:{tag}>", esc(value));
        self
    }
}

// the record a new one is chained to: for an anulación, the invoice it cancels
#[derive(Clone, Debug)]
struct Link {
    issuer_nif: String,
    number: String,
    date: String,
    hash: String,
    generated_at: String,
}

fn last(conn: &Connection, env: Env) -> Result<Option<Link>> {
    Ok(conn
        .query_row(
            "SELECT issuer_nif, number, date, hash, generated_at FROM fiscal_records WHERE env = ?1 ORDER BY id DESC LIMIT 1",
            [env.code()],
            |r| Ok(Link { issuer_nif: r.get(0)?, number: r.get(1)?, date: r.get(2)?, hash: r.get(3)?, generated_at: r.get(4)? }),
        )
        .optional()?)
}

// The PC clock must not run behind the record before (OM art. 7). The sale
// itself never waits for it: a clock gone back is noted for whoever looks.
fn check_clock(prev: Option<&Link>, at: &DateTime<Local>) {
    let Some(p) = prev else { return };
    if let Ok(then) = DateTime::parse_from_rfc3339(&p.generated_at) {
        if *at < then - chrono::TimeDelta::minutes(1) {
            log::warn(format!("VERI*FACTU: the clock ({}) is behind the last record ({})", stamp(at), p.generated_at));
        }
    }
}

fn chain(x: &mut Xml, prev: Option<&Link>) {
    x.open("Encadenamiento");
    match prev {
        None => {
            x.el("PrimerRegistro", "S");
        }
        Some(p) => {
            x.open("RegistroAnterior")
                .el("IDEmisorFactura", &p.issuer_nif)
                .el("NumSerieFactura", &p.number)
                .el("FechaExpedicionFactura", &p.date)
                .el("Huella", &p.hash)
                .close("RegistroAnterior");
        }
    }
    x.close("Encadenamiento");
}

fn system(x: &mut Xml, install: &str) {
    let p = producer();
    x.open("SistemaInformatico")
        .el("NombreRazon", &cut(p.name, 120))
        .el("NIF", &nif::normalize(p.nif))
        .el("NombreSistemaInformatico", SYSTEM_NAME)
        .el("IdSistemaInformatico", SYSTEM_ID)
        .el("Version", VERSION)
        .el("NumeroInstalacion", install)
        .el("TipoUsoPosibleSoloVerifactu", "S")
        .el("TipoUsoPosibleMultiOT", "N")
        .el("IndicadorMultiplesOT", "N")
        .close("SistemaInformatico");
}

fn recipient(x: &mut Xml, c: &Customer) {
    let id = nif::normalize(&c.nif);
    x.open("Destinatarios").open("IDDestinatario").el("NombreRazon", &cut(&c.name, 120));
    if nif::spanish(&id) {
        x.el("NIF", &id);
    } else {
        // a company elsewhere in the EU, by its VAT number
        x.open("IDOtro").el("CodigoPais", id.get(..2).unwrap_or_default()).el("IDType", "02").el("ID", &cut(&id, 20)).close("IDOtro");
    }
    x.close("IDDestinatario").close("Destinatarios");
}

// F2 a ticket, F1 a full invoice, R5 and R1 their returns (art. 80 Dos LIVA)
pub fn invoice_type(t: &TicketRecord) -> &'static str {
    match (t.kind == "refund", t.customer.is_some()) {
        (false, false) => "F2",
        (false, true) => "F1",
        (true, false) => "R5",
        (true, true) => "R1",
    }
}

fn description(t: &TicketRecord) -> String {
    let items = t.lines.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ");
    let what = match &t.refund_of {
        Some(of) => format!("Devolución de {}", of.number),
        None => "Venta".to_string(),
    };
    cut(&format!("{what}: {items}"), 500)
}

// how a record corrects one the AEAT refused or flagged (subsanación)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fix {
    // the invoice is at the AEAT, with errors: Subsanacion S
    Flagged,
    // the AEAT holds nothing for it yet: Subsanacion S, RechazoPrevio X
    NeverStored,
    // a correction of a stored invoice was refused: Subsanacion S, RechazoPrevio S
    Refused,
}

struct Built {
    xml: String,
    hash: String,
    invoice_type: Option<&'static str>,
    vat_total: Option<String>,
    total: Option<String>,
    qr: Option<String>,
}

struct Issuer {
    nif: String,
    name: String,
}

pub fn issuer_name(legal: &str, trade: &str) -> String {
    cut(if legal.trim().is_empty() { trade } else { legal }, 120)
}

fn build_alta(t: &TicketRecord, issuer: &Issuer, env: Env, install: &str, prev: Option<&Link>, at: &str, fix: Option<Fix>) -> Built {
    let ty = invoice_type(t);
    let fecha = date(&t.issued_at);
    let vat_total = amount(t.vat.iter().map(|v| v.vat_cents).sum());
    let total = amount(t.total_cents);
    let mut x = Xml(String::with_capacity(2048));
    x.open("RegistroAlta").el("IDVersion", "1.0");
    x.open("IDFactura")
        .el("IDEmisorFactura", &issuer.nif)
        .el("NumSerieFactura", &t.number)
        .el("FechaExpedicionFactura", &fecha)
        .close("IDFactura");
    x.el("NombreRazonEmisor", &issuer.name);
    match fix {
        Some(Fix::Flagged) => {
            x.el("Subsanacion", "S");
        }
        Some(Fix::NeverStored) => {
            x.el("Subsanacion", "S").el("RechazoPrevio", "X");
        }
        Some(Fix::Refused) => {
            x.el("Subsanacion", "S").el("RechazoPrevio", "S");
        }
        None => {}
    }
    x.el("TipoFactura", ty);
    if let Some(of) = &t.refund_of {
        let original = date(&of.issued_at);
        x.el("TipoRectificativa", "I");
        x.open("FacturasRectificadas")
            .open("IDFacturaRectificada")
            .el("IDEmisorFactura", &issuer.nif)
            .el("NumSerieFactura", &of.number)
            .el("FechaExpedicionFactura", &original)
            .close("IDFacturaRectificada")
            .close("FacturasRectificadas");
        // what is corrected took place on the day of the sale
        if original != fecha {
            x.el("FechaOperacion", &original);
        }
    }
    x.el("DescripcionOperacion", &description(t));
    if let Some(c) = &t.customer {
        recipient(&mut x, c);
    }
    x.open("Desglose");
    for v in &t.vat {
        x.open("DetalleDesglose")
            .el("Impuesto", "01")
            .el("ClaveRegimen", "01")
            .el("CalificacionOperacion", "S1")
            .el("TipoImpositivo", &rate(v.vat_bp))
            .el("BaseImponibleOimporteNoSujeto", &amount(v.base_cents))
            .el("CuotaRepercutida", &amount(v.vat_cents))
            .close("DetalleDesglose");
    }
    x.close("Desglose");
    x.el("CuotaTotal", &vat_total).el("ImporteTotal", &total);
    chain(&mut x, prev);
    system(&mut x, install);
    let hash = huella_alta(&AltaKey {
        issuer_nif: &issuer.nif,
        number: &t.number,
        date: &fecha,
        invoice_type: ty,
        vat_total: &vat_total,
        total: &total,
        prev: prev.map(|p| p.hash.as_str()).unwrap_or_default(),
        generated_at: at,
    });
    x.el("FechaHoraHusoGenRegistro", at).el("TipoHuella", "01").el("Huella", &hash).close("RegistroAlta");
    let qr = qr_url(env, &issuer.nif, &t.number, &fecha, &total);
    Built { xml: x.0, hash, invoice_type: Some(ty), vat_total: Some(vat_total), total: Some(total), qr: Some(qr) }
}

// an invoice as the AEAT identifies it
struct InvoiceId<'a> {
    nif: &'a str,
    number: &'a str,
    date: &'a str,
}

// `stored`: the AEAT holds the alta it cancels; `refused`: an earlier
// anulación of it was refused
fn build_anulacion(id: &InvoiceId, install: &str, prev: Option<&Link>, at: &str, stored: bool, refused: bool) -> Built {
    let mut x = Xml(String::with_capacity(1024));
    x.open("RegistroAnulacion").el("IDVersion", "1.0");
    x.open("IDFactura")
        .el("IDEmisorFacturaAnulada", id.nif)
        .el("NumSerieFacturaAnulada", id.number)
        .el("FechaExpedicionFacturaAnulada", id.date)
        .close("IDFactura");
    if !stored {
        x.el("SinRegistroPrevio", "S");
    }
    if refused {
        x.el("RechazoPrevio", "S");
    }
    chain(&mut x, prev);
    system(&mut x, install);
    let hash = huella_anulacion(&AnulacionKey {
        issuer_nif: id.nif,
        number: id.number,
        date: id.date,
        prev: prev.map(|p| p.hash.as_str()).unwrap_or_default(),
        generated_at: at,
    });
    x.el("FechaHoraHusoGenRegistro", at).el("TipoHuella", "01").el("Huella", &hash).close("RegistroAnulacion");
    Built { xml: x.0, hash, invoice_type: None, vat_total: None, total: None, qr: None }
}

struct NewRecord<'a> {
    ticket_id: i64,
    kind: &'a str,
    env: Env,
    issuer_nif: &'a str,
    number: &'a str,
    date: &'a str,
    prev: Option<&'a Link>,
    generated_at: &'a str,
    corrects: Option<i64>,
    built: Built,
}

fn store(conn: &Connection, r: NewRecord) -> Result<i64> {
    conn.execute(
        "INSERT INTO fiscal_records (ticket_id, kind, env, issuer_nif, number, date, invoice_type, vat_total, total,
                                     prev_hash, generated_at, hash, corrects, qr, xml)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            r.ticket_id,
            r.kind,
            r.env.code(),
            r.issuer_nif,
            r.number,
            r.date,
            r.built.invoice_type,
            r.built.vat_total,
            r.built.total,
            r.prev.map(|p| p.hash.as_str()).unwrap_or_default(),
            r.generated_at,
            r.built.hash,
            r.corrects,
            r.built.qr,
            r.built.xml
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

// The alta of a ticket just issued, inside the transaction that issued it:
// the ticket and its record are committed together or not at all.
pub fn alta(conn: &Connection, ticket_id: i64, s: &Settings, at: &DateTime<Local>) -> Result<()> {
    let Some(env) = Env::of(s) else { return Ok(()) };
    let t = tickets::load(conn, ticket_id)?;
    if t.training {
        return Ok(());
    }
    let issuer = Issuer { nif: nif::normalize(&t.shop.nif), name: issuer_name(&t.shop.legal_name, &t.shop.name) };
    if !nif::spanish(&issuer.nif) || issuer.name.is_empty() {
        return fail("E_VERIFACTU:issuer");
    }
    let install = settings::install_id(conn)?;
    let prev = last(conn, env)?;
    check_clock(prev.as_ref(), at);
    let generated_at = stamp(at);
    let built = build_alta(&t, &issuer, env, &install, prev.as_ref(), &generated_at, None);
    let fecha = date(&t.issued_at);
    store(
        conn,
        NewRecord {
            ticket_id,
            kind: "alta",
            env,
            issuer_nif: &issuer.nif,
            number: &t.number,
            date: &fecha,
            prev: prev.as_ref(),
            generated_at: &generated_at,
            corrects: None,
            built,
        },
    )?;
    Ok(())
}

// the alta of a ticket the AEAT accepted, with or without errors
fn stored_alta(conn: &Connection, ticket_id: i64, env: Env) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM fiscal_records r JOIN fiscal_sends s ON s.record_id = r.id
                        WHERE r.ticket_id = ?1 AND r.env = ?2 AND r.kind = 'alta' AND s.state <> 'Incorrecto')",
        params![ticket_id, env.code()],
        |r| r.get(0),
    )?)
}

// A void cancels the alta in the chain it went to. A ticket issued before
// VERI*FACTU was switched on is cancelled "sin registro previo".
pub fn anulacion(conn: &Connection, ticket_id: i64, s: &Settings, at: &DateTime<Local>) -> Result<()> {
    let first: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT env, issuer_nif, number, date FROM fiscal_records WHERE ticket_id = ?1 AND kind = 'alta' ORDER BY id LIMIT 1",
            [ticket_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let (env, issuer_nif, number, fecha, had_alta) = match first {
        Some((env, nif, number, fecha)) => (Env::parse(&env).unwrap_or(Env::Prod), nif, number, fecha, true),
        None => {
            let Some(env) = Env::of(s) else { return Ok(()) };
            let t = tickets::load(conn, ticket_id)?;
            if t.training {
                return Ok(());
            }
            (env, nif::normalize(&t.shop.nif), t.number.clone(), date(&t.issued_at), false)
        }
    };
    let install = settings::install_id(conn)?;
    let prev = last(conn, env)?;
    check_clock(prev.as_ref(), at);
    let generated_at = stamp(at);
    // an alta still on its way counts as stored: it goes out first
    let stored = had_alta && !refused_only(conn, ticket_id, env)?;
    let id = InvoiceId { nif: &issuer_nif, number: &number, date: &fecha };
    let built = build_anulacion(&id, &install, prev.as_ref(), &generated_at, stored, false);
    store(
        conn,
        NewRecord {
            ticket_id,
            kind: "anulacion",
            env,
            issuer_nif: &issuer_nif,
            number: &number,
            date: &fecha,
            prev: prev.as_ref(),
            generated_at: &generated_at,
            corrects: None,
            built,
        },
    )?;
    Ok(())
}

// every alta of the ticket was answered, and all were refused
fn refused_only(conn: &Connection, ticket_id: i64, env: Env) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) > 0 AND COUNT(*) = COALESCE(SUM(s.state = 'Incorrecto'), 0)
           FROM fiscal_records r LEFT JOIN fiscal_sends s ON s.record_id = r.id
          WHERE r.ticket_id = ?1 AND r.env = ?2 AND r.kind = 'alta'",
        params![ticket_id, env.code()],
        |r| r.get(0),
    )?)
}

// Answers the AEAT gave that need a new record, unless a later one already
// corrects them. 2004 (clock) and 2009 need no correction.
const PROBLEMS: &str = "SELECT r.id, r.ticket_id, r.kind, r.number, r.env, s.state, s.code, s.message, s.at
                          FROM fiscal_records r JOIN fiscal_sends s ON s.record_id = r.id
                         WHERE (s.state = 'Incorrecto' OR (s.state = 'AceptadoConErrores' AND COALESCE(s.code, '') NOT IN ('2004', '2009')))
                           AND NOT EXISTS (SELECT 1 FROM fiscal_records c WHERE c.corrects = r.id)";

// Subsanación: a new record for the same invoice, built from the ticket as
// stored and the shop's details as they are now (a fixed name, say).
pub fn correct(conn: &Connection, record_id: i64, s: &Settings) -> Result<i64> {
    // IMMEDIATE like every other record: the end of the chain is read and
    // extended under the write lock
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let conn: &Connection = &tx;
    let sql = format!("{PROBLEMS} AND r.id = ?1");
    let row: Option<(i64, String, String, String)> =
        conn.query_row(&sql, [record_id], |r| Ok((r.get(1)?, r.get(2)?, r.get(4)?, r.get(5)?))).optional()?;
    let Some((ticket_id, kind, env, state)) = row else {
        return fail("E_NOT_FOUND");
    };
    let env = Env::parse(&env).unwrap_or(Env::Prod);
    let (issuer_nif, number, fecha): (String, String, String) =
        conn.query_row("SELECT issuer_nif, number, date FROM fiscal_records WHERE id = ?1", [record_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    let install = settings::install_id(conn)?;
    let prev = last(conn, env)?;
    let at = Local::now();
    let generated_at = stamp(&at);
    let stored = stored_alta(conn, ticket_id, env)?;
    let built = if kind == "alta" {
        let t = tickets::load(conn, ticket_id)?;
        let issuer = Issuer { nif: issuer_nif.clone(), name: issuer_name(&s.legal_name, &s.shop_name) };
        let fix = match (stored, state.as_str()) {
            (false, _) => Fix::NeverStored,
            (true, "Incorrecto") => Fix::Refused,
            (true, _) => Fix::Flagged,
        };
        build_alta(&t, &issuer, env, &install, prev.as_ref(), &generated_at, Some(fix))
    } else {
        let id = InvoiceId { nif: &issuer_nif, number: &number, date: &fecha };
        build_anulacion(&id, &install, prev.as_ref(), &generated_at, stored, true)
    };
    let id = store(
        conn,
        NewRecord {
            ticket_id,
            kind: &kind,
            env,
            issuer_nif: &issuer_nif,
            number: &number,
            date: &fecha,
            prev: prev.as_ref(),
            generated_at: &generated_at,
            corrects: Some(record_id),
            built,
        },
    )?;
    tx.commit()?;
    log::info(format!("VERI*FACTU: record {record_id} ({kind} {number}) corrected by record {id}"));
    Ok(id)
}

// the QR printed on a ticket: the one its first alta carried
pub fn qr_of(conn: &Connection, ticket_id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT qr FROM fiscal_records WHERE ticket_id = ?1 AND kind = 'alta' ORDER BY id LIMIT 1", [ticket_id], |r| r.get(0))
        .optional()?
        .flatten())
}

// where a ticket stands with the AEAT, for the ticket screen
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TicketAeat {
    pub env: String,
    // pending | Correcto | AceptadoConErrores | Incorrecto, of its latest record
    pub state: String,
    pub code: Option<String>,
    pub message: Option<String>,
    pub csv: Option<String>,
}

pub fn ticket_state(conn: &Connection, ticket_id: i64) -> Result<Option<TicketAeat>> {
    Ok(conn
        .query_row(
            "SELECT r.env, COALESCE(s.state, 'pending'), s.code, s.message, s.csv
               FROM fiscal_records r LEFT JOIN fiscal_sends s ON s.record_id = r.id
              WHERE r.ticket_id = ?1 ORDER BY r.id DESC LIMIT 1",
            [ticket_id],
            |r| Ok(TicketAeat { env: r.get(0)?, state: r.get(1)?, code: r.get(2)?, message: r.get(3)?, csv: r.get(4)? }),
        )
        .optional()?)
}

// Switching it on needs the producer in the build and an issuer the AEAT can
// identify. Once real records exist under a NIF, that NIF is the only one
// this installation ever issues for (TipoUsoPosibleMultiOT N).
pub fn check_settings(conn: &Connection, s: &Settings) -> Result<()> {
    let issued: Option<String> = conn
        .query_row("SELECT issuer_nif FROM fiscal_records WHERE env = 'prod' ORDER BY id LIMIT 1", [], |r| r.get(0))
        .optional()?;
    if issued.is_some_and(|n| n != nif::normalize(&s.shop_nif)) {
        return fail("E_NIF_LOCKED");
    }
    if Env::of(s).is_none() {
        return Ok(());
    }
    if !producer_ready() {
        return fail("E_PRODUCER");
    }
    if !nif::spanish(&s.shop_nif) {
        return fail("E_INVALID:nif");
    }
    if issuer_name(&s.legal_name, &s.shop_name).is_empty() {
        return fail("E_INVALID:legalName");
    }
    Ok(())
}

// one record waiting to go to the AEAT
#[derive(Clone, Debug)]
pub struct Pending {
    pub id: i64,
    pub issuer_nif: String,
    pub kind: String,
    pub number: String,
    pub date: String,
    pub generated_at: String,
    pub xml: String,
}

// the oldest first: they have to arrive in the order they were made
pub fn pending(conn: &Connection, env: Env, limit: i64) -> Result<Vec<Pending>> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.issuer_nif, r.kind, r.number, r.date, r.generated_at, r.xml FROM fiscal_records r
          WHERE r.env = ?1 AND NOT EXISTS (SELECT 1 FROM fiscal_sends s WHERE s.record_id = r.id)
          ORDER BY r.id LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![env.code(), limit], |r| {
        Ok(Pending {
            id: r.get(0)?,
            issuer_nif: r.get(1)?,
            kind: r.get(2)?,
            number: r.get(3)?,
            date: r.get(4)?,
            generated_at: r.get(5)?,
            xml: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// the AEAT's verdict on one record
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub record_id: i64,
    pub state: String,
    pub code: Option<String>,
    pub message: Option<String>,
}

pub fn answered(conn: &Connection, answers: &[Answer], csv: Option<&str>) -> Result<()> {
    let at = tickets::now();
    for a in answers {
        conn.execute(
            "INSERT OR IGNORE INTO fiscal_sends (record_id, at, state, code, message, csv) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![a.record_id, at, a.state, a.code, a.message, csv],
        )?;
    }
    Ok(())
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub record_id: i64,
    pub ticket_id: i64,
    pub kind: String,
    pub number: String,
    pub env: String,
    pub state: String,
    pub code: Option<String>,
    pub message: Option<String>,
    pub at: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub env: Option<String>,
    pub producer: bool,
    pub pending: i64,
    // when the oldest record still waiting was made
    pub oldest: Option<String>,
    pub sent: i64,
    pub problems: Vec<Problem>,
    // the sender's last contact with the AEAT, and what went wrong if it did
    pub last_send: Option<String>,
    pub last_error: Option<String>,
}

// Records still to send count whichever environment they belong to: one made
// before the switch was turned off or moved still has to arrive.
pub fn status(conn: &Connection, s: &Settings) -> Result<Status> {
    let env = Env::of(s);
    let (pending, oldest): (i64, Option<String>) = conn.query_row(
        "SELECT COUNT(*), MIN(r.generated_at) FROM fiscal_records r
          WHERE NOT EXISTS (SELECT 1 FROM fiscal_sends s WHERE s.record_id = r.id)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let sent: i64 = conn.query_row("SELECT COUNT(*) FROM fiscal_sends", [], |r| r.get(0))?;
    let mut stmt = conn.prepare(&format!("{PROBLEMS} ORDER BY r.id DESC LIMIT 50"))?;
    let problems = stmt
        .query_map([], |r| {
            Ok(Problem {
                record_id: r.get(0)?,
                ticket_id: r.get(1)?,
                kind: r.get(2)?,
                number: r.get(3)?,
                env: r.get(4)?,
                state: r.get(5)?,
                code: r.get(6)?,
                message: r.get(7)?,
                at: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Status {
        env: env.map(|e| e.code().to_string()),
        producer: producer_ready(),
        pending,
        oldest,
        sent,
        problems,
        last_send: settings::get(conn, "aeatLastSend")?,
        last_error: settings::get(conn, "aeatLastError")?,
    })
}

// A restored backup knows nothing of the records made after it was taken, and
// those may still have to reach the AEAT. They are copied back in from the
// database that was replaced, oldest first, with what the AEAT answered.
pub fn carry_over(conn: &Connection, from: &Path) -> Result<usize> {
    let old = Connection::open_with_flags(from, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let has: bool = old
        .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'fiscal_records')", [], |r| r.get(0))
        .unwrap_or(false);
    if !has {
        return Ok(0);
    }
    let cols = "ticket_id, kind, env, issuer_nif, number, date, invoice_type, vat_total, total, prev_hash, generated_at, hash, corrects, qr, xml";
    let mut stmt = old.prepare(&format!("SELECT id, {cols} FROM fiscal_records ORDER BY id"))?;
    let mut rows = stmt.query([])?;
    let tx = conn.unchecked_transaction()?;
    let mut ids = std::collections::HashMap::new();
    let mut copied = 0;
    while let Some(r) = rows.next()? {
        let old_id: i64 = r.get(0)?;
        let hash: String = r.get(12)?;
        let here: Option<i64> = tx.query_row("SELECT id FROM fiscal_records WHERE hash = ?1", [&hash], |x| x.get(0)).optional()?;
        let id = match here {
            Some(id) => id,
            None => {
                let corrects = r.get::<_, Option<i64>>(13)?.and_then(|c| ids.get(&c).copied());
                let mut values: Vec<rusqlite::types::Value> = (1..=15).map(|i| r.get(i)).collect::<rusqlite::Result<_>>()?;
                values[12] = corrects.into();
                tx.execute(
                    &format!("INSERT INTO fiscal_records ({cols}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"),
                    rusqlite::params_from_iter(values),
                )?;
                copied += 1;
                tx.last_insert_rowid()
            }
        };
        ids.insert(old_id, id);
        let sent: Option<[rusqlite::types::Value; 5]> = old
            .query_row("SELECT at, state, code, message, csv FROM fiscal_sends WHERE record_id = ?1", [old_id], |x| {
                Ok([x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?, x.get(4)?])
            })
            .optional()?;
        if let Some([at, state, code, message, csv]) = sent {
            tx.execute(
                "INSERT OR IGNORE INTO fiscal_sends (record_id, at, state, code, message, csv) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, at, state, code, message, csv],
            )?;
        }
    }
    tx.commit()?;
    if copied > 0 {
        log::warn(format!("VERI*FACTU: {copied} records made after the restored backup were copied back in"));
    }
    Ok(copied)
}

// The producer's statement that this version complies (RD 1007/2023 art. 13,
// OM art. 15), readable inside Tiquet itself.
pub fn declaration() -> String {
    let p = producer();
    let missing = "(pendiente)";
    let or = |v: &str| if v.trim().is_empty() { missing.to_string() } else { v.trim().to_string() };
    format!(
        "DECLARACIÓN RESPONSABLE DEL SISTEMA INFORMÁTICO DE FACTURACIÓN\n\n\
         a) Nombre del sistema informático: {SYSTEM_NAME}\n\n\
         b) Código identificador del sistema informático: {SYSTEM_ID}\n\n\
         c) Versión: {VERSION}\n\n\
         d) Componentes y funcionalidades: programa para Windows 10 y 11 (64 bits) que se instala en el ordenador \
         de la tienda, con su propia base de datos SQLite en ese mismo equipo, y que imprime en una impresora de \
         tickets ESC/POS conectada a él. Sirve para vender y cobrar en el mostrador: expide facturas simplificadas \
         (tickets), facturas completas con los datos del cliente y facturas rectificativas por devoluciones, anula \
         tickets emitidos por error, lleva la caja y hace informes de ventas. Cada factura, devolución y anulación \
         genera su registro de facturación en la misma transacción de la base de datos que la expide, encadenado \
         al anterior con su huella SHA-256, y lo remite a la Agencia Tributaria por el servicio VERI*FACTU con el \
         certificado electrónico del obligado tributario.\n\n\
         e) Solo puede funcionar como «VERI*FACTU»: Sí.\n\n\
         f) Permite ser usado por varios obligados tributarios: No.\n\n\
         g) Tipos de firma: no se firman los registros de facturación, porque el sistema solo funciona como \
         «VERI*FACTU» y los remite a la Agencia Tributaria con certificado electrónico cualificado.\n\n\
         h) Productor: {}\n\n\
         i) NIF del productor: {}\n\n\
         j) Dirección postal de contacto del productor: {}\n\n\
         k) El productor hace constar que el sistema informático, en la versión indicada, cumple con lo dispuesto \
         en el artículo 29.2.j) de la Ley 58/2003, de 17 de diciembre, General Tributaria, en el Reglamento aprobado \
         por el Real Decreto 1007/2023, de 5 de diciembre, en la Orden HAC/1177/2024, de 17 de octubre, y en la sede \
         electrónica de la Agencia Estatal de Administración Tributaria para todo aquello que complete las \
         especificaciones de dicha orden.\n\n\
         l) Fecha: {}. Lugar: {}.",
        or(p.name),
        or(p.nif),
        or(p.address),
        or(DECLARED_ON),
        or(DECLARED_IN),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // the worked examples in the AEAT's "Detalle de las especificaciones
    // técnicas para generación de la huella o hash de los registros de
    // facturación" v0.1.2: a first record, the one chained after it, and an
    // anulación
    const FIRST: &str = "3C464DAF61ACB827C65FDA19F352A4E3BDC2C640E9E9FC4CC058073F38F12F60";
    const SECOND: &str = "F7B94CFD8924EDFF273501B01EE5153E4CE8F259766F88CF6ACB8935802A2B97";
    const VOID: &str = "177547C0D57AC74748561D054A9CEC14B4C4EA23D1BEFD6F2E69E3A388F90C68";

    fn alta_key<'a>(number: &'a str, prev: &'a str, at: &'a str) -> AltaKey<'a> {
        AltaKey {
            issuer_nif: "89890001K",
            number,
            date: "01-01-2024",
            invoice_type: "F1",
            vat_total: "12.35",
            total: "123.45",
            prev,
            generated_at: at,
        }
    }

    #[test]
    fn huellas_match_the_aeat_examples() {
        assert_eq!(huella_alta(&alta_key("12345678/G33", "", "2024-01-01T19:20:30+01:00")), FIRST);
        assert_eq!(huella_alta(&alta_key("12345679/G34", FIRST, "2024-01-01T19:20:35+01:00")), SECOND);
        let void = AnulacionKey {
            issuer_nif: "89890001K",
            number: "12345679/G34",
            date: "01-01-2024",
            prev: SECOND,
            generated_at: "2024-01-01T19:20:40+01:00",
        };
        assert_eq!(huella_anulacion(&void), VOID);
        // the signed R3 example in the electronic-signature specification
        // v0.1.5, whose amounts are written with one decimal
        let r3 = AltaKey {
            issuer_nif: "89890001K",
            number: "12345678-G66",
            date: "03-02-2025",
            invoice_type: "R3",
            vat_total: "41.4",
            total: "241.4",
            prev: "C9AF4AF1EF5EBBA700350DE3EEF12C2D355C56AC56F13DB2A25E0031BD2B7ED5",
            generated_at: "2025-02-03T14:30:00+01:00",
        };
        assert_eq!(huella_alta(&r3), "FF954378B64ED331A9B2366AD317D86E9DEC1716B12DD0ACCB172A6DC4C105AA");
    }

    #[test]
    fn values_are_trimmed_before_hashing() {
        let padded = AltaKey { issuer_nif: " 89890001K ", number: "12345678/G33 ", ..alta_key("", "", "2024-01-01T19:20:30+01:00") };
        assert_eq!(huella_alta(&padded), FIRST);
    }

    #[test]
    fn record_formats() {
        assert_eq!(amount(1235), "12.35");
        assert_eq!(amount(-1380), "-13.80");
        assert_eq!(amount(5), "0.05");
        assert_eq!(amount(0), "0.00");
        assert_eq!(rate(2100), "21.00");
        assert_eq!(rate(0), "0.00");
        assert_eq!(date("2026-10-17T09:05:00"), "17-10-2026");
        let summer = chrono::TimeZone::with_ymd_and_hms(&Local, 2026, 7, 1, 12, 0, 0).unwrap();
        assert!(stamp(&summer).starts_with("2026-07-01T12:00:00"));
        assert_eq!(stamp(&summer).len(), 25);
        assert_eq!(esc("A&B <\"x\"> 'y'\u{7}"), "A&amp;B &lt;&quot;x&quot;&gt; &apos;y&apos;");
    }

    #[test]
    fn qr_url_matches_the_aeat_example() {
        // the encoded example in the QR specification v0.5.0, section 4.1
        assert_eq!(
            qr_url(Env::Test, "89890001K", "12345678&G33", "01-01-2024", "241.4"),
            "https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR?nif=89890001K&numserie=12345678%26G33&fecha=01-01-2024&importe=241.4"
        );
        assert_eq!(
            qr_url(Env::Prod, "89890001K", "12345678-G33", "01-09-2024", "241.4"),
            "https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR?nif=89890001K&numserie=12345678-G33&fecha=01-09-2024&importe=241.4"
        );
        assert_eq!(form_encode("a b/ñ"), "a+b%2F%C3%B1");
    }

    #[test]
    fn the_qr_prints_between_30_and_40_mm() {
        let url = qr_url(Env::Prod, "B12345674", "TFR2026-000001", "17-10-2026", "-1234567.89");
        let rows = qr_rows(&url).unwrap();
        assert!(rows.iter().all(|r| r.len() == rows.len()));
        // finder patterns in three corners
        assert!(rows[0].starts_with("1111111") && rows[0].ends_with("1111111") && rows[rows.len() - 1].starts_with("1111111"));
        for n in (21..=77).step_by(4) {
            let mm = (n * module_dots(n)) as f64 / 8.0;
            assert!((30.0..=40.0).contains(&mm), "{n} modules: {mm} mm");
        }
    }

    use crate::state::SessionUser;
    use crate::tickets::{PaymentIn, RefundIn, RefundLine, SaleLine, SaleOpts};
    use crate::{backup, db};

    fn shop(env: &str) -> Settings {
        Settings {
            shop_name: "Mi Tienda Móvil".into(),
            legal_name: "Juan Pérez García".into(),
            shop_nif: "B12345674".into(),
            verifactu: env.into(),
            ..Settings::default()
        }
    }

    fn tmp() -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let k = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("tiquet-vf-{}-{n}-{k}", std::process::id()))
    }

    fn setup(env: &str) -> (Connection, SessionUser, Settings, std::path::PathBuf) {
        let dir = tmp();
        let conn = db::open(&dir.join("tiquet.db")).unwrap();
        conn.execute("INSERT INTO users (name, role, pass_hash) VALUES ('Ana', 'admin', 'x')", []).unwrap();
        let s = shop(env);
        settings::put(&conn, "settings", &serde_json::to_string(&s).unwrap()).unwrap();
        (conn, SessionUser { id: 1, name: "Ana".into(), role: "admin".into() }, s, dir)
    }

    fn line(name: &str, qty: i64, unit: i64, vat_bp: i64) -> SaleLine {
        SaleLine { name: name.into(), qty, unit_cents: unit, vat_bp, ..SaleLine::default() }
    }

    fn cash() -> PaymentIn {
        PaymentIn { method: "cash".into(), tendered_cents: None }
    }

    #[derive(Debug)]
    struct Row {
        id: i64,
        kind: String,
        issuer_nif: String,
        number: String,
        date: String,
        invoice_type: Option<String>,
        vat_total: Option<String>,
        total: Option<String>,
        prev_hash: String,
        generated_at: String,
        hash: String,
        xml: String,
    }

    fn records(conn: &Connection) -> Vec<Row> {
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, issuer_nif, number, date, invoice_type, vat_total, total, prev_hash, generated_at, hash, xml
                   FROM fiscal_records ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                kind: r.get(1)?,
                issuer_nif: r.get(2)?,
                number: r.get(3)?,
                date: r.get(4)?,
                invoice_type: r.get(5)?,
                vat_total: r.get(6)?,
                total: r.get(7)?,
                prev_hash: r.get(8)?,
                generated_at: r.get(9)?,
                hash: r.get(10)?,
                xml: r.get(11)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    }

    // the huella worked out again from what the row says
    fn rehash(r: &Row) -> String {
        if r.kind == "alta" {
            huella_alta(&AltaKey {
                issuer_nif: &r.issuer_nif,
                number: &r.number,
                date: &r.date,
                invoice_type: r.invoice_type.as_deref().unwrap(),
                vat_total: r.vat_total.as_deref().unwrap(),
                total: r.total.as_deref().unwrap(),
                prev: &r.prev_hash,
                generated_at: &r.generated_at,
            })
        } else {
            huella_anulacion(&AnulacionKey {
                issuer_nif: &r.issuer_nif,
                number: &r.number,
                date: &r.date,
                prev: &r.prev_hash,
                generated_at: &r.generated_at,
            })
        }
    }

    #[test]
    fn every_sale_return_and_void_is_chained_in_the_aeat_format() {
        let (mut conn, user, s, _) = setup("test");
        let sale = tickets::finalize(&mut conn, &user, &s, vec![line("Cable USB-C", 2, 690, 2100), line("Pilas AA", 1, 450, 1000)], &cash()).unwrap();
        let acme = Customer { name: "Ana López & Hijos".into(), nif: "12345678Z".into(), address: "Rambla 5, Girona".into() };
        let opts = SaleOpts { customer: Some(acme), ..SaleOpts::default() };
        let (invoice, _) = tickets::sell(&mut conn, &user, &s, vec![line("Funda", 1, 1250, 2100)], &cash(), &opts).unwrap();
        let back = RefundIn {
            id: sale.id,
            lines: vec![RefundLine { line_id: sale.lines[1].id.unwrap(), qty: 1 }],
            method: "cash".into(),
            reason: "no cargan".into(),
            clock_ok: false,
        };
        let refund = tickets::refund(&mut conn, &user, &s, &back).unwrap();
        let oops = tickets::finalize(&mut conn, &user, &s, vec![line("Cable USB-C", 1, 690, 2100)], &cash()).unwrap();
        tickets::void(&mut conn, oops.id, "Ana", "cobrado dos veces").unwrap();
        // practice tickets are not invoices and leave nothing
        let practice = Settings { training: true, ..s.clone() };
        tickets::finalize(&mut conn, &user, &practice, vec![line("Cable USB-C", 1, 690, 2100)], &cash()).unwrap();

        let rows = records(&conn);
        let kinds: Vec<(&str, Option<&str>, &str)> =
            rows.iter().map(|r| (r.kind.as_str(), r.invoice_type.as_deref(), r.number.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                ("alta", Some("F2"), sale.number.as_str()),
                ("alta", Some("F1"), invoice.number.as_str()),
                ("alta", Some("R5"), refund.number.as_str()),
                ("alta", Some("F2"), oops.number.as_str()),
                ("anulacion", None, oops.number.as_str()),
            ]
        );
        assert!(rows[0].xml.contains("<sum1:Encadenamiento><sum1:PrimerRegistro>S</sum1:PrimerRegistro></sum1:Encadenamiento>"));
        assert_eq!(rows[0].prev_hash, "");
        for w in rows.windows(2) {
            assert_eq!(w[1].prev_hash, w[0].hash);
            let link = format!(
                "<sum1:RegistroAnterior><sum1:IDEmisorFactura>B12345674</sum1:IDEmisorFactura><sum1:NumSerieFactura>{}</sum1:NumSerieFactura><sum1:FechaExpedicionFactura>{}</sum1:FechaExpedicionFactura><sum1:Huella>{}</sum1:Huella></sum1:RegistroAnterior>",
                w[0].number, w[0].date, w[0].hash
            );
            assert!(w[1].xml.contains(&link), "{}", w[1].xml);
        }
        for r in &rows {
            assert_eq!(rehash(r), r.hash);
            assert!(r.xml.contains(&format!("<sum1:TipoHuella>01</sum1:TipoHuella><sum1:Huella>{}</sum1:Huella>", r.hash)));
            assert!(r.xml.contains("<sum1:IdSistemaInformatico>TQ</sum1:IdSistemaInformatico>"));
            assert!(r.xml.contains("<sum1:TipoUsoPosibleSoloVerifactu>S</sum1:TipoUsoPosibleSoloVerifactu>"));
            roxmltree::Document::parse(&format!("<x xmlns:sum1=\"urn:x\">{}</x>", r.xml)).unwrap();
        }
        // 13,80 at 21 % and 4,50 at 10 %, as the ticket's own VAT table says
        let f2 = &rows[0].xml;
        assert_eq!((rows[0].vat_total.as_deref(), rows[0].total.as_deref()), (Some("2.81"), Some("18.30")));
        assert!(f2.contains("<sum1:NombreRazonEmisor>Juan Pérez García</sum1:NombreRazonEmisor>"));
        assert!(f2.contains("<sum1:ClaveRegimen>01</sum1:ClaveRegimen><sum1:CalificacionOperacion>S1</sum1:CalificacionOperacion><sum1:TipoImpositivo>21.00</sum1:TipoImpositivo><sum1:BaseImponibleOimporteNoSujeto>11.40</sum1:BaseImponibleOimporteNoSujeto><sum1:CuotaRepercutida>2.40</sum1:CuotaRepercutida>"));
        assert!(f2.contains("<sum1:TipoImpositivo>10.00</sum1:TipoImpositivo><sum1:BaseImponibleOimporteNoSujeto>4.09</sum1:BaseImponibleOimporteNoSujeto><sum1:CuotaRepercutida>0.41</sum1:CuotaRepercutida>"));
        assert!(f2.contains("<sum1:DescripcionOperacion>Venta: Cable USB-C, Pilas AA</sum1:DescripcionOperacion>"));
        assert!(!f2.contains("Destinatarios") && !f2.contains("TipoRectificativa"));
        // the full invoice names its customer
        assert!(rows[1].xml.contains("<sum1:Destinatarios><sum1:IDDestinatario><sum1:NombreRazon>Ana López &amp; Hijos</sum1:NombreRazon><sum1:NIF>12345678Z</sum1:NIF></sum1:IDDestinatario></sum1:Destinatarios>"));
        // the return: by differences, negative, pointing at the ticket it corrects
        let r5 = &rows[2].xml;
        assert_eq!((rows[2].vat_total.as_deref(), rows[2].total.as_deref()), (Some("-0.41"), Some("-4.50")));
        assert!(r5.contains(&format!(
            "<sum1:TipoFactura>R5</sum1:TipoFactura><sum1:TipoRectificativa>I</sum1:TipoRectificativa><sum1:FacturasRectificadas><sum1:IDFacturaRectificada><sum1:IDEmisorFactura>B12345674</sum1:IDEmisorFactura><sum1:NumSerieFactura>{}</sum1:NumSerieFactura><sum1:FechaExpedicionFactura>{}</sum1:FechaExpedicionFactura></sum1:IDFacturaRectificada></sum1:FacturasRectificadas>",
            sale.number,
            date(&sale.issued_at)
        )));
        assert!(!r5.contains("Destinatarios") && !r5.contains("ImporteRectificacion"));
        // the void cancels the alta that went to the AEAT
        let void = &rows[4].xml;
        assert!(void.starts_with("<sum1:RegistroAnulacion><sum1:IDVersion>1.0</sum1:IDVersion><sum1:IDFactura><sum1:IDEmisorFacturaAnulada>B12345674</sum1:IDEmisorFacturaAnulada>"));
        assert!(!void.contains("SinRegistroPrevio"));

        // the ticket knows its QR and that it is still to be sent
        let t = tickets::load(&conn, sale.id).unwrap();
        let qr = format!("https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR?nif=B12345674&numserie={}&fecha={}&importe=18.30", sale.number, date(&sale.issued_at));
        assert_eq!(t.qr.as_deref(), Some(qr.as_str()));
        assert_eq!(t.aeat.unwrap().state, "pending");
        assert_eq!(tickets::load(&conn, refund.id).unwrap().qr.unwrap().split("importe=").nth(1), Some("-4.50"));
        // and nothing written can be changed or taken away
        assert!(conn.execute("UPDATE fiscal_records SET total = '0.00'", []).is_err());
        assert!(conn.execute("DELETE FROM fiscal_records", []).is_err());
        assert_eq!(pending(&conn, Env::Test, 1000).unwrap().len(), 5);
        assert!(pending(&conn, Env::Prod, 1000).unwrap().is_empty());
    }

    #[test]
    fn switched_off_nothing_is_recorded_and_an_older_ticket_is_voided_without_a_record() {
        let (mut conn, user, s, _) = setup("off");
        let t = tickets::finalize(&mut conn, &user, &s, vec![line("Funda", 1, 995, 2100)], &cash()).unwrap();
        assert!(records(&conn).is_empty());
        assert!(t.qr.is_none() && t.aeat.is_none());
        // switched on the same day, the void of that ticket says it was never sent
        settings::put(&conn, "settings", &serde_json::to_string(&shop("test")).unwrap()).unwrap();
        tickets::void(&mut conn, t.id, "Ana", "error").unwrap();
        let rows = records(&conn);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].xml.contains("<sum1:SinRegistroPrevio>S</sum1:SinRegistroPrevio>"), "{}", rows[0].xml);
    }

    #[test]
    fn refused_and_flagged_records_are_corrected_by_new_ones() {
        let (mut conn, user, s, _) = setup("test");
        tickets::finalize(&mut conn, &user, &s, vec![line("Funda", 1, 995, 2100)], &cash()).unwrap();
        tickets::finalize(&mut conn, &user, &s, vec![line("Cable", 1, 690, 2100)], &cash()).unwrap();
        let p = pending(&conn, Env::Test, 1000).unwrap();
        let refused = Answer { record_id: p[1].id, state: "Incorrecto".into(), code: Some("1100".into()), message: Some("Valor o tipo incorrecto".into()) };
        let ok = Answer { record_id: p[0].id, state: "Correcto".into(), code: None, message: None };
        answered(&conn, &[ok, refused], Some("A-CSV")).unwrap();
        let st = status(&conn, &s).unwrap();
        assert_eq!((st.pending, st.sent, st.problems.len()), (0, 2, 1));
        assert_eq!(st.problems[0].code.as_deref(), Some("1100"));

        // never stored at the AEAT: Subsanacion S, RechazoPrevio X
        let fixed = Settings { legal_name: "Juan Pérez García (corregido)".into(), ..s.clone() };
        let id = correct(&conn, p[1].id, &fixed).unwrap();
        let rows = records(&conn);
        let new = rows.last().unwrap();
        assert_eq!(new.id, id);
        assert!(new.xml.contains("<sum1:NombreRazonEmisor>Juan Pérez García (corregido)</sum1:NombreRazonEmisor><sum1:Subsanacion>S</sum1:Subsanacion><sum1:RechazoPrevio>X</sum1:RechazoPrevio><sum1:TipoFactura>F2</sum1:TipoFactura>"));
        assert_eq!(new.prev_hash, rows[rows.len() - 2].hash);
        assert_eq!(rehash(new), new.hash);
        let st = status(&conn, &s).unwrap();
        assert_eq!((st.pending, st.problems.len()), (1, 0));
        assert_eq!(correct(&conn, p[1].id, &fixed).unwrap_err().0, "E_NOT_FOUND");

        // stored with errors: Subsanacion S alone; a late clock (2004) needs nothing
        answered(&conn, &[Answer { record_id: id, state: "AceptadoConErrores".into(), code: Some("2005".into()), message: None }], None).unwrap();
        let again = correct(&conn, id, &fixed).unwrap();
        let rows = records(&conn);
        let xml = &rows.last().unwrap().xml;
        assert!(xml.contains("<sum1:Subsanacion>S</sum1:Subsanacion><sum1:TipoFactura>") && !xml.contains("RechazoPrevio"), "{xml}");
        answered(&conn, &[Answer { record_id: again, state: "AceptadoConErrores".into(), code: Some("2004".into()), message: None }], None).unwrap();
        assert!(status(&conn, &s).unwrap().problems.is_empty());
        let t = tickets::load(&conn, 2).unwrap();
        assert_eq!(t.aeat.unwrap().state, "AceptadoConErrores");
    }

    #[test]
    fn a_restored_backup_gets_back_the_records_made_after_it() {
        let (mut conn, user, s, dir) = setup("test");
        tickets::finalize(&mut conn, &user, &s, vec![line("Funda", 1, 995, 2100)], &cash()).unwrap();
        let file = backup::run(&conn, &dir.join("copias")).unwrap();
        for _ in 0..2 {
            tickets::finalize(&mut conn, &user, &s, vec![line("Cable", 1, 690, 2100)], &cash()).unwrap();
        }
        let first = pending(&conn, Env::Test, 1).unwrap()[0].id;
        answered(&conn, &[Answer { record_id: first, state: "Correcto".into(), code: None, message: None }], Some("A-CSV")).unwrap();
        let before = records(&conn);
        backup::restore(&mut conn, &file, &dir).unwrap();
        let after = records(&conn);
        assert_eq!(after.iter().map(|r| &r.hash).collect::<Vec<_>>(), before.iter().map(|r| &r.hash).collect::<Vec<_>>());
        // what the AEAT already answered stays answered, the rest still goes
        assert_eq!(pending(&conn, Env::Test, 1000).unwrap().len(), 2);
        // and the next ticket chains to the newest record, not to the backup's
        tickets::raise_floors(&conn, &tickets::series_marks(&conn).unwrap()).unwrap();
        let t = tickets::finalize(&mut conn, &user, &s, vec![line("Funda", 1, 995, 2100)], &cash()).unwrap();
        let rows = records(&conn);
        assert_eq!(rows.last().unwrap().number, t.number);
        assert_eq!(rows.last().unwrap().prev_hash, before.last().unwrap().hash);
    }

    #[test]
    fn real_records_tie_the_installation_to_one_nif() {
        let (mut conn, user, s, _) = setup("prod");
        tickets::finalize(&mut conn, &user, &s, vec![line("Funda", 1, 995, 2100)], &cash()).unwrap();
        let other = Settings { shop_nif: "12345678Z".into(), verifactu: "off".into(), ..s.clone() };
        assert_eq!(settings::save(&conn, other).unwrap_err().0, "E_NIF_LOCKED");
        let same = Settings { verifactu: "off".into(), ..s.clone() };
        settings::save(&conn, same).unwrap();
        if !producer_ready() {
            assert_eq!(settings::save(&conn, s.clone()).unwrap_err().0, "E_PRODUCER");
        }
    }

    #[test]
    fn rates_the_aeat_does_not_take_are_refused_while_it_is_on() {
        let (mut conn, user, s, _) = setup("test");
        let odd = vec![line("Revista", 1, 300, 700)];
        assert_eq!(tickets::finalize(&mut conn, &user, &s, odd.clone(), &cash()).unwrap_err().0, "E_VAT_AEAT");
        let off = Settings { verifactu: "off".into(), ..s.clone() };
        tickets::finalize(&mut conn, &user, &off, odd, &cash()).unwrap();
        tickets::finalize(&mut conn, &user, &s, vec![line("Libro", 1, 1500, 400), line("Gratis", 1, 100, 0)], &cash()).unwrap();
    }

    #[test]
    fn the_declaration_names_the_system_and_what_is_missing() {
        let d = declaration();
        assert!(d.starts_with("DECLARACIÓN RESPONSABLE DEL SISTEMA INFORMÁTICO DE FACTURACIÓN"));
        assert!(d.contains(&format!("c) Versión: {VERSION}")));
        assert!(d.contains("e) Solo puede funcionar como «VERI*FACTU»: Sí."));
        if !producer_ready() {
            assert!(d.contains("(pendiente)"));
        }
    }
}
