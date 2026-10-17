// Spanish tax ids: DNI and NIE (and the K/L/M kinds) end in a check letter
// worked out from the digits; a CIF ends in a check digit or letter. An EU
// VAT number from another country is accepted as written (no checksum here).

const LETTERS: &[u8; 23] = b"TRWAGMYFPDXBNJZSQVHLCKE";

pub fn normalize(s: &str) -> String {
    let s: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    s.strip_prefix("ES").filter(|rest| rest.len() == 9).map(str::to_string).unwrap_or(s)
}

fn dni_letter(n: u32) -> u8 {
    LETTERS[(n % 23) as usize]
}

fn digits(s: &str) -> Option<u32> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

fn cif_ok(s: &[u8]) -> bool {
    let body = &s[1..8];
    if !body.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let mut sum = 0u32;
    for (i, b) in body.iter().enumerate() {
        let d = (b - b'0') as u32;
        if i % 2 == 0 {
            let x = d * 2;
            sum += x / 10 + x % 10;
        } else {
            sum += d;
        }
    }
    let check = (10 - sum % 10) % 10;
    let last = s[8];
    let letter = b"JABCDEFGHI"[check as usize];
    match s[0] {
        b'N' | b'P' | b'Q' | b'R' | b'S' | b'W' => last == letter,
        b'A' | b'B' | b'E' | b'H' => last == b'0' + check as u8,
        _ => last == letter || last == b'0' + check as u8,
    }
}

pub fn spanish(id: &str) -> bool {
    let s = normalize(id);
    let b = s.as_bytes();
    if b.len() != 9 {
        return false;
    }
    let last = b[8];
    match b[0] {
        b'0'..=b'9' => digits(&s[..8]).is_some_and(|n| dni_letter(n) == last),
        b'X' | b'Y' | b'Z' => digits(&s[1..8]).is_some_and(|n| dni_letter((b[0] - b'X') as u32 * 10_000_000 + n) == last),
        b'K' | b'L' | b'M' => digits(&s[1..8]).is_some_and(|n| dni_letter(n) == last),
        b'A'..=b'H' | b'J' | b'N' | b'P' | b'Q' | b'R' | b'S' | b'U' | b'V' | b'W' => cif_ok(b),
        _ => false,
    }
}

// a customer may be a company from elsewhere in the EU
pub fn customer(id: &str) -> bool {
    let s = normalize(id);
    if spanish(&s) {
        return true;
    }
    let b = s.as_bytes();
    b.len() >= 4 && b.len() <= 14 && b[..2].iter().all(u8::is_ascii_uppercase) && &s[..2] != "ES" && b[2..].iter().all(u8::is_ascii_alphanumeric)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_letters_and_digits() {
        assert!(spanish("Y1234567X"));
        assert!(spanish(" y-1234567-x "));
        assert!(!spanish("Y1234567J"));
        assert!(spanish("12345678Z"));
        assert!(!spanish("12345678A"));
        assert!(spanish("X1234567L"));
        assert!(spanish("B12345674"));
        assert!(!spanish("B12345678"));
        assert!(spanish("Q2826000H"));
        assert!(spanish("ESB12345674"));
        assert!(!spanish("123"));
        assert!(customer("FR40303265045"));
        assert!(customer("B12345674"));
        assert!(!customer("ES12"));
    }
}
