//! Le protocole de la passerelle TCP/UDP.
//!
//! Un navigateur ne peut ouvrir ni connexion TCP ni socket UDP : la passerelle
//! (mxtorrent-tracker) le fait pour lui. Elle interroge les trackers UDP et
//! HTTP et la DHT, se connecte en TCP aux pairs « classiques » (qBittorrent,
//! Transmission…), et relaie les octets. Entre le navigateur et elle, tout
//! passe dans une session Brocolis 🥦 v4 ; ce module ne décrit que les trames
//! qu'elle transporte.
//!
//! Un message Brocolis contient un **lot** de trames, chacune
//! `type (u8) ‖ longueur (u32, gros-boutiste) ‖ contenu`.

use crate::util::InfoHash;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Chercher des pairs : `req ‖ info_hash ‖ drapeaux ‖ n ‖ (len u16 ‖ url)*`.
pub const LOOKUP: u8 = 1;
/// Se connecter à un pair trouvé : `conn ‖ info_hash ‖ adresse`.
pub const CONNECT: u8 = 2;
/// Octets pour un pair : `conn ‖ données`.
pub const DATA: u8 = 3;
/// Fermer : `conn`.
pub const CLOSE: u8 = 4;
/// Des pairs trouvés : `req ‖ source ‖ n ‖ adresse*`.
pub const PEERS: u8 = 17;
/// Connexion établie : `conn ‖ chiffrement` (0 en clair, 1 MSE/RC4).
pub const CONNECTED: u8 = 18;
/// Connexion impossible : `conn ‖ raison`.
pub const FAILED: u8 = 19;
/// Octets d'un pair : `conn ‖ données`.
pub const DATA_IN: u8 = 20;
/// Le pair a fermé : `conn`.
pub const CLOSED: u8 = 21;
/// Fin d'une recherche : `req ‖ nombre de pairs ‖ résumé`.
pub const LOOKUP_DONE: u8 = 22;

/// Recherche aussi dans la DHT (interdit pour un torrent privé).
pub const FLAG_DHT: u8 = 1;

/// Taille maximale d'un lot (avant chiffrement) : bien sous la limite Brocolis
/// de 200 Kio une fois encodé en base64.
pub const MAX_BATCH: usize = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Lookup { req: u32, info_hash: InfoHash, flags: u8, trackers: Vec<String> },
    Connect { conn: u32, info_hash: InfoHash, addr: SocketAddr },
    Data { conn: u32, data: Vec<u8> },
    Close { conn: u32 },
    Peers { req: u32, source: u8, peers: Vec<SocketAddr> },
    Connected { conn: u32, encrypted: bool },
    Failed { conn: u32, reason: String },
    DataIn { conn: u32, data: Vec<u8> },
    Closed { conn: u32 },
    LookupDone { req: u32, found: u32, summary: String },
}

/// Source des pairs d'une trame [`Frame::Peers`].
pub const SOURCE_TRACKER: u8 = 0;
pub const SOURCE_DHT: u8 = 1;

fn put_addr(out: &mut Vec<u8>, a: &SocketAddr) {
    match a.ip() {
        IpAddr::V4(ip) => {
            out.push(4);
            out.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            out.push(6);
            out.extend_from_slice(&ip.octets());
        }
    }
    out.extend_from_slice(&a.port().to_be_bytes());
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_be_bytes(s.try_into().unwrap()))
    }
    fn hash(&mut self) -> Option<InfoHash> {
        self.take(20).map(|s| InfoHash(s.try_into().unwrap()))
    }
    fn addr(&mut self) -> Option<SocketAddr> {
        let ip = match self.u8()? {
            4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(self.take(4)?).ok()?)),
            6 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(self.take(16)?).ok()?)),
            _ => return None,
        };
        Some(SocketAddr::new(ip, self.u16()?))
    }
    fn rest(&mut self) -> &'a [u8] {
        let s = &self.b[self.i..];
        self.i = self.b.len();
        s
    }
}

impl Frame {
    pub fn kind(&self) -> u8 {
        match self {
            Frame::Lookup { .. } => LOOKUP,
            Frame::Connect { .. } => CONNECT,
            Frame::Data { .. } => DATA,
            Frame::Close { .. } => CLOSE,
            Frame::Peers { .. } => PEERS,
            Frame::Connected { .. } => CONNECTED,
            Frame::Failed { .. } => FAILED,
            Frame::DataIn { .. } => DATA_IN,
            Frame::Closed { .. } => CLOSED,
            Frame::LookupDone { .. } => LOOKUP_DONE,
        }
    }

