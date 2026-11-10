// A full invoice (factura completa) on A4 paper, for the customer who needs
// one: a business buying a phone, or anyone who asks. The page is laid out
// once, in millimetres from its top left corner, and then written as a PDF or
// drawn through any Windows printer's own driver, so both come out the same.
// Text is placed and wrapped with Helvetica's widths: the PDF names Helvetica
// (every reader has it, nothing to embed) and the printer draws Arial, whose
// letters are as wide, each one advanced by exactly those widths.

use serde::{Deserialize, Serialize};

use crate::codepage::{byte, printable, CodePage};
use crate::layout::{date, method_label};
use crate::money::{div_round, fmt, fmt_rate};
use crate::settings::{Logo, Settings};
use crate::tickets::{compute, Customer, Payment, SaleLine, ShopInfo, TicketRecord, VatRow};
use crate::verifactu;

// one A4 page
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    pub items: Vec<Mark>,
}

// What is drawn, in millimetres from the page's top left corner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Mark {
    // y is the baseline
    Text { x: f32, y: f32, pt: f32, bold: bool, align: Align, text: String },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, width: f32 },
    // fill is a gray from 0 (black) to 1 (white)
    Rect { x: f32, y: f32, w: f32, h: f32, fill: Option<f32>, stroke: bool },
    // 1-bit: rows padded to whole bytes, most significant bit first, 1 black,
    // as settings::Logo::raster gives it
    Image { x: f32, y: f32, w: f32, h: f32, bits: Vec<u8>, px_w: u32, px_h: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    Left,
    Right,
    Center,
}

#[derive(Clone, Debug, Default)]
pub struct InvoiceOpts {
    // under the totals: guarantee, returns, whatever the shop states; may
    // have several lines
    pub terms: String,
    // a reprint: COPIA next to the title
    pub copy: bool,
}

const PAGE_W: f32 = 210.0;
const PAGE_H: f32 = 297.0;
const LEFT: f32 = 15.0;
const RIGHT: f32 = 195.0;
const TOP: f32 = 15.0;
// nothing but the page number goes below this
const BOTTOM: f32 = 279.0;
const FOOT: f32 = 289.0;
// one point, in mm
const PT: f32 = 25.4 / 72.0;
// a rectangle's outline
const STROKE: f32 = 0.2;
// behind headings: light enough for black text on a mono laser
const SHADE: f32 = 0.86;
// the AEAT's QR must measure 30 to 40 mm (the receipt's does too)
const QR: f32 = 32.0;
// the lines table: description, quantity, unit price without and with VAT,
// rate, amount; numbers right aligned 2 mm inside their column
const COLS: [f32; 7] = [LEFT, 104.0, 116.0, 138.0, 160.0, 173.0, RIGHT];
const BODY: f32 = 9.0;
const ROW_LEAD: f32 = 3.8;
const ROW_PAD: f32 = 0.6;
// between the last row and the totals
const GAP: f32 = 5.0;

// Helvetica's advance widths in thousandths of an em, WinAnsi 32..=255, from
// Adobe's AFM files. Arial has the same widths.
#[rustfmt::skip]
const HELVETICA: [u16; 224] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, 0,
    556, 0, 222, 556, 333, 1000, 556, 556, 333, 1000, 667, 333, 1000, 0, 611, 0,
    0, 222, 222, 333, 333, 350, 556, 1000, 333, 1000, 500, 333, 944, 0, 500, 667,
    278, 333, 556, 556, 556, 556, 260, 556, 333, 737, 370, 556, 584, 333, 737, 333,
    400, 584, 333, 333, 333, 556, 537, 278, 333, 333, 365, 556, 834, 834, 834, 611,
    667, 667, 667, 667, 667, 667, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
    722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
    556, 556, 556, 556, 556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 584, 611, 556, 556, 556, 556, 500, 556, 500,
];

#[rustfmt::skip]
const HELVETICA_BOLD: [u16; 224] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611,
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556,
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611,
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584, 0,
    556, 0, 278, 556, 500, 1000, 556, 556, 333, 1000, 667, 333, 1000, 0, 611, 0,
    0, 278, 278, 500, 500, 350, 556, 1000, 333, 1000, 556, 333, 944, 0, 500, 667,
    278, 333, 556, 556, 556, 556, 280, 556, 333, 737, 370, 556, 584, 333, 737, 333,
    400, 584, 333, 333, 333, 611, 556, 278, 333, 333, 365, 556, 834, 834, 834, 611,
    722, 722, 722, 722, 722, 722, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
    722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
    556, 556, 556, 556, 556, 556, 889, 556, 556, 556, 556, 556, 278, 278, 278, 278,
    611, 611, 611, 611, 611, 611, 611, 584, 611, 611, 611, 611, 611, 556, 611, 556,
];

// What the PDF and the printer both get is Windows-1252 (the PDF's WinAnsi):
// anything else becomes its nearest letter or a '?' before it is measured, so
// the widths are those of what is drawn.
fn clean(s: &str) -> String {
    printable(s, CodePage::Cp1252)
}

fn ansi(s: &str) -> Vec<u8> {
    clean(s).chars().map(|c| byte(c, CodePage::Cp1252).unwrap_or(b'?')).collect()
}

fn advance(b: u8, bold: bool) -> u32 {
    let t = if bold { &HELVETICA_BOLD } else { &HELVETICA };
    b.checked_sub(32).map_or(0, |i| t[i as usize] as u32)
}

