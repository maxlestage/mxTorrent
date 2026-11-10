//! Interroger les trackers classiques pour le compte du navigateur :
//! UDP (BEP 15) et HTTP(S) (BEP 3, réponses compactes BEP 23 et IPv6 BEP 7).

use mxtorrent_core::bencode;
use mxtorrent_core::relay::parse_compact;
use mxtorrent_core::util::percent_encode;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;

const UDP_PROTOCOL_ID: u64 = 0x41727101980;
const UDP_TRIES: usize = 2;
const UDP_TIMEOUT: Duration = Duration::from_secs(4);

/// Ce que la passerelle annonce : elle ne reçoit pas de connexions entrantes,
/// elle se présente donc comme un client qui cherche des pairs.
pub struct Ask {
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
    pub port: u16,
}

fn rand_u32() -> u32 {
    let mut b = [0u8; 4];
    getrandom::fill(&mut b).expect("hasard du système");
    u32::from_be_bytes(b)
}

async fn udp_exchange(sock: &UdpSocket, req: &[u8], tid: u32, action: u32, min_len: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; 8192];
    for attempt in 0..UDP_TRIES {
        sock.send(req).await.map_err(|e| e.to_string())?;
        let wait = UDP_TIMEOUT * (attempt as u32 + 1);
        match tokio::time::timeout(wait, sock.recv(&mut buf)).await {
            Ok(Ok(n)) if n >= 8 => {
                let a = u32::from_be_bytes(buf[0..4].try_into().unwrap());
                let t = u32::from_be_bytes(buf[4..8].try_into().unwrap());
                if t != tid {
                    continue;
                }
                if a == 3 {
                    return Err(String::from_utf8_lossy(&buf[8..n]).into_owned());
                }
                if a == action && n >= min_len {
                    return Ok(buf[..n].to_vec());
                }
            }
            Ok(Ok(_)) => continue,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => continue,
        }
    }
    Err("pas de réponse".into())
}

/// Annonce sur un tracker `udp://hôte:port`.
pub async fn udp_announce(url: &str, ask: &Ask) -> Result<Vec<SocketAddr>, String> {
    let rest = url.strip_prefix("udp://").ok_or("adresse UDP invalide")?;
    let hostport = rest.split('/').next().unwrap_or(rest);
    let addr = tokio::net::lookup_host(hostport).await.map_err(|e| e.to_string())?.next().ok_or("hôte introuvable")?;
    let bind = if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let sock = UdpSocket::bind(bind).await.map_err(|e| e.to_string())?;
    sock.connect(addr).await.map_err(|e| e.to_string())?;

    let tid = rand_u32();
    let mut req = Vec::with_capacity(16);
    req.extend_from_slice(&UDP_PROTOCOL_ID.to_be_bytes());
    req.extend_from_slice(&0u32.to_be_bytes());
    req.extend_from_slice(&tid.to_be_bytes());
    let r = udp_exchange(&sock, &req, tid, 0, 16).await?;
    let conn_id = &r[8..16];

    let tid = rand_u32();
    let mut a = Vec::with_capacity(98);
    a.extend_from_slice(conn_id);
    a.extend_from_slice(&1u32.to_be_bytes());
    a.extend_from_slice(&tid.to_be_bytes());
    a.extend_from_slice(&ask.info_hash);
    a.extend_from_slice(&ask.peer_id);
    a.extend_from_slice(&0u64.to_be_bytes()); // téléchargé
    a.extend_from_slice(&(1u64 << 30).to_be_bytes()); // restant (inconnu : grand)
    a.extend_from_slice(&0u64.to_be_bytes()); // envoyé
    a.extend_from_slice(&2u32.to_be_bytes()); // started
    a.extend_from_slice(&0u32.to_be_bytes()); // IP : celle de l'expéditeur
    a.extend_from_slice(&rand_u32().to_be_bytes());
    a.extend_from_slice(&(-1i32).to_be_bytes()); // autant de pairs que possible
    a.extend_from_slice(&ask.port.to_be_bytes());
    let r = udp_exchange(&sock, &a, tid, 1, 20).await?;
    Ok(parse_compact(&r[20..], addr.is_ipv6()))
}

