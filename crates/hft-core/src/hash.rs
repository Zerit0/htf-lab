//! Hashes FNV-1a. No son criptograficos: sirven para detectar registros
//! corruptos (32 bits) y para comparar salidas de replay (64 bits).

pub fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Digest incremental de 64 bits. Se alimenta con cada linea de salida de la
/// state machine; dos ejecuciones son identicas si y solo si el digest coincide.
#[derive(Clone, Copy, Debug)]
pub struct Digest64(u64);

impl Default for Digest64 {
    fn default() -> Self {
        Digest64(0xcbf2_9ce4_8422_2325)
    }
}

impl Digest64 {
    pub fn update(&mut self, bytes: &[u8]) {
        let mut h = self.0;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }

    pub fn value(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv32_known_vectors() {
        assert_eq!(fnv1a32(b""), 0x811c_9dc5);
        assert_eq!(fnv1a32(b"a"), 0xe40c_292c);
        assert_eq!(fnv1a32(b"foobar"), 0xbf9c_f968);
    }

    #[test]
    fn digest_is_order_sensitive() {
        let mut a = Digest64::default();
        a.update(b"ab");
        let mut b = Digest64::default();
        b.update(b"ba");
        assert_ne!(a.value(), b.value());
    }
}
