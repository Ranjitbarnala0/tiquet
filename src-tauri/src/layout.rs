// Turns a ticket into fixed-width lines. The same lines feed the on-screen
// preview and the ESC/POS encoder, so what the cashier sees is what prints.

use serde::Serialize;

use crate::cash::{Drawer, Session};
use crate::codepage::{printable, CodePage};
use crate::money::{div_round, fmt, fmt_rate};
use crate::reports::Summary;
use crate::settings::Settings;
use crate::tickets::{compute, now, Customer, Payment, SaleLine, ShopInfo, TicketRecord, Totals};

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    Normal,
    Tall,
    Big,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Line {
    Text { text: String, bold: bool, size: Size },
    Logo,
}

pub fn len(s: &str) -> usize {
    s.chars().count()
}

fn pad_right(s: &str, w: usize) -> String {
    let n = len(s);
    if n >= w {
        s.chars().take(w).collect()
    } else {
        format!("{s}{}", " ".repeat(w - n))
    }
}

fn pad_left(s: &str, w: usize) -> String {
    let n = len(s);
    if n >= w {
        s.to_string()
    } else {
        format!("{}{s}", " ".repeat(w - n))
    }
}

pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in s.split('\n') {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let mut word = word.to_string();
            while len(&word) > w {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(word.chars().take(w).collect());
                word = word.chars().skip(w).collect();
            }
            if word.is_empty() {
                continue;
            }
            if cur.is_empty() {
                cur = word;
            } else if len(&cur) + 1 + len(&word) <= w {
                cur.push(' ');
                cur.push_str(&word);
            } else {
                out.push(std::mem::replace(&mut cur, word));
            }
        }
        out.push(cur);
    }
    out
}

pub fn datetime(ts: &str) -> String {
    if ts.len() >= 16 && ts.is_char_boundary(16) {
        format!("{}/{}/{} {}", &ts[8..10], &ts[5..7], &ts[0..4], &ts[11..16])
    } else {
        ts.to_string()
    }
}

pub fn date(d: &str) -> String {
    if d.len() >= 10 && d.is_char_boundary(10) {
        format!("{}/{}/{}", &d[8..10], &d[5..7], &d[0..4])
    } else {
        d.to_string()
    }
}

pub fn method_label(m: &str) -> &'static str {
    match m {
        "cash" => "Efectivo",
        "card" => "Tarjeta",
        "bizum" => "Bizum",
        _ => "Otro",
    }
}

fn refund_label(m: &str) -> &'static str {
    match m {
        "cash" => "Devuelto en efectivo",
        "card" => "Devuelto a la tarjeta",
        "bizum" => "Devuelto por Bizum",
        _ => "Devuelto",
    }
}

pub struct Doc {
    pub cols: usize,
    cp: CodePage,
    pub lines: Vec<Line>,
}

impl Doc {
    pub fn new(s: &Settings) -> Doc {
        Doc { cols: s.columns as usize, cp: CodePage::parse(&s.codepage), lines: Vec::new() }
    }

    fn width(&self, size: Size) -> usize {
        if size == Size::Big {
            self.cols / 2
        } else {
            self.cols
        }
    }

    fn clean(&self, s: &str) -> String {
        printable(s, self.cp)
    }

    // every line goes through here; anything wider than the paper is split
    // so the printer never wraps on its own
    fn raw(&mut self, text: String, bold: bool, size: Size) {
        let w = self.width(size);
        if len(&text) <= w {
            self.lines.push(Line::Text { text, bold, size });
            return;
        }
        let chars: Vec<char> = text.chars().collect();
        for chunk in chars.chunks(w) {
            self.lines.push(Line::Text { text: chunk.iter().collect(), bold, size });
        }
    }

    pub fn blank(&mut self) {
        self.raw(String::new(), false, Size::Normal);
    }

    pub fn rule(&mut self, c: char) {
        self.raw(c.to_string().repeat(self.cols), false, Size::Normal);
    }

    pub fn logo(&mut self) {
        self.lines.push(Line::Logo);
    }