    /// Ajoute la trame, encodée, à un lot.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        let mut p = Vec::new();
        match self {
            Frame::Lookup { req, info_hash, flags, trackers } => {
                p.extend_from_slice(&req.to_be_bytes());
                p.extend_from_slice(&info_hash.0);
                p.push(*flags);
                let list: Vec<&String> = trackers.iter().filter(|t| t.len() <= 1024).take(255).collect();
                p.push(list.len() as u8);
                for t in list {
                    p.extend_from_slice(&(t.len() as u16).to_be_bytes());
                    p.extend_from_slice(t.as_bytes());
                }
            }
            Frame::Connect { conn, info_hash, addr } => {
                p.extend_from_slice(&conn.to_be_bytes());
                p.extend_from_slice(&info_hash.0);
                put_addr(&mut p, addr);
            }
            Frame::Data { conn, data } | Frame::DataIn { conn, data } => {
                p.extend_from_slice(&conn.to_be_bytes());
                p.extend_from_slice(data);
            }
            Frame::Close { conn } | Frame::Closed { conn } => p.extend_from_slice(&conn.to_be_bytes()),
            Frame::Peers { req, source, peers } => {
                p.extend_from_slice(&req.to_be_bytes());
                p.push(*source);
                let n = peers.len().min(u16::MAX as usize);
                p.extend_from_slice(&(n as u16).to_be_bytes());
                for a in &peers[..n] {
                    put_addr(&mut p, a);
                }
            }
            Frame::Connected { conn, encrypted } => {
                p.extend_from_slice(&conn.to_be_bytes());
                p.push(*encrypted as u8);
            }
            Frame::Failed { conn, reason } => {
                p.extend_from_slice(&conn.to_be_bytes());
                p.extend_from_slice(reason.as_bytes());
            }
            Frame::LookupDone { req, found, summary } => {
                p.extend_from_slice(&req.to_be_bytes());
                p.extend_from_slice(&found.to_be_bytes());
                p.extend_from_slice(summary.as_bytes());
            }
        }
        out.push(self.kind());
        out.extend_from_slice(&(p.len() as u32).to_be_bytes());
        out.extend_from_slice(&p);
    }

    /// Taille encodée (pour remplir les lots sans les dépasser).
    pub fn encoded_len(&self) -> usize {
        5 + match self {
            Frame::Data { data, .. } | Frame::DataIn { data, .. } => 4 + data.len(),
            Frame::Peers { peers, .. } => 7 + peers.len() * 19,
            _ => 64,
        }
    }

    fn decode_one(kind: u8, body: &[u8]) -> Option<Frame> {
        let mut r = Reader { b: body, i: 0 };
        let f = match kind {
            LOOKUP => {
                let req = r.u32()?;
                let info_hash = r.hash()?;
                let flags = r.u8()?;
                let n = r.u8()?;
                let mut trackers = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let len = r.u16()? as usize;
                    trackers.push(String::from_utf8(r.take(len)?.to_vec()).ok()?);
                }
                Frame::Lookup { req, info_hash, flags, trackers }
            }
            CONNECT => Frame::Connect { conn: r.u32()?, info_hash: r.hash()?, addr: r.addr()? },
            DATA => Frame::Data { conn: r.u32()?, data: r.rest().to_vec() },
            CLOSE => Frame::Close { conn: r.u32()? },
            PEERS => {
                let req = r.u32()?;
                let source = r.u8()?;
                let n = r.u16()?;
                let mut peers = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    peers.push(r.addr()?);
                }
                Frame::Peers { req, source, peers }
            }
            CONNECTED => Frame::Connected { conn: r.u32()?, encrypted: r.u8()? != 0 },
            FAILED => Frame::Failed { conn: r.u32()?, reason: String::from_utf8_lossy(r.rest()).into_owned() },
            DATA_IN => Frame::DataIn { conn: r.u32()?, data: r.rest().to_vec() },
            CLOSED => Frame::Closed { conn: r.u32()? },
            LOOKUP_DONE => Frame::LookupDone { req: r.u32()?, found: r.u32()?, summary: String::from_utf8_lossy(r.rest()).into_owned() },
            _ => return None,
        };
        Some(f)
    }

    /// Décode un lot entier ; `None` s'il est mal formé.
    pub fn decode_batch(b: &[u8]) -> Option<Vec<Frame>> {
        let mut out = Vec::new();
        let mut r = Reader { b, i: 0 };
        while r.i < b.len() {
            let kind = r.u8()?;
            let len = r.u32()? as usize;
            out.push(Frame::decode_one(kind, r.take(len)?)?);
        }
        Some(out)
    }
}

