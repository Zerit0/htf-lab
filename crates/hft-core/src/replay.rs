//! Consumidor del log: lee eventos y los pasa por la state machine.
//!
//! Es EL MISMO codigo en vivo y en replay (pasos 2 y 3 del hilo):
//!   - en vivo, un hilo sigue el fichero mientras crece (`follow`);
//!   - en replay, se lee un fichero ya cerrado y se termina al llegar al final.
//!
//! Si la salida difiere entre ambos modos, la state machine no es determinista.

use crate::event::Action;
use crate::hash::Digest64;
use crate::log::LogReader;
use crate::sm::{StateMachine, Stats};
use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub digest: u64,
    pub actions: u64,
    pub first_ts_ns: u64,
    pub last_ts_ns: u64,
    pub stats: Stats,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = &self.stats;
        let secs = self.last_ts_ns.saturating_sub(self.first_ts_ns) as f64 / 1e9;
        writeln!(f, "eventos        {}", s.events)?;
        writeln!(f, "duracion       {secs:.1} s")?;
        writeln!(f, "libros         {} actualizaciones, {} cambios de BBO", s.book_updates, s.bbo_changes)?;
        writeln!(f, "trades         {}", s.trades)?;
        writeln!(f, "conexiones     {} abiertas, {} cerradas", s.conn_opens, s.conn_closes)?;
        writeln!(f, "timers         {}", s.timers)?;
        writeln!(f, "acciones       {}", self.actions)?;
        writeln!(f, "lag feed (ms)  ultimo {}, maximo {}", s.last_lag_ms, s.max_lag_ms)?;
        writeln!(f, "anomalias      malformed {}, cruzados {}, desordenados {}", s.malformed, s.crossed, s.unsorted)?;
        write!(f, "digest         {:016x}", self.digest)
    }
}

/// Ejecuta la SM sobre el log.
///
/// - `out`: donde se escriben las lineas de la SM. `None` = solo digest.
/// - `follow`: si es `Some(stop)`, al llegar al final espera mas datos hasta
///   que `stop` sea true (y entonces drena lo que quede).
pub fn run(
    reader: &mut LogReader,
    sm: &mut StateMachine,
    mut out: Option<&mut dyn Write>,
    follow: Option<&AtomicBool>,
) -> io::Result<Summary> {
    let mut digest = Digest64::default();
    let mut line = String::with_capacity(256);
    let mut actions = 0u64;
    let mut first_ts_ns = 0u64;
    let mut last_ts_ns = 0u64;
    let mut seen_any = false;
    let mut drained_after_stop = false;
    loop {
        match reader.next_event()? {
            Some(ev) => {
                if !seen_any {
                    first_ts_ns = ev.ts_ns;
                    seen_any = true;
                }
                last_ts_ns = ev.ts_ns;
                line.clear();
                let action = sm.update(&ev, &mut line);
                if action != Action::Nothing {
                    actions += 1;
                    // F2: aqui se enviara la accion al hilo de red (paso 4).
                    use std::fmt::Write as _;
                    let _ = writeln!(line, "{} {} ACTION {:?}", ev.seq, ev.ts_ns, action);
                }
                if !line.is_empty() {
                    digest.update(line.as_bytes());
                    if let Some(w) = out.as_mut() {
                        w.write_all(line.as_bytes())?;
                    }
                }
            }
            None => match follow {
                Some(stop) => {
                    if stop.load(Ordering::Relaxed) {
                        // El escritor termina ANTES de activar `stop`, asi que
                        // una pasada mas tras verlo garantiza drenar todo.
                        if drained_after_stop {
                            break;
                        }
                        drained_after_stop = true;
                        continue;
                    }
                    // Paso 2 del hilo: "polls the file". El paso 9 lo cambia
                    // por una cola compartida estilo Chronicle-Queue.
                    std::thread::sleep(Duration::from_micros(500));
                }
                None => break,
            },
        }
    }
    if let Some(w) = out.as_mut() {
        w.flush()?;
    }
    Ok(Summary { digest: digest.value(), actions, first_ts_ns, last_ts_ns, stats: sm.stats.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Kind, Venue};
    use crate::log::{temp_path, LogWriter};
    use std::sync::Arc;

    fn write_sample(p: &std::path::Path, n: u64) {
        let mut w = LogWriter::create(p).unwrap();
        w.append(1, Kind::ConnOpen, Venue::Hyperliquid, 1, b"wss://x").unwrap();
        for i in 0..n {
            let msg = format!(
                r#"{{"channel":"l2Book","data":{{"coin":"BTC","time":{},"levels":[[{{"px":"{}","sz":"1","n":1}}],[{{"px":"{}","sz":"1","n":1}}]]}}}}"#,
                i,
                100 + i,
                101 + i
            );
            w.append(1000 + i, Kind::WsIn, Venue::Hyperliquid, 1, msg.as_bytes()).unwrap();
        }
    }

    #[test]
    fn replay_is_deterministic() {
        let p = temp_path("replay-det");
        write_sample(&p, 50);
        let go = || {
            let mut out: Vec<u8> = Vec::new();
            let s = run(&mut LogReader::open(&p).unwrap(), &mut StateMachine::new(), Some(&mut out), None).unwrap();
            (out, s)
        };
        let (o1, s1) = go();
        let (o2, s2) = go();
        assert_eq!(o1, o2);
        assert_eq!(s1, s2);
        assert_eq!(s1.stats.book_updates, 50);
        // sin salida el digest es el mismo
        let s3 = run(&mut LogReader::open(&p).unwrap(), &mut StateMachine::new(), None, None).unwrap();
        assert_eq!(s3.digest, s1.digest);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn live_follow_matches_replay() {
        // Un hilo escribe mientras otro sigue el fichero: la salida debe ser
        // identica a la del replay posterior. Es el paso 3 del hilo.
        let p = temp_path("replay-follow");
        let stop = Arc::new(AtomicBool::new(false));
        let mut w = LogWriter::create(&p).unwrap();

        let (p2, stop2) = (p.clone(), stop.clone());
        let follower = std::thread::spawn(move || {
            let mut out: Vec<u8> = Vec::new();
            let s = run(&mut LogReader::open(&p2).unwrap(), &mut StateMachine::new(), Some(&mut out), Some(&stop2))
                .unwrap();
            (out, s)
        });

        w.append(1, Kind::ConnOpen, Venue::Hyperliquid, 1, b"wss://x").unwrap();
        for i in 0..300u64 {
            let msg = format!(
                r#"{{"channel":"l2Book","data":{{"coin":"ETH","time":{},"levels":[[{{"px":"{}","sz":"1","n":1}}],[{{"px":"{}","sz":"1","n":1}}]]}}}}"#,
                i,
                3000 + (i % 7),
                3010 + (i % 5)
            );
            w.append(1000 + i, Kind::WsIn, Venue::Hyperliquid, 1, msg.as_bytes()).unwrap();
            if i % 50 == 0 {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        stop.store(true, Ordering::Relaxed);
        let (live_out, live_sum) = follower.join().unwrap();

        let mut replay_out: Vec<u8> = Vec::new();
        let replay_sum =
            run(&mut LogReader::open(&p).unwrap(), &mut StateMachine::new(), Some(&mut replay_out), None).unwrap();
        assert_eq!(live_sum.stats.book_updates, 300);
        assert_eq!(live_out, replay_out);
        assert_eq!(live_sum, replay_sum);
        std::fs::remove_file(&p).unwrap();
    }
}
