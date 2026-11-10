//! Le chiffrement des connexions BitTorrent classiques : MSE/PE (« Message
//! Stream Encryption »), compris par qBittorrent, Transmission, µTorrent,
//! Deluge… Échange Diffie-Hellman sur 768 bits, puis RC4 dans chaque sens,
//! clés dérivées du secret commun et de l'empreinte du torrent.
//!
//! La passerelle est toujours l'initiateur ; le côté répondeur sert aux tests.

use mxtorrent_core::util::sha1;
use num_bigint::BigUint;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const P_HEX: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A63A36210000000000090563";
const VC: [u8; 8] = [0; 8];
pub const CRYPTO_PLAIN: u32 = 0x01;
pub const CRYPTO_RC4: u32 = 0x02;
const MAX_PAD: usize = 512;
const TIMEOUT: Duration = Duration::from_secs(12);

/// RC4, avec les 1 024 premiers octets du flux jetés comme le veut MSE.
#[derive(Clone)]
pub struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    pub fn new(key: &[u8]) -> Rc4 {
        let mut s = [0u8; 256];
        for (i, v) in s.iter_mut().enumerate() {
            *v = i as u8;
        }
        let mut j: u8 = 0;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }
        Rc4 { s, i: 0, j: 0 }
    }

    pub fn mse(key: &[u8]) -> Rc4 {
        let mut r = Rc4::new(key);
        r.apply(&mut [0u8; 1024]);
        r
    }

    pub fn apply(&mut self, data: &mut [u8]) {
        for b in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[self.i as usize]);
            self.s.swap(self.i as usize, self.j as usize);
            let k = self.s[(self.s[self.i as usize].wrapping_add(self.s[self.j as usize])) as usize];
            *b ^= k;
        }
    }
}

fn prime() -> BigUint {
    BigUint::parse_bytes(P_HEX.as_bytes(), 16).unwrap()
}

fn to96(n: &BigUint) -> [u8; 96] {
    let b = n.to_bytes_be();
    let mut out = [0u8; 96];
    out[96 - b.len()..].copy_from_slice(&b);
    out
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("hasard du système");
    b
}

fn random_pad() -> Vec<u8> {
    let n = (u16::from_be_bytes(random::<2>()) as usize) % (MAX_PAD + 1);
    let mut v = vec![0u8; n];
    getrandom::fill(&mut v).expect("hasard du système");
    v
}

fn keypair() -> (BigUint, [u8; 96]) {
    let x = BigUint::from_bytes_be(&random::<20>());
    let y = BigUint::from(2u32).modpow(&x, &prime());
    (x, to96(&y))
}

fn secret(x: &BigUint, their: &[u8]) -> Result<[u8; 96], String> {
    let y = BigUint::from_bytes_be(their);
    let p = prime();
    if y <= BigUint::from(1u32) || y >= &p - 1u32 {
        return Err("clé publique MSE invalide".into());
    }
    Ok(to96(&y.modpow(x, &p)))
}

fn h(parts: &[&[u8]]) -> [u8; 20] {
    sha1(&parts.concat())
}

/// Le résultat d'une poignée de main : les chiffreurs (absents en clair) et
/// les octets du pair déjà reçus et déchiffrés.
pub struct Established {
    pub encrypt: Option<Rc4>,
    pub decrypt: Option<Rc4>,
    pub leftover: Vec<u8>,
}

async fn read_more<S: AsyncRead + Unpin>(s: &mut S, buf: &mut Vec<u8>) -> Result<(), String> {
    let mut tmp = [0u8; 4096];
    let n = tokio::time::timeout(TIMEOUT, s.read(&mut tmp)).await.map_err(|_| "délai MSE dépassé")?.map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("connexion fermée pendant la poignée de main MSE".into());
    }
    buf.extend_from_slice(&tmp[..n]);
    Ok(())
}

