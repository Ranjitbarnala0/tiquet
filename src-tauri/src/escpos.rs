use crate::codepage::{byte, printable, CodePage};
use crate::layout::{Line, Size};
use crate::settings::{Logo, Settings};

const ESC: u8 = 0x1B;
const GS: u8 = 0x1D;
const FS: u8 = 0x1C;

// pulse on drawer pin 2: 50 ms on, 500 ms off
const KICK: [u8; 5] = [ESC, b'p', 0, 25, 250];

pub struct Opts {
    pub cols: usize,
    pub cp: CodePage,
    pub cut: bool,
    pub feed: u8,
    pub kick: bool,
    pub cancel_chinese: bool,
}

impl Opts {
    pub fn new(s: &Settings, kick: bool) -> Opts {
        Opts {
            cols: s.columns as usize,
            cp: CodePage::parse(&s.codepage),
            cut: s.cut,
            feed: s.feed_lines.min(12) as u8,
            kick,
            cancel_chinese: s.cancel_chinese,
        }
    }
}

fn init(b: &mut Vec<u8>, o: &Opts, cp: CodePage) {
    b.extend_from_slice(&[ESC, b'@']);
    // Chinese firmwares start in double-byte mode and print every accent as
    // a hanzi until FS . turns it off
    if o.cancel_chinese {
        b.extend_from_slice(&[FS, b'.']);
    }
    b.extend_from_slice(&[ESC, b'R', 0, ESC, b't', cp.table(), ESC, b'M', 0, ESC, b'a', 0, ESC, b'2']);
}

fn finish(b: &mut Vec<u8>, o: &Opts) {
    // the cutter sits above the head, so feed before cutting
    b.extend_from_slice(&[ESC, b'd', o.feed]);
    if o.cut {
        b.extend_from_slice(&[GS, b'V', 1]);
    }
}

pub fn document(lines: &[Line], logo: Option<&Logo>, o: &Opts) -> Vec<u8> {
    let mut b = Vec::with_capacity(4096);
    init(&mut b, o, o.cp);
    if o.kick {
        b.extend_from_slice(&KICK);
    }
    for line in lines {
        match line {
            Line::Logo => {
                if let Some(l) = logo {
                    raster(&mut b, l, o.cols);
                }
            }
            Line::Text { text, bold, size } => {
                let mode = match size {
                    Size::Normal => 0,
                    Size::Tall => 0x01,
                    Size::Big => 0x11,
                };
                if *bold {
                    b.extend_from_slice(&[ESC, b'E', 1]);
                }
                if mode != 0 {
                    b.extend_from_slice(&[GS, b'!', mode]);
                }
                b.extend(text.trim_end().chars().map(|c| byte(c, o.cp).unwrap_or(b'?')));
                b.push(b'\n');
                if mode != 0 {
                    b.extend_from_slice(&[GS, b'!', 0]);
                }
                if *bold {
                    b.extend_from_slice(&[ESC, b'E', 0]);
                }
            }
        }
    }
    finish(&mut b, o);
    b
}

// GS v 0 in bands: some clones drop images taller than their buffer
// A logo wider than the paper is scaled down, the way the preview shows it:
// each output dot is black when at least half of the dots it covers are.
fn fit(bits: &[u8], w: usize, h: usize, out_w: usize) -> (Vec<u8>, usize, usize) {
    let row = w.div_ceil(8);
    if w <= out_w {
        return (bits.to_vec(), w, h);
    }
    let out_h = (h * out_w).div_ceil(w).max(1);
    let out_row = out_w.div_ceil(8);
    let mut out = vec![0u8; out_row * out_h];
    for oy in 0..out_h {
        let (y0, y1) = (oy * h / out_h, ((oy + 1) * h / out_h).max(oy * h / out_h + 1));
        for ox in 0..out_w {
            let (x0, x1) = (ox * w / out_w, ((ox + 1) * w / out_w).max(ox * w / out_w + 1));
            let mut black = 0;
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    black += ((bits[y * row + x / 8] >> (7 - x % 8)) & 1) as usize;
                }
            }
            if black * 2 >= (y1 - y0) * (x1 - x0) {
                out[oy * out_row + ox / 8] |= 0x80 >> (ox % 8);
            }
        }
    }
    (out, out_w, out_h)
}

