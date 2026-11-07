//! Bencode : l'encodage des fichiers `.torrent` et des messages d'extension (BEP 3).

use std::collections::BTreeMap;
use std::fmt;

/// Une valeur bencodée.
#[derive(Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(i) => write!(f, "{i}"),
            Value::Bytes(b) => match std::str::from_utf8(b) {
                Ok(s) if b.len() < 200 => write!(f, "{s:?}"),
                _ => write!(f, "<{} octets>", b.len()),
            },
            Value::List(l) => f.debug_list().entries(l).finish(),
            Value::Dict(d) => f.debug_map().entries(d.iter().map(|(k, v)| (String::from_utf8_lossy(k), v))).finish(),
        }
    }
}

/// Erreur de décodage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    pub position: usize,
    pub message: &'static str,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bencode invalide à l'octet {} : {}", self.position, self.message)
    }
}

impl std::error::Error for DecodeError {}

const MAX_DEPTH: usize = 64;

struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    fn err(&self, message: &'static str) -> DecodeError {
        DecodeError { position: self.pos, message }
    }

    fn peek(&self) -> Result<u8, DecodeError> {
        self.buf.get(self.pos).copied().ok_or_else(|| self.err("fin inattendue"))
    }

    fn integer_until(&mut self, end: u8) -> Result<i64, DecodeError> {
        let start = self.pos;
        let rel = self.buf[start..].iter().position(|&b| b == end).ok_or_else(|| self.err("entier non terminé"))?;
        let digits = &self.buf[start..start + rel];
        let s = std::str::from_utf8(digits).map_err(|_| self.err("entier invalide"))?;
        if s.is_empty() || s == "-" || s == "-0" || (s.len() > 1 && s.starts_with('0')) || (s.len() > 2 && s.starts_with("-0")) {
            return Err(self.err("entier mal formé"));
        }
        let v = s.parse::<i64>().map_err(|_| self.err("entier hors limites"))?;
        self.pos = start + rel + 1;
        Ok(v)
    }

    fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let len = self.integer_until(b':')?;
        if len < 0 {
            return Err(self.err("longueur négative"));
        }
        let len = len as usize;
        let end = self.pos.checked_add(len).ok_or_else(|| self.err("longueur trop grande"))?;
        if end > self.buf.len() {
            return Err(self.err("chaîne tronquée"));
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn value(&mut self, depth: usize) -> Result<Value, DecodeError> {
        if depth > MAX_DEPTH {
            return Err(self.err("imbrication trop profonde"));
        }
        match self.peek()? {
            b'i' => {
                self.pos += 1;
                Ok(Value::Int(self.integer_until(b'e')?))
            }
            b'l' => {
                self.pos += 1;
                let mut list = Vec::new();
                while self.peek()? != b'e' {
                    list.push(self.value(depth + 1)?);
                }
                self.pos += 1;
                Ok(Value::List(list))
            }
            b'd' => {
                self.pos += 1;
                let mut dict = BTreeMap::new();
                while self.peek()? != b'e' {
                    let key = self.bytes()?.to_vec();
                    let value = self.value(depth + 1)?;
                    dict.insert(key, value);
                }
                self.pos += 1;
                Ok(Value::Dict(dict))
            }
            b'0'..=b'9' => Ok(Value::Bytes(self.bytes()?.to_vec())),
            _ => Err(self.err("type inconnu")),
        }
    }
}

/// Décode une valeur qui occupe tout le tampon.
pub fn decode(buf: &[u8]) -> Result<Value, DecodeError> {
    let (v, used) = decode_prefix(buf)?;
    if used != buf.len() {
        return Err(DecodeError { position: used, message: "données après la valeur" });
    }
    Ok(v)
}

/// Décode la première valeur du tampon et renvoie le nombre d'octets lus
/// (les messages `ut_metadata` sont un dictionnaire suivi de données brutes).
pub fn decode_prefix(buf: &[u8]) -> Result<(Value, usize), DecodeError> {
    let mut d = Decoder { buf, pos: 0 };
    let v = d.value(0)?;
    Ok((v, d.pos))
}

/// Renvoie la position exacte (début, fin) de la valeur associée à `key` dans le
/// dictionnaire de premier niveau : l'empreinte d'un torrent se calcule sur les
/// octets d'origine du dictionnaire `info`.
pub fn dict_value_span(buf: &[u8], key: &[u8]) -> Result<Option<(usize, usize)>, DecodeError> {
    let mut d = Decoder { buf, pos: 0 };
    if d.peek()? != b'd' {
        return Err(d.err("dictionnaire attendu"));
    }
    d.pos += 1;
    while d.peek()? != b'e' {
        let k = d.bytes()?;
        let start = d.pos;
        d.value(1)?;
        if k == key {
            return Ok(Some((start, d.pos)));
        }
    }
    Ok(None)
}

/// Encode une valeur.
pub fn encode(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(v, &mut out);
    out
}

fn encode_into(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Int(i) => {
            out.push(b'i');
            out.extend_from_slice(i.to_string().as_bytes());
            out.push(b'e');
        }
        Value::Bytes(b) => {
            out.extend_from_slice(b.len().to_string().as_bytes());
            out.push(b':');
            out.extend_from_slice(b);
        }
        Value::List(l) => {
            out.push(b'l');
            for item in l {
                encode_into(item, out);
            }
            out.push(b'e');
        }
        Value::Dict(d) => {
            out.push(b'd');
            for (k, item) in d {
                out.extend_from_slice(k.len().to_string().as_bytes());
                out.push(b':');
                out.extend_from_slice(k);
                encode_into(item, out);
            }
            out.push(b'e');
        }
    }
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Bytes(s.as_bytes().to_vec())
    }

    pub fn dict() -> Value {
        Value::Dict(BTreeMap::new())
    }

    /// Ajoute une entrée à un dictionnaire (ne fait rien sur un autre type).
    pub fn with(mut self, key: &str, value: Value) -> Value {
        if let Value::Dict(d) = &mut self {
            d.insert(key.as_bytes().to_vec(), value);
        }
        self
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.get(key.as_bytes()),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| std::str::from_utf8(b).ok())
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aller_retour() {
        let v = Value::dict()
            .with("a", Value::Int(-42))
            .with("b", Value::List(vec![Value::str("spam"), Value::Int(0)]))
            .with("c", Value::Bytes(vec![0, 255, 1]));
        let enc = encode(&v);
        assert_eq!(enc, b"d1:ai-42e1:bl4:spami0ee1:c3:\x00\xff\x01e");
        assert_eq!(decode(&enc).unwrap(), v);
    }

    #[test]
    fn refuse_les_entiers_mal_formes() {
        for bad in [&b"i-0e"[..], b"i03e", b"ie", b"i-e", b"i12", b"4:abc", b"l", b"x"] {
            assert!(decode(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
        assert_eq!(decode(b"i0e").unwrap(), Value::Int(0));
    }

    #[test]
    fn prefixe_et_position() {
        let buf = b"d8:msg_typei1e5:piecei0eeDONNEES";
        let (v, used) = decode_prefix(buf).unwrap();
        assert_eq!(v.get("msg_type").unwrap().as_int(), Some(1));
        assert_eq!(&buf[used..], b"DONNEES");

        let t = b"d8:announce3:url4:infod4:name1:xee";
        let (s, e) = dict_value_span(t, b"info").unwrap().unwrap();
        assert_eq!(&t[s..e], b"d4:name1:xe");
    }

    #[test]
    fn imbrication_limitee() {
        let deep = vec![b'l'; 10_000];
        assert!(decode(&deep).is_err());
    }
}