/// Côté initiateur (la passerelle) : propose RC4 (et le clair si `allow_plain`).
pub async fn initiate<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    info_hash: &[u8; 20],
    allow_plain: bool,
) -> Result<Established, String> {
    let (xa, ya) = keypair();
    s.write_all(&[&ya[..], &random_pad()].concat()).await.map_err(|e| e.to_string())?;

    let mut buf = Vec::new();
    while buf.len() < 96 {
        read_more(s, &mut buf).await?;
    }
    let sec = secret(&xa, &buf[..96])?;
    let mut rest = buf.split_off(96); // PadB, puis le début du message 4

    let key_a = h(&[b"keyA", &sec, info_hash]);
    let key_b = h(&[b"keyB", &sec, info_hash]);
    let mut enc = Rc4::mse(&key_a);
    let mut dec = Rc4::mse(&key_b);

    let req1 = h(&[b"req1", &sec]);
    let req2 = h(&[b"req2", info_hash]);
    let req3 = h(&[b"req3", &sec]);
    let x: Vec<u8> = req2.iter().zip(req3.iter()).map(|(a, b)| a ^ b).collect();
    let provide = if allow_plain { CRYPTO_RC4 | CRYPTO_PLAIN } else { CRYPTO_RC4 };
    let pad_c: Vec<u8> = Vec::new();
    let mut tail = Vec::new();
    tail.extend_from_slice(&VC);
    tail.extend_from_slice(&provide.to_be_bytes());
    tail.extend_from_slice(&(pad_c.len() as u16).to_be_bytes());
    tail.extend_from_slice(&pad_c);
    tail.extend_from_slice(&0u16.to_be_bytes()); // pas de charge initiale
    enc.apply(&mut tail);
    s.write_all(&[&req1[..], &x, &tail].concat()).await.map_err(|e| e.to_string())?;

    // Le message 4 commence après PadB (0 à 512 octets) par VC chiffré.
    let mut probe = dec.clone();
    let mut evc = VC;
    probe.apply(&mut evc);
    let start = loop {
        if let Some(p) = rest.windows(8).position(|w| w == evc) {
            break p;
        }
        if rest.len() > MAX_PAD + 8 {
            return Err("le pair ne parle pas MSE".into());
        }
        read_more(s, &mut rest).await?;
    };
    let mut body = rest.split_off(start + 8);
    let mut vc = rest[start..start + 8].to_vec();
    dec.apply(&mut vc);
    while body.len() < 6 {
        read_more(s, &mut body).await?;
    }
    let mut head = body[..6].to_vec();
    dec.apply(&mut head);
    let select = u32::from_be_bytes(head[..4].try_into().unwrap());
    let pad_d = u16::from_be_bytes([head[4], head[5]]) as usize;
    if pad_d > MAX_PAD {
        return Err("bourrage MSE trop long".into());
    }
    while body.len() < 6 + pad_d {
        read_more(s, &mut body).await?;
    }
    let mut pad = body[6..6 + pad_d].to_vec();
    dec.apply(&mut pad);
    let mut leftover = body.split_off(6 + pad_d);
    match select {
        CRYPTO_RC4 => {
            dec.apply(&mut leftover);
            Ok(Established { encrypt: Some(enc), decrypt: Some(dec), leftover })
        }
        CRYPTO_PLAIN if allow_plain => Ok(Established { encrypt: None, decrypt: None, leftover }),
        _ => Err("méthode de chiffrement refusée".into()),
    }
}

