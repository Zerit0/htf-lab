//! Codigo especifico de cada venue. Convierte bytes crudos en tipos comunes.
//! La state machine no conoce el formato de ningun exchange.

pub mod hyperliquid;

use crate::book::Level;
use crate::fixed::Fixed;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BookSnapshot {
    pub coin: String,
    pub exch_ms: u64,
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Trade {
    pub coin: String,
    /// true si el agresor compro.
    pub taker_is_buy: bool,
    pub px: Fixed,
    pub sz: Fixed,
    pub exch_ms: u64,
    pub tid: u64,
}

/// Resultado de decodificar un mensaje entrante.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Decoded {
    Book(BookSnapshot),
    Trades(Vec<Trade>),
    Pong,
    SubAck,
    /// Canal valido que aun no nos interesa.
    Other(String),
    /// No se pudo interpretar. Se cuenta y se informa, nunca se ignora en silencio.
    Malformed(String),
}
