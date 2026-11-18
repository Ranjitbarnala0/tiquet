// Thermal printers take one byte per character from the active code page
// (ESC t n). Text is reduced to what the selected page can encode before it
// is measured, so the preview and the paper always agree on widths.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodePage {
    Cp858,
    Cp850,
    Cp1252,
    Ascii,
}

impl CodePage {
    pub const ALL: [CodePage; 4] = [CodePage::Cp858, CodePage::Cp1252, CodePage::Cp850, CodePage::Ascii];

    pub fn parse(s: &str) -> CodePage {
        match s {
            "cp850" => CodePage::Cp850,
            "cp1252" => CodePage::Cp1252,
            "ascii" => CodePage::Ascii,
            _ => CodePage::Cp858,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            CodePage::Cp858 => "cp858",
            CodePage::Cp850 => "cp850",
            CodePage::Cp1252 => "cp1252",
            CodePage::Ascii => "ascii",
        }
    }

    // table numbers as used by Epson and the common Chinese clones
    pub fn table(self) -> u8 {
        match self {
            CodePage::Cp858 => 19,
            CodePage::Cp850 => 2,
            CodePage::Cp1252 => 16,
            CodePage::Ascii => 0,
        }
    }
}

const CP850: [&str; 8] = [
    "ÇüéâäàåçêëèïîìÄÅ",
    "ÉæÆôöòûùÿÖÜø£Ø×ƒ",
    "áíóúñÑªº¿®¬½¼¡«»",
    "░▒▓│┤ÁÂÀ©╣║╗╝¢¥┐",
    "└┴┬├─┼ãÃ╚╔╩╦╠═╬¤",
    "ðÐÊËÈıÍÎÏ┘┌█▄¦Ì▀",
    "ÓßÔÒõÕµþÞÚÛÙýÝ¯´",
    "\u{ad}±‗¾¶§÷¸°¨·¹³²■\u{a0}",
];

const CP1252_HIGH: [char; 32] = [
    '€', '\0', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\0', 'Ž', '\0',
    '\0', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\0', 'ž', 'Ÿ',
];

pub fn byte(c: char, cp: CodePage) -> Option<u8> {
    if (' '..='~').contains(&c) {
        return Some(c as u8);
    }
    match cp {
        CodePage::Ascii => None,
        CodePage::Cp1252 => {
            let u = c as u32;
            if (0xA0..=0xFF).contains(&u) {
                return Some(u as u8);
            }
            CP1252_HIGH
                .iter()
                .position(|&x| x == c && x != '\0')
                .map(|i| 0x80 + i as u8)
        }
        CodePage::Cp850 | CodePage::Cp858 => {
            if cp == CodePage::Cp858 {
                if c == '€' {
                    return Some(0xD5);
                }
                if c == 'ı' {
                    return None;
                }
            }
            CP850
                .iter()
                .flat_map(|row| row.chars())
                .position(|x| x == c)
                .map(|i| 0x80 + i as u8)
        }
    }
}