/// Côté répondeur (pour les tests) : accepte RC4 de préférence.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn respond<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S, info_hash: &[u8; 20]) -> Result<Established, String> {
    let mut buf = Vec::new();
    while buf.len() < 96 {
        read_more(s, &mut buf).await?;
    }
    let (xb, yb) = keypair();
    s.write_all(&[&yb[..], &random_pad()].concat()).await.map_err(|e| e.to_string())?;
    let sec = secret(&xb, &buf[..96])?;
    let mut rest = buf.split_off(96);
    let req1 = h(&[b"req1", &sec]);
    let start = loop {
        if let Some(p) = rest.windows(20).position(|w| w == req1) {
            break p;
        }
        if rest.len() > MAX_PAD + 20 {
            return Err("pas de req1".into());
        }
        read_more(s, &mut rest).await?;
    };
    let mut body = rest.split_off(start + 20);
    while body.len() < 20 + 14 {
        read_more(s, &mut body).await?;
    }
    let req2 = h(&[b"req2", info_hash]);
    let req3 = h(&[b"req3", &sec]);
    let x: Vec<u8> = req2.iter().zip(req3.iter()).map(|(a, b)| a ^ b).collect();
    if body[..20] != x[..] {
        return Err("autre torrent".into());
    }
    let mut dec = Rc4::mse(&h(&[b"keyA", &sec, info_hash]));
    let mut enc = Rc4::mse(&h(&[b"keyB", &sec, info_hash]));
    let mut head = body[20..34].to_vec();
    dec.apply(&mut head);
    let provide = u32::from_be_bytes(head[8..12].try_into().unwrap());
    let pad_c = u16::from_be_bytes([head[12], head[13]]) as usize;
    while body.len() < 34 + pad_c + 2 {
        read_more(s, &mut body).await?;
    }
    let mut pad_and_len = body[34..34 + pad_c + 2].to_vec();
    dec.apply(&mut pad_and_len);
    let ia_len = u16::from_be_bytes([pad_and_len[pad_c], pad_and_len[pad_c + 1]]) as usize;
    while body.len() < 36 + pad_c + ia_len {
        read_more(s, &mut body).await?;
    }
    let mut leftover = body.split_off(36 + pad_c);
    dec.apply(&mut leftover);
    let select = if provide & CRYPTO_RC4 != 0 { CRYPTO_RC4 } else { CRYPTO_PLAIN };
    let mut msg = [&VC[..], &select.to_be_bytes(), &0u16.to_be_bytes()].concat();
    enc.apply(&mut msg);
    s.write_all(&msg).await.map_err(|e| e.to_string())?;
    if select == CRYPTO_RC4 {
        Ok(Established { encrypt: Some(enc), decrypt: Some(dec), leftover })
    } else {
        Ok(Established { encrypt: None, decrypt: None, leftover })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_vecteur_connu() {
        // RFC 6229, clé 0x0102030405, premiers octets du flux.
        let mut r = Rc4::new(&[1, 2, 3, 4, 5]);
        let mut b = [0u8; 8];
        r.apply(&mut b);
        assert_eq!(b, [0xb2, 0x39, 0x63, 0x05, 0xf0, 0x3d, 0xc0, 0x27]);
    }

    #[tokio::test]
    async fn poignee_de_main_complete() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let ih = [7u8; 20];
        let (ra, rb) = tokio::join!(initiate(&mut a, &ih, false), respond(&mut b, &ih));
        let (mut ea, mut eb) = (ra.unwrap(), rb.unwrap());
        // Le flux chiffré circule dans les deux sens.
        let mut msg = b"bonjour du navigateur".to_vec();
        ea.encrypt.as_mut().unwrap().apply(&mut msg);
        assert_ne!(&msg[..], b"bonjour du navigateur");
        eb.decrypt.as_mut().unwrap().apply(&mut msg);
        assert_eq!(&msg[..], b"bonjour du navigateur");
        let mut back = b"et retour".to_vec();
        eb.encrypt.as_mut().unwrap().apply(&mut back);
        ea.decrypt.as_mut().unwrap().apply(&mut back);
        assert_eq!(&back[..], b"et retour");
    }

    #[tokio::test]
    async fn mauvais_torrent_refuse() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let r = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(initiate(&mut a, &[1; 20], false), respond(&mut b, &[2; 20]))
        })
        .await;
        assert!(r.map(|(_, rb)| rb.is_err()).unwrap_or(true));
    }
}
