//! Bucle de sesion de un feed WebSocket, generico sobre el transporte.
//!
//! Toda la logica (suscribir, heartbeat, temporizador, deteccion de feed
//! muerto, reconexion con backoff y, sobre todo, REGISTRAR CADA EVENTO) vive
//! aqui y se testea con un transporte simulado. El crate `hft-recorder` solo
//! aporta ~80 lineas que implementan `Connector`/`Transport` con tungstenite.
//!
//! Paso 1 del hilo: un solo hilo, sin async, entrantes registrados en cuanto
//! se ven, salientes registrados antes de enviarse.
//! Paso 7 (F2): este bucle bloqueante con timeout de lectura se sustituye por
//! un reactor sobre mio/kqueue/epoll. La interfaz de eventos no cambia.

use crate::clock::{Clock, NS_PER_MS, NS_PER_SEC};
use crate::event::{Kind, Venue};
use crate::log::LogWriter;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

/// Lo que devuelve una lectura del transporte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recv {
    Text(String),
    Binary(Vec<u8>),
    Ping,
    Pong,
    /// Vencio el timeout de lectura sin datos. No es un error.
    Idle,
    Closed(String),
    Error(String),
}

pub trait Transport {
    /// Bloquea como mucho el timeout de lectura configurado por el Connector.
    fn recv(&mut self) -> Recv;
    fn send_text(&mut self, msg: &str) -> Result<(), String>;
}

/// Datos de la respuesta HTTP del upgrade, para el log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handshake {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

pub trait Connector {
    type T: Transport;
    fn connect(&mut self, url: &str) -> Result<(Self::T, Handshake), String>;
}

#[derive(Clone, Debug)]
pub struct SessionCfg {
    pub venue: Venue,
    pub url: String,
    /// Mensajes a enviar tras conectar (suscripciones).
    pub subscribe: Vec<String>,
    /// Mensaje de heartbeat a nivel de aplicacion.
    pub ping: String,
    pub ping_every_ns: u64,
    /// Periodo del evento Timer. 0 = desactivado.
    pub timer_every_ns: u64,
    /// Sin ningun mensaje entrante durante este tiempo => reconectar.
    pub stale_after_ns: u64,
    pub backoff_min_ns: u64,
    pub backoff_max_ns: u64,
    /// fsync del log cada tanto. 0 = nunca (lo decide el SO).
    pub sync_every_ns: u64,
    /// Limite de conexiones (para tests). None = sin limite.
    pub max_connections: Option<u32>,
}

impl SessionCfg {
    pub fn hyperliquid(url: &str, coins: &[String]) -> SessionCfg {
        SessionCfg {
            venue: Venue::Hyperliquid,
            url: url.to_string(),
            subscribe: crate::venue::hyperliquid::subscriptions(coins),
            ping: crate::venue::hyperliquid::ping(),
            ping_every_ns: 20 * NS_PER_SEC,
            timer_every_ns: NS_PER_SEC,
            stale_after_ns: 45 * NS_PER_SEC,
            backoff_min_ns: 500 * NS_PER_MS,
            backoff_max_ns: 30 * NS_PER_SEC,
            sync_every_ns: 5 * NS_PER_SEC,
            max_connections: None,
        }
    }
}