fn translit(c: char) -> Option<&'static str> {
    Some(match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ª' => "a",
        'Á' | 'À' | 'Â' | 'Ä' | 'Ã' | 'Å' => "A",
        'é' | 'è' | 'ê' | 'ë' => "e",
        'É' | 'È' | 'Ê' | 'Ë' => "E",
        'í' | 'ì' | 'î' | 'ï' | 'ı' => "i",
        'Í' | 'Ì' | 'Î' | 'Ï' => "I",
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'º' | '°' => "o",
        'Ó' | 'Ò' | 'Ô' | 'Ö' | 'Õ' | 'Ø' => "O",
        'ú' | 'ù' | 'û' | 'ü' => "u",
        'Ú' | 'Ù' | 'Û' | 'Ü' => "U",
        'ñ' => "n",
        'Ñ' => "N",
        'ç' => "c",
        'Ç' => "C",
        'ý' | 'ÿ' => "y",
        'Ý' => "Y",
        'ß' => "ss",
        // names from Romania, Poland, Czechia, Turkey, Hungary: no code page
        // of a receipt printer has their letters, and a "?" in the middle of
        // a customer's name helps nobody
        'ă' | 'ā' | 'ą' => "a",
        'Ă' | 'Ā' | 'Ą' => "A",
        'ć' | 'č' => "c",
        'Ć' | 'Č' => "C",
        'ď' | 'đ' => "d",
        'Ď' | 'Đ' => "D",
        'ę' | 'ě' | 'ē' | 'ė' => "e",
        'Ę' | 'Ě' | 'Ē' | 'Ė' => "E",
        'ğ' => "g",
        'Ğ' => "G",
        'ī' | 'į' => "i",
        'İ' | 'Ī' | 'Į' => "I",
        'ł' | 'ľ' | 'ĺ' => "l",
        'Ł' | 'Ľ' | 'Ĺ' => "L",
        'ń' | 'ň' => "n",
        'Ń' | 'Ň' => "N",
        'ő' | 'ō' => "o",
        'Ő' | 'Ō' => "O",
        'ř' => "r",
        'Ř' => "R",
        'ś' | 'š' | 'ş' | 'ș' => "s",
        'Ś' | 'Š' | 'Ş' | 'Ș' => "S",
        'ť' | 'ţ' | 'ț' => "t",
        'Ť' | 'Ţ' | 'Ț' => "T",
        'ů' | 'ű' | 'ū' | 'ų' => "u",
        'Ů' | 'Ű' | 'Ū' | 'Ų' => "U",
        'ź' | 'ż' | 'ž' => "z",
        'Ź' | 'Ż' | 'Ž' => "Z",
        '€' => "EUR",
        '£' => "GBP",
        '¿' => "?",
        '¡' => "!",
        '‘' | '’' | '´' | '‚' => "'",
        '“' | '”' | '«' | '»' | '„' => "\"",
        '–' | '—' | '‐' | '‑' | '−' => "-",
        '…' => "...",
        '·' | '•' => ".",
        '×' => "x",
        '\u{a0}' | '\u{2007}' | '\u{202f}' => " ",
        '™' => "TM",
        '©' => "(c)",
        '®' => "(R)",
        _ => return None,
    })
}

pub fn printable(s: &str, cp: CodePage) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        // Marks nobody sees take no room on paper: the direction marks
        // WhatsApp wraps a copied phone number in, zero-width spaces, a soft
        // hyphen, an accent typed apart from its letter.
        if matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}' | '\u{ad}' | '\u{300}'..='\u{36f}') {
            continue;
        }
        if c.is_control() {
            out.push(' ');
        } else if byte(c, cp).is_some() {
            out.push(c);
        } else if let Some(t) = translit(c) {
            out.push_str(t);
        } else {
            out.push('?');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp850_table_is_complete() {
        assert_eq!(CP850.iter().map(|r| r.chars().count()).sum::<usize>(), 128);
    }

    #[test]
    fn spanish_letters_map_to_known_bytes() {
        assert_eq!(byte('ñ', CodePage::Cp858), Some(0xA4));
        assert_eq!(byte('Ñ', CodePage::Cp858), Some(0xA5));
        assert_eq!(byte('á', CodePage::Cp858), Some(0xA0));
        assert_eq!(byte('Ú', CodePage::Cp858), Some(0xE9));
        assert_eq!(byte('¿', CodePage::Cp858), Some(0xA8));
        assert_eq!(byte('€', CodePage::Cp858), Some(0xD5));
        assert_eq!(byte('€', CodePage::Cp850), None);
        assert_eq!(byte('€', CodePage::Cp1252), Some(0x80));
        assert_eq!(byte('ñ', CodePage::Cp1252), Some(0xF1));
        assert_eq!(byte('ç', CodePage::Cp850), Some(0x87));
        assert_eq!(byte('ñ', CodePage::Ascii), None);
    }

    #[test]
    fn printable_falls_back_per_page() {
        assert_eq!(printable("Café 5 €", CodePage::Ascii), "Cafe 5 EUR");
        assert_eq!(printable("Café 5 €", CodePage::Cp850), "Café 5 EUR");
        assert_eq!(printable("Café 5 €", CodePage::Cp858), "Café 5 €");
        assert_eq!(printable("a\tb\nc", CodePage::Cp858), "a b c");
        assert_eq!(printable("móvil 📱", CodePage::Cp858), "móvil ?");
        // a phone number copied from WhatsApp, a Romanian name
        assert_eq!(printable("\u{202a}+34 612 34 56 78\u{202c}", CodePage::Cp858), "+34 612 34 56 78");
        assert_eq!(printable("Ionuț Dănilă", CodePage::Cp858), "Ionut Danila");
    }
}
