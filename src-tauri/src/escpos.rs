use crate::codepage::{byte, printable, CodePage};
use crate::layout::{classify, Line, Size, Visual};
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
    pub modern: bool,
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
            modern: s.ticket_style == "modern" && s.printer_kind != "serial",
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
    if o.modern {
        if let Some((bits, row, h)) = render_modern(lines, logo, o.cols) {
            // Pure raster job: send only reset, optional drawer kick, left-align,
            // and a 0-dot buffer flush (ESC J 0). Text-mode setup commands such
            // as ESC 2 directly before GS v 0 cause some 58 mm clones to reject
            // GS v 0 and print the raw bitmap bytes as gibberish text.
            b.extend_from_slice(&[ESC, b'@']);
            if o.kick {
                b.extend_from_slice(&KICK);
            }
            b.extend_from_slice(&[ESC, b'a', 0, ESC, b'J', 0]);
            bands(&mut b, &bits, row, h);
            finish(&mut b, o);
            return b;
        }
    }
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
            Line::Qr { rows, module } => qr(&mut b, rows, *module, o.cols),
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
    let mut out = vec![0u8; out_row * h];
    for r in 0..h {
        for x in 0..w {
            if (bits[r * row + x / 8] >> (7 - x % 8)) & 1 == 1 {
                let ox = x + pad;
                out[r * out_row + ox / 8] |= 0x80 >> (ox % 8);
            }
        }
    }
    bands(b, &out, out_row, h);
}

// GS v 0, at most 24 dot rows at a time (1,152 bytes on 58 mm, 1,728 on 80 mm):
// stays safely below cheap POS-58 receive buffers and avoids yL >= 0x80 signed-byte bugs.
const MAX_BAND: usize = 24;

fn bands(b: &mut Vec<u8>, bits: &[u8], row: usize, h: usize) {
    let mut y = 0;
    while y < h {
        let band = (h - y).min(MAX_BAND);
        b.extend_from_slice(&[GS, b'v', b'0', 0, row as u8, (row >> 8) as u8, band as u8, (band >> 8) as u8]);
        b.extend_from_slice(&bits[y * row..(y + band) * row]);
        y += band;
    }
}