    pub fn left(&mut self, s: &str, bold: bool, size: Size) {
        let w = self.width(size);
        for l in wrap(&self.clean(s), w) {
            self.raw(l, bold, size);
        }
    }

    pub fn center(&mut self, s: &str, bold: bool, size: Size) {
        let w = self.width(size);
        for l in wrap(&self.clean(s), w) {
            let pad = (w - len(&l)) / 2;
            self.raw(format!("{}{l}", " ".repeat(pad)), bold, size);
        }
    }

    pub fn pair(&mut self, l: &str, r: &str, bold: bool, size: Size) {
        let w = self.width(size);
        let (l, r) = (self.clean(l), self.clean(r));
        if len(&r) >= w {
            self.left(&l, bold, size);
            self.raw(r, bold, size);
            return;
        }
        let mut parts = wrap(&l, w);
        let last = parts.pop().unwrap_or_default();
        for p in parts {
            self.raw(p, bold, size);
        }
        if len(&last) + 1 + len(&r) <= w {
            self.raw(format!("{}{r}", pad_right(&last, w - len(&r))), bold, size);
        } else {
            self.raw(last, bold, size);
            self.raw(pad_left(&r, w), bold, size);
        }
    }
}

pub struct TicketView<'a> {
    pub shop: &'a ShopInfo,
    pub kind: &'a str,
    pub number: &'a str,
    pub issued_at: &'a str,
    pub user_name: &'a str,
    pub lines: &'a [SaleLine],
    pub totals: &'a Totals,
    pub payment: Option<&'a Payment>,
    // number and date of the sale a refund corrects
    pub refund_of: Option<(&'a str, &'a str)>,
    pub reason: Option<&'a str>,
    pub show_vat: bool,
    pub copy: bool,
    pub voided: bool,
    // a full invoice is made out to someone
    pub customer: Option<&'a Customer>,
    pub training: bool,
}

fn header(d: &mut Doc, shop: &ShopInfo) {
    let name = shop.name.trim();
    if !name.is_empty() {
        let size = if len(name) <= d.cols / 2 { Size::Big } else { Size::Tall };
        d.center(name, true, size);
    }
    // the trade name is not enough on an invoice: the owner's name goes too
    let legal = shop.legal_name.trim();
    if !legal.is_empty() && !legal.eq_ignore_ascii_case(name) {
        d.center(legal, false, Size::Normal);
    }
    for l in shop.address.lines().map(str::trim).filter(|l| !l.is_empty()) {
        d.center(l, false, Size::Normal);
    }
    if !shop.phone.trim().is_empty() {
        d.center(&format!("Tel. {}", shop.phone.trim()), false, Size::Normal);
    }
    if !shop.nif.trim().is_empty() {
        d.center(&format!("NIF: {}", shop.nif.trim()), false, Size::Normal);
    }
}

// a full invoice states each unit price without VAT
fn net_unit(l: &SaleLine) -> String {
    format!("P.U. sin IVA {}", fmt(div_round(l.unit_cents * 10_000, 10_000 + l.vat_bp)))
}

fn items(d: &mut Doc, lines: &[SaleLine], full: bool) {
    if lines.is_empty() {
        d.center("(sin artículos)", false, Size::Normal);
        return;
    }
    if d.cols < 42 {
        for l in lines {
            item_two_lines(d, l);
            if full {
                d.left(&format!("  {}", net_unit(l)), false, Size::Normal);
            }
        }
        return;
    }
    // returns carry negative quantities, so the column grows to fit "-999"
    let qw = lines.iter().map(|l| len(&l.qty.to_string())).max().unwrap_or(0).max(3);
    let desc = d.cols - qw - 1 - 9 - 9;
    let head = format!(
        "{} {}{}{}",
        pad_right("UDS", qw),
        pad_right(&d.clean("DESCRIPCIÓN"), desc),
        pad_left("PRECIO", 9),
        pad_left("IMPORTE", 9)
    );
    d.raw(head, false, Size::Normal);
    for l in lines {
        let price = fmt(l.unit_cents);
        let total = fmt(l.total());
        if len(&price) > 8 || len(&total) > 8 {
            item_two_lines(d, l);
            if full {
                d.left(&format!("  {}", net_unit(l)), false, Size::Normal);
            }
            continue;
        }
        let mut name = wrap(&d.clean(&l.name), desc).into_iter();
        let first = name.next().unwrap_or_default();
        d.raw(
            format!("{:>qw$} {}{}{}", l.qty, pad_right(&first, desc), pad_left(&price, 9), pad_left(&total, 9)),
            false,
            Size::Normal,
        );
        for rest in name {
            d.raw(format!("{}{rest}", " ".repeat(qw + 1)), false, Size::Normal);
        }
        if full {
            d.raw(format!("{}{}", " ".repeat(qw + 1), net_unit(l)), false, Size::Normal);
        }
    }
}