pub fn text_width_mm(text: &str, pt: f32, bold: bool) -> f32 {
    let units: u32 = ansi(text).into_iter().map(|b| advance(b, bold)).sum();
    units as f32 / 1000.0 * pt * PT
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Font {
    pt: f32,
    bold: bool,
}

const fn font(pt: f32, bold: bool) -> Font {
    Font { pt, bold }
}

impl Font {
    fn em(self) -> f32 {
        self.pt * PT
    }

    // from one line to the next
    fn lead(self) -> f32 {
        self.em() * 1.3
    }

    // from the top of a line to its baseline: capitals and descenders sit in
    // the middle of the line
    fn ascent(self) -> f32 {
        self.em() * 0.9
    }

    // the height of a capital, to centre a line on something
    fn cap(self) -> f32 {
        self.em() * 0.718
    }

    fn width(self, s: &str) -> f32 {
        text_width_mm(s, self.pt, self.bold)
    }
}

// Lines no wider than `width`, words kept whole where they fit. IMEI stays
// with its number, and a lone "·" (or dash) between two parts of a name is
// where a line breaks best: it is left out at the break, so a phone reads
// "iPhone 15 128 GB Negro" and then "IMEI 356789012345678".
fn wrap(text: &str, f: Font, width: f32) -> Vec<String> {
    let fits = |s: &str| f.width(s) <= width;
    let mut out = Vec::new();
    for para in text.lines() {
        let mut line = String::new();
        let mut sep: Option<String> = None;
        for unit in units(&clean(para)) {
            if !line.is_empty() && is_sep(&unit) {
                sep = Some(unit);
                continue;
            }
            let joined = match sep.take() {
                _ if line.is_empty() => unit.clone(),
                Some(s) => format!("{line} {s} {unit}"),
                None => format!("{line} {unit}"),
            };
            if fits(&joined) {
                line = joined;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            // a word wider than the column is cut where it has to be
            let mut rest = unit;
            while !fits(&rest) && rest.chars().count() > 1 {
                let mut cut = String::new();
                for c in rest.chars() {
                    let longer = format!("{cut}{c}");
                    if !cut.is_empty() && !fits(&longer) {
                        break;
                    }
                    cut = longer;
                }
                rest = rest[cut.len()..].to_string();
                out.push(cut);
            }
            line = rest;
        }
        if let Some(s) = sep {
            let joined = format!("{line} {s}");
            if fits(&joined) {
                line = joined;
            }
        }
        out.push(line);
    }
    out
}

// words, with a label such as IMEI joined to the number after it
fn units(para: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut glue = false;
    for w in para.split_whitespace() {
        match out.last_mut() {
            Some(last) if glue => {
                last.push(' ');
                last.push_str(w);
            }
            _ => out.push(w.to_string()),
        }
        glue = matches!(w.trim_end_matches(':').to_uppercase().as_str(), "IMEI" | "IMEI1" | "IMEI2" | "SN" | "S/N" | "N/S" | "Nº" | "N.º");
    }
    out
}

fn is_sep(w: &str) -> bool {
    matches!(w, "·" | "•" | "|" | "-" | "–" | "—")
}

// The narrowest width that takes no more lines: lines aligned on the right
// come out of even length instead of one word left alone on the last.
fn even(s: &str, f: Font, width: f32) -> f32 {
    let n = wrap(s, f, width).len();
    let (mut lo, mut hi) = (0.0, width);
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        if wrap(s, f, mid).len() <= n {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

// cut to fit `width`, with "…" where it was cut
fn fit(s: &str, f: Font, width: f32) -> String {
    let s = clean(s);
    if f.width(&s) <= width {
        return s;
    }
    let mut out = String::new();
    for c in s.chars() {
        if f.width(&format!("{out}{c}…")) > width {
            break;
        }
        out.push(c);
    }
    format!("{}…", out.trim_end())
}

struct Doc {
    sheets: Vec<Sheet>,
    // across the top of every page: PRUEBA, ANULADA
    banner: String,
    // what heads the pages after the first
    shop: String,
    title: String,
}

impl Doc {
    // for measuring a block before it is placed
    fn scratch() -> Doc {
        Doc { sheets: vec![Sheet::default()], banner: String::new(), shop: String::new(), title: String::new() }
    }

    fn put(&mut self, m: Mark) {
        if self.sheets.is_empty() {
            self.sheets.push(Sheet::default());
        }
        if let Some(s) = self.sheets.last_mut() {
            s.items.push(m);
        }
    }

    fn text(&mut self, x: f32, y: f32, f: Font, align: Align, s: &str) {
        let text = clean(s);
        if !text.trim().is_empty() {
            self.put(Mark::Text { x, y, pt: f.pt, bold: f.bold, align, text });
        }
    }

    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32) {
        self.put(Mark::Line { x1, y1, x2, y2, width });
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: Option<f32>, stroke: bool) {
        self.put(Mark::Rect { x, y, w, h, fill, stroke });
    }

    // Wrapped text whose first line's top is at y, x being the edge it is
    // aligned on. Returns the top of the line after it.
    fn para(&mut self, x: f32, y: f32, width: f32, f: Font, align: Align, s: &str) -> f32 {
        let width = if align == Align::Right { even(s, f, width) } else { width };
        let mut y = y;
        for l in wrap(s, f, width) {
            self.text(x, y + f.ascent(), f, align, &l);
            y += f.lead();
        }
        y
    }

    // A new page: the banner when there is one, and from the second page on
    // the shop and the invoice number above a rule. Returns where its
    // content starts.
    fn sheet(&mut self) -> f32 {
        self.sheets.push(Sheet::default());
        let mut y = TOP;
        if !self.banner.is_empty() {
            let f = font(14.0, true);
            let banner = self.banner.clone();
            self.rect(LEFT, y, RIGHT - LEFT, 9.0, Some(SHADE), false);
            self.text(PAGE_W / 2.0, y + 4.5 + f.cap() / 2.0, f, Align::Center, &banner);
            y += 13.0;
        }
        if self.sheets.len() > 1 {
            let f = font(10.0, true);
            let title = self.title.clone();
            let shop = fit(&self.shop, f, RIGHT - LEFT - f.width(&title) - 6.0);
            self.text(LEFT, y + 4.0, f, Align::Left, &shop);
            self.text(RIGHT, y + 4.0, f, Align::Right, &title);
            self.line(LEFT, y + 6.5, RIGHT, y + 6.5, 0.2);
            y += 11.0;
        }
        y
    }
}

// the trade name, or the owner's name when there is none, and the owner's
// name under it when it says something more
fn names(shop: &ShopInfo) -> (&str, &str) {
    let (name, legal) = (shop.name.trim(), shop.legal_name.trim());
    match (name.is_empty(), legal.eq_ignore_ascii_case(name)) {
        (true, _) => (legal, ""),
        (false, true) => (name, ""),
        (false, false) => (name, legal),
    }
}

// the logo at the size it prints on a receipt (203 dpi), made smaller to fit
// 45 x 20 mm
fn logo_mark(logo: &Logo, x: f32, y: f32) -> Option<Mark> {
    let bits = logo.raster()?;
    if logo.w == 0 || logo.h == 0 {
        return None;
    }
    let (w, h) = (logo.w as f32 * 25.4 / 203.0, logo.h as f32 * 25.4 / 203.0);
    let k = (45.0 / w).min(20.0 / h).min(1.0);
    Some(Mark::Image { x, y, w: w * k, h: h * k, bits, px_w: logo.w, px_h: logo.h })
}

// The first page's top: the shop on the left under its logo; the title,
// number, date and time on the right. Returns the y under both.
fn head(d: &mut Doc, y: f32, rec: &TicketRecord, logo: Option<&Logo>, copy: bool) -> f32 {
    let (big, small) = (font(14.0, true), font(9.0, false));
    let col = 92.0;
    let mut l = y;
    if let Some(m) = logo.and_then(|g| logo_mark(g, LEFT, y)) {
        if let Mark::Image { h, .. } = m {
            l += h + 4.0;
        }
        d.put(m);
    }
    let shop = &rec.shop;
    let (name, legal) = names(shop);
    l = d.para(LEFT, l, col, big, Align::Left, name) + 0.5;
    if !legal.is_empty() {
        l = d.para(LEFT, l, col, small, Align::Left, legal);
    }
    // a full invoice always names the shop's NIF and address, whatever the
    // receipts show
    if !shop.nif.trim().is_empty() {
        l = d.para(LEFT, l, col, small, Align::Left, &format!("NIF: {}", shop.nif.trim()));
    }
    for a in shop.address.lines().map(str::trim).filter(|a| !a.is_empty()) {
        l = d.para(LEFT, l, col, small, Align::Left, a);
    }
    if shop.show_phone.unwrap_or(true) && !shop.phone.trim().is_empty() {
        l = d.para(LEFT, l, col, small, Align::Left, &format!("Tel. {}", shop.phone.trim()));
    }

    // the title as big as fits beside COPIA, its capitals level with the shop's
    let refund = rec.kind == "refund";
    let title = if refund { "FACTURA RECTIFICATIVA" } else { "FACTURA" };
    let width = 80.0;
    let stamp = font(10.0, true);
    let stamp_w = stamp.width("COPIA") + 4.0;
    let room = if copy { width - stamp_w - 4.0 } else { width };
    let t = font(20f32.min(room / text_width_mm(title, 1.0, true)), true);
    let base = y + big.ascent() - big.cap() + t.cap();
    d.text(RIGHT, base, t, Align::Right, title);
    if copy {
        let x = RIGHT - t.width(title) - 4.0 - stamp_w;
        let mid = base - t.cap() / 2.0;
        d.rect(x, mid - 3.25, stamp_w, 6.5, None, true);
        d.text(x + stamp_w / 2.0, mid + stamp.cap() / 2.0, stamp, Align::Center, "COPIA");
    }
    let (label, value) = (font(9.0, false), font(10.0, true));
    let mut r = base + 3.0;
    let day = date(&rec.issued_at);
    let time = rec.issued_at.get(11..16).unwrap_or_default();
    for (k, v) in [("Número", rec.number.as_str()), ("Fecha", day.as_str()), ("Hora", time)] {
        d.text(RIGHT - 62.0, r + value.ascent(), label, Align::Left, k);
        d.text(RIGHT, r + value.ascent(), value, Align::Right, v);
        r += value.lead();
    }
    // a rectifying invoice names the one it corrects and why
    if refund {
        r += 1.5;
        if let Some(t) = &rec.refund_of {
            let s = format!("Rectifica a: {} del {}", t.number, date(&t.issued_at));
            r = d.para(RIGHT, r, width, label, Align::Right, &s);
        }
        if let Some(m) = rec.refund_reason.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            r = d.para(RIGHT, r, width, label, Align::Right, &format!("Motivo: {m}"));
        }
    }
    l.max(r) + 6.0
}

// Who it is made out to, in a box, and beside it the AEAT's QR: it goes at
// the start of the invoice, before what was sold, as on the receipt,
// labelled above and with VERI*FACTU under it.
fn customer(d: &mut Doc, y: f32, rec: &TicketRecord) -> f32 {
    let (w, pad, band) = (100.0, 3.0, 6.0);
    let inner = w - 2.0 * pad;
    let (name, body, head) = (font(10.5, true), font(9.5, false), font(8.0, true));
    let mut lines: Vec<(Font, String)> = Vec::new();
    let mut add = |f: Font, s: &str| lines.extend(wrap(s, f, inner).into_iter().map(|l| (f, l)));
    match &rec.customer {
        Some(c) => {
            add(name, &c.name);
            if !c.nif.trim().is_empty() {
                add(body, &format!("NIF: {}", c.nif.trim()));
            }
            for a in c.address.lines().map(str::trim).filter(|a| !a.is_empty()) {
                add(body, a);
            }
        }
        // the app makes these out to someone; without anyone it still prints
        None => add(body, "Cliente: —"),
    }
    let h = band + 2.0 + lines.iter().map(|(f, _)| f.lead()).sum::<f32>() + 1.5;
    d.rect(LEFT, y, w, band, Some(SHADE), false);
    d.text(LEFT + pad, y + band / 2.0 + head.cap() / 2.0, head, Align::Left, "Cliente");
    d.rect(LEFT, y, w, h, None, true);
    let mut t = y + band + 2.0;
    for (f, l) in &lines {
        d.text(LEFT + pad, t + f.ascent(), *f, Align::Left, l);
        t += f.lead();
    }
    let mut bottom = y + h;
    if let Some(rows) = rec.qr.as_deref().and_then(verifactu::qr_rows) {
        let x = RIGHT - QR;
        d.text(x + QR / 2.0, y + head.cap(), head, Align::Center, "QR tributario:");
        // white all round, four modules and more, as a reader needs
        let top = y + head.cap() + 4.0;
        modules(d, x, top, &rows);
        let under = top + QR + 3.5 + head.cap();
        d.text(x + QR / 2.0, under, head, Align::Center, "VERI*FACTU");
        bottom = bottom.max(under + 1.0);
    }
    bottom + 7.0
}

// the QR's dark modules as filled squares, each run along a row as one
fn modules(d: &mut Doc, x: f32, y: f32, rows: &[String]) {
    let m = QR / rows.len().max(1) as f32;
    for (r, row) in rows.iter().enumerate() {
        let row = row.as_bytes();
        let mut c = 0;
        while c < row.len() {
            if row[c] != b'1' {
                c += 1;
                continue;
            }
            let start = c;
            while c < row.len() && row[c] == b'1' {
                c += 1;
            }
            d.rect(x + start as f32 * m, y + r as f32 * m, (c - start) as f32 * m, m, Some(0.0), false);
        }
    }
}

// the lines table's shaded heading, on every page that has rows
fn table_head(d: &mut Doc, y: f32) -> f32 {
    let h = 9.0;
    let f = font(8.0, true);
    d.rect(LEFT, y, RIGHT - LEFT, h, Some(SHADE), false);
    let one = y + h / 2.0 + f.cap() / 2.0;
    let (a, b) = (y + 3.7, y + 7.1);
    d.text(COLS[0] + 2.0, one, f, Align::Left, "Descripción");
    d.text(COLS[2] - 2.0, one, f, Align::Right, "Cant.");
    d.text(COLS[3] - 2.0, a, f, Align::Right, "Precio");
    d.text(COLS[3] - 2.0, b, f, Align::Right, "sin IVA");
    d.text(COLS[4] - 2.0, a, f, Align::Right, "Precio");
    d.text(COLS[4] - 2.0, b, f, Align::Right, "con IVA");
    d.text(COLS[5] - 2.0, one, f, Align::Right, "IVA %");
    d.text(COLS[6] - 2.0, one, f, Align::Right, "Importe");
    y + h
}

struct Row {
    desc: Vec<String>,
    cells: [String; 5],
}

impl Row {
    // Prices are stored with VAT. A full invoice also states each unit price
    // without it (RD 1619/2012, art. 6.1 f), as the receipt's does.
    fn of(l: &SaleLine) -> Row {
        let net = div_round(l.unit_cents * 10_000, 10_000 + l.vat_bp);
        Row {
            desc: wrap(&l.name, font(BODY, false), COLS[1] - COLS[0] - 4.0),
            cells: [l.qty.to_string(), fmt(net), fmt(l.unit_cents), fmt_rate(l.vat_bp), fmt(l.total())],
        }
    }

    fn height(&self) -> f32 {
        2.0 * ROW_PAD + self.desc.len().max(1) as f32 * ROW_LEAD
    }
}

fn draw_row(d: &mut Doc, y: f32, r: &Row) -> f32 {
    let f = font(BODY, false);
    let base = |i: usize| y + ROW_PAD + ROW_LEAD * (i as f32 + 0.5) + f.cap() / 2.0;
    for (i, l) in r.desc.iter().enumerate() {
        d.text(COLS[0] + 2.0, base(i), f, Align::Left, l);
    }
    for (i, c) in r.cells.iter().enumerate() {
        // a number too long for its column is set smaller, never over the next
        let room = COLS[i + 2] - COLS[i + 1] - 3.0;
        let w = f.width(c);
        let g = if w > room { font(BODY * room / w, false) } else { f };
        d.text(COLS[i + 2] - 2.0, base(0), g, Align::Right, c);
    }
    let bottom = y + r.height();
    d.line(LEFT, bottom, RIGHT, bottom, 0.1);
    bottom
}

// under the table on the right: VAT per rate, the total, how it was paid
fn totals(d: &mut Doc, y: f32, rec: &TicketRecord, vat: &[VatRow]) -> f32 {
    let x0 = 100.0;
    let edges = [133.0, 149.0, 171.0, RIGHT - 2.0];
    let (head, f, big) = (font(8.0, true), font(BODY, false), font(14.0, true));
    d.rect(x0, y, RIGHT - x0, 6.5, Some(SHADE), false);
    for (e, l) in edges.iter().zip(["Base imponible", "Tipo IVA", "Cuota IVA", "Total"]) {
        d.text(*e, y + 3.25 + head.cap() / 2.0, head, Align::Right, l);
    }
    let mut y = y + 6.5;
    for v in vat {
        for (e, s) in edges.iter().zip([fmt(v.base_cents), fmt_rate(v.vat_bp), fmt(v.vat_cents), fmt(v.total_cents)]) {
            d.text(*e, y + 2.5 + f.cap() / 2.0, f, Align::Right, &s);
        }
        y += 5.0;
        d.line(x0, y, RIGHT, y, 0.1);
    }
    y += 2.5;
    d.rect(x0, y, RIGHT - x0, 10.0, Some(SHADE), false);
    let base = y + 5.0 + big.cap() / 2.0;
    d.text(x0 + 3.0, base, big, Align::Left, "TOTAL");
    d.text(RIGHT - 2.0, base, big, Align::Right, &format!("{} €", fmt(rec.total_cents)));
    y += 10.0;
    let label = if rec.kind == "refund" { "Forma de devolución" } else { "Forma de pago" };
    let pay = font(9.5, false);
    d.text(RIGHT - 2.0, y + 5.0, pay, Align::Right, &format!("{label}: {}", method_label(&rec.payment.method)));
    y + 6.5
}

// The shop's terms at the foot of the last page, or after the totals when
// they do not fit there, onto more pages if they must.
fn terms(d: &mut Doc, y: f32, text: &str) {
    let f = font(8.0, false);
    let lines = wrap(text.trim(), f, RIGHT - LEFT);
    if lines.is_empty() {
        return;
    }
    let h = lines.len() as f32 * f.lead();
    let mut y = if y + 8.0 + h <= BOTTOM {
        BOTTOM - h
    } else if y + 8.0 + f.lead() <= BOTTOM {
        y + 8.0
    } else {
        d.sheet() + 2.0
    };
    d.line(LEFT, y - 2.0, RIGHT, y - 2.0, 0.1);
    for l in &lines {
        if y + f.lead() > BOTTOM {
            y = d.sheet();
        }
        d.text(LEFT, y + f.ascent(), f, Align::Left, l);
        y += f.lead();
    }
}

// The pages of a full invoice. Rows that do not fit go on to another page
// under the table's heading again; the totals only come on the last, which
// always keeps at least the last row with them.
// The page Ajustes prints to try the A4 printer: the shop's own header, a
// customer and a phone, marked as a test so it can never pass for an invoice.
pub fn sample(s: &Settings) -> TicketRecord {
    let mut lines = vec![SaleLine {
        name: "Teléfono de ejemplo · IMEI 350000000000000".into(),
        qty: 1,
        unit_cents: 19_900,
        vat_bp: s.default_vat_bp,
        ..SaleLine::default()
    }];
    lines.extend(crate::tickets::sample_lines(s.default_vat_bp));
    let t = compute(&lines);
    TicketRecord {
        id: 0,
        kind: "sale".into(),
        number: "PRUEBA".into(),
        issued_at: crate::tickets::now(),
        user_name: String::new(),
        total_cents: t.total_cents,
        item_count: t.item_count,
        payment: Payment { method: "card".into(), tendered_cents: t.total_cents, change_cents: 0, machine: None },
        voided_at: None,
        voided_by: None,
        void_reason: None,
        refund_of: None,
        refund_reason: None,
        refunds: Vec::new(),
        lines,
        vat: t.vat,
        prints: Vec::new(),
        payment_changes: Vec::new(),
        shop: ShopInfo::from_settings(s),
        customer: Some(Customer { name: "Cliente de prueba".into(), nif: "B12345674".into(), address: "Calle Mayor, 1\n17600 Figueres".into() }),
        training: true,
        qr: None,
        aeat: None,
    }
}

#[cfg(not(windows))]
pub fn print(_printer: &str, _sheets: &[Sheet], _copies: u32) -> crate::error::Result<()> {
    crate::error::fail("E_PRINT:printing needs Windows")
}

pub fn invoice(rec: &TicketRecord, logo: Option<&Logo>, opts: &InvoiceOpts) -> Vec<Sheet> {
    let title = if rec.kind == "refund" { "FACTURA RECTIFICATIVA" } else { "FACTURA" };
    let mut banner = Vec::new();
    if rec.training {
        banner.push("PRUEBA · SIN VALIDEZ");
    }
    if rec.voided_at.is_some() {
        banner.push("ANULADA");
    }
    let copy = if opts.copy { " · COPIA" } else { "" };
    let mut d = Doc {
        sheets: Vec::new(),
        banner: banner.join(" · "),
        shop: names(&rec.shop).0.to_string(),
        title: format!("{title} {}{copy}", rec.number),
    };
    let y = d.sheet();
    let y = head(&mut d, y, rec, logo, opts.copy);
    let y = customer(&mut d, y, rec);
    let mut y = table_head(&mut d, y);

    // a reprint shows the VAT stored when the ticket was issued
    let vat = if rec.vat.is_empty() { compute(&rec.lines).vat } else { rec.vat.clone() };
    let tail = totals(&mut Doc::scratch(), 0.0, rec, &vat);
    let mut rows: Vec<Row> = rec.lines.iter().map(Row::of).collect();
    if rows.is_empty() {
        rows.push(Row { desc: vec!["(sin artículos)".into()], cells: Default::default() });
    }
    let mut fresh = true;
    for (i, r) in rows.iter().enumerate() {
        let need = r.height() + if i + 1 == rows.len() { GAP + tail } else { 0.0 };
        if !fresh && y + need > BOTTOM {
            y = d.sheet();
            y = table_head(&mut d, y);
        }
        y = draw_row(&mut d, y, r);
        fresh = false;
    }
    if y + GAP + tail > BOTTOM {
        y = d.sheet();
    }
    let y = totals(&mut d, y + GAP, rec, &vat);
    terms(&mut d, y, &opts.terms);

    let n = d.sheets.len();
    for (i, s) in d.sheets.iter_mut().enumerate() {
        let text = format!("Página {} de {n}", i + 1);
        s.items.push(Mark::Text { x: PAGE_W / 2.0, y: FOOT, pt: 8.0, bold: false, align: Align::Center, text });
    }
    d.sheets
}

// --- PDF ---------------------------------------------------------------

fn num(v: f32) -> String {
    format!("{v:.2}")
}

// mm from the top left to points from the bottom left
fn pdf_x(mm: f32) -> String {
    num(mm / PT)
}

fn pdf_y(mm: f32) -> String {
    num((PAGE_H - mm) / PT)
}

// WinAnsi bytes as they are; only ( ) and \ need a backslash
fn literal(s: &str) -> Vec<u8> {
    let mut out = vec![b'('];
    for b in ansi(s) {
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
    out
}

fn image_ok(bits: &[u8], px_w: u32, px_h: u32) -> bool {
    px_w > 0 && px_h > 0 && bits.len() == (px_w as usize).div_ceil(8) * px_h as usize
}

// a page's content stream, and its images as XObjects named /Im1, /Im2...
fn page(s: &Sheet) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut out = Vec::new();
    let mut images = Vec::new();
    // filled rectangles one after another (a QR's modules) are one path,
    // filled once: filled one by one, readers show hairlines between them
    let mut open: Option<f32> = None;
    let close = |out: &mut Vec<u8>, open: &mut Option<f32>| {
        if open.take().is_some() {
            out.extend_from_slice(b"f Q\n");
        }
    };
    for m in &s.items {
        if let Mark::Rect { x, y, w, h, fill: Some(g), stroke: false } = m {
            if open != Some(*g) {
                close(&mut out, &mut open);
                out.extend(format!("q {:.3} g\n", g.clamp(0.0, 1.0)).bytes());
                open = Some(*g);
            }
            out.extend(format!("{} {} {} {} re\n", pdf_x(*x), pdf_y(y + h), num(w / PT), num(h / PT)).bytes());
            continue;
        }
        close(&mut out, &mut open);
        match m {
            Mark::Text { x, y, pt, bold, align, text } => {
                let w = text_width_mm(text, *pt, *bold);
                let left = match align {
                    Align::Left => *x,
                    Align::Right => x - w,
                    Align::Center => x - w / 2.0,
                };
                let f = if *bold { "F2" } else { "F1" };
                out.extend(format!("BT /{f} {} Tf {} {} Td ", num(*pt), pdf_x(left), pdf_y(*y)).bytes());
                out.extend(literal(text));
                out.extend_from_slice(b" Tj ET\n");
            }
            Mark::Line { x1, y1, x2, y2, width } => {
                let line = format!("{} w {} {} m {} {} l S\n", num(width / PT), pdf_x(*x1), pdf_y(*y1), pdf_x(*x2), pdf_y(*y2));
                out.extend(line.bytes());
            }
            Mark::Rect { x, y, w, h, fill, stroke } => {
                let rect = format!("{} {} {} {} re", pdf_x(*x), pdf_y(y + h), num(w / PT), num(h / PT));
                let op = match (fill, stroke) {
                    (Some(g), true) => format!("q {:.3} g {} w {rect} B Q\n", g.clamp(0.0, 1.0), num(STROKE / PT)),
                    (Some(g), false) => format!("q {:.3} g {rect} f Q\n", g.clamp(0.0, 1.0)),
                    (None, true) => format!("{} w {rect} S\n", num(STROKE / PT)),
                    (None, false) => String::new(),
                };
                out.extend(op.bytes());
            }
            Mark::Image { x, y, w, h, bits, px_w, px_h } => {
                if !image_ok(bits, *px_w, *px_h) {
                    continue;
                }
                // 1 = black in the raster; /Decode turns it into the gray 0
                let mut img = format!(
                    "<< /Type /XObject /Subtype /Image /Width {px_w} /Height {px_h} /ColorSpace /DeviceGray /BitsPerComponent 1 /Decode [1 0] /Length {} >>\nstream\n",
                    bits.len()
                )
                .into_bytes();
                img.extend_from_slice(bits);
                img.extend_from_slice(b"\nendstream");
                images.push(img);
                let place = format!("q {} 0 0 {} {} {} cm /Im{} Do Q\n", num(w / PT), num(h / PT), pdf_x(*x), pdf_y(y + h), images.len());
                out.extend(place.bytes());
            }
        }
    }
    close(&mut out, &mut open);
    (out, images)
}

// The sheets as an A4 PDF: Helvetica and Helvetica-Bold in WinAnsi, nothing
// embedded but the logo, a few objects and an xref, as pdf.rs writes tickets.
pub fn to_pdf(sheets: &[Sheet], title: &str) -> Vec<u8> {
    let blank = [Sheet::default()];
    let sheets = if sheets.is_empty() { &blank[..] } else { sheets };
    // 1 catalog, 2 page tree, 3 and 4 the fonts; then each page, its content
    // and its images; the document info last
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        Vec::new(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".to_vec(),
    ];
    let mut kids = Vec::new();
    for s in sheets {
        let (body, images) = page(s);
        let page_id = objects.len() + 1;
        let content_id = page_id + 1;
        kids.push(format!("{page_id} 0 R"));
        let xobjects: Vec<String> = (0..images.len()).map(|i| format!("/Im{} {} 0 R", i + 1, content_id + 1 + i)).collect();
        let xobject = if xobjects.is_empty() { String::new() } else { format!(" /XObject << {} >>", xobjects.join(" ")) };
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >>{xobject} >> /Contents {content_id} 0 R >>",
                num(PAGE_W / PT),
                num(PAGE_H / PT)
            )
            .into_bytes(),
        );
        let mut stream = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
        stream.extend(body);
        stream.extend_from_slice(b"\nendstream");
        objects.push(stream);
        objects.extend(images);
    }
    objects[1] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), kids.len()).into_bytes();
    let mut info = b"<< /Producer (Tiquet) /Title ".to_vec();
    info.extend(literal(title));
    info.extend_from_slice(b" >>");
    objects.push(info);

    let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R /Info {} 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1, objects.len()).as_bytes(),
    );
    out
}

