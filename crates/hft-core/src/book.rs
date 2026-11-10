//! Libro de ordenes L2 agregado por nivel de precio.

use crate::fixed::Fixed;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Level {
    pub px: Fixed,
    pub sz: Fixed,
    /// Numero de ordenes en el nivel (si el venue lo informa).
    pub n: u32,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Book {
    /// De mejor a peor: precio descendente.
    pub bids: Vec<Level>,
    /// De mejor a peor: precio ascendente.
    pub asks: Vec<Level>,
    /// Marca de tiempo del exchange (ms Unix).
    pub exch_ms: u64,
    pub updates: u64,
}

impl Book {
    pub fn best_bid(&self) -> Option<Level> {
        self.bids.first().copied()
    }

    pub fn best_ask(&self) -> Option<Level> {
        self.asks.first().copied()
    }

    /// Precio medio. Division entera sobre el valor escalado.
    pub fn mid(&self) -> Option<Fixed> {
        let (b, a) = (self.best_bid()?, self.best_ask()?);
        Some(Fixed(((b.px.0 as i128 + a.px.0 as i128) / 2) as i64))
    }

    /// Spread en centesimas de punto basico.
    pub fn spread_bps_x100(&self) -> Option<i64> {
        let (b, a) = (self.best_bid()?, self.best_ask()?);
        a.px.diff_bps_x100(b.px, self.mid()?)
    }

    /// Un libro cruzado (bid >= ask) indica datos corruptos o desfasados.
    pub fn is_crossed(&self) -> bool {
        match (self.best_bid(), self.best_ask()) {
            (Some(b), Some(a)) => b.px >= a.px,
            _ => false,
        }
    }

    /// Comprueba el orden de los niveles. La SM lo usa como sanidad.
    pub fn is_sorted(&self) -> bool {
        self.bids.windows(2).all(|w| w[0].px > w[1].px) && self.asks.windows(2).all(|w| w[0].px < w[1].px)
    }

    /// Precio medio de ejecucion de una orden taker de tamano `sz` y cuanto
    /// se pudo llenar. Es la pregunta base del estudio de F1: "a que precio
    /// puedo cubrir en Hyperliquid ahora mismo?".
    pub fn taker_fill(&self, is_buy: bool, sz: Fixed) -> Option<(Fixed, Fixed)> {
        if sz.0 <= 0 {
            return None;
        }
        let side = if is_buy { &self.asks } else { &self.bids };
        let mut left = sz.0 as i128;
        let mut notional: i128 = 0; // px * sz, escala SCALE^2
        for l in side {
            let take = left.min(l.sz.0 as i128);
            notional += take * l.px.0 as i128;
            left -= take;
            if left == 0 {
                break;
            }
        }
        let filled = sz.0 as i128 - left;
        if filled == 0 {
            return None;
        }
        Some((Fixed((notional / filled) as i64), Fixed(filled as i64)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lv(px: &str, sz: &str) -> Level {
        Level { px: Fixed::parse(px).unwrap(), sz: Fixed::parse(sz).unwrap(), n: 1 }
    }

    fn book() -> Book {
        Book {
            bids: vec![lv("99.9", "1"), lv("99.8", "2")],
            asks: vec![lv("100.1", "1"), lv("100.3", "3")],
            exch_ms: 1,
            updates: 1,
        }
    }

    #[test]
    fn top_of_book() {
        let b = book();
        assert_eq!(b.mid().unwrap().to_string(), "100");
        assert_eq!(b.spread_bps_x100(), Some(2000)); // 0.2/100 = 20 bps
        assert!(!b.is_crossed());
        assert!(b.is_sorted());
        assert_eq!(Book::default().mid(), None);
    }

    #[test]
    fn crossed_and_unsorted() {
        let mut b = book();
        b.bids[0] = lv("100.2", "1");
        assert!(b.is_crossed());
        let mut c = book();
        c.asks.swap(0, 1);
        assert!(!c.is_sorted());
    }

    #[test]
    fn taker_fill_walks_the_book() {
        let b = book();
        // compra 2: 1 @100.1 + 1 @100.3 => 100.2
        let (px, filled) = b.taker_fill(true, Fixed::parse("2").unwrap()).unwrap();
        assert_eq!((px.to_string(), filled.to_string()), ("100.2".into(), "2".into()));
        // venta 10: solo hay 3 => llenado parcial, media (99.9 + 2*99.8)/3
        let (px, filled) = b.taker_fill(false, Fixed::parse("10").unwrap()).unwrap();
        assert_eq!(filled.to_string(), "3");
        assert_eq!(px.to_string(), "99.83333333");
        assert!(b.taker_fill(true, Fixed::ZERO).is_none());
        assert!(Book::default().taker_fill(true, Fixed::parse("1").unwrap()).is_none());
    }
}