fn raster(b: &mut Vec<u8>, logo: &Logo, cols: usize) {
    let Some(src) = logo.raster() else { return };
    let paper = (cols * 12).min(576);
    let (bits, w, h) = fit(&src, logo.w as usize, logo.h as usize, paper);
    let row = w.div_ceil(8);
    let out_row = paper.div_ceil(8);
    let pad = (paper - w) / 2;

    let mut y = 0;
    while y < h {
        let band = (h - y).min(128);
        b.extend_from_slice(&[GS, b'v', b'0', 0, out_row as u8, (out_row >> 8) as u8, band as u8, (band >> 8) as u8]);
        for r in y..y + band {
            let mut out = vec![0u8; out_row];
            for x in 0..w {
                if (bits[r * row + x / 8] >> (7 - x % 8)) & 1 == 1 {
                    let ox = x + pad;
                    out[ox / 8] |= 0x80 >> (ox % 8);
                }
            }
            b.extend_from_slice(&out);
        }
        y += band;
    }
}

pub fn kick(s: &Settings) -> Vec<u8> {
    let o = Opts::new(s, true);
    let mut b = Vec::new();
    init(&mut b, &o, o.cp);
    b.extend_from_slice(&KICK);
    b
}

// one line per code page; the shop picks whichever prints cleanly
pub fn charset_test(s: &Settings) -> Vec<u8> {
    let o = Opts::new(s, false);
    let mut b = Vec::new();
    init(&mut b, &o, CodePage::Ascii);
    for l in ["PRUEBA DE CARACTERES", "Elija la linea que se lea bien y", "seleccionela en Ajustes > Impresora.", ""] {
        b.extend_from_slice(l.as_bytes());
        b.push(b'\n');
    }
    let sample = "áéíóú ÁÉÍÓÚ ñÑ çÇ ü € ¿? ¡!";
    for cp in CodePage::ALL {
        b.extend_from_slice(&[ESC, b't', cp.table()]);
        b.extend_from_slice(format!("{:<7}", cp.id().to_uppercase()).as_bytes());
        b.extend(printable(sample, cp).chars().map(|c| byte(c, cp).unwrap_or(b'?')));
        b.push(b'\n');
    }
    finish(&mut b, &o);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codepage::CodePage;
    use crate::layout::{self, TicketView};
    use crate::tickets::{compute, sample_lines, Payment, ShopInfo};
    use base64::Engine;

    // minimal ESC/POS reader: enough to prove the bytes say what the preview shows
    fn decode(b: &[u8], cp: CodePage) -> (Vec<String>, Vec<String>) {
        let mut rev = std::collections::HashMap::new();
        for c in (0x80..0x2600u32).filter_map(char::from_u32) {
            if let Some(x) = byte(c, cp) {
                rev.entry(x).or_insert(c);
            }
        }
        let back = |x: u8| if x < 0x80 { x as char } else { rev.get(&x).copied().unwrap_or('?') };
        let (mut lines, mut cmds, mut cur) = (Vec::new(), Vec::new(), String::new());
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                ESC => {
                    let n = match b[i + 1] {
                        b'@' | b'2' => 2,
                        b'p' => 5,
                        _ => 3,
                    };
                    cmds.push(format!("ESC {}", b[i + 1] as char));
                    i += n;
                }
                FS => {
                    cmds.push("FS .".into());
                    i += 2;
                }
                GS if b[i + 1] == b'v' => {
                    let w = b[i + 4] as usize | (b[i + 5] as usize) << 8;
                    let h = b[i + 6] as usize | (b[i + 7] as usize) << 8;
                    cmds.push(format!("RASTER {w}x{h}"));
                    i += 8 + w * h;
                }
                GS => {
                    cmds.push(format!("GS {}", b[i + 1] as char));
                    i += 3;
                }
                b'\n' => {
                    lines.push(std::mem::take(&mut cur));
                    i += 1;
                }
                x => {
                    cur.push(back(x));
                    i += 1;
                }
            }
        }
        (lines, cmds)
    }

    fn logo() -> Logo {
        let (w, h) = (100u32, 130u32);
        let bits = vec![0xAAu8; (w as usize).div_ceil(8) * h as usize];
        Logo { w, h, bits: base64::engine::general_purpose::STANDARD.encode(bits), png: String::new() }
    }

    #[test]
    fn bytes_carry_exactly_the_preview_text() {
        for cp in ["cp858", "cp1252", "cp850", "ascii"] {
            let s = Settings { codepage: cp.into(), ..Settings::default() };
            let lines = sample_lines(2100);
            let totals = compute(&lines);
            let pay = Payment { method: "cash".into(), tendered_cents: 3000, change_cents: 825 };
            let shop = ShopInfo { name: "Óptica Ñandú".into(), footer: "¡Gracias! 5 € de descuento".into(), ..Default::default() };
            let v = TicketView {
                shop: &shop,
                kind: "sale",
                number: "T2026-000001",
                issued_at: "2026-10-16T17:00:00",
                user_name: "Begoña",
                lines: &lines,
                totals: &totals,
                payment: Some(&pay),
                refund_of: None,
                reason: None,
                show_vat: true,
                copy: false,
                voided: false,
                customer: None,
                training: false,
            };
            let preview = layout::ticket(&v, &s, true);
            let lg = logo();
            let bytes = document(&preview, Some(&lg), &Opts::new(&s, true));
            let (printed, cmds) = decode(&bytes, CodePage::parse(cp));
            let expected: Vec<String> = preview
                .iter()
                .filter_map(|l| match l {
                    Line::Text { text, .. } => Some(text.trim_end().to_string()),
                    Line::Logo => None,
                })
                .collect();
            assert_eq!(printed, expected, "{cp}");
            assert_eq!(cmds[0], "ESC @");
            assert_eq!(cmds[1], "FS .");
            assert!(cmds.contains(&"ESC p".to_string()));
            assert!(cmds.contains(&"RASTER 72x128".to_string()) && cmds.contains(&"RASTER 72x2".to_string()));
            assert_eq!(cmds.last().unwrap(), "GS V");
        }
    }

    #[test]
    fn a_logo_wider_than_the_paper_is_scaled_not_cut() {
        // 576 dots wide, left half black: on 58 mm paper (384 dots) it must
        // still show both halves, half black and half white
        let (w, h) = (576usize, 4usize);
        let mut row = vec![0u8; w / 8];
        row[..w / 16].fill(0xFF);
        let bits: Vec<u8> = row.iter().cycle().take(row.len() * h).copied().collect();
        let lg = Logo { w: w as u32, h: h as u32, bits: base64::engine::general_purpose::STANDARD.encode(&bits), png: String::new() };
        let mut b = Vec::new();
        raster(&mut b, &lg, 32);
        assert_eq!(&b[..8], &[GS, b'v', b'0', 0, 48, 0, 3, 0]);
        let first = &b[8..8 + 48];
        assert!(first[..24].iter().all(|&x| x == 0xFF) && first[24..].iter().all(|&x| x == 0), "{first:?}");
    }

    #[test]
    fn logo_is_centred_on_the_paper() {
        let lg = Logo { w: 8, h: 1, bits: base64::engine::general_purpose::STANDARD.encode([0xFFu8]), png: String::new() };
        let mut b = Vec::new();
        raster(&mut b, &lg, 48);
        let row = &b[8..];
        assert_eq!(row.len(), 72);
        // 576 dots wide, 8 black dots in the middle: bytes 35 and 36 hold dots 284..292
        assert_eq!(row.iter().map(|x| x.count_ones()).sum::<u32>(), 8);
        assert_eq!((row[35], row[36]), (0x0F, 0xF0));
    }
}