fn item_two_lines(d: &mut Doc, l: &SaleLine) {
    d.left(&l.name, false, Size::Normal);
    d.pair(&format!("  {} x {}", l.qty, fmt(l.unit_cents)), &fmt(l.total()), false, Size::Normal);
}

fn vat_table(d: &mut Doc, t: &Totals) {
    let w = ((d.cols - 6) / 3).min(12);
    let first = d.cols - 3 * w;
    let row = |a: &str, b: &str, c: &str, e: &str| {
        format!("{}{}{}{}", pad_right(a, first), pad_left(b, w), pad_left(c, w), pad_left(e, w))
    };
    d.raw(row("IVA", "BASE", "CUOTA", "TOTAL"), false, Size::Normal);
    for r in &t.vat {
        let line = row(&fmt_rate(r.vat_bp), &fmt(r.base_cents), &fmt(r.vat_cents), &fmt(r.total_cents));
        d.raw(line, false, Size::Normal);
    }
}

pub fn ticket(v: &TicketView, s: &Settings, logo: bool) -> Vec<Line> {
    let mut d = Doc::new(s);
    if logo {
        d.logo();
    }
    if v.training {
        d.center("*** PRUEBA · SIN VALIDEZ ***", true, Size::Normal);
    }
    header(&mut d, v.shop);
    d.blank();
    let refund = v.kind == "refund";
    let full = v.customer.is_some();
    let title = match (full, refund) {
        (false, false) => "FACTURA SIMPLIFICADA",
        (false, true) => "FACTURA SIMPLIFICADA RECTIFICATIVA",
        (true, false) => "FACTURA",
        (true, true) => "FACTURA RECTIFICATIVA",
    };
    d.center(title, true, Size::Normal);
    d.pair(&format!("Nº {}", v.number), &datetime(v.issued_at), false, Size::Normal);
    if let Some(c) = v.customer {
        d.left(&format!("Cliente: {}", c.name), false, Size::Normal);
        d.left(&format!("NIF: {}", c.nif), false, Size::Normal);
        for l in c.address.lines().map(str::trim).filter(|l| !l.is_empty()) {
            d.left(l, false, Size::Normal);
        }
    }
    if let Some((number, at)) = v.refund_of {
        d.left(&format!("Rectifica a: {number} del {}", date(at)), false, Size::Normal);
    }
    if let Some(r) = v.reason.map(str::trim).filter(|r| !r.is_empty()) {
        d.left(&format!("Motivo: {r}"), false, Size::Normal);
    }
    if !v.user_name.trim().is_empty() {
        d.left(&format!("Le atendió: {}", v.user_name.trim()), false, Size::Normal);
    }
    if v.copy {
        d.center("*** COPIA ***", true, Size::Normal);
    }
    if v.voided {
        d.center("*** TICKET ANULADO ***", true, Size::Normal);
    }
    d.rule('-');
    items(&mut d, v.lines, full);
    d.rule('-');
    d.pair("TOTAL", &format!("{} €", fmt(v.totals.total_cents)), true, Size::Big);
    if let Some(p) = v.payment {
        d.rule('-');
        if refund {
            d.pair(refund_label(&p.method), &fmt(p.tendered_cents.abs()), false, Size::Normal);
        } else {
            d.pair(method_label(&p.method), &fmt(p.tendered_cents), false, Size::Normal);
            if p.method == "cash" {
                d.pair("Cambio", &fmt(p.change_cents), false, Size::Normal);
            }
        }
    }
    let table = (v.show_vat || full) && !v.totals.vat.is_empty();
    if table {
        d.rule('-');
        vat_table(&mut d, v.totals);
    }
    d.rule('-');
    let count = if refund {
        format!("Artículos devueltos: {}", -v.totals.item_count)
    } else {
        format!("Artículos: {}", v.totals.item_count)
    };
    // without the table the rate is still printed: a simplified invoice must
    // state the VAT rate applied
    let rates: Vec<String> = v.totals.vat.iter().map(|r| fmt_rate(r.vat_bp)).collect();
    let note = if table || rates.is_empty() { "IVA incluido".to_string() } else { format!("IVA {} incluido", rates.join(" y ")) };
    d.pair(&count, &note, false, Size::Normal);
    let footer = v.shop.footer.trim();
    if !footer.is_empty() {
        d.blank();
        for l in footer.lines() {
            if l.trim().is_empty() {
                d.blank();
            } else {
                d.center(l.trim(), false, Size::Normal);
            }
        }
    }
    if v.training {
        d.blank();
        d.center("*** PRUEBA · SIN VALIDEZ ***", true, Size::Normal);
    }
    d.lines
}

