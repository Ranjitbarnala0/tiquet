// A ticket as a PDF the size of the paper roll: 80 mm wide and as long as
// the ticket, in Courier, with the logo and the AEAT's QR as they print.
// Built by hand, a few objects and an xref: no library, no fonts to embed
// (Courier is one of the fonts every PDF reader has).

use crate::codepage::{byte, CodePage};
use crate::layout::{Line, Size};
use crate::printer::Page;

const MM: f64 = 72.0 / 25.4;
const PAPER: f64 = 80.0 * MM;
const MARGIN: f64 = 4.0 * MM;
// thermal printers have 203 dots per inch
const DOT: f64 = 72.0 / 203.0;

// one character cell of the receipt and the height of a line, in points
fn cell(cols: usize) -> (f64, f64) {
    let cw = (PAPER - 2.0 * MARGIN) / cols.clamp(16, 64) as f64;
    (cw, cw * 2.25)
}

fn height(p: &Page) -> f64 {
    let (cw, lh) = cell(p.cols);
    let width = cw * p.cols.clamp(16, 64) as f64;
    let body: f64 = p
        .lines
        .iter()
        .map(|l| match l {
            Line::Text { size: Size::Normal, .. } => lh,
            Line::Text { .. } => 2.0 * lh,
            Line::Logo => p.logo.as_ref().map_or(0.0, |g| logo_size(g.w, g.h, width).1),
            Line::Qr { rows, module } => rows.len() as f64 * *module as f64 * DOT + lh,
        })
        .sum();
    body + 2.0 * MARGIN
}

fn logo_size(w: u32, h: u32, width: f64) -> (f64, f64) {
    let lw = (w as f64 * DOT).min(width);
    (lw, lw * h as f64 / w.max(1) as f64)
}

// WinAnsi, which is Windows-1252 for every character a ticket uses
fn pdf_string(s: &str) -> Vec<u8> {
    let mut out = vec![b'('];
    for c in s.chars() {
        let b = byte(c, CodePage::Cp1252).unwrap_or(b'?');
        match b {
            b'(' | b')' | b'\\' => out.extend_from_slice(&[b'\\', b]),
            0x20..=0x7e => out.push(b),
            _ => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
        }
    }
    out.push(b')');
    out
}

fn content(p: &Page, page_h: f64) -> Vec<u8> {
    let (cw, lh) = cell(p.cols);
    let cols = p.cols.clamp(16, 64) as f64;
    let width = cw * cols;
    let x0 = (PAPER - width) / 2.0;
    let mut y = page_h - MARGIN;
    let mut out = Vec::new();
    for l in &p.lines {
        match l {
            Line::Text { text, bold, size } => {
                let (advance, fs, tz) = match size {
                    Size::Normal => (lh, cw / 0.6, 100),
                    Size::Tall => (2.0 * lh, 2.0 * cw / 0.6, 50),
                    Size::Big => (2.0 * lh, 2.0 * cw / 0.6, 100),
                };
                let t = text.trim_end();
                if !t.is_empty() {
                    let font = if *bold { "F2" } else { "F1" };
                    let base = y - advance * 0.78;
                    out.extend_from_slice(format!("BT /{font} {fs:.2} Tf {tz} Tz 1 0 0 1 {x0:.2} {base:.2} Tm ").as_bytes());
                    out.extend(pdf_string(t));
                    out.extend_from_slice(b" Tj ET\n");
                }
                y -= advance;
            }
            Line::Logo => {
                if let Some(g) = &p.logo {
                    let (w, h) = logo_size(g.w, g.h, width);
                    let x = x0 + (width - w) / 2.0;
                    out.extend_from_slice(format!("q {w:.2} 0 0 {h:.2} {x:.2} {:.2} cm /Im1 Do Q\n", y - h).as_bytes());
                    y -= h;
                }
            }
            Line::Qr { rows, module } => {
                let m = *module as f64 * DOT;
                let size = rows.len() as f64 * m;
                let x = x0 + (width - size) / 2.0;
                let top = y - lh / 2.0;
                out.extend_from_slice(b"0 g\n");
                for (r, row) in rows.iter().enumerate() {
                    for (c, ch) in row.chars().enumerate() {
                        if ch == '1' {
                            let (cx, cy) = (x + c as f64 * m, top - (r + 1) as f64 * m);
                            out.extend_from_slice(format!("{cx:.2} {cy:.2} {m:.2} {m:.2} re\n").as_bytes());
                        }
                    }
                }
                out.extend_from_slice(b"f\n");
                y -= size + lh;
            }
        }
    }
    out
}

