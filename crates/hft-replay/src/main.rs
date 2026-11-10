//! hft-replay: todo lo que se hace con un log SIN tocar la red.
//!
//!   hft-replay run <log> [--quiet]       ejecuta la SM e imprime su salida + resumen
//!   hft-replay verify <log>              ejecuta dos veces y compara digests
//!   hft-replay dump <log> [--limit N] [--kind K]   vuelca eventos crudos
//!   hft-replay stats <log>               recuento por tipo, bytes, duracion
//!   hft-replay gen <log> [--events N] [--seed S]   genera un log sintetico

use hft_core::event::{Kind, Venue};
use hft_core::log::{LogReader, LogWriter};
use hft_core::replay;
use hft_core::sm::StateMachine;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "\
uso:
  hft-replay run <log> [--quiet]
  hft-replay verify <log>
  hft-replay dump <log> [--limit N] [--kind NOMBRE]
  hft-replay stats <log>
  hft-replay gen <log> [--events N] [--seed S]
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match real_main(&args) {
        Ok(code) => code,
        Err(e) => {
            // Un pipe cerrado (| head) no es un fallo.
            if e.kind() == io::ErrorKind::BrokenPipe {
                return ExitCode::SUCCESS;
            }
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn parse_num(args: &[String], name: &str, default: u64) -> io::Result<u64> {
    match flag_value(args, name) {
        None => Ok(default),
        Some(v) => {
            v.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("{name}: numero invalido '{v}'")))
        }
    }
}

