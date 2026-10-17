// All amounts are integer cents, VAT rates are basis points (21% = 2100).

pub fn div_round(n: i64, d: i64) -> i64 {
    if n >= 0 {
        (2 * n + d) / (2 * d)
    } else {
        -((-2 * n + d) / (2 * d))
    }
}

/// Splits a VAT-inclusive amount into (base, vat).
pub fn split_vat(gross: i64, vat_bp: i64) -> (i64, i64) {
    let base = div_round(gross * 10_000, 10_000 + vat_bp);
    (base, gross - base)
}

pub fn fmt(cents: i64) -> String {
    let abs = cents.unsigned_abs();
    let euros = (abs / 100).to_string();
    let mut out = String::new();
    if cents < 0 {
        out.push('-');
    }
    for (i, c) in euros.chars().enumerate() {
        if i > 0 && (euros.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    out.push_str(&format!(",{:02}", abs % 100));
    out
}

pub fn fmt_rate(bp: i64) -> String {
    let frac = bp % 100;
    if frac == 0 {
        format!("{}%", bp / 100)
    } else {
        let f = format!("{frac:02}");
        format!("{},{}%", bp / 100, f.trim_end_matches('0'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vat_split_rounds_half_away_from_zero() {
        assert_eq!(split_vat(2610, 2100), (2157, 453));
        assert_eq!(split_vat(10, 2100), (8, 2));
        assert_eq!(split_vat(1210, 2100), (1000, 210));
        assert_eq!(split_vat(-2610, 2100), (-2157, -453));
        assert_eq!(split_vat(0, 2100), (0, 0));
        assert_eq!(split_vat(1000, 0), (1000, 0));
        assert_eq!(split_vat(105, 500), (100, 5));
    }

    #[test]
    fn spanish_number_format() {
        assert_eq!(fmt(0), "0,00");
        assert_eq!(fmt(5), "0,05");
        assert_eq!(fmt(99999), "999,99");
        assert_eq!(fmt(123456), "1.234,56");
        assert_eq!(fmt(-250), "-2,50");
        assert_eq!(fmt(100_000_000), "1.000.000,00");
        assert_eq!(fmt_rate(2100), "21%");
        assert_eq!(fmt_rate(550), "5,5%");
        assert_eq!(fmt_rate(1025), "10,25%");
    }
}
