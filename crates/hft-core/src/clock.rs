//! Reloj. Solo los BORDES (red, recorder) usan un reloj real. La state
//! machine jamas llama aqui: toma el tiempo del evento.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub trait Clock {
    /// Nanosegundos desde el epoch Unix.
    fn now_ns(&self) -> u64;
    fn sleep_ns(&self, ns: u64);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ns(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
    }

    fn sleep_ns(&self, ns: u64) {
        std::thread::sleep(Duration::from_nanos(ns));
    }
}

pub const NS_PER_MS: u64 = 1_000_000;
pub const NS_PER_SEC: u64 = 1_000_000_000;

/// "20261110T112233Z" a partir de segundos Unix. Sin dependencias
/// (algoritmo civil-from-days de Howard Hinnant).
pub fn utc_compact(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}{mo:02}{d:02}T{h:02}{m:02}{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_formatting() {
        assert_eq!(utc_compact(0), "19700101T000000Z");
        assert_eq!(utc_compact(951_782_400), "20000229T000000Z"); // bisiesto
        assert_eq!(utc_compact(1_700_000_000), "20231114T221320Z");
        assert_eq!(utc_compact(1_794_310_556), "20261110T113556Z");
    }
}
