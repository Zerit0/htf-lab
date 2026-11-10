//! La state machine. Pura y determinista.
//!
//! Contrato:
//!   - Entrada: un `Event`. Nada mas. Ni reloj, ni red, ni disco, ni azar.
//!   - Salida: una `Action` y, opcionalmente, lineas de texto en `out`.
//!   - El "ahora" es `ev.ts_ns`. Jamas `SystemTime::now()`.
//!   - Colecciones ordenadas (BTreeMap), nunca HashMap: el orden de
//!     iteracion de un HashMap cambia entre ejecuciones.
//!
//! Si se respeta, `hft-replay verify` da el mismo digest en vivo y en replay,
//! y cualquier bug se puede reproducir offline con el log de la sesion.
//!
//! F0: sigue libros y estado de conexion. No opera.
//! F2 (pasos 4-5): `update` empezara a devolver Place/Cancel.
//! F3 (paso 11): se partira en tres maquinas: estado, estrategia, reconciliador.

use crate::book::Book;
use crate::clock::NS_PER_MS;
use crate::event::{Action, Event, Kind, Venue};
use crate::fixed::fmt_bps_x100;
use crate::venue::{hyperliquid, Decoded};
use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnState {
    Connecting,
    Up,
    Down,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub events: u64,
    pub book_updates: u64,
    pub bbo_changes: u64,
    pub trades: u64,
    pub malformed: u64,
    pub crossed: u64,
    pub unsorted: u64,
    pub timers: u64,
    pub conn_opens: u64,
    pub conn_closes: u64,
    pub other_msgs: u64,
    /// Retardo local - exchange del ultimo libro (ms). Mide la latencia del
    /// feed: unos pocos ms en Tokio, del orden de cientos desde Madrid.
    pub last_lag_ms: i64,
    pub max_lag_ms: i64,
}

#[derive(Default)]
pub struct StateMachine {
    books: BTreeMap<(Venue, String), Book>,
    conns: BTreeMap<(Venue, u16), ConnState>,
    now_ns: u64,
    pub stats: Stats,
}

