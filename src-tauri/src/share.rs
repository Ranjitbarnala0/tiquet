// A ticket sent to the customer instead of, or as well as, on paper: as text
// for WhatsApp, as a PDF to keep or attach, and by email.

use std::path::PathBuf;

use crate::layout;
use crate::money;
use crate::printer::Page;
use crate::settings::{Logo, Settings};
use crate::tickets::TicketRecord;

// narrow enough for a phone held upright
pub const TEXT_COLS: u32 = 32;

// A ticket that already reached paper goes out marked COPIA, like a reprint;
// one never printed goes out as the original.
fn copy(rec: &TicketRecord) -> bool {
    rec.prints.iter().any(|p| ["printed", "sent", "unknown"].contains(&p.status.as_str()))
}

pub fn text(rec: &TicketRecord, s: &Settings) -> String {
    let s = Settings { columns: TEXT_COLS, codepage: "cp1252".into(), ..s.clone() };
    let mut t = layout::plain(&layout::record(rec, &s, false, copy(rec)), TEXT_COLS as usize);
    if let Some(url) = &rec.qr {
        t.push_str("\n\nQR de la AEAT:\n");
        t.push_str(url);
    }
    t
}

// a full invoice is an A4 page, like any invoice; a ticket keeps its own shape
pub fn pdf(rec: &TicketRecord, s: &Settings, logo: Option<&Logo>) -> Vec<u8> {
    if rec.customer.is_some() {
        let opts = crate::invoice::InvoiceOpts { terms: s.invoice_terms.clone(), copy: copy(rec) };
        return crate::invoice::to_pdf(&crate::invoice::invoice(rec, logo, &opts), &rec.number);
    }
    let s = Settings { codepage: "cp1252".into(), ..s.clone() };
    let lines = layout::record(rec, &s, logo.is_some(), copy(rec));
    crate::pdf::render(&[Page { lines, logo: logo.cloned(), cols: s.columns as usize }], &rec.number)
}

pub fn file_name(number: &str) -> String {
    let safe: String = number.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    format!("Ticket-{safe}.pdf")
}

// Factura-TF2026-000001.pdf for a full invoice
pub fn file_name_of(rec: &TicketRecord) -> String {
    let name = file_name(&rec.number);
    if rec.customer.is_some() {
        name.replacen("Ticket-", "Factura-", 1)
    } else {
        name
    }
}

// Documents\Tiquet\Tickets\Ticket-T2026-000001.pdf, and the invoices in
// Documents\Tiquet\Facturas
pub fn save_pdf(rec: &TicketRecord, s: &Settings, logo: Option<&Logo>) -> crate::error::Result<PathBuf> {
    let dir = crate::backup::documents().join(if rec.customer.is_some() { "Facturas" } else { "Tickets" });
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(file_name_of(rec));
    std::fs::write(&path, pdf(rec, s, logo))?;
    Ok(path)
}

pub fn subject(rec: &TicketRecord) -> String {
    let what = if rec.customer.is_some() { "Factura" } else { "Ticket" };
    let shop = rec.shop.name.trim();
    if shop.is_empty() {
        format!("{what} {}", rec.number)
    } else {
        format!("{what} {} · {shop}", rec.number)
    }
}

pub fn email_body(rec: &TicketRecord) -> String {
    let shop = rec.shop.name.trim();
    let mut b = format!("Hola:\n\nLe enviamos el {} {} por {} €.", if rec.customer.is_some() { "documento" } else { "ticket" }, rec.number, money::fmt(rec.total_cents));
    b.push_str("\nLo encontrará adjunto en PDF.\n\nGracias por su visita.");
    if !shop.is_empty() {
        b.push_str(&format!("\n{shop}"));
    }
    let phone = rec.shop.phone.trim();
    if !phone.is_empty() {
        b.push_str(&format!("\nTel. {phone}"));
    }
    b
}

// A phone number as WhatsApp takes it: country code and number, digits only.
// Nine digits starting with 6, 7, 8 or 9 are Spanish.
pub fn phone(raw: &str) -> Option<String> {
    let t = raw.trim();
    let mut d: String = t.chars().filter(char::is_ascii_digit).collect();
    if t.starts_with("00") {
        d.drain(..2);
    }
    if d.len() == 9 && d.starts_with(['6', '7', '8', '9']) {
        d.insert_str(0, "34");
    }
    (8..=15).contains(&d.len()).then_some(d)
}

// percent-encoding for a query value: only the unreserved characters stay
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// The WhatsApp app when this PC has it, WhatsApp Web otherwise; the ticket
// goes in a monospaced block so its columns line up on the phone.
pub fn whatsapp_url(phone: &str, message: &str, app: bool) -> String {
    if app {
        format!("whatsapp://send?phone={phone}&text={}", encode(message))
    } else {
        format!("https://web.whatsapp.com/send?phone={phone}&text={}", encode(message))
    }
}

pub fn whatsapp_ticket(text: &str) -> String {
    format!("```\n{text}\n```")
}

// for the mail app this PC has; mail clients cut long links, so the body is
// left short and the PDF is offered to attach
pub fn mailto_url(to: &str, subject: &str, body: &str) -> String {
    let to: String = to.trim().chars().filter(|c| !c.is_whitespace() && !matches!(c, '?' | '&' | '#')).collect();
    format!("mailto:{to}?subject={}&body={}", encode(subject), encode(&body.replace('\n', "\r\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_numbers_get_their_country_code() {
        assert_eq!(phone("612 34 56 78").as_deref(), Some("34612345678"));
        assert_eq!(phone("+34 612-345-678").as_deref(), Some("34612345678"));
        assert_eq!(phone("0034612345678").as_deref(), Some("34612345678"));
        assert_eq!(phone("+44 7700 900123").as_deref(), Some("447700900123"));
        assert_eq!(phone("12345"), None);
        assert_eq!(phone(""), None);
    }

    #[test]
    fn links_carry_the_text_safely() {
        assert_eq!(encode("Total: 12,50 €\n&"), "Total%3A%2012%2C50%20%E2%82%AC%0A%26");
        let u = whatsapp_url("34612345678", "a b", false);
        assert_eq!(u, "https://web.whatsapp.com/send?phone=34612345678&text=a%20b");
        assert!(whatsapp_url("34612345678", "x", true).starts_with("whatsapp://send?phone=34612345678&text="));
        let m = mailto_url(" ana@example.com?cc=x ", "Ticket T1", "Hola\nAdiós");
        assert_eq!(m, "mailto:ana@example.comcc=x?subject=Ticket%20T1&body=Hola%0D%0AAdi%C3%B3s");
    }
}