// a reprint shows the totals and VAT stored when the ticket was issued
pub fn record(r: &TicketRecord, s: &Settings, logo: bool, copy: bool) -> Vec<Line> {
    let vat = if r.vat.is_empty() { compute(&r.lines).vat } else { r.vat.clone() };
    let totals = Totals { total_cents: r.total_cents, item_count: r.item_count, vat };
    let v = TicketView {
        shop: &r.shop,
        kind: &r.kind,
        number: &r.number,
        issued_at: &r.issued_at,
        user_name: &r.user_name,
        lines: &r.lines,
        totals: &totals,
        payment: Some(&r.payment),
        refund_of: r.refund_of.as_ref().map(|t| (t.number.as_str(), t.issued_at.as_str())),
        reason: r.refund_reason.as_deref(),
        show_vat: r.shop.show_vat.unwrap_or(s.show_vat),
        copy,
        voided: r.voided_at.is_some(),
        customer: r.customer.as_ref(),
        training: r.training,
    };
    ticket(&v, s, logo)
}

pub fn summary(sum: &Summary, from: &str, to: &str, s: &Settings, user: &str) -> Vec<Line> {
    let mut d = Doc::new(s);
    if !s.shop_name.trim().is_empty() {
        d.center(s.shop_name.trim(), true, Size::Tall);
    }
    d.center("RESUMEN DE VENTAS", true, Size::Normal);
    if from == to {
        d.center(&date(from), false, Size::Normal);
    } else {
        d.center(&format!("Del {} al {}", date(from), date(to)), false, Size::Normal);
    }
    d.rule('-');
    d.pair("Tickets", &sum.tickets.to_string(), false, Size::Normal);
    if let (Some(a), Some(b)) = (&sum.first_number, &sum.last_number) {
        d.pair("Primero", a, false, Size::Normal);
        d.pair("Último", b, false, Size::Normal);
    }
    d.pair("Artículos", &sum.items.to_string(), false, Size::Normal);
    if sum.voided > 0 {
        d.pair("Anulados", &sum.voided.to_string(), false, Size::Normal);
    }
    if sum.refunds > 0 {
        d.rule('-');
        d.pair("Ventas", &fmt(sum.sales_cents), false, Size::Normal);
        d.pair(&format!("Devoluciones ({})", sum.refunds), &fmt(sum.refund_cents), false, Size::Normal);
    }
    if sum.discounts > 0 || sum.changed > 0 {
        d.rule('-');
        if sum.discounts > 0 {
            d.pair(&format!("Descuentos ({})", sum.discounts), &fmt(sum.discount_cents), false, Size::Normal);
        }
        if sum.changed > 0 {
            d.pair(&format!("Precios cambiados ({})", sum.changed), &fmt(sum.changed_cents), false, Size::Normal);
        }
    }
    if !sum.by_method.is_empty() {
        d.rule('-');
        for m in &sum.by_method {
            d.pair(&format!("{} ({})", method_label(&m.method), m.count), &fmt(m.total_cents), false, Size::Normal);
        }
    }
    d.rule('-');
    d.pair("TOTAL", &format!("{} €", fmt(sum.total_cents)), true, Size::Big);
    if !sum.vat.is_empty() {
        d.rule('-');
        vat_table(
            &mut d,
            &Totals { total_cents: sum.total_cents, item_count: sum.items, vat: sum.vat.clone() },
        );
    }
    d.rule('-');
    d.left(&format!("Impreso: {} · {}", datetime(&now()), user), false, Size::Normal);
    d.lines
}

