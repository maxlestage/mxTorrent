//! Petits outils : hexadécimal, base32, encodage d'URL, SHA-1, aléatoire.

use std::fmt;

/// Empreinte SHA-1 du dictionnaire `info` : l'identifiant d'un torrent.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct InfoHash(pub [u8; 20]);

impl InfoHash {
    pub fn of(bytes: &[u8]) -> InfoHash {
        InfoHash(sha1(bytes))
    }

    pub fn to_hex(&self) -> String {
        to_hex(&self.0)
    }

    /// Accepte 40 caractères hexadécimaux ou 32 caractères base32.
    pub fn parse(s: &str) -> Option<InfoHash> {
        let s = s.trim();
        let bytes = match s.len() {
            40 => from_hex(s)?,
            32 => base32_decode(s)?,
            _ => return None,
        };
        bytes.try_into().ok().map(InfoHash)
    }
}

impl fmt::Debug for InfoHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InfoHash({})", self.to_hex())
    }
}

impl fmt::Display for InfoHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

pub fn sha1(bytes: &[u8]) -> [u8; 20] {
    sha1_smol::Sha1::from(bytes).digest().bytes()
}

pub fn to_hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let v = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    s.as_bytes().chunks(2).map(|p| Some(v(p[0])? << 4 | v(p[1])?)).collect()
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc: u64 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a',
            b'2'..=b'7' => c - b'2' + 26,
            b'=' => continue,
            _ => return None,
        };
        acc = (acc << 5) | v as u64;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Décode `%XX` et `+` (composants de liens magnet).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hi = (b[i + 1] as char).to_digit(16);
                let lo = (b[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                        continue;
                    }
                    _ => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Générateur pseudo-aléatoire minuscule (xorshift64*), semé par l'appelant :
/// le cœur n'a pas besoin d'aléa cryptographique, seulement de variété.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1) ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }

    pub fn fill(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(8) {
            let v = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
}

/// Identifiant de pair au format Azureus : `-MX0100-` suivi de 12 caractères.
pub fn make_peer_id(rng: &mut Rng) -> [u8; 20] {
    const ALPHA: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut id = [0u8; 20];
    id[..8].copy_from_slice(b"-MX0100-");
    for b in &mut id[8..] {
        *b = ALPHA[rng.below(ALPHA.len())];
    }
    id
}

/// Nom lisible du logiciel d'un pair, déduit de son identifiant.
pub fn client_name(peer_id: &[u8; 20]) -> String {
    if peer_id[0] == b'-' && peer_id[7] == b'-' {
        let code = std::str::from_utf8(&peer_id[1..3]).unwrap_or("??");
        let v = &peer_id[3..7];
        let name = match code {
            "MX" => "mxTorrent",
            "WW" => "WebTorrent",
            "WD" => "WebTorrent Desktop",
            "qB" => "qBittorrent",
            "TR" => "Transmission",
            "UT" => "µTorrent",
            "DE" => "Deluge",
            "LT" => "libtorrent",
            "lt" => "rTorrent",
            "BT" => "BitTorrent",
            "AZ" => "Vuze",
            "BC" => "BitComet",
            "FD" => "Free Download Manager",
            other => return format!("{other} {}", String::from_utf8_lossy(v)),
        };
        let digits: Vec<String> = v.iter().map(|c| (*c as char).to_digit(36).map(|d| d.to_string()).unwrap_or_default()).collect();
        return format!("{name} {}", digits.join("."));
    }
    "Inconnu".to_string()
}

/// Les trackers WebTorrent transportent les identifiants binaires de 20 octets
/// dans des chaînes JSON, un caractère par octet (latin-1).
pub fn binary_to_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

pub fn latin1_to_binary(s: &str) -> Option<Vec<u8>> {
    s.chars().map(|c| u8::try_from(c as u32).ok()).collect()
}

/// Formate une taille en octets (unités binaires).
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["o", "Kio", "Mio", "Gio", "Tio", "Pio"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} o")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_et_base32() {
        let h = InfoHash::parse("c9e15763f722f23e98a29decdfae341b98d53056").unwrap();
        assert_eq!(h.to_hex(), "c9e15763f722f23e98a29decdfae341b98d53056");
        let b = InfoHash::parse("ZHQVOY7XELZD5GFCTXWN7LRUDOMNKMCW").unwrap();
        assert_eq!(b, h);
        assert!(InfoHash::parse("zz").is_none());
    }

    #[test]
    fn sha1_connu() {
        assert_eq!(to_hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn url() {
        assert_eq!(percent_decode("Big+Buck%20Bunny%2"), "Big Buck Bunny%2");
        assert_eq!(percent_decode("wss%3A%2F%2Ft.io"), "wss://t.io");
        assert_eq!(percent_encode("a b/é"), "a%20b%2F%C3%A9");
    }

    #[test]
    fn latin1() {
        let b: Vec<u8> = (0..=255).collect();
        assert_eq!(latin1_to_binary(&binary_to_latin1(&b)).unwrap(), b);
    }

    #[test]
    fn noms_de_clients() {
        assert_eq!(client_name(b"-WW0208-abcdefghijkl"), "WebTorrent 0.2.0.8");
        assert_eq!(client_name(b"-MX0100-abcdefghijkl"), "mxTorrent 0.1.0.0");
        assert_eq!(human_bytes(1536), "1.5 Kio");
    }
}
