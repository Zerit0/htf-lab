//! Aritmetica de punto fijo para precios y tamanos.
//!
//! Los exchanges envian precios como strings decimales ("67123.5"). Usar f64
//! introduce redondeos que dependen de la plataforma y del orden de las
//! operaciones; con enteros escalados el replay es bit a bit identico en el
//! Mac (aarch64-darwin) y en el VPS (aarch64-linux).

use std::fmt;

/// 8 decimales: suficiente para precios y tamanos de perps cripto.
pub const SCALE: i64 = 100_000_000;
const DECIMALS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Fixed(pub i64);

impl Fixed {
    pub const ZERO: Fixed = Fixed(0);

    /// Parsea un decimal plano ("123", "-0.5", "67123.50"). Sin exponentes.
    /// Los decimales mas alla del octavo se truncan.
    pub fn parse(s: &str) -> Option<Fixed> {
        let b = s.as_bytes();
        if b.is_empty() {
            return None;
        }
        let (neg, mut i) = match b[0] {
            b'-' => (true, 1),
            b'+' => (false, 1),
            _ => (false, 0),
        };
        let mut int: i64 = 0;
        let mut seen_digit = false;
        while i < b.len() && b[i].is_ascii_digit() {
            int = int.checked_mul(10)?.checked_add((b[i] - b'0') as i64)?;
            seen_digit = true;
            i += 1;
        }
        let mut frac: i64 = 0;
        let mut frac_digits = 0usize;
        if i < b.len() && b[i] == b'.' {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                if frac_digits < DECIMALS {
                    frac = frac * 10 + (b[i] - b'0') as i64;
                    frac_digits += 1;
                }
                seen_digit = true;
                i += 1;
            }
        }
        if i != b.len() || !seen_digit {
            return None;
        }
        for _ in frac_digits..DECIMALS {
            frac *= 10;
        }
        let v = int.checked_mul(SCALE)?.checked_add(frac)?;
        Some(Fixed(if neg { -v } else { v }))
    }

    pub fn raw(self) -> i64 {
        self.0
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Diferencia relativa `(self - other) / reference` en centesimas de
    /// punto basico (1 bp = 100). Entero: determinista.
    pub fn diff_bps_x100(self, other: Fixed, reference: Fixed) -> Option<i64> {
        if reference.0 == 0 {
            return None;
        }
        let num = (self.0 as i128 - other.0 as i128) * 1_000_000;
        Some((num / reference.0 as i128) as i64)
    }
}

impl fmt::Display for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let neg = self.0 < 0;
        let abs = self.0.unsigned_abs();
        let int = abs / SCALE as u64;
        let frac = abs % SCALE as u64;
        if neg {
            write!(f, "-")?;
        }
        if frac == 0 {
            return write!(f, "{int}");
        }
        let mut s = format!("{frac:08}");
        while s.ends_with('0') {
            s.pop();
        }
        write!(f, "{int}.{s}")
    }
}

/// Formatea centesimas de bp como "12.34".
pub fn fmt_bps_x100(v: i64) -> String {
    let neg = v < 0;
    let a = v.unsigned_abs();
    format!("{}{}.{:02}", if neg { "-" } else { "" }, a / 100, a % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display_roundtrip() {
        for s in ["0", "1", "67123.5", "0.00000001", "-12.25", "100000"] {
            assert_eq!(Fixed::parse(s).unwrap().to_string(), s, "{s}");
        }
    }

    #[test]
    fn parse_normalises() {
        assert_eq!(Fixed::parse("1.50").unwrap(), Fixed(150_000_000));
        assert_eq!(Fixed::parse(".5").unwrap(), Fixed(50_000_000));
        assert_eq!(Fixed::parse("5.").unwrap(), Fixed(500_000_000));
        // mas de 8 decimales: se trunca, no se redondea
        assert_eq!(Fixed::parse("0.123456789").unwrap(), Fixed(12_345_678));
    }

    #[test]
    fn parse_rejects_garbage() {
        for s in ["", "-", ".", "1e5", "1,5", "abc", "1.2.3", " 1"] {
            assert!(Fixed::parse(s).is_none(), "{s:?}");
        }
        assert!(Fixed::parse("99999999999999999999").is_none());
    }

    #[test]
    fn bps() {
        let bid = Fixed::parse("99.95").unwrap();
        let ask = Fixed::parse("100.05").unwrap();
        let mid = Fixed::parse("100").unwrap();
        // 0.10 / 100 = 10 bps
        assert_eq!(ask.diff_bps_x100(bid, mid), Some(1000));
        assert_eq!(fmt_bps_x100(1000), "10.00");
        assert_eq!(fmt_bps_x100(-5), "-0.05");
        assert_eq!(ask.diff_bps_x100(bid, Fixed::ZERO), None);
    }
}
