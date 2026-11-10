//! Parser JSON minimo y sin dependencias.
//!
//! Por que no serde: el nucleo debe compilar y testearse sin red y sin
//! dependencias, y queremos los numeros como texto crudo (para pasarlos a
//! `Fixed` sin tocar f64). No es rapido; cuando la latencia importe (paso 13+)
//! se sustituye por un decoder especifico por venue sin cambiar la interfaz.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Texto crudo del numero, sin interpretar.
    Num(String),
    Str(String),
    Arr(Vec<Value>),
    /// Vector en orden de aparicion (determinista, sin hashing).
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// Texto numerico tanto si llego como numero (123) como string ("123").
    pub fn as_num_str(&self) -> Option<&str> {
        match self {
            Value::Num(s) | Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_num_str()?.parse().ok()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonError {
    pub pos: usize,
    pub msg: &'static str,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "json: {} en byte {}", self.msg, self.pos)
    }
}

const MAX_DEPTH: usize = 64;

pub fn parse(input: &[u8]) -> Result<Value, JsonError> {
    let mut p = Parser { b: input, i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.err("datos tras el valor"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, msg: &'static str) -> JsonError {
        JsonError { pos: self.i, msg }
    }

    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn lit(&mut self, s: &[u8], v: Value) -> Result<Value, JsonError> {
        if self.b[self.i..].starts_with(s) {
            self.i += s.len();
            Ok(v)
        } else {
            Err(self.err("literal invalido"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err("anidamiento excesivo"));
        }
        match self.peek() {
            None => Err(self.err("fin inesperado")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') => self.lit(b"true", Value::Bool(true)),
            Some(b'f') => self.lit(b"false", Value::Bool(false)),
            Some(b'n') => self.lit(b"null", Value::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(_) => Err(self.err("caracter inesperado")),
        }
    }

    fn number(&mut self) -> Result<Value, JsonError> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'e' | b'E') {
                self.i += 1;
            } else {
                break;
            }
        }
        let s = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("utf8"))?;
        if s == "-" || s.is_empty() {
            return Err(self.err("numero invalido"));
        }
        Ok(Value::Num(s.to_string()))
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        if self.i + 4 > self.b.len() {
            return Err(self.err("escape \\u truncado"));
        }
        let mut v = 0u32;
        for k in 0..4 {
            let d = (self.b[self.i + k] as char).to_digit(16).ok_or_else(|| self.err("escape \\u invalido"))?;
            v = v * 16 + d;
        }
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.i += 1; // comilla de apertura
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = self.peek().ok_or_else(|| self.err("string sin cerrar"))?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = self.peek().ok_or_else(|| self.err("escape truncado"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            // par sustituto
                            if (0xD800..0xDC00).contains(&cp) && self.b[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                } else {
                                    return Err(self.err("par sustituto invalido"));
                                }
                            }
                            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err(self.err("escape desconocido")),
                    }
                }
                _ => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|_| self.err("utf8 invalido en string"))
    }

    fn array(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Value::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Arr(items));
                }
                _ => return Err(self.err("se esperaba , o ]")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.i += 1;
        let mut kv = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Obj(kv));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("se esperaba clave"));
            }
            let k = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.err("se esperaba :"));
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth + 1)?;
            kv.push((k, v));
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Obj(kv));
                }
                _ => return Err(self.err("se esperaba , o }")),
            }
        }
    }
}

/// Escapa un string para incrustarlo en JSON (para mensajes salientes).
pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exchange_like_message() {
        let v = parse(br#"{"channel":"l2Book","data":{"coin":"BTC","time":1700000000123,"levels":[[{"px":"67000.5","sz":"1.25","n":3}],[]]}}"#).unwrap();
        assert_eq!(v.get("channel").unwrap().as_str(), Some("l2Book"));
        let d = v.get("data").unwrap();
        assert_eq!(d.get("time").unwrap().as_u64(), Some(1_700_000_000_123));
        let levels = d.get("levels").unwrap().as_arr().unwrap();
        assert_eq!(levels.len(), 2);
        let b0 = &levels[0].as_arr().unwrap()[0];
        assert_eq!(b0.get("px").unwrap().as_num_str(), Some("67000.5"));
        assert_eq!(b0.get("n").unwrap().as_u64(), Some(3));
    }

    #[test]
    fn scalars_and_escapes() {
        assert_eq!(parse(b" null ").unwrap(), Value::Null);
        assert_eq!(parse(b"true").unwrap(), Value::Bool(true));
        assert_eq!(parse(b"-1.5e3").unwrap(), Value::Num("-1.5e3".into()));
        assert_eq!(
            parse(br#""a\n\"b\" \u00e9 \ud83d\ude00""#).unwrap(),
            Value::Str("a\n\"b\" \u{e9} \u{1F600}".into())
        );
        assert_eq!(parse(b"[]").unwrap(), Value::Arr(vec![]));
        assert_eq!(parse(b"{}").unwrap(), Value::Obj(vec![]));
    }

    #[test]
    fn rejects_malformed() {
        for s in [&b"{"[..], b"[1,]", b"{\"a\"}", b"\"abc", b"tru", b"1 2", b"", b"{\"a\":1,}"] {
            assert!(parse(s).is_err(), "{:?}", std::str::from_utf8(s));
        }
        let deep = "[".repeat(200);
        assert!(parse(deep.as_bytes()).is_err());
    }

    #[test]
    fn escape_roundtrip() {
        let s = "a\"b\\c\n\u{1}";
        let j = format!("\"{}\"", escape(s));
        assert_eq!(parse(j.as_bytes()).unwrap(), Value::Str(s.into()));
    }
}