/// Ejecuta la sesion hasta que `stop` sea true o se alcance `max_connections`.
/// Solo devuelve error si falla la escritura del log: sin log no se sigue.
pub fn run<C: Connector, K: Clock>(
    cfg: &SessionCfg,
    connector: &mut C,
    clock: &K,
    log: &mut LogWriter,
    stop: &AtomicBool,
) -> io::Result<()> {
    let v = cfg.venue;
    let mut conn: u16 = 0;
    let mut backoff = cfg.backoff_min_ns;
    let mut last_sync = clock.now_ns();
    let mut attempts: u32 = 0;

    while !stop.load(Ordering::Relaxed) {
        if let Some(max) = cfg.max_connections {
            if attempts >= max {
                break;
            }
        }
        attempts += 1;
        conn = conn.wrapping_add(1);

        log.append(clock.now_ns(), Kind::ConnOpen, v, conn, cfg.url.as_bytes())?;
        log.append(clock.now_ns(), Kind::HttpReq, v, conn, format!("GET {} (upgrade: websocket)", cfg.url).as_bytes())?;

        let mut t = match connector.connect(&cfg.url) {
            Ok((t, hs)) => {
                let mut p = format!("{}", hs.status);
                for (k, val) in &hs.headers {
                    p.push('\n');
                    p.push_str(k);
                    p.push_str(": ");
                    p.push_str(val);
                }
                log.append(clock.now_ns(), Kind::HttpResp, v, conn, p.as_bytes())?;
                t
            }
            Err(e) => {
                log.append(clock.now_ns(), Kind::ConnClose, v, conn, format!("connect_error: {e}").as_bytes())?;
                clock.sleep_ns(backoff);
                backoff = (backoff * 2).min(cfg.backoff_max_ns);
                continue;
            }
        };

        let mut reason: Option<String> = None;
        for m in &cfg.subscribe {
            log.append(clock.now_ns(), Kind::WsOut, v, conn, m.as_bytes())?;
            if let Err(e) = t.send_text(m) {
                reason = Some(format!("send_error: {e}"));
                break;
            }
        }

        let opened = clock.now_ns();
        let mut last_rx = opened;
        let mut last_ping = opened;
        let mut last_timer = opened;

        while reason.is_none() {
            if stop.load(Ordering::Relaxed) {
                reason = Some("shutdown".into());
                break;
            }
            let r = t.recv();
            let now = clock.now_ns();
            match r {
                Recv::Text(s) => {
                    log.append(now, Kind::WsIn, v, conn, s.as_bytes())?;
                    last_rx = now;
                    backoff = cfg.backoff_min_ns; // la conexion funciona
                }
                Recv::Binary(b) => {
                    log.append(now, Kind::WsIn, v, conn, &b)?;
                    last_rx = now;
                    backoff = cfg.backoff_min_ns;
                }
                Recv::Ping => {
                    log.append(now, Kind::WsCtl, v, conn, b"ping_in")?;
                    last_rx = now;
                }
                Recv::Pong => {
                    log.append(now, Kind::WsCtl, v, conn, b"pong_in")?;
                    last_rx = now;
                }
                Recv::Idle => {}
                Recv::Closed(why) => reason = Some(format!("closed: {why}")),
                Recv::Error(why) => reason = Some(format!("error: {why}")),
            }
            if reason.is_some() {
                break;
            }

            // Temporizadores. Se evaluan tras CADA lectura, no solo en Idle:
            // un feed muy activo no debe dejar sin ticks a la state machine.
            if cfg.timer_every_ns > 0 && now.saturating_sub(last_timer) >= cfg.timer_every_ns {
                log.append(now, Kind::Timer, Venue::None, 0, b"")?;
                last_timer = now;
            }
            if now.saturating_sub(last_ping) >= cfg.ping_every_ns {
                log.append(now, Kind::WsOut, v, conn, cfg.ping.as_bytes())?;
                if let Err(e) = t.send_text(&cfg.ping) {
                    reason = Some(format!("send_error: {e}"));
                }
                last_ping = now;
            }
            if now.saturating_sub(last_rx) >= cfg.stale_after_ns {
                reason = Some("stale: sin mensajes entrantes".into());
            }
            if cfg.sync_every_ns > 0 && now.saturating_sub(last_sync) >= cfg.sync_every_ns {
                log.sync()?;
                last_sync = now;
            }
        }

        let why = reason.unwrap_or_else(|| "unknown".into());
        log.append(clock.now_ns(), Kind::ConnClose, v, conn, why.as_bytes())?;
        log.sync()?;
        drop(t);
        if why == "shutdown" {
            break;
        }
        clock.sleep_ns(backoff);
        backoff = (backoff * 2).min(cfg.backoff_max_ns);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;
    use crate::log::{temp_path, LogReader};
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// Reloj simulado: avanza 100 ms en cada consulta y al dormir.
    struct MockClock {
        now: Cell<u64>,
        slept: RefCell<Vec<u64>>,
    }
    impl MockClock {
        fn new() -> Self {
            MockClock { now: Cell::new(1_000 * NS_PER_SEC), slept: RefCell::new(vec![]) }
        }
    }
    impl Clock for MockClock {
        fn now_ns(&self) -> u64 {
            let n = self.now.get() + 100 * NS_PER_MS;
            self.now.set(n);
            n
        }
        fn sleep_ns(&self, ns: u64) {
            self.slept.borrow_mut().push(ns);
            self.now.set(self.now.get() + ns);
        }
    }

    struct MockTransport {
        script: VecDeque<Recv>,
        sent: Rc<RefCell<Vec<String>>>,
    }
    impl Transport for MockTransport {
        fn recv(&mut self) -> Recv {
            self.script.pop_front().unwrap_or(Recv::Closed("fin del guion".into()))
        }
        fn send_text(&mut self, msg: &str) -> Result<(), String> {
            self.sent.borrow_mut().push(msg.to_string());
            Ok(())
        }
    }

    struct MockConnector {
        /// Un guion por conexion; `None` = fallo de conexion.
        scripts: VecDeque<Option<Vec<Recv>>>,
        sent: Rc<RefCell<Vec<String>>>,
    }
    impl Connector for MockConnector {
        type T = MockTransport;
        fn connect(&mut self, _url: &str) -> Result<(MockTransport, Handshake), String> {
            match self.scripts.pop_front() {
                Some(Some(s)) => Ok((
                    MockTransport { script: s.into(), sent: self.sent.clone() },
                    Handshake { status: 101, headers: vec![("upgrade".into(), "websocket".into())] },
                )),
                _ => Err("refused".into()),
            }
        }
    }

    fn read_all(p: &std::path::Path) -> Vec<Event> {
        let mut r = LogReader::open(p).unwrap();
        let mut v = vec![];
        while let Some(e) = r.next_event().unwrap() {
            v.push(e);
        }
        v
    }

    fn cfg(max: u32) -> SessionCfg {
        let mut c = SessionCfg::hyperliquid("wss://test/ws", &["BTC".to_string()]);
        c.max_connections = Some(max);
        c
    }

    #[test]
    fn logs_everything_in_order() {
        let p = temp_path("session-order");
        let mut log = LogWriter::create(&p).unwrap();
        let sent = Rc::new(RefCell::new(vec![]));
        let mut conn = MockConnector {
            scripts: vec![Some(vec![
                Recv::Text(r#"{"channel":"subscriptionResponse"}"#.into()),
                Recv::Text(r#"{"channel":"l2Book"}"#.into()),
                Recv::Ping,
            ])]
            .into(),
            sent: sent.clone(),
        };
        run(&cfg(1), &mut conn, &MockClock::new(), &mut log, &AtomicBool::new(false)).unwrap();
        let evs = read_all(&p);
        let kinds: Vec<Kind> = evs.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                Kind::ConnOpen,
                Kind::HttpReq,
                Kind::HttpResp,
                Kind::WsOut,
                Kind::WsOut, // l2Book + trades
                Kind::WsIn,
                Kind::WsIn,
                Kind::WsCtl,
                Kind::ConnClose,
            ]
        );
        assert_eq!(evs[0].payload_str(), "wss://test/ws");
        assert!(evs[2].payload_str().starts_with("101\nupgrade: websocket"));
        assert!(evs[3].payload_str().contains("l2Book"));
        assert!(evs[8].payload_str().starts_with("closed"));
        // Lo registrado como saliente es exactamente lo enviado, y en ese orden.
        let logged_out: Vec<String> =
            evs.iter().filter(|e| e.kind == Kind::WsOut).map(|e| e.payload_str().to_string()).collect();
        assert_eq!(*sent.borrow(), logged_out);
        // Tiempos monotonos, secuencia sin huecos.
        assert!(evs.windows(2).all(|w| w[0].ts_ns <= w[1].ts_ns && w[0].seq + 1 == w[1].seq));
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn timer_ping_and_stale_detection() {
        let p = temp_path("session-timers");
        let mut log = LogWriter::create(&p).unwrap();
        let sent = Rc::new(RefCell::new(vec![]));
        // 600 lecturas Idle a 100 ms = 60 s simulados sin datos.
        let mut conn = MockConnector { scripts: vec![Some(vec![Recv::Idle; 600])].into(), sent: sent.clone() };
        run(&cfg(1), &mut conn, &MockClock::new(), &mut log, &AtomicBool::new(false)).unwrap();
        let evs = read_all(&p);
        let timers = evs.iter().filter(|e| e.kind == Kind::Timer).count();
        let pings = sent.borrow().iter().filter(|m| m.contains("ping")).count();
        let close = evs.iter().rev().find(|e| e.kind == Kind::ConnClose).unwrap();
        assert!(close.payload_str().starts_with("stale"), "{}", close.payload_str());
        // ~45 s hasta declararlo muerto: ~44 timers de 1 s y 2 pings de 20 s.
        assert!((40..=46).contains(&timers), "timers={timers}");
        assert_eq!(pings, 2);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn reconnects_with_exponential_backoff() {
        let p = temp_path("session-backoff");
        let mut log = LogWriter::create(&p).unwrap();
        let clock = MockClock::new();
        let mut conn = MockConnector {
            scripts: vec![None, None, None, Some(vec![Recv::Text("{}".into())])].into(),
            sent: Rc::new(RefCell::new(vec![])),
        };
        run(&cfg(4), &mut conn, &clock, &mut log, &AtomicBool::new(false)).unwrap();
        let evs = read_all(&p);
        assert_eq!(evs.iter().filter(|e| e.kind == Kind::ConnOpen).count(), 4);
        let errs = evs.iter().filter(|e| e.payload_str().starts_with("connect_error")).count();
        assert_eq!(errs, 3);
        let conns: Vec<u16> = evs.iter().filter(|e| e.kind == Kind::ConnOpen).map(|e| e.conn).collect();
        assert_eq!(conns, vec![1, 2, 3, 4]);
        // 0.5 s, 1 s, 2 s tras los fallos; al recibir datos el backoff vuelve al minimo.
        let slept = clock.slept.borrow();
        assert_eq!(&slept[..3], &[500 * NS_PER_MS, NS_PER_SEC, 2 * NS_PER_SEC]);
        assert_eq!(slept[3], 500 * NS_PER_MS);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn stop_flag_ends_cleanly() {
        let p = temp_path("session-stop");
        let mut log = LogWriter::create(&p).unwrap();
        let mut conn =
            MockConnector { scripts: vec![Some(vec![Recv::Idle; 10])].into(), sent: Rc::new(RefCell::new(vec![])) };
        let stop = AtomicBool::new(true);
        run(&cfg(5), &mut conn, &MockClock::new(), &mut log, &stop).unwrap();
        assert!(read_all(&p).is_empty(), "con stop activo no se abre ninguna conexion");
        std::fs::remove_file(&p).unwrap();
    }
}
