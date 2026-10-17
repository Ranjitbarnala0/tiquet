// Turns a ticket into fixed-width lines. The same lines feed the on-screen
// preview and the ESC/POS encoder, so what the cashier sees is what prints.

use serde::Serialize;

use crate::codepage::{printable, CodePage};
use crate::money::{fmt, fmt_rate};
use crate::reports::Summary;
use crate::settings::Settings;
use crate::tickets::{compute, now, Payment, SaleLine, ShopInfo, TicketRecord, Totals};

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
    pub number: &'a str,
    pub issued_at: &'a str,
    pub user_name: &'a str,
    pub lines: &'a [SaleLine],
    pub totals: &'a Totals,
    pub payment: Option<&'a Payment>,
    pub copy: bool,
    pub voided: bool,
}

fn header(d: &mut Doc, shop: &ShopInfo) {
    let name = shop.name.trim();
    if !name.is_empty() {
        let size = if len(name) <= d.cols / 2 { Size::Big } else { Size::Tall };
        d.center(name, true, size);
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

fn items(d: &mut Doc, lines: &[SaleLine]) {
    if lines.is_empty() {
        d.center("(sin artículos)", false, Size::Normal);
        return;
    }
    if d.cols < 42 {
        for l in lines {
            item_two_lines(d, l);
        }
        return;
    }
    let desc = d.cols - 4 - 9 - 9;
    let head = format!(
        "UDS {}{}{}",
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
            continue;
        }
        let mut name = wrap(&d.clean(&l.name), desc).into_iter();
        let first = name.next().unwrap_or_default();
        d.raw(
            format!("{:>3} {}{}{}", l.qty, pad_right(&first, desc), pad_left(&price, 9), pad_left(&total, 9)),
            false,
            Size::Normal,
        );
        for rest in name {
            d.raw(format!("    {rest}"), false, Size::Normal);
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
    header(&mut d, v.shop);
    d.blank();
    d.center("FACTURA SIMPLIFICADA", true, Size::Normal);
    d.pair(&format!("Nº {}", v.number), &datetime(v.issued_at), false, Size::Normal);
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
    items(&mut d, v.lines);
    d.rule('-');
    d.pair("TOTAL", &format!("{} €", fmt(v.totals.total_cents)), true, Size::Big);
    if let Some(p) = v.payment {
        d.rule('-');
        d.pair(method_label(&p.method), &fmt(p.tendered_cents), false, Size::Normal);
        if p.method == "cash" {
            d.pair("Cambio", &fmt(p.change_cents), false, Size::Normal);
        }
    }
    if s.show_vat && !v.totals.vat.is_empty() {
        d.rule('-');
        vat_table(&mut d, v.totals);
    }
    d.rule('-');
    d.pair(&format!("Artículos: {}", v.totals.item_count), "IVA incluido", false, Size::Normal);
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
    d.lines
}

pub fn record(r: &TicketRecord, s: &Settings, logo: bool, copy: bool) -> Vec<Line> {
    let totals = compute(&r.lines);
    let v = TicketView {
        shop: &r.shop,
        number: &r.number,
        issued_at: &r.issued_at,
        user_name: &r.user_name,
        lines: &r.lines,
        totals: &totals,
        payment: Some(&r.payment),
        copy,
        voided: r.voided_at.is_some(),
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
        }
    }

    fn render(cols: u32, codepage: &str, lines: &[SaleLine]) -> Vec<Line> {
        let s = Settings { columns: cols, codepage: codepage.into(), ..Settings::default() };
        let totals = compute(lines);
        let pay = Payment { method: "cash".into(), tendered_cents: 5000, change_cents: 5000 - totals.total_cents };
        let shop = shop();
        let v = TicketView {
            shop: &shop,
            number: "T2026-000124",
            issued_at: "2026-10-16T17:00:12",
            user_name: "Ana",
            lines,
            totals: &totals,
            payment: Some(&pay),
            copy: true,
            voided: false,
        };
        ticket(&v, &s, true)
    }

    #[test]
    fn no_line_is_wider_than_the_paper() {
        let mut lines = sample_lines(2100);
        lines.push(SaleLine {
            product_id: None,
            name: "Funda con un nombre larguísimo que no cabe en una sola línea del ticket de ningún modo".into(),
            qty: 12,
            unit_cents: 1250,
            vat_bp: 1000,
        });
        lines.push(SaleLine { product_id: None, name: "Pantalla".into(), qty: 999, unit_cents: 9_999_999, vat_bp: 2100 });
        lines.push(SaleLine { product_id: None, name: "Supercalifragilisticoespialidoso".repeat(3), qty: 1, unit_cents: -100, vat_bp: 0 });
        for cols in [32, 42, 48] {
            for cp in ["cp858", "cp1252", "cp850", "ascii"] {
                for (text, size) in texts(&render(cols, cp, &lines)) {
                    let w = if size == Size::Big { cols as usize / 2 } else { cols as usize };
                    assert!(len(&text) <= w, "{cols}/{cp}: {text:?} is {} > {w}", len(&text));
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
