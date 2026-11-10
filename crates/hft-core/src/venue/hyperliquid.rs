//! Hyperliquid: feed publico por WebSocket.
//!
//! Referencia: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket
//!
//! OJO: estos formatos se han escrito a partir de la documentacion publica y
//! NO se han validado contra el exchange desde el entorno donde se genero el
//! scaffold (sin red). El primer `hft-recorder` real es la validacion: si
//! `hft-replay stats` muestra `malformed > 0`, hay que ajustar este fichero
//! mirando los mensajes con `hft-replay dump`.
//!
//! ```text
//! Suscripcion  {"method":"subscribe","subscription":{"type":"l2Book","coin":"BTC"}}
//! l2Book       {"channel":"l2Book","data":{"coin":"BTC","time":<ms>,
//!                "levels":[[{"px":"..","sz":"..","n":N},..],[..]]}}
//! trades       {"channel":"trades","data":[{"coin","side":"B"|"A","px","sz","time","tid",..}]}
//! Heartbeat    {"method":"ping"}  ->  {"channel":"pong"}
//! ```
//!
//! En l2Book, `levels[0]` son bids y `levels[1]` asks, y cada mensaje es un
//! SNAPSHOT completo de los niveles visibles, no un delta. El servidor cierra
//! las conexiones que pasan ~60 s sin trafico saliente, de ahi el heartbeat.

use super::{BookSnapshot, Decoded, Trade};
use crate::book::Level;
use crate::fixed::Fixed;
use crate::json::{self, Value};

pub const WS_URL: &str = "wss://api.hyperliquid.xyz/ws";
pub const WS_URL_TESTNET: &str = "wss://api.hyperliquid-testnet.xyz/ws";