fn real_main(args: &[String]) -> io::Result<ExitCode> {
    let (cmd, path) = match (args.first(), args.get(1)) {
        (Some(c), Some(p)) => (c.as_str(), Path::new(p)),
        _ => {
            eprint!("{USAGE}");
            return Ok(ExitCode::from(2));
        }
    };
    let rest = &args[2..];
    match cmd {
        "run" => {
            let quiet = rest.iter().any(|a| a == "--quiet");
            let stdout = io::stdout();
            let mut w = BufWriter::new(stdout.lock());
            let out: Option<&mut dyn Write> = if quiet { None } else { Some(&mut w) };
            let s = replay::run(&mut LogReader::open(path)?, &mut StateMachine::new(), out, None)?;
            w.flush()?;
            eprintln!("{s}");
            Ok(ExitCode::SUCCESS)
        }
        "verify" => {
            let a = replay::run(&mut LogReader::open(path)?, &mut StateMachine::new(), None, None)?;
            let b = replay::run(&mut LogReader::open(path)?, &mut StateMachine::new(), None, None)?;
            println!("{a}");
            if a == b {
                println!("DETERMINISTA OK  (dos ejecuciones, mismo digest)");
                Ok(ExitCode::SUCCESS)
            } else {
                println!("NO DETERMINISTA  {:016x} != {:016x}", a.digest, b.digest);
                Ok(ExitCode::from(3))
            }
        }
        "dump" => {
            let limit = parse_num(rest, "--limit", u64::MAX)?;
            let kind = flag_value(rest, "--kind");
            let mut r = LogReader::open(path)?;
            let stdout = io::stdout();
            let mut w = BufWriter::new(stdout.lock());
            let mut n = 0u64;
            while n < limit {
                let Some(ev) = r.next_event()? else { break };
                if let Some(k) = kind {
                    if !ev.kind.name().eq_ignore_ascii_case(k) {
                        continue;
                    }
                }
                let p = ev.payload_str();
                let preview: String = p.chars().take(400).map(|c| if c == '\n' { '|' } else { c }).collect();
                let more = if p.chars().count() > 400 { "..." } else { "" };
                writeln!(w, "{ev} {preview}{more}")?;
                n += 1;
            }
            w.flush()?;
            Ok(ExitCode::SUCCESS)
        }
        "stats" => {
            let mut r = LogReader::open(path)?;
            let mut counts = [0u64; 16];
            let mut bytes = [0u64; 16];
            let (mut first, mut last, mut total) = (0u64, 0u64, 0u64);
            while let Some(ev) = r.next_event()? {
                if total == 0 {
                    first = ev.ts_ns;
                }
                last = ev.ts_ns;
                total += 1;
                counts[ev.kind as usize] += 1;
                bytes[ev.kind as usize] += ev.payload.len() as u64;
            }
            let secs = last.saturating_sub(first) as f64 / 1e9;
            println!("fichero   {}", path.display());
            println!("tamano    {} bytes", std::fs::metadata(path)?.len());
            println!("eventos   {total}");
            println!("duracion  {secs:.1} s");
            if secs > 0.0 {
                println!("ritmo     {:.1} eventos/s", total as f64 / secs);
            }
            println!("{:<14} {:>12} {:>14}", "tipo", "eventos", "bytes payload");
            for k in Kind::ALL {
                let i = k as usize;
                if counts[i] > 0 {
                    println!("{:<14} {:>12} {:>14}", k.name(), counts[i], bytes[i]);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        "gen" => {
            let events = parse_num(rest, "--events", 10_000)?;
            let seed = parse_num(rest, "--seed", 42)?;
            generate(path, events, seed)?;
            println!("generado {} con {events} mensajes (seed {seed})", path.display());
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            eprint!("{USAGE}");
            Ok(ExitCode::from(2))
        }
    }
}

/// Generador congruencial: mismo seed => mismo log, en cualquier maquina.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Escribe un log con forma de sesion real de Hyperliquid (2 monedas, libro
/// de 5 niveles, trades, timers, una reconexion a mitad). Sirve para probar
/// replay y determinismo sin red. NO es un modelo de mercado.
fn generate(path: &Path, n: u64, seed: u64) -> io::Result<()> {
    let mut w = LogWriter::create(path)?;
    let mut rng = Lcg(seed);
    let hl = Venue::Hyperliquid;
    let mut ts: u64 = 1_794_300_000_000_000_000; // 2026-11-10 aprox, ns
    let mut conn: u16 = 1;
    // precio en centimos
    let mut px: [i64; 2] = [10_300_000, 350_000];
    let coins = ["BTC", "ETH"];
    let mut tid = 1u64;
    let mut last_timer = ts;

    w.append(ts, Kind::SessionStart, Venue::None, 0, format!("synthetic seed={seed}").as_bytes())?;
    let open = |w: &mut LogWriter, ts: u64, conn: u16| -> io::Result<()> {
        w.append(ts, Kind::ConnOpen, hl, conn, b"wss://synthetic/ws")?;
        w.append(ts, Kind::HttpReq, hl, conn, b"GET wss://synthetic/ws (upgrade: websocket)")?;
        w.append(ts + 150_000_000, Kind::HttpResp, hl, conn, b"101\nupgrade: websocket")?;
        for c in ["BTC", "ETH"] {
            w.append(
                ts + 150_000_000,
                Kind::WsOut,
                hl,
                conn,
                format!(r#"{{"method":"subscribe","subscription":{{"type":"l2Book","coin":"{c}"}}}}"#).as_bytes(),
            )?;
        }
        Ok(())
    };
    open(&mut w, ts, conn)?;
    ts += 200_000_000;

    let cents = |v: i64| format!("{}.{:02}", v / 100, v % 100);
    for i in 0..n {
        ts += 1_000_000 + rng.below(40_000_000); // 1-41 ms entre mensajes
        if ts - last_timer >= 1_000_000_000 {
            w.append(ts, Kind::Timer, Venue::None, 0, b"")?;
            last_timer = ts;
        }
        if n >= 100 && i == n / 2 {
            w.append(ts, Kind::ConnClose, hl, conn, b"closed: synthetic reconnect")?;
            conn += 1;
            ts += 500_000_000;
            open(&mut w, ts, conn)?;
            ts += 200_000_000;
        }
        let c = rng.below(2) as usize;
        px[c] += (rng.below(11) as i64 - 5) * 10;
        let exch_ms = ts / 1_000_000 - 180 - rng.below(60); // ~200 ms de lag
        if rng.below(5) == 0 {
            let buy = rng.below(2) == 0;
            let tpx = if buy { px[c] + 50 } else { px[c] - 50 };
            let msg = format!(
                r#"{{"channel":"trades","data":[{{"coin":"{}","side":"{}","px":"{}","sz":"0.{:03}","time":{},"tid":{}}}]}}"#,
                coins[c],
                if buy { "B" } else { "A" },
                cents(tpx),
                1 + rng.below(999),
                exch_ms,
                tid
            );
            tid += 1;
            w.append(ts, Kind::WsIn, hl, conn, msg.as_bytes())?;
        } else {
            let mut bids = String::new();
            let mut asks = String::new();
            for l in 0..5i64 {
                let sep = if l == 0 { "" } else { "," };
                bids.push_str(&format!(
                    r#"{sep}{{"px":"{}","sz":"{}.{:02}","n":{}}}"#,
                    cents(px[c] - 50 - l * 100),
                    1 + rng.below(9),
                    rng.below(100),
                    1 + rng.below(6)
                ));
                asks.push_str(&format!(
                    r#"{sep}{{"px":"{}","sz":"{}.{:02}","n":{}}}"#,
                    cents(px[c] + 50 + l * 100),
                    1 + rng.below(9),
                    rng.below(100),
                    1 + rng.below(6)
                ));
            }
            let msg = format!(
                r#"{{"channel":"l2Book","data":{{"coin":"{}","time":{},"levels":[[{}],[{}]]}}}}"#,
                coins[c], exch_ms, bids, asks
            );
            w.append(ts, Kind::WsIn, hl, conn, msg.as_bytes())?;
        }
    }
    w.append(ts + 1, Kind::ConnClose, hl, conn, b"shutdown")?;
    w.sync()
}
