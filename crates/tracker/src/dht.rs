//! La DHT de BitTorrent (BEP 5), en client : trouver des pairs sans tracker.
//! Recherche itérative `get_peers` : on interroge les nœuds les plus proches
//! de l'empreinte (distance XOR), qui renvoient des pairs ou des nœuds encore
//! plus proches.

use mxtorrent_core::bencode::{self, Value};
use mxtorrent_core::relay::parse_compact;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::UnboundedSender;

pub const BOOTSTRAP: &[&str] =
    &["router.bittorrent.com:6881", "dht.transmissionbt.com:6881", "router.utorrent.com:6881", "dht.libtorrent.org:25401"];

const ALPHA: usize = 8;
const BUDGET: Duration = Duration::from_secs(12);
const MAX_PEERS: usize = 300;

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("hasard du système");
    b
}

fn distance(a: &[u8; 20], b: &[u8; 20]) -> [u8; 20] {
    let mut d = [0u8; 20];
    for i in 0..20 {
        d[i] = a[i] ^ b[i];
    }
    d
}

/// Une requête `get_peers` encodée.
pub fn get_peers_query(tid: &[u8], node_id: &[u8; 20], info_hash: &[u8; 20]) -> Vec<u8> {
    let a = Value::dict().with("id", Value::Bytes(node_id.to_vec())).with("info_hash", Value::Bytes(info_hash.to_vec()));
    bencode::encode(
        &Value::dict().with("a", a).with("q", Value::str("get_peers")).with("t", Value::Bytes(tid.to_vec())).with("y", Value::str("q")),
    )
}

/// Un nœud de la DHT : son identifiant et son adresse.
pub type Node = ([u8; 20], SocketAddr);

/// Ce qu'une réponse contient : identifiant de transaction, pairs et nœuds.
pub type Response = (Vec<u8>, Vec<SocketAddr>, Vec<Node>);

/// Lit une réponse `get_peers`.
pub fn parse_response(b: &[u8]) -> Option<Response> {
    let v = bencode::decode(b).ok()?;
    let tid = v.get("t")?.as_bytes()?.to_vec();
    let r = v.get("r")?;
    let mut peers = Vec::new();
    if let Some(list) = r.get("values").and_then(|x| x.as_list()) {
        for p in list {
            if let Some(b) = p.as_bytes() {
                peers.extend(parse_compact(b, b.len() == 18));
            }
        }
    }
    let mut nodes = Vec::new();
    if let Some(n) = r.get("nodes").and_then(|x| x.as_bytes()) {
        for c in n.as_chunks::<26>().0 {
            let id: [u8; 20] = c[..20].try_into().unwrap();
            let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(c[20], c[21], c[22], c[23])), u16::from_be_bytes([c[24], c[25]]));
            if addr.port() != 0 {
                nodes.push((id, addr));
            }
        }
    }
    Some((tid, peers, nodes))
}

/// Cherche des pairs dans la DHT ; les envoie au fur et à mesure.
pub async fn lookup(info_hash: [u8; 20], out: UnboundedSender<Vec<SocketAddr>>) -> Result<usize, String> {
    let sock = UdpSocket::bind("0.0.0.0:0").await.map_err(|e| e.to_string())?;
    let me: [u8; 20] = random();
    let start = Instant::now();
    // Candidats triés par distance ; on garde les adresses déjà interrogées.
    let mut candidates: Vec<Node> = Vec::new();
    for host in BOOTSTRAP {
        if let Ok(mut addrs) = tokio::net::lookup_host(host).await
            && let Some(a) = addrs.find(|a| a.is_ipv4())
        {
            candidates.push((random(), a));
        }
    }
    if candidates.is_empty() {
        return Err("aucun nœud de démarrage joignable".into());
    }
    let mut asked: HashSet<SocketAddr> = HashSet::new();
    let mut inflight: HashMap<Vec<u8>, (SocketAddr, Instant)> = HashMap::new();
    let mut found: HashSet<SocketAddr> = HashSet::new();
    let mut buf = vec![0u8; 4096];
    let mut counter: u16 = 0;
    while start.elapsed() < BUDGET && found.len() < MAX_PEERS {
        // Lance des requêtes vers les candidats les plus proches pas encore interrogés.
        candidates.sort_by_key(|(id, _)| distance(id, &info_hash));
        candidates.dedup_by_key(|(_, a)| *a);
        for (_, addr) in
            candidates.iter().filter(|(_, a)| !asked.contains(a)).take(ALPHA.saturating_sub(inflight.len())).copied().collect::<Vec<_>>()
        {
            counter = counter.wrapping_add(1);
            let tid = counter.to_be_bytes().to_vec();
            let _ = sock.send_to(&get_peers_query(&tid, &me, &info_hash), addr).await;
            asked.insert(addr);
            inflight.insert(tid, (addr, Instant::now()));
        }
        if inflight.is_empty() {
            break;
        }
        if let Ok(Ok((n, from))) = tokio::time::timeout(Duration::from_millis(400), sock.recv_from(&mut buf)).await {
            {
                let Some((tid, peers, nodes)) = parse_response(&buf[..n]) else { continue };
                if inflight.get(&tid).is_some_and(|(a, _)| *a == from) {
                    inflight.remove(&tid);
                } else {
                    continue;
                }
                let new: Vec<SocketAddr> = peers.into_iter().filter(|p| found.insert(*p)).collect();
                if !new.is_empty() && out.send(new).is_err() {
                    break;
                }
                candidates.extend(nodes);
                // Garde les 200 meilleurs candidats.
                candidates.sort_by_key(|(id, _)| distance(id, &info_hash));
                candidates.truncate(200);
            }
        }
        inflight.retain(|_, (_, t)| t.elapsed() < Duration::from_secs(3));
    }
    Ok(found.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requete_et_reponse() {
        let q = get_peers_query(b"aa", &[1; 20], &[2; 20]);
        let v = bencode::decode(&q).unwrap();
        assert_eq!(v.get("q").unwrap().as_str(), Some("get_peers"));
        // Réponse avec un pair et un nœud.
        let r = Value::dict()
            .with("id", Value::Bytes(vec![3; 20]))
            .with("values", Value::List(vec![Value::Bytes(vec![1, 2, 3, 4, 0x1a, 0xe1])]))
            .with("nodes", Value::Bytes([&[5u8; 20][..], &[6, 7, 8, 9, 0x1a, 0xe2]].concat()));
        let resp = bencode::encode(&Value::dict().with("r", r).with("t", Value::str("aa")).with("y", Value::str("r")));
        let (tid, peers, nodes) = parse_response(&resp).unwrap();
        assert_eq!(tid, b"aa");
        assert_eq!(peers, vec!["1.2.3.4:6881".parse().unwrap()]);
        assert_eq!(nodes, vec![([5; 20], "6.7.8.9:6882".parse().unwrap())]);
    }
}