// --- Windows printers ----------------------------------------------------

// Prints the sheets on a Windows printer through its own driver, `copies`
// times (1 to 3), each copy its own document so that duplex never puts one
// copy on the back of another. Nothing is ever asked: a queue that would ask
// where to save (PDF and XPS writers) or open a window of its own (fax,
// OneNote) is refused before anything is sent.
#[cfg(windows)]
pub fn print(printer: &str, sheets: &[Sheet], copies: u32) -> crate::error::Result<()> {
    use crate::error::fail;

    let Some((port, driver)) = win::queue(printer) else {
        return fail(format!("E_PRINTER_MISSING:{printer}"));
    };
    if crate::printer::is_virtual(printer, &port, &driver) {
        return fail(format!("E_PRINTER_VIRTUAL:{printer}"));
    }
    match win::draw(printer, "Factura", sheets, copies.clamp(1, 3), None) {
        Ok(()) => Ok(()),
        Err(e) => fail(format!("E_PRINT:{e}")),
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{HANDLE, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateDCW, CreateFontW, CreateSolidBrush, DeleteDC, DeleteObject, ExtCreatePen, ExtTextOutW, FillRect, GetDeviceCaps,
        GetStockObject, LineTo, MoveToEx, SelectObject, SetBkMode, SetTextAlign, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, BLACK_BRUSH, BS_SOLID, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_QUALITY, DEVMODEW, DIB_RGB_COLORS,
        DMORIENT_PORTRAIT, DMPAPER_A4, DM_FORMNAME, DM_IN_BUFFER, DM_ORIENTATION, DM_OUT_BUFFER, DM_PAPERLENGTH, DM_PAPERSIZE,
        DM_PAPERWIDTH, FF_SWISS, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT, LOGBRUSH, LOGPIXELSX, LOGPIXELSY, OUT_TT_PRECIS,
        PHYSICALHEIGHT, PHYSICALOFFSETX, PHYSICALOFFSETY, PHYSICALWIDTH, PS_ENDCAP_FLAT, PS_GEOMETRIC, PS_SOLID, RGBQUAD,
        SRCCOPY, SYSTEM_FONT, TA_BASELINE, TA_LEFT, TA_NOUPDATECP, TRANSPARENT, VARIABLE_PITCH,
    };
    use windows_sys::Win32::Graphics::Printing::{ClosePrinter, DocumentPropertiesW, GetPrinterW, OpenPrinterW, PRINTER_INFO_2W};
    use windows_sys::Win32::Storage::Xps::{AbortDoc, EndDoc, EndPage, StartDocW, StartPage, DOCINFOW};

    use super::{advance, clean, Align, Mark, Sheet, PAGE_H, PAGE_W, PT, STROKE};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn from_wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }

    // the queue's port and driver; None when Windows has no such printer
    pub fn queue(name: &str) -> Option<(String, String)> {
        let w = wide(name);
        let mut h: HANDLE = null_mut();
        unsafe {
            if OpenPrinterW(w.as_ptr(), &mut h, null()) == 0 {
                return None;
            }
            let mut needed = 0u32;
            GetPrinterW(h, 2, null_mut(), 0, &mut needed);
            // u64 backing keeps PRINTER_INFO_2W pointer-aligned
            let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
            let out = if needed > 0 && GetPrinterW(h, 2, buf.as_mut_ptr() as *mut u8, needed, &mut needed) != 0 {
                let p = &*(buf.as_ptr() as *const PRINTER_INFO_2W);
                Some((from_wide(p.pPortName), from_wide(p.pDriverName)))
            } else {
                Some((String::new(), String::new()))
            };
            ClosePrinter(h);
            out
        }
    }

    // The printer's own settings with A4 upright asked for: a page laid out
    // for A4 must not come out on Letter or sideways. The driver checks the
    // change against what the printer can do; None when it will not say,
    // and the page is then fitted to whatever paper it has.
    unsafe fn a4(device: &[u16]) -> Option<Vec<u64>> {
        let mut h: HANDLE = null_mut();
        if OpenPrinterW(device.as_ptr(), &mut h, null()) == 0 {
            return None;
        }
        let size = DocumentPropertiesW(null_mut(), h, device.as_ptr(), null_mut(), null(), 0);
        let mut out = None;
        if size > 0 {
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let dm = buf.as_mut_ptr() as *mut DEVMODEW;
            if DocumentPropertiesW(null_mut(), h, device.as_ptr(), dm, null(), DM_OUT_BUFFER) == 1 {
                // a custom size or a form name would win over the paper size
                (*dm).dmFields &= !(DM_PAPERLENGTH | DM_PAPERWIDTH | DM_FORMNAME);
                (*dm).dmFields |= DM_PAPERSIZE | DM_ORIENTATION;
                (*dm).Anonymous1.Anonymous1.dmPaperSize = DMPAPER_A4 as i16;
                (*dm).Anonymous1.Anonymous1.dmOrientation = DMORIENT_PORTRAIT as i16;
                if DocumentPropertiesW(null_mut(), h, device.as_ptr(), dm, dm, DM_IN_BUFFER | DM_OUT_BUFFER) == 1 {
                    out = Some(buf);
                }
            }
        }
        ClosePrinter(h);
        out
    }

    // `out`: a file a PDF or XPS writer saves to instead of asking where
    pub fn draw(printer: &str, doc: &str, sheets: &[Sheet], copies: u32, out: Option<&Path>) -> Result<(), String> {
        let (driver, device) = (wide("WINSPOOL"), wide(printer));
        let mode = unsafe { a4(&device) };
        let dm = mode.as_ref().map_or(null(), |m| m.as_ptr() as *const DEVMODEW);
        let mut hdc = unsafe { CreateDCW(driver.as_ptr(), device.as_ptr(), null(), dm) };
        // a driver that will not take A4 prints on what it has, fitted
        if hdc.is_null() && !dm.is_null() {
            hdc = unsafe { CreateDCW(driver.as_ptr(), device.as_ptr(), null(), null()) };
        }
        if hdc.is_null() {
            return Err("offline".into());
        }
        let name = wide(doc);
        let out_w = out.map(|p| wide(&p.to_string_lossy()));
        let info = DOCINFOW {
            cbSize: std::mem::size_of::<DOCINFOW>() as i32,
            lpszDocName: name.as_ptr(),
            lpszOutput: out_w.as_ref().map_or(null(), |w| w.as_ptr()),
            lpszDatatype: null(),
            fwType: 0,
        };
        let mut result = Ok(());
        for n in 0..copies.max(1) {
            if let Err(e) = unsafe { document(hdc, &info, sheets) } {
                // the copies before it may be on paper already
                result = Err(if n == 0 { e } else { format!("partial-{e}") });
                break;
            }
        }
        unsafe {
            DeleteDC(hdc);
        }
        result
    }

    // a half-sent document must never reach the paper: aborted, not ended
    unsafe fn document(hdc: HDC, info: &DOCINFOW, sheets: &[Sheet]) -> Result<(), String> {
        if StartDocW(hdc, info) <= 0 {
            return Err("error".into());
        }
        let dev = Device::of(hdc);
        for s in sheets {
            if StartPage(hdc) <= 0 {
                AbortDoc(hdc);
                return Err("error".into());
            }
            let mut c = Canvas { hdc, dev, fonts: Vec::new(), brushes: Vec::new() };
            c.sheet(s);
            c.free();
            if EndPage(hdc) <= 0 {
                AbortDoc(hdc);
                return Err("error".into());
            }
        }
        if EndDoc(hdc) <= 0 {
            AbortDoc(hdc);
            return Err("error".into());
        }
        Ok(())
    }

    // Millimetres to the printer's dots. Its (0, 0) is the corner of what it
    // can print, PHYSICALOFFSET in from the paper's: the page is placed on
    // the paper, not on that area. Paper smaller than A4 (Letter) gets the
    // whole page made smaller to fit, centred across.
    #[derive(Clone, Copy)]
    struct Device {
        kx: f32,
        ky: f32,
        dx: f32,
        dy: f32,
    }

    impl Device {
        unsafe fn of(hdc: HDC) -> Device {
            let cap = |i: u32| GetDeviceCaps(hdc, i as i32) as f32;
            let (dpi_x, dpi_y) = (cap(LOGPIXELSX).max(72.0), cap(LOGPIXELSY).max(72.0));
            let (a4_w, a4_h) = (PAGE_W * dpi_x / 25.4, PAGE_H * dpi_y / 25.4);
            let paper_w = Some(cap(PHYSICALWIDTH)).filter(|w| *w > 0.0).unwrap_or(a4_w);
            let paper_h = Some(cap(PHYSICALHEIGHT)).filter(|h| *h > 0.0).unwrap_or(a4_h);
            let s = (paper_w / a4_w).min(paper_h / a4_h).min(1.0);
            Device {
                kx: s * dpi_x / 25.4,
                ky: s * dpi_y / 25.4,
                dx: ((paper_w - a4_w * s) / 2.0).max(0.0) - cap(PHYSICALOFFSETX),
                dy: -cap(PHYSICALOFFSETY),
            }
        }

        fn x(&self, mm: f32) -> i32 {
            (mm * self.kx + self.dx).round() as i32
        }

        fn y(&self, mm: f32) -> i32 {
            (mm * self.ky + self.dy).round() as i32
        }
    }

    struct Canvas {
        hdc: HDC,
        dev: Device,
        fonts: Vec<((i32, bool), HFONT)>,
        brushes: Vec<(u8, HBRUSH)>,
    }

    impl Canvas {
        unsafe fn sheet(&mut self, s: &Sheet) {
            SetBkMode(self.hdc, TRANSPARENT as i32);
            SetTextAlign(self.hdc, TA_BASELINE | TA_LEFT | TA_NOUPDATECP);
            for m in &s.items {
                match m {
                    Mark::Text { x, y, pt, bold, align, text } => self.text(*x, *y, *pt, *bold, *align, text),
                    Mark::Line { x1, y1, x2, y2, width } => self.line(*x1, *y1, *x2, *y2, *width),
                    Mark::Rect { x, y, w, h, fill, stroke } => {
                        if let Some(g) = fill {
                            let r = RECT { left: self.dev.x(*x), top: self.dev.y(*y), right: self.dev.x(x + w), bottom: self.dev.y(y + h) };
                            FillRect(self.hdc, &r, self.brush(*g));
                        }
                        if *stroke {
                            let half = STROKE / 2.0;
                            self.line(x - half, *y, x + w + half, *y, STROKE);
                            self.line(x - half, y + h, x + w + half, y + h, STROKE);
                            self.line(*x, *y, *x, y + h, STROKE);
                            self.line(x + w, *y, x + w, y + h, STROKE);
                        }
                    }
                    Mark::Image { .. } => self.image(m),
                }
            }
        }

        unsafe fn font(&mut self, pt: f32, bold: bool) -> HFONT {
            let h = (pt * PT * self.dev.ky).round().max(1.0) as i32;
            if let Some((_, f)) = self.fonts.iter().find(|(k, _)| *k == (h, bold)) {
                return *f;
            }
            let face = wide("Arial");
            let weight = if bold { FW_BOLD } else { FW_NORMAL };
            // a negative height is the em, as a PDF's font size is
            let f = CreateFontW(
                -h,
                0,
                0,
                0,
                weight as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_TT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                DEFAULT_QUALITY as u32,
                (VARIABLE_PITCH | FF_SWISS) as u32,
                face.as_ptr(),
            );
            self.fonts.push(((h, bold), f));
            f
        }

        unsafe fn brush(&mut self, gray: f32) -> HBRUSH {
            let v = (gray.clamp(0.0, 1.0) * 255.0).round() as u8;
            if let Some((_, b)) = self.brushes.iter().find(|(k, _)| *k == v) {
                return *b;
            }
            let b = CreateSolidBrush(v as u32 * 0x01_01_01);
            self.brushes.push((v, b));
            b
        }

        // each letter advanced by Helvetica's width, rounded as it goes so
        // the line is exactly as long as the PDF's: right alignment and
        // wrapping hold on paper too
        unsafe fn text(&mut self, x: f32, y: f32, pt: f32, bold: bool, align: Align, text: &str) {
            let s = clean(text);
            let units: Vec<u16> = s.encode_utf16().collect();
            if units.is_empty() || units.len() != s.chars().count() {
                return;
            }
            let em = pt * PT * self.dev.kx;
            let (mut at, mut prev) = (0.0f32, 0i32);
            let mut dx = Vec::with_capacity(units.len());
            for c in s.chars() {
                let b = windows_byte(c);
                at += advance(b, bold) as f32 * em / 1000.0;
                let r = at.round() as i32;
                dx.push(r - prev);
                prev = r;
            }
            let x0 = self.dev.x(x)
                - match align {
                    Align::Left => 0,
                    Align::Right => prev,
                    Align::Center => prev / 2,
                };
            SelectObject(self.hdc, self.font(pt, bold));
            ExtTextOutW(self.hdc, x0, self.dev.y(y), 0, null(), units.as_ptr(), units.len() as u32, dx.as_ptr());
        }

        // Rules across or down the page are filled rectangles, square ended
        // and at least a dot thick; anything else is a pen.
        unsafe fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32) {
            let half = width / 2.0;
            let black = GetStockObject(BLACK_BRUSH);
            if (y1 - y2).abs() < 0.001 {
                let top = self.dev.y(y1 - half);
                let r = RECT { left: self.dev.x(x1.min(x2)), top, right: self.dev.x(x1.max(x2)), bottom: self.dev.y(y1 + half).max(top + 1) };
                FillRect(self.hdc, &r, black);
            } else if (x1 - x2).abs() < 0.001 {
                let left = self.dev.x(x1 - half);
                let r = RECT { left, top: self.dev.y(y1.min(y2)), right: self.dev.x(x1 + half).max(left + 1), bottom: self.dev.y(y1.max(y2)) };
                FillRect(self.hdc, &r, black);
            } else {
                let brush = LOGBRUSH { lbStyle: BS_SOLID, lbColor: 0, lbHatch: 0 };
                let w = (width * self.dev.kx).round().max(1.0) as u32;
                let pen = ExtCreatePen((PS_GEOMETRIC | PS_SOLID | PS_ENDCAP_FLAT) as u32, w, &brush, 0, null());
                let old = SelectObject(self.hdc, pen);
                MoveToEx(self.hdc, self.dev.x(x1), self.dev.y(y1), null_mut());
                LineTo(self.hdc, self.dev.x(x2), self.dev.y(y2));
                SelectObject(self.hdc, old);
                DeleteObject(pen);
            }
        }

        // a 1-bit DIB, white and black, its rows padded to 4 bytes
        unsafe fn image(&mut self, m: &Mark) {
            let Mark::Image { x, y, w, h, bits, px_w, px_h } = m else { return };
            if !super::image_ok(bits, *px_w, *px_h) {
                return;
            }
            let row = (*px_w as usize).div_ceil(8);
            let stride = (*px_w as usize).div_ceil(32) * 4;
            let mut buf = vec![0u8; stride * *px_h as usize];
            for r in 0..*px_h as usize {
                buf[r * stride..r * stride + row].copy_from_slice(&bits[r * row..(r + 1) * row]);
            }
            #[repr(C)]
            struct Bmi {
                head: BITMAPINFOHEADER,
                colors: [RGBQUAD; 2],
            }
            let white = RGBQUAD { rgbBlue: 255, rgbGreen: 255, rgbRed: 255, rgbReserved: 0 };
            let black = RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 };
            let bmi = Bmi {
                head: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: *px_w as i32,
                    biHeight: -(*px_h as i32),
                    biPlanes: 1,
                    biBitCount: 1,
                    biCompression: BI_RGB,
                    biSizeImage: buf.len() as u32,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 2,
                    biClrImportant: 2,
                },
                colors: [white, black],
            };
            let (left, top) = (self.dev.x(*x), self.dev.y(*y));
            StretchDIBits(
                self.hdc,
                left,
                top,
                (self.dev.x(x + w) - left).max(1),
                (self.dev.y(y + h) - top).max(1),
                0,
                0,
                *px_w as i32,
                *px_h as i32,
                buf.as_ptr() as *const c_void,
                &bmi as *const Bmi as *const BITMAPINFO,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        }

        unsafe fn free(&mut self) {
            SelectObject(self.hdc, GetStockObject(SYSTEM_FONT));
            for (_, f) in self.fonts.drain(..) {
                DeleteObject(f);
            }
            for (_, b) in self.brushes.drain(..) {
                DeleteObject(b);
            }
        }
    }

    fn windows_byte(c: char) -> u8 {
        crate::codepage::byte(c, crate::codepage::CodePage::Cp1252).unwrap_or(b'?')
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tickets::{Customer, Payment, TicketRef};
    use base64::Engine;

    fn shop() -> ShopInfo {
        ShopInfo {
            name: "Mi Tienda Móvil".into(),
            legal_name: "Juan Pérez García".into(),
            address: "Plaça Major, 1\n17600 Figueres (Girona)".into(),
            phone: "972 000 000".into(),
            nif: "12345678Z".into(),
            ..ShopInfo::default()
        }
    }

    fn acme() -> Customer {
        Customer { name: "Acme Telecom SL".into(), nif: "B12345674".into(), address: "Rambla Nova, 5, 2º 1ª\n17001 Girona".into() }
    }

    fn line(name: &str, qty: i64, unit_cents: i64, vat_bp: i64) -> SaleLine {
        SaleLine { name: name.into(), qty, unit_cents, vat_bp, ..SaleLine::default() }
    }

    fn record(kind: &str, number: &str, lines: Vec<SaleLine>) -> TicketRecord {
        let t = compute(&lines);
        TicketRecord {
            id: 1,
            kind: kind.into(),
            number: number.into(),
            issued_at: "2026-11-10T17:05:12".into(),
            user_name: "Ana".into(),
            total_cents: t.total_cents,
            item_count: t.item_count,
            payment: Payment { method: "card".into(), tendered_cents: t.total_cents, change_cents: 0, machine: None },
            voided_at: None,
            voided_by: None,
            void_reason: None,
            refund_of: None,
            refund_reason: None,
            refunds: Vec::new(),
            lines,
            vat: t.vat,
            prints: Vec::new(),
            payment_changes: Vec::new(),
            shop: shop(),
            customer: Some(acme()),
            training: false,
            qr: None,
            aeat: None,
        }
    }

    fn phone_sale() -> TicketRecord {
        let lines = vec![
            line("iPhone 15 128 GB Negro · IMEI 356789012345678", 1, 79_900, 2100),
            line("Funda de silicona con MagSafe para iPhone 15", 1, 1_990, 2100),
            line("Protector de pantalla cristal templado", 2, 995, 2100),
            line("Botella de agua 50 cl", 1, 120, 1000),
        ];
        record("sale", "TF2026-000123", lines)
    }

    fn many(n: usize) -> TicketRecord {
        let lines = (0..n).map(|i| line(&format!("Cable USB-C 1 m, artículo {}", i + 1), 1 + (i % 3) as i64, 690 + i as i64, 2100)).collect();
        record("sale", "TF2026-000124", lines)
    }

    fn texts(sheets: &[Sheet]) -> Vec<String> {
        sheets
            .iter()
            .flat_map(|s| &s.items)
            .filter_map(|m| match m {
                Mark::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn has(pdf: &[u8], needle: &[u8]) -> bool {
        pdf.windows(needle.len()).any(|w| w == needle)
    }

    fn cents(s: &str) -> i64 {
        let v: i64 = s.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap();
        if s.starts_with('-') {
            -v
        } else {
            v
        }
    }

    // a phone and its signal, 1-bit
    fn logo() -> Logo {
        let (w, h) = (300usize, 120usize);
        let row = w.div_ceil(8);
        let mut bits = vec![0u8; row * h];
        for y in 0..h {
            for x in 0..w {
                let phone = (10..70).contains(&x) && (5..115).contains(&y) && !((18..62).contains(&x) && (16..96).contains(&y));
                let (dx, dy) = (x as f32 - 40.0, y as f32 - 60.0);
                let r = (dx * dx + dy * dy).sqrt();
                let waves = dx > 40.0 && dy.abs() < dx * 0.8 && [70.0, 100.0, 130.0].iter().any(|k| (r - k).abs() < 6.0);
                let bar = (190..290).contains(&x) && [(30..44), (56..70), (82..96)].iter().any(|b| b.contains(&y));
                if phone || waves || bar {
                    bits[y * row + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        Logo { w: w as u32, h: h as u32, bits: base64::engine::general_purpose::STANDARD.encode(&bits), png: String::new() }
    }

    #[test]
    fn a_short_invoice_is_one_page_with_everything_a_factura_needs() {
        let rec = phone_sale();
        let sheets = invoice(&rec, None, &InvoiceOpts::default());
        assert_eq!(sheets.len(), 1);
        let pdf = to_pdf(&sheets, &rec.number);
        assert!(pdf.starts_with(b"%PDF-1."));
        assert!(pdf.ends_with(b"%%EOF\n"));
        assert!(has(&pdf, b"/Count 1"));
        for s in ["NIF: 12345678Z", "NIF: B12345674", "TF2026-000123", "TOTAL", "Acme Telecom SL", "17001 Girona", "Juan P"] {
            assert!(has(&pdf, s.as_bytes()), "missing {s}");
        }
        // the euro sign is WinAnsi's byte 0x80, accents their Latin-1 bytes
        assert_eq!(rec.total_cents, 84_000);
        assert!(has(&pdf, b"(840,00 \x80)"));
        assert!(has(&pdf, b"Descripci\xf3n"));
        assert!(has(&pdf, b"(N\xfamero)"));
        let all = texts(&sheets);
        for s in ["FACTURA", "Página 1 de 1", "Forma de pago: Tarjeta", "QR tributario:", "Base imponible"] {
            assert_eq!(all.iter().any(|t| t == s), s != "QR tributario:", "{s}");
        }
        // the unit price without VAT a full invoice must state: 799,00 / 1,21
        assert!(all.iter().any(|t| t == "660,33"), "{all:?}");
    }

    #[test]
    fn a_long_invoice_goes_on_with_the_heading_again_and_the_totals_last() {
        let rec = many(70);
        let sheets = invoice(&rec, None, &InvoiceOpts { terms: "Garantía de 3 años.\nDevoluciones en 15 días.".into(), copy: false });
        let pdf = to_pdf(&sheets, &rec.number);
        assert!(has(&pdf, b"/Count 2"), "{} pages", sheets.len());
        for (i, s) in sheets.iter().enumerate() {
            let t = texts(std::slice::from_ref(s));
            assert!(t.iter().any(|x| x == "Descripción"), "page {}", i + 1);
            assert!(t.iter().any(|x| x == &format!("Página {} de 2", i + 1)));
            assert_eq!(t.iter().any(|x| x == "TOTAL"), i == 1, "page {}", i + 1);
        }
        // every row on one page or the other, once
        let rows = texts(&sheets).iter().filter(|t| t.starts_with("Cable USB-C")).count();
        assert_eq!(rows, 70);
        assert!(texts(&sheets[1..]).iter().any(|t| t == "FACTURA TF2026-000124"));
        // a hundred and fifty go on to more pages, numbered
        let sheets = invoice(&many(150), None, &InvoiceOpts::default());
        assert!(sheets.len() >= 4);
        assert!(texts(&sheets).iter().any(|t| t == &format!("Página {n} de {n}", n = sheets.len())));
    }

    // the amounts printed in the table and in the VAT breakdown, read back
    #[test]
    fn what_the_page_says_adds_up_to_the_total() {
        for rec in [phone_sale(), many(70)] {
            let sheets = invoice(&rec, None, &InvoiceOpts::default());
            let mut amounts = 0;
            let mut vat_rows = Vec::new();
            for s in &sheets {
                // the VAT table starts under its heading; rows are above it
                let vat_top = s.items.iter().find_map(|m| match m {
                    Mark::Text { text, y, .. } if text == "Base imponible" => Some(*y),
                    _ => None,
                });
                let mut by_y: Vec<(f32, Vec<(f32, String)>)> = Vec::new();
                for m in &s.items {
                    let Mark::Text { x, y, pt, bold: false, align: Align::Right, text } = m else { continue };
                    if *pt > BODY || !text.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '-') {
                        continue;
                    }
                    if vat_top.is_some_and(|v| *y > v) {
                        match by_y.iter_mut().find(|(k, _)| (*k - *y).abs() < 0.01) {
                            Some((_, cells)) => cells.push((*x, text.clone())),
                            None => by_y.push((*y, vec![(*x, text.clone())])),
                        }
                    } else if (*x - (COLS[6] - 2.0)).abs() < 0.01 {
                        amounts += cents(text);
                    }
                }
                vat_rows.extend(by_y.into_iter().map(|(_, cells)| cells));
            }
            assert_eq!(amounts, rec.total_cents, "{}", rec.number);
            assert_eq!(vat_rows.len(), rec.vat.len());
            let mut total = 0;
            for row in &vat_rows {
                let get = |i: usize| cents(&row[i].1);
                assert_eq!(row.len(), 4, "{row:?}");
                assert_eq!(get(0) + get(2), get(3), "{row:?}");
                total += get(3);
            }
            assert_eq!(total, rec.total_cents);
            let shown = texts(&sheets);
            assert!(shown.contains(&format!("{} €", fmt(rec.total_cents))));
        }
    }

    #[test]
    fn a_refund_is_a_rectifying_invoice_naming_the_one_it_corrects() {
        let mut rec = record("refund", "TFR2026-000007", vec![line("Cable USB-C 1 m", -2, 690, 2100)]);
        rec.refund_of = Some(TicketRef { id: 1, number: "TF2026-000100".into(), issued_at: "2026-11-05T10:00:00".into(), total_cents: 1380, voided: false });
        rec.refund_reason = Some("No funciona la carga rápida".into());
        rec.payment.method = "cash".into();
        let sheets = invoice(&rec, None, &InvoiceOpts::default());
        let all = texts(&sheets);
        for s in ["FACTURA RECTIFICATIVA", "Rectifica a: TF2026-000100 del 05/11/2026", "Motivo: No funciona la carga rápida", "-13,80 €", "-2", "Forma de devolución: Efectivo"] {
            assert!(all.iter().any(|t| t == s), "missing {s:?} in {all:?}");
        }
        let pdf = to_pdf(&sheets, &rec.number);
        assert!(has(&pdf, b"(FACTURA RECTIFICATIVA)") && has(&pdf, b"TF2026-000100"));
    }

    #[test]
    fn practice_voided_copies_and_no_customer_say_so() {
        let mut rec = many(70);
        rec.training = true;
        rec.customer = None;
        let sheets = invoice(&rec, None, &InvoiceOpts { terms: String::new(), copy: true });
        assert!(sheets.len() > 1);
        for s in &sheets {
            assert!(texts(std::slice::from_ref(s)).iter().any(|t| t == "PRUEBA · SIN VALIDEZ"));
        }
        let all = texts(&sheets);
        assert!(all.iter().any(|t| t == "COPIA"));
        assert!(all.iter().any(|t| t == "Cliente: —"));
        assert!(all.iter().any(|t| t.ends_with(" · COPIA")));
        let mut voided = phone_sale();
        voided.voided_at = Some("2026-11-10T18:00:00".into());
        assert!(texts(&invoice(&voided, None, &InvoiceOpts::default())).iter().any(|t| t == "ANULADA"));
    }

    #[test]
    fn the_qr_goes_before_the_lines_at_30_to_40_mm() {
        let mut rec = phone_sale();
        rec.qr = Some("https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR?nif=12345678Z&numserie=TF2026-000123&fecha=10-11-2026&importe=840.00".into());
        let sheets = invoice(&rec, None, &InvoiceOpts::default());
        let items = &sheets[0].items;
        let black: Vec<(f32, f32, f32, f32)> = items
            .iter()
            .filter_map(|m| match m {
                Mark::Rect { x, y, w, h, fill: Some(g), stroke: false } if *g == 0.0 => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect();
        let (x0, y0) = black.iter().fold((f32::MAX, f32::MAX), |(a, b), r| (a.min(r.0), b.min(r.1)));
        let (x1, y1) = black.iter().fold((0f32, 0f32), |(a, b), r| (a.max(r.0 + r.2), b.max(r.1 + r.3)));
        assert!((30.0..=40.0).contains(&(x1 - x0)) && (30.0..=40.0).contains(&(y1 - y0)), "{} x {}", x1 - x0, y1 - y0);
        let heading = items.iter().find_map(|m| match m {
            Mark::Text { text, y, .. } if text == "Descripción" => Some(*y),
            _ => None,
        });
        assert!(heading.is_some_and(|h| h > y1));
        let all = texts(&sheets);
        assert!(all.iter().any(|t| t == "QR tributario:") && all.iter().any(|t| t == "VERI*FACTU"));
    }

    // nothing runs off the page, past the margins, or out of its column
    #[test]
    fn everything_stays_inside_the_page_and_its_column() {
        let mut long = phone_sale();
        long.lines.push(line(&"Supercalifragilisticoespialidoso".repeat(4), 999, 9_999_999, 2100));
        long.lines.push(line("Pantalla", -999, 9_999_999, 2100));
        long.shop.name = "Reparaciones y Accesorios de Telefonía Móvil del Alt Empordà, Sociedad Limitada".into();
        long.qr = Some("https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR?nif=12345678Z&numserie=TF2026-000123&fecha=10-11-2026&importe=840.00".into());
        let terms = "Garantía: ".repeat(60);
        for rec in [phone_sale(), many(70), long] {
            for copy in [false, true] {
                let sheets = invoice(&rec, Some(&logo()), &InvoiceOpts { terms: terms.clone(), copy });
                for s in &sheets {
                    for m in &s.items {
                        let (l, r, top, bottom) = match m {
                            Mark::Text { x, y, pt, bold, align, text } => {
                                let w = text_width_mm(text, *pt, *bold);
                                let l = match align {
                                    Align::Left => *x,
                                    Align::Right => x - w,
                                    Align::Center => x - w / 2.0,
                                };
                                if *align == Align::Left && (*x - (COLS[0] + 2.0)).abs() < 0.01 && *pt == BODY {
                                    assert!(l + w <= COLS[1] - 1.0, "{text:?} runs into the next column");
                                }
                                (l, l + w, y - pt * PT, *y)
                            }
                            Mark::Line { x1, y1, x2, y2, .. } => (x1.min(*x2), x1.max(*x2), y1.min(*y2), y1.max(*y2)),
                            Mark::Rect { x, y, w, h, .. } | Mark::Image { x, y, w, h, .. } => (*x, x + w, *y, y + h),
                        };
                        assert!(l >= LEFT - 0.3 && r <= RIGHT + 0.3 && top >= TOP - 3.0 && bottom <= FOOT + 0.1, "{m:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn helvetica_widths_measure_the_text() {
        // "Total" is 2223 thousandths of an em: 22.23 pt at 10 pt
        let w = text_width_mm("Total", 10.0, false);
        assert!((w - 22.23 * PT).abs() < 0.001 && (7.5..8.2).contains(&w), "{w}");
        assert!(text_width_mm("Total", 10.0, true) > w);
        assert_eq!(text_width_mm("", 10.0, false), 0.0);
        // a letter Windows-1252 lacks is measured as what is drawn for it
        assert_eq!(text_width_mm("📱", 10.0, false), text_width_mm("?", 10.0, false));
        assert!((text_width_mm("€ñ", 10.0, false) - 11.12 * PT).abs() < 0.001);
        assert!((text_width_mm("€ñ", 10.0, true) - 11.67 * PT).abs() < 0.001);
    }

    #[test]
    fn names_wrap_on_words_and_imei_stays_with_its_number() {
        let f = font(BODY, false);
        let name = "Samsung Galaxy S24 Ultra 256 GB Titanio Negro · IMEI 356789012345678";
        // the description column: the break falls on the "·", which goes
        let lines = wrap(name, f, COLS[1] - COLS[0] - 4.0);
        assert_eq!(lines, vec!["Samsung Galaxy S24 Ultra 256 GB Titanio Negro", "IMEI 356789012345678"]);
        assert_eq!(wrap(name, f, 200.0), vec![name]);
        for l in wrap(name, f, 25.0) {
            assert!(f.width(&l) <= 25.0 && !l.ends_with("IMEI") && !l.starts_with('·'), "{l:?}");
        }
        // a word wider than the column is cut, nothing is lost
        let long = "Supercalifragilisticoespialidoso".repeat(3);
        let cut = wrap(&long, f, 30.0);
        assert!(cut.len() > 1 && cut.concat() == long && cut.iter().all(|l| f.width(l) <= 30.0));
        // right aligned lines come out even, the same number of them
        let why = "Motivo: No carga: defecto de fábrica, se devuelve el importe";
        let w = even(why, f, 80.0);
        assert!(w < 80.0 && wrap(why, f, w).len() == wrap(why, f, 80.0).len());
        assert!(wrap(why, f, w).iter().all(|l| f.width(l) <= w && l.contains(' ')), "{:?}", wrap(why, f, w));
        assert_eq!(wrap("uno\n\ndos", f, 50.0), vec!["uno", "", "dos"]);
        assert!(wrap("", f, 50.0).is_empty());
    }

    #[test]
    fn the_pdf_is_well_formed() {
        let rec = phone_sale();
        let sheets = invoice(&rec, Some(&logo()), &InvoiceOpts { terms: "Funda (silicona) \\ 2".into(), copy: true });
        let pdf = to_pdf(&sheets, "TF2026-000123");
        assert!(has(&pdf, br"(Funda \(silicona\) \\ 2)"));
        assert!(has(&pdf, b"/XObject << /Im1 7 0 R >>") && has(&pdf, b"/Subtype /Image /Width 300 /Height 120"));
        assert!(has(&pdf, b"/MediaBox [0 0 595.28 841.89]"));
        // every xref offset points at its object
        let at = pdf.windows(10).rposition(|w| w == b"startxref\n").unwrap() + 10;
        let digits: String = pdf[at..].iter().take_while(|b| b.is_ascii_digit()).map(|&b| b as char).collect();
        let table = String::from_utf8_lossy(&pdf[digits.parse::<usize>().unwrap()..]).to_string();
        let entries: Vec<usize> = table.lines().skip(3).take_while(|l| l.ends_with(" n ")).map(|l| l[..10].parse().unwrap()).collect();
        assert_eq!(entries.len(), 4 + 3 + 1);
        for (i, off) in entries.iter().enumerate() {
            assert!(pdf[*off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()), "object {}", i + 1);
        }
        // nothing to print is still a PDF a reader opens
        assert!(has(&to_pdf(&[], "x"), b"/Count 1"));
    }

    // cargo test --lib invoice -- --ignored
    #[test]
    #[ignore = "writes sample PDFs to F:\\tiquet-check"]
    fn writes_samples() {
        let dir = std::path::Path::new(r"F:\tiquet-check");
        std::fs::create_dir_all(dir).unwrap();
        let terms = "Garantía de 3 años en productos nuevos (Real Decreto Legislativo 1/2007). Conserve esta factura: sin ella no se \
                     atienden garantías ni devoluciones.\nDevoluciones en 15 días con el embalaje original. Los terminales desprecintados \
                     solo se cambian por avería.";
        let mut rec = phone_sale();
        rec.qr = Some("https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR?nif=12345678Z&numserie=TF2026-000123&fecha=10-11-2026&importe=840.00".into());
        let opts = InvoiceOpts { terms: terms.into(), copy: false };
        let sheets = invoice(&rec, Some(&logo()), &opts);
        std::fs::write(dir.join("invoice-sample.pdf"), to_pdf(&sheets, &rec.number)).unwrap();

        let mut long = many(60);
        long.lines.insert(0, line("Samsung Galaxy S24 Ultra 256 GB Titanio Negro · IMEI 356789012345678", 1, 129_900, 2100));
        let t = compute(&long.lines);
        (long.total_cents, long.vat) = (t.total_cents, t.vat);
        let sheets = invoice(&long, Some(&logo()), &InvoiceOpts { terms: terms.into(), copy: true });
        assert_eq!(sheets.len(), 2);
        std::fs::write(dir.join("invoice-sample-long.pdf"), to_pdf(&sheets, &long.number)).unwrap();

        let mut back = record("refund", "TFR2026-000007", vec![line("iPhone 15 128 GB Negro · IMEI 356789012345678", -1, 79_900, 2100)]);
        back.refund_of = Some(TicketRef { id: 1, number: "TF2026-000123".into(), issued_at: "2026-11-05T10:00:00".into(), total_cents: 84_000, voided: false });
        back.refund_reason = Some("No carga: defecto de fábrica, se devuelve el importe".into());
        back.training = true;
        let sheets = invoice(&back, None, &InvoiceOpts::default());
        std::fs::write(dir.join("invoice-sample-refund.pdf"), to_pdf(&sheets, &back.number)).unwrap();
    }

    // The same pages drawn through a real Windows driver, the PDF writer
    // every Windows has, given a file so it asks nothing. Only when asked:
    // TIQUET_INVOICE_GDI=<file.pdf> cargo test --lib invoice -- --ignored
    #[cfg(windows)]
    #[test]
    #[ignore = "talks to the Windows spooler"]
    fn draws_through_the_windows_pdf_writer() {
        let Some(out) = std::env::var_os("TIQUET_INVOICE_GDI").map(std::path::PathBuf::from) else { return };
        let _ = std::fs::remove_file(&out);
        let mut rec = phone_sale();
        rec.qr = Some("https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR?nif=12345678Z&numserie=TF2026-000123&fecha=10-11-2026&importe=840.00".into());
        let sheets = invoice(&rec, Some(&logo()), &InvoiceOpts { terms: "Garantía de 3 años. Devoluciones en 15 días.".into(), copy: true });
        win::draw("Microsoft Print to PDF", "Factura prueba", &sheets, 1, Some(&out)).unwrap();
        // the same sheets written directly, to lay one over the other
        std::fs::write(out.with_extension("direct.pdf"), to_pdf(&sheets, &rec.number)).unwrap();
        // the spooler writes the file a moment after the job ends
        for _ in 0..100 {
            if out.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(std::fs::read(&out).unwrap().starts_with(b"%PDF"));
    }
}
