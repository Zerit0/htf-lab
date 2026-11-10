//! Log de eventos append-only.
//!
//! Formato (little-endian). Ver docs/ARCHITECTURE.md.
//!
//!   cabecera de fichero:  8 bytes  "HFTLOG01"
//!   registro:
//!     u32  len        bytes que siguen a este campo (20 + payload + 4)
//!     u64  seq
//!     u64  ts_ns
//!     u8   kind
//!     u8   venue
//!     u16  conn
//!     [..] payload
//!     u32  fnv1a32 de todo lo anterior salvo `len`
//!
//! El escritor hace UNA sola llamada write por registro, de modo que un
//! lector que sigue el fichero (tail) nunca ve un registro a medias salvo en
//! la cola, donde simplemente espera. Un corte de luz deja como mucho un
//! registro truncado al final, que el lector ignora.

use crate::event::{Event, Kind, Venue};
use crate::hash::fnv1a32;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"HFTLOG01";
const FIXED: usize = 8 + 8 + 1 + 1 + 2; // seq ts kind venue conn
const TRAILER: usize = 4;
/// Un mensaje de exchange de mas de 16 MiB es un bug, no un dato.
pub const MAX_RECORD: usize = 16 * 1024 * 1024;

pub struct LogWriter {
    file: File,
    next_seq: u64,
    buf: Vec<u8>,
    bytes: u64,
}

impl LogWriter {
    /// Crea un fichero nuevo. Falla si ya existe: una sesion, un fichero.
    pub fn create(path: &Path) -> io::Result<LogWriter> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(MAGIC)?;
        Ok(LogWriter { file, next_seq: 0, buf: Vec::with_capacity(4096), bytes: MAGIC.len() as u64 })
    }

    /// Anade un registro y devuelve su numero de secuencia.
    pub fn append(&mut self, ts_ns: u64, kind: Kind, venue: Venue, conn: u16, payload: &[u8]) -> io::Result<u64> {
        let body = FIXED + payload.len() + TRAILER;
        if body > MAX_RECORD {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "registro demasiado grande"));
        }
        let seq = self.next_seq;
        self.buf.clear();
        self.buf.extend_from_slice(&(body as u32).to_le_bytes());
        self.buf.extend_from_slice(&seq.to_le_bytes());
        self.buf.extend_from_slice(&ts_ns.to_le_bytes());
        self.buf.push(kind as u8);
        self.buf.push(venue as u8);
        self.buf.extend_from_slice(&conn.to_le_bytes());
        self.buf.extend_from_slice(payload);
        let ck = fnv1a32(&self.buf[4..]);
        self.buf.extend_from_slice(&ck.to_le_bytes());
        self.file.write_all(&self.buf)?;
        self.next_seq += 1;
        self.bytes += self.buf.len() as u64;
        Ok(seq)
    }

    /// fsync. El recorder lo llama cada pocos segundos, no por evento.
    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }

    pub fn events_written(&self) -> u64 {
        self.next_seq
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes
    }
}

pub struct LogReader {
    file: File,
    buf: Vec<u8>,
    start: usize,
    header_ok: bool,
    expect_seq: u64,
}

impl LogReader {
    pub fn open(path: &Path) -> io::Result<LogReader> {
        Ok(LogReader {
            file: File::open(path)?,
            buf: Vec::with_capacity(1 << 16),
            start: 0,
            header_ok: false,
            expect_seq: 0,
        })
    }