impl StateMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn book(&self, venue: Venue, coin: &str) -> Option<&Book> {
        self.books.get(&(venue, coin.to_string()))
    }

    pub fn conn_state(&self, venue: Venue, conn: u16) -> Option<ConnState> {
        self.conns.get(&(venue, conn)).copied()
    }

    /// Tiempo de la SM: el del ultimo evento procesado.
    pub fn now_ns(&self) -> u64 {
        self.now_ns
    }

    pub fn update(&mut self, ev: &Event, out: &mut String) -> Action {
        self.now_ns = ev.ts_ns;
        self.stats.events += 1;
        let v = ev.venue.tag();
        match ev.kind {
            Kind::ConnOpen => {
                self.stats.conn_opens += 1;
                self.conns.insert((ev.venue, ev.conn), ConnState::Connecting);
                let _ = writeln!(out, "{} {} CONN {v} c{} OPEN {}", ev.seq, ev.ts_ns, ev.conn, ev.payload_str());
            }
            Kind::HttpResp => {
                // El upgrade a WebSocket ha terminado: la conexion esta viva.
                self.conns.insert((ev.venue, ev.conn), ConnState::Up);
            }
            Kind::ConnClose => {
                self.stats.conn_closes += 1;
                self.conns.insert((ev.venue, ev.conn), ConnState::Down);
                // Un libro sin conexion es un libro caducado. Se descarta para
                // que ninguna estrategia cotice contra datos viejos.
                self.books.retain(|(venue, _), _| *venue != ev.venue);
                let _ = writeln!(out, "{} {} CONN {v} c{} CLOSE {}", ev.seq, ev.ts_ns, ev.conn, ev.payload_str());
            }
            Kind::WsIn => self.on_ws_in(ev, out),
            Kind::Timer => self.stats.timers += 1,
            Kind::SessionStart | Kind::HttpReq | Kind::WsOut | Kind::Action | Kind::Note | Kind::WsCtl => {}
        }
        Action::Nothing
    }

    fn on_ws_in(&mut self, ev: &Event, out: &mut String) {
        let v = ev.venue.tag();
        let decoded = match ev.venue {
            Venue::Hyperliquid => hyperliquid::decode(&ev.payload),
            Venue::Lighter | Venue::None => Decoded::Malformed("venue sin decoder".into()),
        };
        match decoded {
            Decoded::Book(snap) => {
                self.stats.book_updates += 1;
                let key = (ev.venue, snap.coin.clone());
                let prev_bbo = self.books.get(&key).map(|b| (b.best_bid().map(|l| l.px), b.best_ask().map(|l| l.px)));
                let updates = self.books.get(&key).map(|b| b.updates).unwrap_or(0) + 1;
                let book = Book { bids: snap.bids, asks: snap.asks, exch_ms: snap.exch_ms, updates };

                let lag_ms = (ev.ts_ns / NS_PER_MS) as i64 - snap.exch_ms as i64;
                self.stats.last_lag_ms = lag_ms;
                self.stats.max_lag_ms = self.stats.max_lag_ms.max(lag_ms);

                if !book.is_sorted() {
                    self.stats.unsorted += 1;
                    let _ = writeln!(out, "{} {} WARN {v} {} libro desordenado", ev.seq, ev.ts_ns, snap.coin);
                }
                if book.is_crossed() {
                    self.stats.crossed += 1;
                    let _ = writeln!(out, "{} {} WARN {v} {} libro cruzado", ev.seq, ev.ts_ns, snap.coin);
                }
                let bbo = (book.best_bid().map(|l| l.px), book.best_ask().map(|l| l.px));
                if prev_bbo != Some(bbo) {
                    self.stats.bbo_changes += 1;
                    if let (Some(b), Some(a), Some(mid), Some(sp)) =
                        (book.best_bid(), book.best_ask(), book.mid(), book.spread_bps_x100())
                    {
                        let _ = writeln!(
                            out,
                            "{} {} BBO {v} {} bid={}@{} ask={}@{} mid={} spread_bps={} lag_ms={}",
                            ev.seq,
                            ev.ts_ns,
                            snap.coin,
                            b.px,
                            b.sz,
                            a.px,
                            a.sz,
                            mid,
                            fmt_bps_x100(sp),
                            lag_ms
                        );
                    }
                }
                self.books.insert(key, book);
            }
            Decoded::Trades(trades) => {
                for t in trades {
                    self.stats.trades += 1;
                    let _ = writeln!(
                        out,
                        "{} {} TRADE {v} {} {} {}@{} tid={}",
                        ev.seq,
                        ev.ts_ns,
                        t.coin,
                        if t.taker_is_buy { "BUY" } else { "SELL" },
                        t.sz,
                        t.px,
                        t.tid
                    );
                }
            }
            Decoded::Pong | Decoded::SubAck | Decoded::Other(_) => self.stats.other_msgs += 1,
            Decoded::Malformed(why) => {
                self.stats.malformed += 1;
                let _ = writeln!(out, "{} {} WARN {v} malformed: {why}", ev.seq, ev.ts_ns);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(seq: u64, ts_ns: u64, kind: Kind, payload: &str) -> Event {
        Event { seq, ts_ns, kind, venue: Venue::Hyperliquid, conn: 1, payload: payload.as_bytes().to_vec() }
    }

    fn book_msg(bid: &str, ask: &str, t: u64) -> String {
        format!(
            r#"{{"channel":"l2Book","data":{{"coin":"BTC","time":{t},"levels":[[{{"px":"{bid}","sz":"1","n":1}}],[{{"px":"{ask}","sz":"2","n":1}}]]}}}}"#
        )
    }

    #[test]
    fn tracks_book_and_connection() {
        let mut sm = StateMachine::new();
        let mut out = String::new();
        sm.update(&ev(0, 1_000_000_000, Kind::ConnOpen, "wss://x"), &mut out);
        assert_eq!(sm.conn_state(Venue::Hyperliquid, 1), Some(ConnState::Connecting));
        sm.update(&ev(1, 1_000_000_000, Kind::HttpResp, "101"), &mut out);
        assert_eq!(sm.conn_state(Venue::Hyperliquid, 1), Some(ConnState::Up));

        // evento a t=1250 ms local, exchange dice 1000 ms => lag 250 ms
        let a = sm.update(&ev(2, 1_250_000_000, Kind::WsIn, &book_msg("100", "100.2", 1000)), &mut out);
        assert_eq!(a, Action::Nothing);
        let b = sm.book(Venue::Hyperliquid, "BTC").unwrap();
        assert_eq!(b.mid().unwrap().to_string(), "100.1");
        assert_eq!(sm.stats.last_lag_ms, 250);
        assert!(out.contains("BBO HL BTC bid=100@1 ask=100.2@2 mid=100.1"), "{out}");
        assert!(out.contains("lag_ms=250"));

        // mismo BBO: cuenta la actualizacion pero no imprime
        let before = out.len();
        sm.update(&ev(3, 1_300_000_000, Kind::WsIn, &book_msg("100", "100.2", 1100)), &mut out);
        assert_eq!(out.len(), before);
        assert_eq!((sm.stats.book_updates, sm.stats.bbo_changes), (2, 1));

        // desconexion: el libro se invalida
        sm.update(&ev(4, 1_400_000_000, Kind::ConnClose, "stale"), &mut out);
        assert!(sm.book(Venue::Hyperliquid, "BTC").is_none());
        assert_eq!(sm.conn_state(Venue::Hyperliquid, 1), Some(ConnState::Down));
        assert_eq!(sm.now_ns(), 1_400_000_000);
    }

    #[test]
    fn flags_bad_data() {
        let mut sm = StateMachine::new();
        let mut out = String::new();
        sm.update(&ev(0, 1, Kind::WsIn, &book_msg("101", "100", 1)), &mut out);
        sm.update(&ev(1, 2, Kind::WsIn, "garbage"), &mut out);
        assert_eq!((sm.stats.crossed, sm.stats.malformed), (1, 1));
        assert!(out.contains("libro cruzado") && out.contains("malformed"));
    }

    #[test]
    fn same_input_same_output() {
        let events = vec![
            ev(0, 10, Kind::ConnOpen, "wss://x"),
            ev(1, 20, Kind::WsIn, &book_msg("100", "100.2", 1)),
            ev(
                2,
                30,
                Kind::WsIn,
                r#"{"channel":"trades","data":[{"coin":"BTC","side":"B","px":"100.2","sz":"0.5","time":2,"tid":7}]}"#,
            ),
            ev(3, 40, Kind::Timer, ""),
            ev(4, 50, Kind::WsIn, &book_msg("100.1", "100.3", 3)),
        ];
        let run = || {
            let mut sm = StateMachine::new();
            let mut out = String::new();
            for e in &events {
                sm.update(e, &mut out);
            }
            (out, sm.stats)
        };
        let (a, sa) = run();
        let (b, sb) = run();
        assert_eq!(a, b);
        assert_eq!(sa, sb);
        assert_eq!(sa.trades, 1);
        assert!(a.contains("TRADE HL BTC BUY 0.5@100.2 tid=7"));
    }
}