// the Z report: what the drawer should hold against what was counted
pub fn cash_close(s: &Settings, session: &Session, drawer: &Drawer, sum: &Summary, user: &str) -> Vec<Line> {
    let mut d = Doc::new(s);
    if !s.shop_name.trim().is_empty() {
        d.center(s.shop_name.trim(), true, Size::Tall);
    }
    d.center("CIERRE DE CAJA", true, Size::Normal);
    d.rule('-');
    d.pair("Abierta", &format!("{} · {}", datetime(&session.opened_at), session.opened_by), false, Size::Normal);
    if let (Some(at), Some(by)) = (&session.closed_at, &session.closed_by) {
        d.pair("Cerrada", &format!("{} · {by}", datetime(at)), false, Size::Normal);
    }
    d.rule('-');
    d.pair("Tickets", &sum.tickets.to_string(), false, Size::Normal);
    if let (Some(a), Some(b)) = (&sum.first_number, &sum.last_number) {
        d.pair("Primero", a, false, Size::Normal);
        d.pair("Último", b, false, Size::Normal);
    }
    if sum.refunds > 0 {
        d.pair(&format!("Devoluciones ({})", sum.refunds), &fmt(sum.refund_cents), false, Size::Normal);
    }
    if sum.voided > 0 {
        d.pair("Anulados", &sum.voided.to_string(), false, Size::Normal);
    }
    for m in &sum.by_method {
        d.pair(&format!("{} ({})", method_label(&m.method), m.count), &fmt(m.total_cents), false, Size::Normal);
    }
    d.rule('-');
    d.pair("TOTAL", &format!("{} €", fmt(sum.total_cents)), true, Size::Big);
    if !sum.vat.is_empty() {
        d.rule('-');
        vat_table(&mut d, &Totals { total_cents: sum.total_cents, item_count: sum.items, vat: sum.vat.clone() });
    }
    d.rule('=');
    d.center("EFECTIVO", true, Size::Normal);
    d.pair("Fondo inicial", &fmt(session.float_cents), false, Size::Normal);
    d.pair("Ventas en efectivo", &fmt(drawer.cash_cents), false, Size::Normal);
    for m in &drawer.moves {
        d.pair(&m.reason, &fmt(m.cents), false, Size::Normal);
    }
    d.pair("Debería haber", &fmt(drawer.expected_cents), true, Size::Normal);
    if let Some(counted) = session.counted_cents {
        d.pair("Contado", &fmt(counted), true, Size::Normal);
        let diff = counted - drawer.expected_cents;
        let label = match diff {
            0 => "Cuadra",
            x if x > 0 => "Sobra",
            _ => "Falta",
        };
        d.pair(label, &fmt(diff.abs()), true, Size::Normal);
    }
    if let Some(note) = session.note.as_deref().filter(|n| !n.trim().is_empty()) {
        d.left(&format!("Nota: {note}"), false, Size::Normal);
    }
    d.rule('-');
    d.left(&format!("Impreso: {} · {}", datetime(&now()), user), false, Size::Normal);
    d.lines
}