/// Une adresse à laquelle la passerelle ne doit jamais se connecter : réseau
/// local, boucle, adresses réservées (sinon elle servirait à sonder le réseau
/// interne de l'hébergeur).
pub fn is_private(addr: &SocketAddr) -> bool {
    if addr.port() == 0 {
        return true;
    }
    match addr.ip() {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || ip.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10
                || o[0] >= 240
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // fc00::/7
                || (s[0] & 0xffc0) == 0xfe80 // fe80::/10
                || ip.to_ipv4_mapped().is_some_and(|v4| is_private(&SocketAddr::new(IpAddr::V4(v4), addr.port())))
        }
    }
}

/// Pairs au format compact des trackers (6 octets IPv4, ou 18 octets IPv6).
pub fn parse_compact(b: &[u8], v6: bool) -> Vec<SocketAddr> {
    let size = if v6 { 18 } else { 6 };
    b.chunks_exact(size)
        .map(|c| {
            let ip = if v6 {
                IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&c[..16]).unwrap()))
            } else {
                IpAddr::V4(Ipv4Addr::new(c[0], c[1], c[2], c[3]))
            };
            SocketAddr::new(ip, u16::from_be_bytes([c[size - 2], c[size - 1]]))
        })
        .filter(|a| a.port() != 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lots_aller_retour() {
        let frames = vec![
            Frame::Lookup {
                req: 7,
                info_hash: InfoHash([1; 20]),
                flags: FLAG_DHT,
                trackers: vec!["udp://t.example:6969".into(), "http://x/announce".into()],
            },
            Frame::Connect { conn: 3, info_hash: InfoHash([2; 20]), addr: "1.2.3.4:51413".parse().unwrap() },
            Frame::Data { conn: 3, data: vec![9; 1000] },
            Frame::Close { conn: 3 },
            Frame::Peers {
                req: 7,
                source: SOURCE_DHT,
                peers: vec!["5.6.7.8:6881".parse().unwrap(), "[2001:db8::1]:6881".parse().unwrap()],
            },
            Frame::Connected { conn: 3, encrypted: true },
            Frame::Failed { conn: 4, reason: "délai dépassé".into() },
            Frame::DataIn { conn: 3, data: vec![] },
            Frame::Closed { conn: 3 },
            Frame::LookupDone { req: 7, found: 2, summary: "ok".into() },
        ];
        let mut batch = Vec::new();
        for f in &frames {
            f.encode_into(&mut batch);
        }
        assert_eq!(Frame::decode_batch(&batch).unwrap(), frames);
        assert!(Frame::decode_batch(&batch[..batch.len() - 1]).is_none());
        assert!(Frame::decode_batch(&[99, 0, 0, 0, 0]).is_none());
    }

    #[test]
    fn adresses_privees_refusees() {
        for a in [
            "127.0.0.1:1",
            "10.0.0.1:1",
            "192.168.1.1:1",
            "172.16.0.1:1",
            "169.254.1.1:1",
            "100.64.0.1:1",
            "0.0.0.0:1",
            "[::1]:1",
            "[fd00::1]:1",
            "[::ffff:10.0.0.1]:1",
            "8.8.8.8:0",
        ] {
            assert!(is_private(&a.parse().unwrap()), "{a}");
        }
        for a in ["8.8.8.8:6881", "[2001:4860::8888]:6881"] {
            assert!(!is_private(&a.parse().unwrap()), "{a}");
        }
    }

    #[test]
    fn pairs_compacts() {
        let b = [1, 2, 3, 4, 0x1a, 0xe1, 5, 6, 7, 8, 0, 0];
        assert_eq!(parse_compact(&b, false), vec!["1.2.3.4:6881".parse::<SocketAddr>().unwrap()]);
    }
}
