//! Modelo de eventos. TODO lo que cruza un borde del proceso es un evento y
//! se escribe en el log: los entrantes en cuanto se ven, los salientes ANTES
//! de enviarse. El log es la unica fuente de verdad de una sesion.

use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Kind {
    /// Arranque del recorder. Payload: version, argumentos, host.
    SessionStart = 1,
    /// Conexion TCP abierta (intento). Payload: URL.
    ConnOpen = 2,
    /// Conexion TCP cerrada. Payload: motivo.
    ConnClose = 3,
    /// Peticion HTTP saliente (upgrade a WebSocket, POST /exchange...).
    HttpReq = 4,
    /// Respuesta HTTP entrante. Payload: status + cabeceras.
    HttpResp = 5,
    /// Mensaje WebSocket entrante. Payload: bytes crudos del exchange.
    WsIn = 6,
    /// Mensaje WebSocket saliente. Se registra antes de enviarlo.
    WsOut = 7,
    /// Tick del temporizador: deja actuar a la state machine sin trafico.
    Timer = 8,
    /// Accion decidida por la state machine (place/cancel). Paso 4.
    Action = 9,
    /// Anotacion libre para humanos. La state machine la ignora.
    Note = 10,
    /// Control WebSocket (ping/pong).
    WsCtl = 11,
}

impl Kind {
    pub fn from_u8(v: u8) -> Option<Kind> {
        Some(match v {
            1 => Kind::SessionStart,
            2 => Kind::ConnOpen,
            3 => Kind::ConnClose,
            4 => Kind::HttpReq,
            5 => Kind::HttpResp,
            6 => Kind::WsIn,
            7 => Kind::WsOut,
            8 => Kind::Timer,
            9 => Kind::Action,
            10 => Kind::Note,
            11 => Kind::WsCtl,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::SessionStart => "SESSION_START",
            Kind::ConnOpen => "CONN_OPEN",
            Kind::ConnClose => "CONN_CLOSE",
            Kind::HttpReq => "HTTP_REQ",
            Kind::HttpResp => "HTTP_RESP",
            Kind::WsIn => "WS_IN",
            Kind::WsOut => "WS_OUT",
            Kind::Timer => "TIMER",
            Kind::Action => "ACTION",
            Kind::Note => "NOTE",
            Kind::WsCtl => "WS_CTL",
        }
    }

    pub const ALL: [Kind; 11] = [
        Kind::SessionStart,
        Kind::ConnOpen,
        Kind::ConnClose,
        Kind::HttpReq,
        Kind::HttpResp,
        Kind::WsIn,
        Kind::WsOut,
        Kind::Timer,
        Kind::Action,
        Kind::Note,
        Kind::WsCtl,
    ];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Venue {
    None = 0,
    Hyperliquid = 1,
    /// Reservado para F3 (paso 12 del hilo).
    Lighter = 2,
}

impl Venue {
    pub fn from_u8(v: u8) -> Venue {
        match v {
            1 => Venue::Hyperliquid,
            2 => Venue::Lighter,
            _ => Venue::None,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Venue::None => "--",
            Venue::Hyperliquid => "HL",
            Venue::Lighter => "LT",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Event {
    /// Numero de secuencia dentro del fichero, empezando en 0.
    pub seq: u64,
    /// Reloj local (ns Unix) en el momento de ver/emitir el evento.
    pub ts_ns: u64,
    pub kind: Kind,
    pub venue: Venue,
    /// Identificador de conexion dentro de la sesion.
    pub conn: u16,
    pub payload: Vec<u8>,
}

impl Event {
    pub fn payload_str(&self) -> &str {
        std::str::from_utf8(&self.payload).unwrap_or("<binario>")
    }
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} {} {} c{} len={}",
            self.seq,
            self.ts_ns,
            self.kind.name(),
            self.venue.tag(),
            self.conn,
            self.payload.len()
        )
    }
}

/// Lo que la state machine quiere hacer tras un evento (paso 4 del hilo).
/// En F0 siempre es `Nothing`; las variantes existen para fijar la interfaz.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    Nothing,
    Place {
        venue: Venue,
        coin: String,
        is_buy: bool,
        px: crate::fixed::Fixed,
        sz: crate::fixed::Fixed,
        /// Id de cliente generado de forma determinista por la SM.
        cloid: u64,
    },
    Cancel {
        venue: Venue,
        coin: String,
        cloid: u64,
    },
}
