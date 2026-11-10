//! hft-core: el nucleo determinista del stack.
//!
//! Regla de oro (paso 6 del hilo "How to HFT"):
//!   paquetes entran -> state machine -> paquetes salen.
//!   El codigo no determinista (red, reloj, disco) vive en los bordes.
//!
//! Este crate NO tiene dependencias y NO lee el reloj del sistema salvo en
//! `clock::SystemClock`, que solo usan los bordes. La state machine obtiene
//! el tiempo del propio evento, de modo que un replay produce exactamente la
//! misma salida que la sesion en vivo.

pub mod book;
pub mod clock;
pub mod event;
pub mod fixed;
pub mod hash;
pub mod json;
pub mod log;
pub mod replay;
pub mod session;
pub mod sm;
pub mod venue;