pub fn render(pages: &[Page], title: &str) -> Vec<u8> {
    // 1 catalog, 2 page tree, 3 and 4 the fonts; then each page, its content
    // and its logo; the document info last
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        Vec::new(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier-Bold /Encoding /WinAnsiEncoding >>".to_vec(),
    ];
    let mut kids = Vec::new();
    for p in pages {
        let page_h = height(p);
        let page_id = objects.len() + 1;
        let content_id = page_id + 1;
        let logo = p.logo.as_ref().and_then(|g| g.raster().map(|bits| (g, bits)));
        let image_id = logo.as_ref().map(|_| page_id + 2);
        kids.push(format!("{page_id} 0 R"));
        let xobject = image_id.map(|id| format!(" /XObject << /Im1 {id} 0 R >>")).unwrap_or_default();
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAPER:.2} {page_h:.2}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >>{xobject} >> /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        );
        let body = content(p, page_h);
        let mut stream = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
        stream.extend(body);
        stream.extend_from_slice(b"\nendstream");
        objects.push(stream);
        if let Some((g, bits)) = logo {
            // 1 = black in the raster; /Decode turns it into the gray 0
            let mut img = format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 1 /Decode [1 0] /Length {} >>\nstream\n",
                g.w,
                g.h,
                bits.len()
            )
            .into_bytes();
            img.extend(bits);
            img.extend_from_slice(b"\nendstream");
            objects.push(img);
        }
    }
    objects[1] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), kids.len()).into_bytes();
    let mut info = b"<< /Producer (Tiquet) /Title ".to_vec();
    info.extend(pdf_string(title));
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
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info {} 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1, objects.len()).as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Logo;
    use base64::Engine;

    fn text(t: &str, size: Size) -> Line {
        Line::Text { text: t.into(), bold: false, size }
    }

    #[test]
    fn a_ticket_becomes_a_well_formed_pdf() {
        let bits = vec![0xF0u8; 2 * 16];
        let logo = Logo { w: 16, h: 16, bits: base64::engine::general_purpose::STANDARD.encode(&bits), png: String::new() };
        let page = Page {
            lines: vec![
                Line::Logo,
                text("   MI TIENDA", Size::Big),
                text("Funda (silicona) 12,50 €", Size::Normal),
                text("TOTAL", Size::Tall),
                Line::Qr { rows: vec!["101".into(), "010".into(), "101".into()], module: 4 },
            ],
            logo: Some(logo),
            cols: 32,
        };
        let pdf = render(&[page.clone(), page], "T2026-000001");
        let has = |n: &[u8]| pdf.windows(n.len()).any(|w| w == n);
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        assert!(has(b"/Count 2"));
        // parentheses escaped and the euro sign in WinAnsi
        assert!(has(br"Funda \(silicona\) 12,50 \200"));
        // every xref offset points at its object
        let at = pdf.windows(10).rposition(|w| w == b"startxref\n").unwrap() + 10;
        let digits: String = pdf[at..].iter().take_while(|b| b.is_ascii_digit()).map(|&b| b as char).collect();
        let table = String::from_utf8(pdf[digits.parse::<usize>().unwrap()..].to_vec()).unwrap();
        let entries: Vec<usize> = table.lines().skip(3).take_while(|l| l.ends_with(" n ")).map(|l| l[..10].parse().unwrap()).collect();
        assert_eq!(entries.len(), 2 + 2 + 2 * 3 + 1);
        for (i, off) in entries.iter().enumerate() {
            assert!(pdf[*off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()), "object {}", i + 1);
        }
    }
}
