//! Protocole des pairs : poignée de main, messages (BEP 3) et extensions (BEP 10).

use crate::util::InfoHash;

pub const PROTOCOL: &[u8; 19] = b"BitTorrent protocol";
pub const HANDSHAKE_LEN: usize = 68;
pub const BLOCK_SIZE: u32 = 16 * 1024;
/// Taille maximale acceptée pour un message (une pièce de 16 Kio et son en-tête, avec de la marge).
pub const MAX_MESSAGE: usize = 1024 * 1024 + 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handshake {
    pub reserved: [u8; 8],
    pub info_hash: InfoHash,
    pub peer_id: [u8; 20],
}

impl Handshake {
    pub fn new(info_hash: InfoHash, peer_id: [u8; 20]) -> Handshake {
        let mut reserved = [0u8; 8];
        reserved[5] |= 0x10; // BEP 10 : protocole d'extension
        Handshake { reserved, info_hash, peer_id }
    }

    pub fn supports_extensions(&self) -> bool {
        self.reserved[5] & 0x10 != 0
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HANDSHAKE_LEN);
        out.push(19);
        out.extend_from_slice(PROTOCOL);
        out.extend_from_slice(&self.reserved);
        out.extend_from_slice(&self.info_hash.0);
        out.extend_from_slice(&self.peer_id);
        out
    }

    pub fn decode(b: &[u8]) -> Result<Handshake, WireError> {
        if b.len() < HANDSHAKE_LEN {
            return Err(WireError("poignée de main tronquée"));
        }
        if b[0] != 19 || &b[1..20] != PROTOCOL {
            return Err(WireError("ce pair ne parle pas BitTorrent"));
        }
        Ok(Handshake {
            reserved: b[20..28].try_into().unwrap(),
            info_hash: InfoHash(b[28..48].try_into().unwrap()),
            peer_id: b[48..68].try_into().unwrap(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Vec<u8>),
    Request { index: u32, begin: u32, length: u32 },
    Piece { index: u32, begin: u32, data: Vec<u8> },
    Cancel { index: u32, begin: u32, length: u32 },
    Port(u16),
    Extended { id: u8, payload: Vec<u8> },
    Unknown { id: u8, payload: Vec<u8> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireError(pub &'static str);

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes(b[i..i + 4].try_into().unwrap())
}

impl Message {
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        let mut frame = |id: Option<u8>, parts: &[&[u8]]| {
            let len: usize = id.is_some() as usize + parts.iter().map(|p| p.len()).sum::<usize>();
            out.extend_from_slice(&(len as u32).to_be_bytes());
            if let Some(id) = id {
                out.push(id);
            }
            for p in parts {
                out.extend_from_slice(p);
            }
        };
        match self {
            Message::KeepAlive => frame(None, &[]),
            Message::Choke => frame(Some(0), &[]),
            Message::Unchoke => frame(Some(1), &[]),
            Message::Interested => frame(Some(2), &[]),
            Message::NotInterested => frame(Some(3), &[]),
            Message::Have(i) => frame(Some(4), &[&i.to_be_bytes()]),
            Message::Bitfield(b) => frame(Some(5), &[b]),
            Message::Request { index, begin, length } => {
                frame(Some(6), &[&index.to_be_bytes(), &begin.to_be_bytes(), &length.to_be_bytes()])
            }
            Message::Piece { index, begin, data } => frame(Some(7), &[&index.to_be_bytes(), &begin.to_be_bytes(), data]),
            Message::Cancel { index, begin, length } => {
                frame(Some(8), &[&index.to_be_bytes(), &begin.to_be_bytes(), &length.to_be_bytes()])
            }
            Message::Port(p) => frame(Some(9), &[&p.to_be_bytes()]),
            Message::Extended { id, payload } => frame(Some(20), &[&[*id], payload]),
            Message::Unknown { id, payload } => frame(Some(*id), &[payload]),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::new();
        self.encode_into(&mut v);
        v
    }

    /// Décode le corps d'un message (sans le préfixe de longueur).
    pub fn decode(body: &[u8]) -> Result<Message, WireError> {
        if body.is_empty() {
            return Ok(Message::KeepAlive);
        }
        let p = &body[1..];
        let need = |n: usize| if p.len() == n { Ok(()) } else { Err(WireError("message de taille incorrecte")) };
        Ok(match body[0] {
            0 => {
                need(0)?;
                Message::Choke
            }
            1 => {
                need(0)?;
                Message::Unchoke
            }
            2 => {
                need(0)?;
                Message::Interested
            }
            3 => {
                need(0)?;
                Message::NotInterested
            }
            4 => {
                need(4)?;
                Message::Have(u32_at(p, 0))
            }
            5 => Message::Bitfield(p.to_vec()),
            6 => {
                need(12)?;
                Message::Request { index: u32_at(p, 0), begin: u32_at(p, 4), length: u32_at(p, 8) }
            }
            7 => {
                if p.len() < 8 {
                    return Err(WireError("bloc tronqué"));
                }
                Message::Piece { index: u32_at(p, 0), begin: u32_at(p, 4), data: p[8..].to_vec() }
            }
            8 => {
                need(12)?;
                Message::Cancel { index: u32_at(p, 0), begin: u32_at(p, 4), length: u32_at(p, 8) }
            }
            9 => {
                need(2)?;
                Message::Port(u16::from_be_bytes([p[0], p[1]]))
            }
            20 => {
                if p.is_empty() {
                    return Err(WireError("message d'extension vide"));
                }
                Message::Extended { id: p[0], payload: p[1..].to_vec() }
            }
            id => Message::Unknown { id, payload: p.to_vec() },
        })
    }
}

/// Reconstitue les messages à partir d'un flux d'octets découpé n'importe comment
/// (les canaux WebRTC livrent des morceaux arbitraires).
#[derive(Default, Debug)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    start: usize,
    handshake_done: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    Handshake(Handshake),
    Message(Message),
}

impl FrameDecoder {
    pub fn push(&mut self, data: &[u8]) {
        if self.start > 0 && self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        }
        self.buf.extend_from_slice(data);
    }

    pub fn next_frame(&mut self) -> Result<Option<Frame>, WireError> {
        let avail = &self.buf[self.start..];
        if !self.handshake_done {
            if avail.len() < HANDSHAKE_LEN {
                if !avail.is_empty() && avail[0] != 19 {
                    return Err(WireError("ce pair ne parle pas BitTorrent"));
                }
                return Ok(None);
            }
            let h = Handshake::decode(avail)?;
            self.start += HANDSHAKE_LEN;
            self.handshake_done = true;
            return Ok(Some(Frame::Handshake(h)));
        }
        if avail.len() < 4 {
            self.compact();
            return Ok(None);
        }
        let len = u32_at(avail, 0) as usize;
        if len > MAX_MESSAGE {
            return Err(WireError("message trop grand"));
        }
        if avail.len() < 4 + len {
            self.compact();
            return Ok(None);
        }
        let msg = Message::decode(&avail[4..4 + len])?;
        self.start += 4 + len;
        Ok(Some(Frame::Message(msg)))
    }

    fn compact(&mut self) {
        if self.start > 64 * 1024 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
    }

    pub fn buffered(&self) -> usize {
        self.buf.len() - self.start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_aller_retour() {
        let msgs = vec![
            Message::KeepAlive,
            Message::Choke,
            Message::Unchoke,
            Message::Interested,
            Message::NotInterested,
            Message::Have(7),
            Message::Bitfield(vec![0b1010_0000]),
            Message::Request { index: 1, begin: 16384, length: 16384 },
            Message::Piece { index: 1, begin: 0, data: vec![9; 100] },
            Message::Cancel { index: 1, begin: 16384, length: 16384 },
            Message::Port(6881),
            Message::Extended { id: 0, payload: b"de".to_vec() },
        ];
        let h = Handshake::new(InfoHash([3; 20]), *b"-MX0100-abcdefghijkl");
        let mut stream = h.encode();
        for m in &msgs {
            m.encode_into(&mut stream);
        }
        // Livré octet par octet : le découpage ne doit rien changer.
        let mut d = FrameDecoder::default();
        let mut got = Vec::new();
        for b in stream {
            d.push(&[b]);
            while let Some(f) = d.next_frame().unwrap() {
                got.push(f);
            }
        }
        assert_eq!(got[0], Frame::Handshake(h));
        assert!(h.supports_extensions());
        let rest: Vec<Message> = got[1..]
            .iter()
            .map(|f| match f {
                Frame::Message(m) => m.clone(),
                _ => panic!(),
            })
            .collect();
        assert_eq!(rest, msgs);
        assert_eq!(d.buffered(), 0);
    }

    #[test]
    fn refuse_le_bruit() {
        let mut d = FrameDecoder::default();
        d.push(b"GET / HTTP/1.1\r\n");
        assert!(d.next_frame().is_err());
        let mut d = FrameDecoder::default();
        let mut s = Handshake::new(InfoHash([0; 20]), [0; 20]).encode();
        s.extend_from_slice(&(10_000_000u32).to_be_bytes());
        d.push(&s);
        assert!(matches!(d.next_frame(), Ok(Some(Frame::Handshake(_)))));
        assert!(d.next_frame().is_err());
    }
}
