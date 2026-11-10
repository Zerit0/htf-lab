//! hft-recorder: paso 1 (+2) del hilo "How to HFT".
//!
//! Hilo principal  -> red + escritura del log (un solo hilo, sin async).
//! Hilo secundario -> sigue el fichero y ejecuta la state machine (el mismo
//!                    codigo que `hft-replay run`).
//!
//! AVISO: compila y pasa clippy en la CI, pero todavia no se ha ejecutado
//! contra el exchange real. Toda la logica vive en `hft_core::session`, que
//! si esta testeada; aqui solo hay el adaptador de tungstenite y el arranque.

use hft_core::clock::{utc_compact, Clock, SystemClock, NS_PER_SEC};
use hft_core::event::{Kind, Venue};
use hft_core::log::{LogReader, LogWriter};
use hft_core::replay;
use hft_core::session::{self, Connector, Handshake, Recv, SessionCfg, Transport};
use hft_core::sm::StateMachine;
use hft_core::venue::hyperliquid;
use std::io::{self, ErrorKind, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Error as WsError, Message, WebSocket};

const USAGE: &str = "\
uso: hft-recorder [opciones]
  --coins BTC,ETH,HYPE   monedas a grabar (libro L2 + trades). Por defecto BTC,ETH
  --out-dir DIR          carpeta de logs. Por defecto ./data
  --url URL              endpoint WebSocket. Por defecto mainnet de Hyperliquid
  --testnet              usa el endpoint de testnet
  --read-timeout-ms N    timeout de lectura del socket. Por defecto 100
  --no-sm                no ejecuta la state machine en vivo (solo graba)
  --quiet                ejecuta la SM pero no imprime sus lineas
  -h, --help
";

struct Args {
    coins: Vec<String>,
    out_dir: PathBuf,
    url: String,
    read_timeout_ms: u64,
    run_sm: bool,
    quiet: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        coins: vec!["BTC".into(), "ETH".into()],
        out_dir: PathBuf::from("data"),
        url: hyperliquid::WS_URL.to_string(),
        read_timeout_ms: 100,
        run_sm: true,
        quiet: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} necesita un valor"));
        match arg.as_str() {
            "--coins" => {
                a.coins = val("--coins")?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if a.coins.is_empty() {
                    return Err("--coins esta vacio".into());
                }
            }
            "--out-dir" => a.out_dir = PathBuf::from(val("--out-dir")?),
            "--url" => a.url = val("--url")?,
            "--testnet" => a.url = hyperliquid::WS_URL_TESTNET.to_string(),
            "--read-timeout-ms" => {
                a.read_timeout_ms =
                    val("--read-timeout-ms")?.parse().map_err(|_| "--read-timeout-ms: numero invalido".to_string())?;
                if a.read_timeout_ms == 0 {
                    return Err("--read-timeout-ms debe ser > 0".into());
                }
            }
            "--no-sm" => a.run_sm = false,
            "--quiet" => a.quiet = true,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("opcion desconocida: {other}")),
        }
    }
    Ok(a)
}

/// Adaptador: tungstenite sobre un socket bloqueante con timeout de lectura.
struct TungTransport {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl Transport for TungTransport {
    fn recv(&mut self) -> Recv {
        match self.ws.read() {
            Ok(Message::Text(s)) => Recv::Text(s),
            Ok(Message::Binary(b)) => Recv::Binary(b),
            // tungstenite encola el pong de respuesta y lo envia en la
            // siguiente lectura o escritura.
            Ok(Message::Ping(_)) => Recv::Ping,
            Ok(Message::Pong(_)) => Recv::Pong,
            Ok(Message::Close(frame)) => Recv::Closed(match frame {
                Some(f) => format!("{} {}", f.code, f.reason),
                None => "sin motivo".to_string(),
            }),
            Ok(Message::Frame(_)) => Recv::Idle,
            Err(WsError::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Recv::Idle,
            Err(WsError::ConnectionClosed) | Err(WsError::AlreadyClosed) => {
                Recv::Closed("conexion cerrada".to_string())
            }
            Err(e) => Recv::Error(e.to_string()),
        }
    }

    fn send_text(&mut self, msg: &str) -> Result<(), String> {
        self.ws.send(Message::Text(msg.to_string())).map_err(|e| e.to_string())
    }
}

struct TungConnector {
    read_timeout: Duration,
}

impl Connector for TungConnector {
    type T = TungTransport;

    fn connect(&mut self, url: &str) -> Result<(TungTransport, Handshake), String> {
        // TODO(paso 7): esta llamada bloquea sin timeout durante DNS, TCP y
        // TLS. El reactor sobre mio lo resuelve; hasta entonces systemd
        // reinicia el proceso si se queda colgado (WatchdogSec en F2).
        let (mut ws, resp) = tungstenite::connect(url).map_err(|e| e.to_string())?;
        let t = Some(self.read_timeout);
        match ws.get_mut() {
            MaybeTlsStream::Plain(s) => {
                let _ = s.set_read_timeout(t);
                let _ = s.set_nodelay(true);
            }
            MaybeTlsStream::Rustls(s) => {
                let _ = s.sock.set_read_timeout(t);
                let _ = s.sock.set_nodelay(true);
            }
            _ => {}
        }
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("?").to_string()))
            .collect();
        Ok((TungTransport { ws }, Handshake { status: resp.status().as_u16(), headers }))
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}\n");
            }
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error fatal: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(args: Args) -> io::Result<()> {
    let clock = SystemClock;
    std::fs::create_dir_all(&args.out_dir)?;
    let start_ns = clock.now_ns();
    let path = args.out_dir.join(format!("hl-{}.hftlog", utc_compact(start_ns / NS_PER_SEC)));
    let mut log = LogWriter::create(&path)?;
    eprintln!("grabando en {}", path.display());
    eprintln!("monedas: {}  url: {}", args.coins.join(","), args.url);

    let meta = format!(
        "hft-recorder {} coins={} url={} read_timeout_ms={}",
        env!("CARGO_PKG_VERSION"),
        args.coins.join(","),
        args.url,
        args.read_timeout_ms
    );
    log.append(start_ns, Kind::SessionStart, Venue::None, 0, meta.as_bytes())?;

    // El proceso termina por senal (Ctrl-C, systemd stop). No hace falta un
    // cierre ordenado: el log es append-only y el lector tolera una cola
    // truncada. `stop` existe para los tests y para F2.
    let stop = Arc::new(AtomicBool::new(false));

    if args.run_sm {
        let (p, s, quiet) = (path.clone(), stop.clone(), args.quiet);
        std::thread::Builder::new().name("sm".into()).spawn(move || {
            let mut reader = match LogReader::open(&p) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("sm: no se pudo abrir el log: {e}");
                    return;
                }
            };
            let stdout = io::stdout();
            let mut lock = stdout.lock();
            let out: Option<&mut dyn Write> = if quiet { None } else { Some(&mut lock) };
            match replay::run(&mut reader, &mut StateMachine::new(), out, Some(&s)) {
                Ok(sum) => eprintln!("{sum}"),
                Err(e) => eprintln!("sm: {e}"),
            }
        })?;
    }

    let cfg = SessionCfg::hyperliquid(&args.url, &args.coins);
    let mut connector = TungConnector { read_timeout: Duration::from_millis(args.read_timeout_ms) };
    session::run(&cfg, &mut connector, &clock, &mut log, &stop)
}