pub fn recovery_slip(s: &Settings, code: &str, user: &str) -> Vec<Line> {
    let mut d = Doc::new(s);
    if !s.shop_name.trim().is_empty() {
        d.center(s.shop_name.trim(), true, Size::Tall);
    }
    d.center("CÓDIGO DE RECUPERACIÓN", true, Size::Normal);
    d.rule('=');
    let size = if len(code) <= d.cols / 2 { Size::Big } else { Size::Tall };
    d.center(code, true, size);
    d.rule('=');
    d.left(
        "Sirve para poner una contraseña nueva de administrador en Tiquet si se olvida. Guárdalo fuera de la tienda y no lo enseñes.",
        false,
        Size::Normal,
    );
    d.blank();
    d.left("Al usarlo, Tiquet da un código nuevo y este deja de valer.", false, Size::Normal);
    d.blank();
    d.center(&format!("{} · {}", datetime(&now()), user), false, Size::Normal);
    d.lines
}

pub fn test_page(s: &Settings, user: &str, logo: bool) -> Vec<Line> {
    let mut d = Doc::new(s);
    if logo {
        d.logo();
    }
    d.center("PRUEBA DE IMPRESIÓN", true, Size::Big);
    d.blank();
    d.pair("Impresora", &s.printer_name, false, Size::Normal);
    d.pair("Ancho", &format!("{} columnas", s.columns), false, Size::Normal);
    d.pair("Caracteres", &s.codepage.to_uppercase(), false, Size::Normal);
    d.rule('=');
    let ruler: String = (1..=d.cols).map(|i| char::from(b'0' + (i % 10) as u8)).collect();
    d.raw(ruler, false, Size::Normal);
    d.left("Acentos: áéíóú ÁÉÍÓÚ ñÑ çÇ üÜ", false, Size::Normal);
    d.left("Signos: € ¿? ¡! ºª", false, Size::Normal);
    d.rule('-');
    d.left("Texto en negrita", true, Size::Normal);
    d.left("Doble alto", false, Size::Tall);
    d.left("Grande", true, Size::Big);
    d.rule('-');
    d.center("Si las tildes y el símbolo € salen bien,", false, Size::Normal);
    d.center("la impresora está lista.", false, Size::Normal);
    d.blank();
    d.center(&format!("{} · {}", datetime(&now()), user), false, Size::Normal);
    d.lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tickets::sample_lines;

    fn texts(lines: &[Line]) -> Vec<(String, Size)> {
        lines
            .iter()
            .filter_map(|l| match l {
                Line::Text { text, size, .. } => Some((text.clone(), *size)),
                Line::Logo => None,
            })
            .collect()
    }

    fn shop() -> ShopInfo {
        ShopInfo {
            name: "Mi Tienda Móvil".into(),
            address: "Plaça Major, 1\n17600 Figueres (Girona)".into(),
            phone: "972 000 000".into(),
            nif: "B00000000".into(),
            footer: "¡Gracias por su visita!\n\nCambios con ticket en 15 días".into(),
            ..ShopInfo::default()
        }
    }

    fn render(cols: u32, codepage: &str, lines: &[SaleLine]) -> Vec<Line> {
        let s = Settings { columns: cols, codepage: codepage.into(), ..Settings::default() };
        let totals = compute(lines);
        let pay = Payment { method: "cash".into(), tendered_cents: 5000, change_cents: 5000 - totals.total_cents };
        let shop = shop();
        let v = TicketView {
            shop: &shop,
            kind: "sale",
            number: "T2026-000124",
            issued_at: "2026-10-16T17:00:12",
            user_name: "Ana",
            lines,
            totals: &totals,
            payment: Some(&pay),
            refund_of: None,
            reason: None,
            show_vat: s.show_vat,
            copy: true,
            voided: false,
            customer: None,
            training: false,
        };
        ticket(&v, &s, true)
    }

    fn render_refund(cols: u32, lines: &[SaleLine]) -> Vec<Line> {
        let s = Settings { columns: cols, ..Settings::default() };
        let totals = compute(lines);
        let pay = Payment { method: "cash".into(), tendered_cents: totals.total_cents, change_cents: 0 };
        let shop = shop();
        let v = TicketView {
            shop: &shop,
            kind: "refund",
            number: "TR2026-000003",
            issued_at: "2026-10-17T10:30:00",
            user_name: "Ana",
            lines,
            totals: &totals,
            payment: Some(&pay),
            refund_of: Some(("T2026-000124", "2026-10-16T17:00:12")),
            reason: Some("No funciona la carga rápida"),
            show_vat: true,
            copy: false,
            voided: false,
            customer: None,
            training: false,
        };
        ticket(&v, &s, false)
    }

    #[test]
    fn a_full_invoice_names_the_customer_and_states_net_prices() {
        let s = Settings { show_vat: false, ..Settings::default() };
        let lines = sample_lines(2100);
        let totals = compute(&lines);
        let pay = Payment { method: "card".into(), tendered_cents: totals.total_cents, change_cents: 0 };
        let shop = ShopInfo { legal_name: "Juan Pérez García".into(), ..shop() };
        let acme = Customer { name: "Acme Telecom SL".into(), nif: "B12345674".into(), address: "Rambla 5\n17001 Girona".into() };
        let v = TicketView {
            shop: &shop,
            kind: "sale",
            number: "TF2026-000001",
            issued_at: "2026-10-17T12:00:00",
            user_name: "Ana",
            lines: &lines,
            totals: &totals,
            payment: Some(&pay),
            refund_of: None,
            reason: None,
            show_vat: false,
            copy: false,
            voided: false,
            customer: Some(&acme),
            training: false,
        };
        let body: Vec<String> = texts(&ticket(&v, &s, false)).into_iter().map(|(t, _)| t).collect();
        let has = |x: &str| assert!(body.iter().any(|l| l.contains(x)), "missing {x:?} in\n{}", body.join("\n"));
        has("Juan Pérez García");
        has("FACTURA");
        has("Cliente: Acme Telecom SL");
        has("NIF: B12345674");
        has("17001 Girona");
        has("P.U. sin IVA 5,70");
        // the VAT table prints even with the breakdown switched off
        assert!(body.iter().any(|l| l.starts_with("21%")), "{}", body.join("\n"));
        assert!(!body.iter().any(|l| l.contains("SIMPLIFICADA")));
    }

    #[test]
    fn a_simplified_ticket_without_the_table_still_states_the_rate() {
        let s = Settings { show_vat: false, ..Settings::default() };
        let body: Vec<String> = texts(&render(48, "cp858", &sample_lines(2100))).into_iter().map(|(t, _)| t).collect();
        let _ = s;
        assert!(body.iter().any(|l| l.ends_with("IVA incluido")));
        let mut lines = sample_lines(2100);
        lines.push(SaleLine { name: "Pilas".into(), qty: 1, unit_cents: 450, vat_bp: 1000, ..SaleLine::default() });
        let totals = compute(&lines);
        let shop = shop();
        let v = TicketView {
            shop: &shop,
            kind: "sale",
            number: "T2026-000009",
            issued_at: "2026-10-17T12:00:00",
            user_name: "Ana",
            lines: &lines,
            totals: &totals,
            payment: None,
            refund_of: None,
            reason: None,
            show_vat: false,
            copy: false,
            voided: false,
            customer: None,
            training: true,
        };
        let body: Vec<String> = texts(&ticket(&v, &Settings { show_vat: false, ..Settings::default() }, false)).into_iter().map(|(t, _)| t).collect();
        assert!(body.iter().any(|l| l.ends_with("IVA 21% y 10% incluido")), "{}", body.join("\n"));
        assert_eq!(body.iter().filter(|l| l.contains("PRUEBA")).count(), 2);
    }

    #[test]
    fn no_line_is_wider_than_the_paper() {
        let mut lines = sample_lines(2100);
        lines.push(SaleLine {
            name: "Funda con un nombre larguísimo que no cabe en una sola línea del ticket de ningún modo".into(),
            qty: 12,
            unit_cents: 1250,
            vat_bp: 1000,
            ..SaleLine::default()
        });
        lines.push(SaleLine { name: "Pantalla".into(), qty: 999, unit_cents: 9_999_999, vat_bp: 2100, ..SaleLine::default() });
        lines.push(SaleLine {
            name: "Supercalifragilisticoespialidoso".repeat(3),
            qty: 1,
            unit_cents: -100,
            vat_bp: 0,
            ..SaleLine::default()
        });
        let back: Vec<SaleLine> = lines.iter().map(|l| SaleLine { qty: -l.qty, ..l.clone() }).collect();
        for cols in [32, 42, 48] {
            let mut pages = Vec::new();
            for cp in ["cp858", "cp1252", "cp850", "ascii"] {
                pages.push(render(cols, cp, &lines));
            }
            pages.push(render_refund(cols, &back));
            for page in pages {
                for (text, size) in texts(&page) {
                    let w = if size == Size::Big { cols as usize / 2 } else { cols as usize };
                    assert!(len(&text) <= w, "{cols}: {text:?} is {} > {w}", len(&text));
                }
            }
        }
    }

    #[test]
    fn ticket_reads_like_a_receipt() {
        let out = texts(&render(48, "cp858", &sample_lines(2100)));
        let body: Vec<String> = out.iter().map(|(t, _)| t.clone()).collect();
        let has = |s: String| assert!(body.contains(&s), "missing {s:?} in\n{}", body.join("\n"));
        let sp = |n: usize| " ".repeat(n);

        assert_eq!(out[0], (format!("{}Mi Tienda Móvil", sp(4)), Size::Big));
        has(format!("{}NIF: B00000000", sp(17)));
        has(format!("Nº T2026-000124{}16/10/2026 17:00", sp(17)));
        has(format!("{}*** COPIA ***", sp(17)));
        has(format!("Efectivo{}50,00", sp(35)));
        has(format!("Cambio{}28,25", sp(37)));
        has(format!("Artículos: 3{}IVA incluido", sp(24)));
        has(format!("TOTAL{}21,75 €", sp(12)));
        has("    cristal templado".into());

        let row = |start: &str| body.iter().find(|l| l.starts_with(start)).cloned().unwrap();
        let cable = row("  2 Cable USB-C 1 m ");
        assert!(cable.ends_with("     6,90    13,80") && len(&cable) == 48, "{cable:?}");
        let prot = row("  1 Protector de pantalla ");
        assert!(prot.ends_with("     7,95     7,95") && len(&prot) == 48, "{prot:?}");
        let vat = row("21%");
        assert!(vat.contains("17,98") && vat.contains("3,77") && vat.ends_with("21,75") && len(&vat) == 48, "{vat:?}");
        assert!(!body.iter().any(|l| l.to_uppercase().contains("ABARROTES")));
    }

    #[test]
    fn refund_names_the_ticket_it_corrects() {
        let lines = vec![SaleLine { name: "Cable USB-C 1 m".into(), qty: -2, unit_cents: 690, vat_bp: 2100, ..SaleLine::default() }];
        let body: Vec<String> = texts(&render_refund(48, &lines)).into_iter().map(|(t, _)| t).collect();
        let has = |s: &str| assert!(body.iter().any(|l| l.contains(s)), "missing {s:?} in\n{}", body.join("\n"));
        has("FACTURA SIMPLIFICADA RECTIFICATIVA");
        has("Nº TR2026-000003");
        has("Rectifica a: T2026-000124 del 16/10/2026");
        has("Motivo: No funciona la carga rápida");
        has("Devuelto en efectivo");
        has("Artículos devueltos: 2");
        has("-13,80 €");
        let row = body.iter().find(|l| l.starts_with(" -2 Cable")).unwrap();
        assert!(row.ends_with("     6,90   -13,80") && len(row) == 48, "{row:?}");
        assert!(!body.iter().any(|l| l.starts_with("Cambio ")));
    }

    #[test]
    fn ascii_page_has_no_accents_and_spells_euro() {
        let out = texts(&render(48, "ascii", &sample_lines(2100)));
        let all: String = out.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join("\n");
        assert!(all.is_ascii(), "{all}");
        assert!(all.contains(&format!("TOTAL{}21,75 EUR", " ".repeat(10))), "{all}");
        assert!(all.contains("No T2026-000124"));
    }

    #[test]
    fn wrap_keeps_words_and_splits_long_ones() {
        assert_eq!(wrap("uno dos tres", 7), vec!["uno dos", "tres"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
        assert_eq!(wrap("", 10), vec![""]);
    }
}