pub fn subscribe_l2book(coin: &str) -> String {
    format!(r#"{{"method":"subscribe","subscription":{{"type":"l2Book","coin":"{}"}}}}"#, json::escape(coin))
}

pub fn subscribe_trades(coin: &str) -> String {
    format!(r#"{{"method":"subscribe","subscription":{{"type":"trades","coin":"{}"}}}}"#, json::escape(coin))
}

/// Mensajes de suscripcion para una lista de monedas: libro + trades.
pub fn subscriptions(coins: &[String]) -> Vec<String> {
    let mut v = Vec::with_capacity(coins.len() * 2);
    for c in coins {
        v.push(subscribe_l2book(c));
        v.push(subscribe_trades(c));
    }
    v
}

pub fn ping() -> String {
    r#"{"method":"ping"}"#.to_string()
}

pub fn decode(raw: &[u8]) -> Decoded {
    let v = match json::parse(raw) {
        Ok(v) => v,
        Err(e) => return Decoded::Malformed(e.to_string()),
    };
    let channel = match v.get("channel").and_then(Value::as_str) {
        Some(c) => c,
        None => return Decoded::Malformed("sin campo channel".into()),
    };
    match channel {
        "l2Book" => match v.get("data").and_then(decode_book) {
            Some(b) => Decoded::Book(b),
            None => Decoded::Malformed("l2Book con forma inesperada".into()),
        },
        "trades" => match v.get("data").and_then(decode_trades) {
            Some(t) => Decoded::Trades(t),
            None => Decoded::Malformed("trades con forma inesperada".into()),
        },
        "pong" => Decoded::Pong,
        "subscriptionResponse" => Decoded::SubAck,
        other => Decoded::Other(other.to_string()),
    }
}

fn decode_side(v: &Value) -> Option<Vec<Level>> {
    let arr = v.as_arr()?;
    let mut out = Vec::with_capacity(arr.len());
    for l in arr {
        out.push(Level {
            px: Fixed::parse(l.get("px")?.as_num_str()?)?,
            sz: Fixed::parse(l.get("sz")?.as_num_str()?)?,
            n: l.get("n").and_then(Value::as_u64).unwrap_or(0) as u32,
        });
    }
    Some(out)
}

fn decode_book(d: &Value) -> Option<BookSnapshot> {
    let levels = d.get("levels")?.as_arr()?;
    if levels.len() != 2 {
        return None;
    }
    Some(BookSnapshot {
        coin: d.get("coin")?.as_str()?.to_string(),
        exch_ms: d.get("time")?.as_u64()?,
        bids: decode_side(&levels[0])?,
        asks: decode_side(&levels[1])?,
    })
}

fn decode_trades(d: &Value) -> Option<Vec<Trade>> {
    let arr = d.as_arr()?;
    let mut out = Vec::with_capacity(arr.len());
    for t in arr {
        let side = t.get("side")?.as_str()?;
        out.push(Trade {
            coin: t.get("coin")?.as_str()?.to_string(),
            taker_is_buy: match side {
                "B" => true,
                "A" => false,
                _ => return None,
            },
            px: Fixed::parse(t.get("px")?.as_num_str()?)?,
            sz: Fixed::parse(t.get("sz")?.as_num_str()?)?,
            exch_ms: t.get("time")?.as_u64()?,
            tid: t.get("tid").and_then(Value::as_u64).unwrap_or(0),
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_messages_are_valid_json() {
        for m in subscriptions(&["BTC".into(), "kPEPE".into()]) {
            let v = json::parse(m.as_bytes()).unwrap();
            assert_eq!(v.get("method").unwrap().as_str(), Some("subscribe"));
        }
        assert!(json::parse(ping().as_bytes()).is_ok());
    }

    #[test]
    fn decodes_book() {
        let raw = br#"{"channel":"l2Book","data":{"coin":"ETH","time":1700000000500,"levels":[[{"px":"3000.1","sz":"2.5","n":4},{"px":"3000.0","sz":"1","n":1}],[{"px":"3000.3","sz":"0.7","n":2}]]}}"#;
        match decode(raw) {
            Decoded::Book(b) => {
                assert_eq!(b.coin, "ETH");
                assert_eq!(b.exch_ms, 1_700_000_000_500);
                assert_eq!(b.bids.len(), 2);
                assert_eq!(b.bids[0].px.to_string(), "3000.1");
                assert_eq!(b.bids[0].n, 4);
                assert_eq!(b.asks[0].sz.to_string(), "0.7");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn decodes_trades() {
        let raw = br#"{"channel":"trades","data":[{"coin":"ETH","side":"B","px":"3000.3","sz":"0.2","time":1700000000600,"hash":"0xabc","tid":42},{"coin":"ETH","side":"A","px":"3000.1","sz":"1","time":1700000000601,"tid":43}]}"#;
        match decode(raw) {
            Decoded::Trades(t) => {
                assert_eq!(t.len(), 2);
                assert!(t[0].taker_is_buy);
                assert!(!t[1].taker_is_buy);
                assert_eq!(t[0].tid, 42);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn control_and_unknown() {
        assert_eq!(decode(br#"{"channel":"pong"}"#), Decoded::Pong);
        assert_eq!(decode(br#"{"channel":"subscriptionResponse","data":{"method":"subscribe"}}"#), Decoded::SubAck);
        assert_eq!(decode(br#"{"channel":"bbo","data":{}}"#), Decoded::Other("bbo".into()));
    }

    #[test]
    fn malformed_is_reported_not_swallowed() {
        for raw in [
            &b"not json"[..],
            br#"{"data":1}"#,
            br#"{"channel":"l2Book","data":{"coin":"X","time":1,"levels":[[]]}}"#,
            br#"{"channel":"l2Book","data":{"coin":"X","time":1,"levels":[[{"px":"a","sz":"1"}],[]]}}"#,
            br#"{"channel":"trades","data":[{"coin":"X","side":"?","px":"1","sz":"1","time":1}]}"#,
        ] {
            assert!(matches!(decode(raw), Decoded::Malformed(_)), "{:?}", std::str::from_utf8(raw));
        }
    }
}