    /// Lee mas bytes del fichero. Devuelve false si no habia nada nuevo.
    fn fill(&mut self) -> io::Result<bool> {
        if self.start > (1 << 20) {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        let old = self.buf.len();
        self.buf.resize(old + (1 << 16), 0);
        let n = self.file.read(&mut self.buf[old..])?;
        self.buf.truncate(old + n);
        Ok(n > 0)
    }

    fn avail(&self) -> usize {
        self.buf.len() - self.start
    }

    /// Asegura `n` bytes disponibles; false si el fichero (aun) no los tiene.
    fn need(&mut self, n: usize) -> io::Result<bool> {
        while self.avail() < n {
            if !self.fill()? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn corrupt(msg: &str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, format!("log corrupto: {msg}"))
    }

    /// Siguiente evento. `Ok(None)` significa "no hay mas POR AHORA": en
    /// replay es el final; siguiendo un fichero vivo, se reintenta despues.
    pub fn next_event(&mut self) -> io::Result<Option<Event>> {
        if !self.header_ok {
            if !self.need(MAGIC.len())? {
                return Ok(None);
            }
            if &self.buf[self.start..self.start + MAGIC.len()] != MAGIC {
                return Err(Self::corrupt("cabecera desconocida"));
            }
            self.start += MAGIC.len();
            self.header_ok = true;
        }
        if !self.need(4)? {
            return Ok(None);
        }
        let s = self.start;
        let body = u32::from_le_bytes(self.buf[s..s + 4].try_into().unwrap()) as usize;
        if !(FIXED + TRAILER..=MAX_RECORD).contains(&body) {
            return Err(Self::corrupt("longitud de registro imposible"));
        }
        if !self.need(4 + body)? {
            return Ok(None); // registro a medias en la cola: esperar
        }
        let s = self.start;
        let rec = &self.buf[s + 4..s + 4 + body];
        let (data, ck) = rec.split_at(body - TRAILER);
        if fnv1a32(data) != u32::from_le_bytes(ck.try_into().unwrap()) {
            return Err(Self::corrupt("checksum"));
        }
        let seq = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let ts_ns = u64::from_le_bytes(data[8..16].try_into().unwrap());
        let kind = Kind::from_u8(data[16]).ok_or_else(|| Self::corrupt("kind desconocido"))?;
        let venue = Venue::from_u8(data[17]);
        let conn = u16::from_le_bytes(data[18..20].try_into().unwrap());
        if seq != self.expect_seq {
            return Err(Self::corrupt("salto de secuencia"));
        }
        let ev = Event { seq, ts_ns, kind, venue, conn, payload: data[FIXED..].to_vec() };
        self.start += 4 + body;
        self.expect_seq += 1;
        Ok(Some(ev))
    }
}

#[cfg(test)]
pub(crate) fn temp_path(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let mut p = std::env::temp_dir();
    p.push(format!("hftlog-test-{}-{}-{}.hftlog", std::process::id(), N.fetch_add(1, Ordering::Relaxed), name));
    let _ = std::fs::remove_file(&p);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let p = temp_path("roundtrip");
        let mut w = LogWriter::create(&p).unwrap();
        assert_eq!(w.append(10, Kind::ConnOpen, Venue::Hyperliquid, 1, b"wss://x").unwrap(), 0);
        assert_eq!(w.append(20, Kind::WsIn, Venue::Hyperliquid, 1, b"{\"a\":1}").unwrap(), 1);
        assert_eq!(w.append(30, Kind::Timer, Venue::None, 0, b"").unwrap(), 2);
        let mut r = LogReader::open(&p).unwrap();
        let e0 = r.next_event().unwrap().unwrap();
        assert_eq!((e0.seq, e0.ts_ns, e0.kind, e0.conn), (0, 10, Kind::ConnOpen, 1));
        assert_eq!(e0.payload, b"wss://x");
        let e1 = r.next_event().unwrap().unwrap();
        assert_eq!(e1.payload_str(), "{\"a\":1}");
        let e2 = r.next_event().unwrap().unwrap();
        assert_eq!((e2.kind, e2.venue, e2.payload.len()), (Kind::Timer, Venue::None, 0));
        assert!(r.next_event().unwrap().is_none());
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn refuses_to_overwrite() {
        let p = temp_path("overwrite");
        let _w = LogWriter::create(&p).unwrap();
        assert!(LogWriter::create(&p).is_err());
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn tail_sees_new_records_and_waits_on_torn_tail() {
        let p = temp_path("tail");
        let mut w = LogWriter::create(&p).unwrap();
        let mut r = LogReader::open(&p).unwrap();
        assert!(r.next_event().unwrap().is_none());
        w.append(1, Kind::Note, Venue::None, 0, b"uno").unwrap();
        assert_eq!(r.next_event().unwrap().unwrap().payload, b"uno");
        assert!(r.next_event().unwrap().is_none());

        // Simula un registro a medias: se escribe solo la mitad.
        let full_len = {
            let before = std::fs::metadata(&p).unwrap().len();
            w.append(2, Kind::Note, Venue::None, 0, b"dos-dos-dos").unwrap();
            std::fs::metadata(&p).unwrap().len() - before
        };
        let bytes = std::fs::read(&p).unwrap();
        let cut = bytes.len() - (full_len as usize) / 2;
        let p2 = temp_path("tail-torn");
        std::fs::write(&p2, &bytes[..cut]).unwrap();
        let mut r2 = LogReader::open(&p2).unwrap();
        assert_eq!(r2.next_event().unwrap().unwrap().payload, b"uno");
        assert!(r2.next_event().unwrap().is_none(), "cola truncada => esperar, no error");
        // Llega el resto.
        let mut f = OpenOptions::new().append(true).open(&p2).unwrap();
        f.write_all(&bytes[cut..]).unwrap();
        assert_eq!(r2.next_event().unwrap().unwrap().payload, b"dos-dos-dos");
        std::fs::remove_file(&p).unwrap();
        std::fs::remove_file(&p2).unwrap();
    }

    #[test]
    fn detects_corruption() {
        let p = temp_path("corrupt");
        let mut w = LogWriter::create(&p).unwrap();
        w.append(1, Kind::WsIn, Venue::Hyperliquid, 1, b"payload-payload").unwrap();
        let mut bytes = std::fs::read(&p).unwrap();
        let n = bytes.len();
        bytes[n - 8] ^= 0xff; // voltea un byte del payload
        std::fs::write(&p, &bytes).unwrap();
        let mut r = LogReader::open(&p).unwrap();
        let err = r.next_event().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        std::fs::write(&p, b"NOTALOG!rest").unwrap();
        assert!(LogReader::open(&p).unwrap().next_event().is_err());
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn large_log_crosses_buffer_boundaries() {
        let p = temp_path("large");
        let mut w = LogWriter::create(&p).unwrap();
        let payload = vec![b'x'; 5000];
        for i in 0..2000u64 {
            w.append(i, Kind::WsIn, Venue::Hyperliquid, 1, &payload).unwrap();
        }
        let mut r = LogReader::open(&p).unwrap();
        let mut n = 0u64;
        while let Some(e) = r.next_event().unwrap() {
            assert_eq!(e.seq, n);
            assert_eq!(e.payload.len(), 5000);
            n += 1;
        }
        assert_eq!(n, 2000);
        std::fs::remove_file(&p).unwrap();
    }
}