// The AEAT's QR as an image rather than the printer's own QR command, which
// clones implement unevenly: each module a square of `module` dots, four
// modules of white all round, centred like the preview shows it.
fn qr(b: &mut Vec<u8>, rows: &[String], module: usize, cols: usize) {
    let n = rows.len();
    let paper = (cols * 12).min(576);
    if n == 0 || (n + 8) * module > paper {
        return;
    }
    let side = (n + 8) * module;
    let out_row = paper.div_ceil(8);
    let left = (paper - side) / 2 + 4 * module;
    let mut bits = vec![0u8; out_row * side];
    for (y, row) in rows.iter().enumerate() {
        for (x, _) in row.bytes().enumerate().filter(|(_, c)| *c == b'1') {
            for dy in 0..module {
                let r = (4 + y) * module + dy;
                for dx in 0..module {
                    let ox = left + x * module + dx;
                    bits[r * out_row + ox / 8] |= 0x80 >> (ox % 8);
                }
            }
        }
    }
    bands(b, &bits, out_row, side);
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

#[cfg(not(windows))]
fn render_modern(_lines: &[Line], _logo: Option<&Logo>, _cols: usize) -> Option<(Vec<u8>, usize, usize)> {
    None
}

// Crisp 203-dpi monochrome thermal rendering using Windows GDI hinted 1-bit
// TrueType fonts (Segoe UI + Consolas) and thermal-safe geometric dividers and
// frames that never exceed ~45% black dots per row (preventing voltage-droop
// dropout and smudging on 58 mm USB thermal print heads).
#[cfg(windows)]
fn render_modern(lines: &[Line], logo: Option<&Logo>, cols: usize) -> Option<(Vec<u8>, usize, usize)> {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW, ExtTextOutW,
        GdiFlush, SelectObject, SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CALCRECT, DT_CENTER,
        DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_WORDBREAK, FF_MODERN, FF_SWISS, FIXED_PITCH,
        FW_BOLD, FW_SEMIBOLD, HBITMAP, HFONT, NONANTIALIASED_QUALITY, OUT_TT_PRECIS, TRANSPARENT,
    };

    let paper = (cols * 12).min(576);
    if paper == 0 || lines.is_empty() {
        return None;
    }
    let scale = if cols <= 32 { 0.86 } else if cols <= 42 { 0.94 } else { 1.0 };
    let sc = |v: f32| (v * scale).round() as i32;
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let wide_raw = |s: &str| -> Vec<u16> { s.encode_utf16().collect() };
    let make_font = |face: &str, h: i32, w: i32, weight: u32, fixed: bool| -> HFONT {
        let f = wide(face);
        let pitch = if fixed { FIXED_PITCH | FF_MODERN } else { DEFAULT_PITCH | FF_SWISS };
        unsafe {
            CreateFontW(
                -h,
                w,
                0,
                0,
                weight as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_TT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                NONANTIALIASED_QUALITY as u32,
                pitch as u32,
                f.as_ptr(),
            )
        }
    };

    let hdc = unsafe { CreateCompatibleDC(null_mut()) };
    if hdc.is_null() {
        return None;
    }

    let pad_x = if cols <= 32 { 6 } else { 10 };
    let inner_w = paper as i32 - 2 * pad_x;

    let f_big = make_font("Segoe UI", sc(36.0), 0, FW_BOLD, false);
    let f_tall = make_font("Segoe UI", sc(28.0), 0, FW_BOLD, false);
    let f_badge = make_font("Segoe UI", sc(22.0), 0, FW_BOLD, false);
    let f_body = make_font("Segoe UI", sc(22.0), 0, FW_SEMIBOLD, false);
    let f_semi = make_font("Segoe UI", sc(22.0), 0, FW_BOLD, false);
    let f_bold = make_font("Segoe UI", sc(23.0), 0, FW_BOLD, false);
    let f_tot_lbl = make_font("Segoe UI", sc(27.0), 0, FW_BOLD, false);
    let f_tot_amt = make_font("Segoe UI", sc(36.0), 0, FW_BOLD, false);
    let cw = ((inner_w as usize) / cols.max(16)).max(8) as i32;
    let table_x0 = (paper as i32 - cw * cols as i32) / 2;
    let f_table = make_font("Consolas", (cw * 2 + 1).max(20), cw, FW_BOLD, true);
    let f_table_b = make_font("Consolas", (cw * 2 + 2).max(21), cw, FW_BOLD, true);

    let items = classify(lines, cols);

    let measure_text = |font: HFONT, text: &str, width: i32, flags: u32| -> (i32, i32) {
        let mut r = RECT { left: 0, top: 0, right: width.max(1), bottom: 0 };
        let w = wide_raw(text);
        if w.is_empty() {
            return (0, 0);
        }
        unsafe {
            SelectObject(hdc, font);
            DrawTextW(hdc, w.as_ptr(), w.len() as i32, &mut r, flags | DT_CALCRECT | DT_NOPREFIX);
        }
        (r.right - r.left, r.bottom - r.top)
    };

    let draw_text = |font: HFONT, text: &str, x: i32, y: i32, w_box: i32, h_box: i32, flags: u32, color: u32| {
        let mut r = RECT { left: x, top: y, right: x + w_box.max(1), bottom: y + h_box.max(1) };
        let w = wide_raw(text);
        if w.is_empty() {
            return;
        }
        unsafe {
            SelectObject(hdc, font);
            SetTextColor(hdc, color);
            DrawTextW(hdc, w.as_ptr(), w.len() as i32, &mut r, flags | DT_NOPREFIX);
        }
    };

    // Direct 1-bit pixel helpers for thermal-safe dashed dividers & framed cards
    // (peak black dots per row stays <= 45% so cheap 58 mm print heads burn pitch black).
    let set_px = |pixels: &mut [u32], x: i32, y: i32| {
        if x >= 0 && y >= 0 && (x as usize) < paper {
            if let Some(p) = pixels.get_mut(y as usize * paper + x as usize) {
                *p = 0x0000_0000;
            }
        }
    };
    let hline_dash = |pixels: &mut [u32], x0: i32, x1: i32, y: i32, thick: i32, on: i32, off: i32, phase: i32| {
        let period = (on + off).max(1);
        for dy in 0..thick {
            for x in x0..x1 {
                if ((x - x0 + phase).rem_euclid(period)) < on {
                    set_px(pixels, x, y + dy);
                }
            }
        }
    };
    let vline = |pixels: &mut [u32], x: i32, y0: i32, y1: i32, thick: i32| {
        for py in y0..y1 {
            for dx in 0..thick {
                set_px(pixels, x + dx, py);
            }
        }
    };
    let dashed_frame = |pixels: &mut [u32], x0: i32, y0: i32, x1: i32, y1: i32, thick: i32, on: i32, off: i32| {
        // Symmetric chamfered corners with solid side bars and thermal-safe dashed top/bottom rules
        hline_dash(pixels, x0 + 2, x1 - 2, y0, thick, on, off, 0);
        hline_dash(pixels, x0 + 2, x1 - 2, y1 - thick, thick, on, off, 0);
        // Clean 4px right-corner caps so both ends meet the vertical bars neatly
        let cap_x = (x1 - 2 - on).max(x0 + 2);
        for dy in 0..thick {
            for x in cap_x..(x1 - 2) {
                set_px(pixels, x, y0 + dy);
                set_px(pixels, x, y1 - thick + dy);
            }
        }
        vline(pixels, x0, y0 + 2, y1 - 2, thick);
        vline(pixels, x1 - thick, y0 + 2, y1 - 2, thick);
    };

    let logo_fit = logo.and_then(|l| l.raster().map(|b| fit(&b, l.w as usize, l.h as usize, paper)));

    let run = |draw: bool, pixels: &mut [u32]| -> i32 {
        let mut y = sc(6.0);
        for item in &items {
            match item {
                Visual::Blank => {
                    y += sc(9.0);
                }
                Visual::Logo => {
                    if let Some((bits, w, h)) = &logo_fit {
                        if draw {
                            let row = w.div_ceil(8);
                            let x0 = (paper - w) / 2;
                            for ry in 0..*h {
                                let py = y as usize + ry;
                                for rx in 0..*w {
                                    if (bits[ry * row + rx / 8] >> (7 - rx % 8)) & 1 == 1 {
                                        if let Some(p) = pixels.get_mut(py * paper + x0 + rx) {
                                            *p = 0x0000_0000;
                                        }
                                    }
                                }
                            }
                        }
                        y += *h as i32 + sc(8.0);
                    }
                }
                Visual::Qr { rows, module } => {
                    let n = rows.len();
                    if n > 0 && (n + 8) * *module <= paper {
                        let side = (n + 8) * *module;
                        if draw {
                            let left = (paper - side) / 2 + 4 * *module;
                            let top = y as usize + 4 * *module;
                            for (ry, row) in rows.iter().enumerate() {
                                for (rx, _) in row.bytes().enumerate().filter(|(_, c)| *c == b'1') {
                                    for dy in 0..*module {
                                        let py = top + ry * *module + dy;
                                        for dx in 0..*module {
                                            if let Some(p) = pixels.get_mut(py * paper + left + rx * *module + dx) {
                                                *p = 0x0000_0000;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        y += side as i32 + sc(6.0);
                    }
                }
                Visual::Rule { heavy } => {
                    let pad = sc(7.0);
                    let h_rule = if *heavy { 5 } else { 2 };
                    if draw {
                        unsafe { GdiFlush(); }
                        let ry = y + pad;
                        if *heavy {
                            // Crisp double staggered rule (~40% dot density per row)
                            hline_dash(pixels, pad_x, paper as i32 - pad_x, ry, 2, 4, 6, 0);
                            hline_dash(pixels, pad_x, paper as i32 - pad_x, ry + 3, 2, 4, 6, 5);
                        } else {
                            // Crisp geometric dotted rule (~37.5% dot density per row)
                            hline_dash(pixels, pad_x, paper as i32 - pad_x, ry, 2, 3, 5, 0);
                        }
                    }
                    y += pad * 2 + h_rule;
                }
                Visual::Heading { text, big } => {
                    let f = if *big { f_big } else { f_tall };
                    let (_, h) = measure_text(f, text, inner_w, DT_CENTER | DT_WORDBREAK);
                    if draw {
                        draw_text(f, text, pad_x, y, inner_w, h, DT_CENTER | DT_WORDBREAK, 0x0000_0000);
                    }
                    y += h + sc(4.0);
                }
                Visual::Badge(text) => {
                    // Clean bold uppercase banner framed by thermal-safe top/bottom
                    // accent rules (never a solid black filled box that starves 58 mm heads).
                    let (_, th) = measure_text(f_badge, text, inner_w - sc(8.0), DT_CENTER | DT_WORDBREAK);
                    let pad_v = sc(7.0);
                    let bh = th + pad_v * 2 + 4;
                    if draw {
                        unsafe { GdiFlush(); }
                        hline_dash(pixels, pad_x, paper as i32 - pad_x, y, 2, 4, 5, 0);
                        draw_text(f_badge, text, pad_x + sc(4.0), y + 2 + pad_v, inner_w - sc(8.0), th, DT_CENTER | DT_WORDBREAK, 0x0000_0000);
                        unsafe { GdiFlush(); }
                        hline_dash(pixels, pad_x, paper as i32 - pad_x, y + bh - 2, 2, 4, 5, 0);
                    }
                    y += bh + sc(8.0);
                }
                Visual::Pill(text) => {
                    let (tw, th) = measure_text(f_badge, text, inner_w - sc(24.0), DT_CENTER | DT_WORDBREAK);
                    let bw = (tw + sc(30.0)).min(inner_w);
                    let bh = th + sc(14.0);
                    let bx = (paper as i32 - bw) / 2;
                    if draw {
                        draw_text(f_badge, text, bx + sc(6.0), y + (bh - th) / 2, bw - sc(12.0), th, DT_CENTER | DT_WORDBREAK, 0x0000_0000);
                        unsafe { GdiFlush(); }
                        dashed_frame(pixels, bx, y, bx + bw, y + bh, 2, 4, 5);
                    }
                    y += bh + sc(8.0);
                }
                Visual::Total { label, amount } => {
                    let (_, lh) = measure_text(f_tot_lbl, label, inner_w / 2, DT_LEFT | DT_SINGLELINE);
                    let (aw, ah) = measure_text(f_tot_amt, amount, inner_w, DT_RIGHT | DT_SINGLELINE);
                    let th = lh.max(ah);
                    let bh = th + sc(20.0);
                    y += sc(4.0);
                    if draw {
                        let ip = sc(12.0);
                        draw_text(f_tot_lbl, label, pad_x + ip, y + (bh - lh) / 2, inner_w - aw - ip * 2, lh, DT_LEFT | DT_SINGLELINE, 0x0000_0000);
                        draw_text(f_tot_amt, amount, pad_x + ip, y + (bh - ah) / 2, inner_w - ip * 2, ah, DT_RIGHT | DT_SINGLELINE, 0x0000_0000);
                        unsafe { GdiFlush(); }
                        dashed_frame(pixels, pad_x, y, paper as i32 - pad_x, y + bh, 3, 4, 5);
                    }
                    y += bh + sc(8.0);
                }
                Visual::TableHead(text) => {
                    let row_h = cw * 2 + sc(10.0);
                    if draw {
                        let w = wide_raw(text);
                        let dxs = vec![cw; w.len()];
                        unsafe {
                            SelectObject(hdc, f_table_b);
                            SetTextColor(hdc, 0x0000_0000);
                            ExtTextOutW(hdc, table_x0, y, 0, null(), w.as_ptr(), w.len() as u32, dxs.as_ptr());
                            GdiFlush();
                        }
                        hline_dash(pixels, pad_x, paper as i32 - pad_x, y + row_h - 3, 2, 3, 5, 0);
                    }
                    y += row_h + sc(4.0);
                }
                Visual::TableRow { text, bold } => {
                    let row_h = cw * 2 + sc(4.0);
                    if draw {
                        let w = wide_raw(text);
                        if !w.is_empty() {
                            let dxs = vec![cw; w.len()];
                            unsafe {
                                SelectObject(hdc, if *bold { f_table_b } else { f_table });
                                SetTextColor(hdc, 0x0000_0000);
                                ExtTextOutW(hdc, table_x0, y, 0, null(), w.as_ptr(), w.len() as u32, dxs.as_ptr());
                            }
                        }
                    }
                    y += row_h;
                }
                Visual::Pair { left, right, bold } => {
                    let f = if *bold { f_bold } else { f_body };
                    let (rw, rh) = measure_text(f_semi, right, inner_w, DT_RIGHT | DT_SINGLELINE);
                    let lw = (inner_w - rw - sc(10.0)).max(inner_w / 3);
                    let (_, lh) = measure_text(f, left, lw, DT_LEFT | DT_WORDBREAK);
                    let h = lh.max(rh);
                    if draw {
                        draw_text(f, left, pad_x, y, lw, lh, DT_LEFT | DT_WORDBREAK, 0x0000_0000);
                        draw_text(if *bold { f_bold } else { f_semi }, right, pad_x, y, inner_w, rh, DT_RIGHT | DT_SINGLELINE, 0x0000_0000);
                    }
                    y += h + sc(4.0);
                }
                Visual::Center { text, bold } => {
                    let f = if *bold { f_bold } else { f_body };
                    let (_, h) = measure_text(f, text, inner_w, DT_CENTER | DT_WORDBREAK);
                    if draw {
                        draw_text(f, text, pad_x, y, inner_w, h, DT_CENTER | DT_WORDBREAK, 0x0000_0000);
                    }
                    y += h + sc(3.0);
                }
                Visual::Text { text, bold } => {
                    let f = if *bold { f_bold } else { f_body };
                    let (_, h) = measure_text(f, text, inner_w, DT_LEFT | DT_WORDBREAK);
                    if draw {
                        draw_text(f, text, pad_x, y, inner_w, h, DT_LEFT | DT_WORDBREAK, 0x0000_0000);
                    }
                    y += h + sc(3.0);
                }
            }
        }
        y + sc(6.0)
    };

    let mut empty = [];
    let total_h = run(false, &mut empty).clamp(1, 16_384) as usize;

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: paper as i32,
            biHeight: -(total_h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [windows_sys::Win32::Graphics::Gdi::RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
    };

    let mut bits_ptr: *mut c_void = null_mut();
    let hbmp: HBITMAP = unsafe { CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits_ptr, null_mut(), 0) };
    let out = if !hbmp.is_null() && !bits_ptr.is_null() {
        let pixels = unsafe { std::slice::from_raw_parts_mut(bits_ptr as *mut u32, paper * total_h) };
        pixels.fill(0x00FF_FFFF);
        unsafe {
            let old_bmp = SelectObject(hdc, hbmp);
            SetBkMode(hdc, TRANSPARENT as i32);
            run(true, pixels);
            GdiFlush();
            SelectObject(hdc, old_bmp);
        }
        let out_row = paper.div_ceil(8);
        let mut mono = vec![0u8; out_row * total_h];
        for py in 0..total_h {
            for px in 0..paper {
                let c = pixels[py * paper + px];
                let r = (c >> 16) & 0xFF;
                let g = (c >> 8) & 0xFF;
                let b = c & 0xFF;
                let lum = (r * 30 + g * 59 + b * 11) / 100;
                if lum < 180 {
                    mono[py * out_row + px / 8] |= 0x80 >> (px % 8);
                }
            }
        }
        Some((mono, out_row, total_h))
    } else {
        None
    };

    unsafe {
        if !hbmp.is_null() {
            DeleteObject(hbmp);
        }
        for f in [f_big, f_tall, f_badge, f_body, f_semi, f_bold, f_tot_lbl, f_tot_amt, f_table, f_table_b] {
            DeleteObject(f);
        }
        DeleteDC(hdc);
    }
    out
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
            let s = Settings { codepage: cp.into(), ticket_style: "classic".into(), ..Settings::default() };
            let lines = sample_lines(2100);
            let totals = compute(&lines);
            let pay = Payment { method: "cash".into(), tendered_cents: 3000, change_cents: 825, machine: None };
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
                vat: crate::tickets::VatView::Full,
                note: None,
                copy: false,
                voided: false,
                customer: None,
                training: false,
                qr: None,
            };
            let preview = layout::ticket(&v, &s, true);
            let lg = logo();
            let bytes = document(&preview, Some(&lg), &Opts::new(&s, true));
            let (printed, cmds) = decode(&bytes, CodePage::parse(cp));
            let expected: Vec<String> = preview
                .iter()
                .filter_map(|l| match l {
                    Line::Text { text, .. } => Some(text.trim_end().to_string()),
                    Line::Logo | Line::Qr { .. } => None,
                })
                .collect();
            assert_eq!(printed, expected, "{cp}");
            assert_eq!(cmds[0], "ESC @");
            assert_eq!(cmds[1], "FS .");
            assert!(cmds.contains(&"ESC p".to_string()));
            assert!(cmds.contains(&"RASTER 72x24".to_string()) && cmds.contains(&"RASTER 72x10".to_string()));
            assert_eq!(cmds.last().unwrap(), "GS V");
        }
    }

    #[cfg(windows)]
    #[test]
    fn modern_ticket_renders_smooth_raster_bands() {
        for cols in [32u32, 48] {
            let s = Settings { columns: cols, ticket_style: "modern".into(), ..Settings::default() };
            let lines = sample_lines(2100);
            let totals = compute(&lines);
            let pay = Payment { method: "card".into(), tendered_cents: totals.total_cents, change_cents: 0, machine: None };
            let shop = ShopInfo { name: "Mi Tienda Móvil".into(), nif: "B12345674".into(), email: "hola@ejemplo.es".into(), ..Default::default() };
            let v = TicketView {
                shop: &shop,
                kind: "sale",
                number: "T2026-000001",
                issued_at: "2026-10-16T17:00:00",
                user_name: "Ana",
                lines: &lines,
                totals: &totals,
                payment: Some(&pay),
                refund_of: None,
                reason: None,
                vat: crate::tickets::VatView::Full,
                note: None,
                copy: true,
                voided: false,
                customer: None,
                training: false,
                qr: None,
            };
            let preview = layout::ticket(&v, &s, true);
            let lg = logo();
            let bytes = document(&preview, Some(&lg), &Opts::new(&s, false));
            let (_, cmds) = decode(&bytes, CodePage::Cp858);
            let row_bytes = (cols as usize * 12).min(576) / 8;
            assert!(cmds.iter().any(|c| c.starts_with(&format!("RASTER {row_bytes}x"))), "{cols}: {cmds:?}");
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
    fn the_qr_prints_as_the_preview_draws_it() {
        let rows = vec!["101".to_string(), "010".to_string(), "111".to_string()];
        let mut b = Vec::new();
        qr(&mut b, &rows, 4, 48);
        // 3 modules and 8 of white margin, 4 dots each: 44 dot rows of 72 bytes, in 24-row bands (24 + 20)
        assert_eq!(&b[..8], &[GS, b'v', b'0', 0, 72, 0, 24, 0]);
        assert_eq!(&b[8 + 24 * 72..8 + 24 * 72 + 8], &[GS, b'v', b'0', 0, 72, 0, 20, 0]);
        let row = |y: usize| {
            if y < 24 {
                &b[8 + y * 72..8 + (y + 1) * 72]
            } else {
                let off = 8 + 24 * 72 + 8 + (y - 24) * 72;
                &b[off..off + 72]
            }
        };
        let dots = |y: usize| -> Vec<usize> { (0..576).filter(|x| row(y)[x / 8] & (0x80 >> (x % 8)) != 0).collect() };
        // the white margin above, then the first module row: dots 282..286 and 290..294
        assert!(dots(15).is_empty());
        assert_eq!(dots(16), (282..286).chain(290..294).collect::<Vec<_>>());
        assert_eq!(dots(20), (286..290).collect::<Vec<_>>());
        assert_eq!(dots(27).len(), 12);
        assert!(dots(28).is_empty());
        // too big for the paper: left out rather than cut
        let mut none = Vec::new();
        qr(&mut none, &rows, 40, 32);
        assert!(none.is_empty());
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

    #[cfg(windows)]
    #[test]
    fn modern_copy_and_original_use_clean_raster_header_and_safe_thermal_density() {
        let lines = sample_lines(2100);
        let totals = compute(&lines);
        let pay = Payment { method: "cash".into(), tendered_cents: 3000, change_cents: 825, machine: None };
        let shop = ShopInfo {
            name: "Mi Tienda Móvil".into(),
            address: "Calle Mayor, 12\n17600 Figueres".into(),
            phone: "972 000 000".into(),
            nif: "B12345674".into(),
            footer: "¡Gracias por su visita!".into(),
            ..Default::default()
        };
        for cols in [32u32, 48] {
            let s = Settings { columns: cols, ticket_style: "modern".into(), ..Settings::default() };
            let paper = (cols as usize * 12).min(576);
            let row_bytes = paper / 8;
            for copy in [false, true] {
                let v = TicketView {
                    shop: &shop,
                    kind: "sale",
                    number: "T2026-000042",
                    issued_at: "2026-10-16T18:15:00",
                    user_name: "Ana",
                    lines: &lines,
                    totals: &totals,
                    payment: Some(&pay),
                    refund_of: None,
                    reason: None,
                    vat: crate::tickets::VatView::Full,
                    note: None,
                    copy,
                    voided: false,
                    customer: None,
                    training: false,
                    qr: None,
                };
                let preview = layout::ticket(&v, &s, false);
                let raw = document(&preview, None, &Opts::new(&s, !copy));
                let (printed, cmds) = decode(&raw, CodePage::Cp858);
                assert!(printed.is_empty(), "modern raster should emit no raw text lines");
                assert_eq!(cmds[0], "ESC @");
                // Never emit text-mode setup (FS ., ESC R, ESC t, ESC M, ESC 2) before GS v 0
                assert!(!cmds.iter().any(|c| matches!(c.as_str(), "FS ." | "ESC R" | "ESC t" | "ESC M" | "ESC 2")), "{cmds:?}");
                assert!(cmds.contains(&"ESC J".to_string()));
                assert_eq!(cmds.contains(&"ESC p".to_string()), !copy);
                assert_eq!(cmds.last().unwrap(), "GS V");
                // Every raster band height must stay <= MAX_BAND (24) so POS-58 buffers never overflow
                let raster_cmds: Vec<&str> = cmds.iter().map(String::as_str).filter(|c| c.starts_with("RASTER ")).collect();
                assert!(!raster_cmds.is_empty());
                for rc in raster_cmds {
                    let dim = rc.strip_prefix("RASTER ").unwrap();
                    let (w, h) = dim.split_once('x').unwrap();
                    assert_eq!(w.parse::<usize>().unwrap(), row_bytes);
                    assert!(h.parse::<usize>().unwrap() <= MAX_BAND, "{rc}");
                }
                // Thermal safety: no horizontal pixel row may exceed 50% black dot density
                let (bits, row, h) = render_modern(&preview, None, cols as usize).unwrap();
                let max_dots = (0..h)
                    .map(|y| bits[y * row..(y + 1) * row].iter().map(|b| b.count_ones()).sum::<u32>())
                    .max()
                    .unwrap_or(0);
                assert!(max_dots as usize <= paper / 2, "cols={cols} copy={copy} max_dots={max_dots}/{paper}");
                if let Some(dir) = std::env::var_os("TIQUET_DUMP_MODERN") {
                    let mut pbm = format!("P4\n{} {}\n", row * 8, h).into_bytes();
                    pbm.extend_from_slice(&bits);
                    let _ = std::fs::write(std::path::Path::new(&dir).join(format!("final_{cols}_copy_{copy}.pbm")), pbm);
                }
            }
        }
    }
}