/// Annonce sur un tracker `http(s)://…/announce`.
pub async fn http_announce(client: &reqwest::Client, url: &str, ask: &Ask) -> Result<Vec<SocketAddr>, String> {
    let bin = |b: &[u8]| b.iter().map(|x| format!("%{x:02X}")).collect::<String>();
    let sep = if url.contains('?') { '&' } else { '?' };
    let full = format!(
        "{url}{sep}info_hash={}&peer_id={}&port={}&uploaded=0&downloaded=0&left=1073741824&compact=1&numwant=200&event=started&key={}",
        bin(&ask.info_hash),
        bin(&ask.peer_id),
        ask.port,
        percent_encode(&format!("{:08x}", rand_u32()))
    );
    let resp = client.get(full).timeout(Duration::from_secs(10)).send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    parse_http_response(&body)
}

pub fn parse_http_response(body: &[u8]) -> Result<Vec<SocketAddr>, String> {
    let v = bencode::decode(body).map_err(|e| e.to_string())?;
    if let Some(f) = v.get("failure reason").and_then(|f| f.as_str()) {
        return Err(f.to_string());
    }
    let mut out = Vec::new();
    match v.get("peers") {
        Some(bencode::Value::Bytes(b)) => out.extend(parse_compact(b, false)),
        Some(bencode::Value::List(l)) => {
            for p in l {
                let ip = p.get("ip").and_then(|i| i.as_str()).and_then(|s| s.parse().ok());
                let port = p.get("port").and_then(|i| i.as_int()).and_then(|i| u16::try_from(i).ok());
                if let (Some(ip), Some(port)) = (ip, port) {
                    out.push(SocketAddr::new(ip, port));
                }
            }
        }
        _ => {}
    }
    if let Some(b) = v.get("peers6").and_then(|p| p.as_bytes()) {
        out.extend(parse_compact(b, true));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reponse_http() {
        let body = b"d8:intervali1800e5:peers12:\x01\x02\x03\x04\x1a\xe1\x05\x06\x07\x08\x1a\xe2e";
        let p = parse_http_response(body).unwrap();
        assert_eq!(p, vec!["1.2.3.4:6881".parse().unwrap(), "5.6.7.8:6882".parse().unwrap()]);
        assert_eq!(parse_http_response(b"d14:failure reason4:nopee").unwrap_err(), "nope");
    }

    #[tokio::test]
    async fn tracker_udp_simule() {
        // Un faux tracker UDP qui répond comme BEP 15.
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = server.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            for _ in 0..2 {
                let (n, from) = server.recv_from(&mut buf).await.unwrap();
                let tid = &buf[12..16];
                if n == 16 {
                    let r = [&0u32.to_be_bytes()[..], tid, &42u64.to_be_bytes()].concat();
                    server.send_to(&r, from).await.unwrap();
                } else {
                    assert_eq!(&buf[16..36], &[9u8; 20]);
                    let r = [
                        &1u32.to_be_bytes()[..],
                        tid,
                        &1800u32.to_be_bytes(),
                        &0u32.to_be_bytes(),
                        &1u32.to_be_bytes(),
                        &[9, 9, 9, 9, 0x1a, 0xe1],
                    ]
                    .concat();
                    server.send_to(&r, from).await.unwrap();
                }
            }
        });
        let ask = Ask { info_hash: [9; 20], peer_id: [1; 20], port: 6881 };
        let peers = udp_announce(&format!("udp://127.0.0.1:{port}/announce"), &ask).await.unwrap();
        assert_eq!(peers, vec!["9.9.9.9:6881".parse().unwrap()]);
    }
}
