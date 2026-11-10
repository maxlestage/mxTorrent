//! La passerelle TCP/UDP : ce qu'un navigateur ne peut pas faire, elle le fait
//! pour lui, sans jamais voir autre chose que des octets BitTorrent.
//!
//! - Navigateur ↔ passerelle : WebSocket (TLS sur Heroku) **et**, dedans, une
//!   session Brocolis 🥦 v4 (X25519 + ML-KEM-768, triple cliquet, en-têtes
//!   chiffrés). Même l'hébergeur qui termine TLS ne lit rien.
//! - Passerelle ↔ pairs classiques : MSE/RC4, le chiffrement que ces clients
//!   comprennent ; exigé par défaut (`MXT_RELAY_ALLOW_PLAIN=1` accepte le clair).
//!
//! Pour ne pas devenir un mandataire ouvert, la passerelle ne se connecte
//! qu'aux pairs qu'elle a elle-même trouvés pour ce torrent, jamais à une
//! adresse privée, et le premier message envoyé doit être la poignée de main
//! BitTorrent de ce torrent.

use crate::{dht, mse, trackers};
use axum::extract::ws::{Message, WebSocket};
use base64::Engine;
use brocolis_proto::{Content, Device, Token4, hasard};
use futures_util::{SinkExt, StreamExt};
use mxtorrent_core::relay::{self, FLAG_DHT, Frame, MAX_BATCH, SOURCE_DHT, SOURCE_TRACKER};
use mxtorrent_core::util::InfoHash;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

const MAX_SESSIONS: usize = 300;
const MAX_CONNS: usize = 80;
const MAX_LOOKUPS: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub struct Gateway {
    device: Mutex<Device>,
    pub bundle: String,
    pub fingerprint: String,
    http: reqwest::Client,
    allow_private: bool,
    allow_plain: bool,
    sessions: AtomicUsize,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

impl Gateway {
    pub fn new() -> Gateway {
        let device = Device::generate(&mut hasard::system(), now(), true);
        let bundle = device.bundle().expect("paquet de clés Brocolis");
        let fingerprint = bundle.identity.fingerprint();
        Gateway {
            bundle: bundle.to_base64(),
            fingerprint,
            device: Mutex::new(device),
            http: reqwest::Client::builder().user_agent(concat!("mxTorrent/", env!("CARGO_PKG_VERSION"))).build().expect("client HTTP"),
            allow_private: flag("MXT_RELAY_ALLOW_PRIVATE"),
            allow_plain: flag("MXT_RELAY_ALLOW_PLAIN"),
            sessions: AtomicUsize::new(0),
        }
    }

    /// Ce que le navigateur lit avant de se connecter (servi en HTTPS).
    pub fn info_json(&self) -> String {
        serde_json::json!({
            "protocol": "brocolis-v4",
            "bundle": self.bundle,
            "fingerprint": self.fingerprint,
            "dht": true,
            "require_encryption": !self.allow_plain,
        })
        .to_string()
    }
}

/// L'état d'une session de navigateur.
struct Conn {
    out: mpsc::Sender<Frame>,
    allowed: Mutex<HashSet<(InfoHash, SocketAddr)>>,
    lookups: AtomicUsize,
    peer_id: [u8; 20],
}

pub async fn serve(ws: WebSocket, gw: Arc<Gateway>) {
    if gw.sessions.fetch_add(1, Ordering::SeqCst) >= MAX_SESSIONS {
        gw.sessions.fetch_sub(1, Ordering::SeqCst);
        return;
    }
    run(ws, gw.clone()).await;
    gw.sessions.fetch_sub(1, Ordering::SeqCst);
}

async fn run(ws: WebSocket, gw: Arc<Gateway>) {
    let (mut sink, mut stream) = ws.split();
    let mut rng = hasard::system();

    // 1. Le premier message ouvre la session Brocolis (préambule v4).
    let first = match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
        Ok(Some(Ok(Message::Binary(b)))) => b,
        _ => return,
    };
    let Ok(token) = Token4::from_bytes(&first) else { return };
    let accepted = gw.device.lock().unwrap().accept4(&token, now(), &mut rng);
    let Ok((session, Content::Text(_hello))) = accepted else { return };
    let session = Arc::new(Mutex::new(session));

    // 2. Écriture : regroupe les trames en lots, chiffre chaque lot.
    let (tx, mut rx) = mpsc::channel::<Frame>(512);
    let sess_w = session.clone();
    let writer = tokio::spawn(async move {
        // Réponse immédiate (lot vide) : la session est confirmée côté navigateur.
        let mut pending: Option<Frame> = None;
        let mut first_flush = true;
        loop {
            let mut batch = Vec::new();
            if !first_flush {
                let f = match pending.take() {
                    Some(f) => f,
                    None => match rx.recv().await {
                        Some(f) => f,
                        None => break,
                    },
                };
                f.encode_into(&mut batch);
                while batch.len() < MAX_BATCH {
                    match rx.try_recv() {
                        Ok(f) if batch.len() + f.encoded_len() <= MAX_BATCH => f.encode_into(&mut batch),
                        Ok(f) => {
                            pending = Some(f);
                            break;
                        }
                        Err(_) => break,
                    }
                }
            }
            first_flush = false;
            let sealed = sess_w.lock().unwrap().encrypt_token(&B64.encode(&batch)).map(|t| t.to_bytes());
            let Ok(bytes) = sealed else { break };
            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
    });

    let mut rng_b = [0u8; 12];
    getrandom::fill(&mut rng_b).expect("hasard du système");
    let mut peer_id = *b"-MX0100-000000000000";
    for (i, b) in rng_b.iter().enumerate() {
        peer_id[8 + i] = b"0123456789abcdefghijklmnopqrstuvwxyz"[*b as usize % 36];
    }
    let conn = Arc::new(Conn { out: tx, allowed: Mutex::new(HashSet::new()), lookups: AtomicUsize::new(0), peer_id });
    let mut peers: HashMap<u32, mpsc::Sender<Vec<u8>>> = HashMap::new();

    // 3. Lecture : déchiffre, décode, exécute.
    while let Some(Ok(msg)) = stream.next().await {
        let Message::Binary(bytes) = msg else {
            if matches!(msg, Message::Close(_)) {
                break;
            }
            continue;
        };
        let Ok(token) = Token4::from_bytes(&bytes) else { break };
        let opened = session.lock().unwrap().decrypt(&token, now(), &mut rng);
        let Ok(Content::Text(text)) = opened else { break };
        let Ok(batch) = B64.decode(text.as_bytes()) else { break };
        let Some(frames) = Frame::decode_batch(&batch) else { break };
        for f in frames {
            match f {
                Frame::Lookup { req, info_hash, flags, trackers } => {
                    if conn.lookups.load(Ordering::SeqCst) >= MAX_LOOKUPS {
                        let _ = conn.out.send(Frame::LookupDone { req, found: 0, summary: "trop de recherches en cours".into() }).await;
                        continue;
                    }
                    conn.lookups.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(lookup(gw.clone(), conn.clone(), req, info_hash, flags, trackers));
                }
                Frame::Connect { conn: id, info_hash, addr } => {
                    let known = conn.allowed.lock().unwrap().contains(&(info_hash, addr));
                    let refuse = if !known {
                        Some("pair inconnu de la passerelle")
                    } else if !gw.allow_private && relay::is_private(&addr) {
                        Some("adresse privée")
                    } else if peers.len() >= MAX_CONNS {
                        Some("trop de connexions")
                    } else if peers.contains_key(&id) {
                        Some("identifiant déjà utilisé")
                    } else {
                        None
                    };
                    if let Some(reason) = refuse {
                        let _ = conn.out.send(Frame::Failed { conn: id, reason: reason.into() }).await;
                        continue;
                    }
                    let (ptx, prx) = mpsc::channel::<Vec<u8>>(64);
                    peers.insert(id, ptx);
                    tokio::spawn(peer(gw.clone(), conn.out.clone(), id, info_hash, addr, prx));
                }
                Frame::Data { conn: id, data } => {
                    let gone = match peers.get(&id) {
                        Some(p) => p.send(data).await.is_err(),
                        None => false,
                    };
                    if gone {
                        peers.remove(&id);
                    }
                }
                Frame::Close { conn: id } => {
                    peers.remove(&id);
                }
                _ => {}
            }
        }
        peers.retain(|_, p| !p.is_closed());
    }
    drop(peers);
    drop(conn);
    writer.abort();
}

/// Trackers UDP et HTTP, puis DHT : les pairs sont envoyés au fil de l'eau.
async fn lookup(gw: Arc<Gateway>, conn: Arc<Conn>, req: u32, info_hash: InfoHash, flags: u8, list: Vec<String>) {
    let ask = trackers::Ask { info_hash: info_hash.0, peer_id: conn.peer_id, port: 6881 };
    let (ptx, mut prx) = mpsc::unbounded_channel::<(u8, Vec<SocketAddr>)>();
    let mut tasks = Vec::new();
    let mut tried = 0;
    for url in list.into_iter().take(30) {
        let ptx = ptx.clone();
        let http = gw.http.clone();
        let ask = trackers::Ask { info_hash: ask.info_hash, peer_id: ask.peer_id, port: ask.port };
        if url.starts_with("udp://") || url.starts_with("http://") || url.starts_with("https://") {
            tried += 1;
            tasks.push(tokio::spawn(async move {
                let r = if url.starts_with("udp://") {
                    trackers::udp_announce(&url, &ask).await
                } else {
                    trackers::http_announce(&http, &url, &ask).await
                };
                match r {
                    Ok(p) => {
                        let _ = ptx.send((SOURCE_TRACKER, p));
                        None
                    }
                    Err(e) => Some(format!("{url} : {e}")),
                }
            }));
        }
    }
    if flags & FLAG_DHT != 0 {
        let ptx = ptx.clone();
        tasks.push(tokio::spawn(async move {
            let (dtx, mut drx) = mpsc::unbounded_channel();
            let search = tokio::spawn(dht::lookup(info_hash.0, dtx));
            while let Some(p) = drx.recv().await {
                let _ = ptx.send((SOURCE_DHT, p));
            }
            match search.await {
                Ok(Err(e)) => Some(format!("DHT : {e}")),
                _ => None,
            }
        }));
    }
    drop(ptx);
    let mut found: HashSet<SocketAddr> = HashSet::new();
    let forward = async {
        while let Some((source, peers)) = prx.recv().await {
            let fresh: Vec<SocketAddr> =
                peers.into_iter().filter(|a| gw.allow_private || !relay::is_private(a)).filter(|a| found.insert(*a)).collect();
            if fresh.is_empty() {
                continue;
            }
            {
                let mut allowed = conn.allowed.lock().unwrap();
                for a in &fresh {
                    allowed.insert((info_hash, *a));
                }
            }
            for chunk in fresh.chunks(500) {
                if conn.out.send(Frame::Peers { req, source, peers: chunk.to_vec() }).await.is_err() {
                    return;
                }
            }
        }
    };
    forward.await;
    let mut errors = Vec::new();
    for t in tasks {
        if let Ok(Some(e)) = t.await {
            errors.push(e);
        }
    }
    let summary = format!("{} trackers, {} erreurs{}", tried, errors.len(), errors.first().map(|e| format!(" ({e})")).unwrap_or_default());
    let _ = conn.out.send(Frame::LookupDone { req, found: found.len() as u32, summary }).await;
    conn.lookups.fetch_sub(1, Ordering::SeqCst);
}

/// Une connexion TCP vers un pair classique, chiffrée par MSE.
async fn peer(gw: Arc<Gateway>, out: mpsc::Sender<Frame>, id: u32, info_hash: InfoHash, addr: SocketAddr, mut rx: mpsc::Receiver<Vec<u8>>) {
    let fail = |reason: String| {
        let out = out.clone();
        async move {
            let _ = out.send(Frame::Failed { conn: id, reason }).await;
        }
    };
    let connect = || async {
        tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
            .await
            .map_err(|_| "délai de connexion".to_string())?
            .map_err(|e| e.to_string())
    };
    let mut stream = match connect().await {
        Ok(s) => s,
        Err(e) => return fail(e).await,
    };
    let _ = stream.set_nodelay(true);
    let est = match mse::initiate(&mut stream, &info_hash.0, false).await {
        Ok(e) => e,
        Err(e) if gw.allow_plain => {
            // Ce pair ne chiffre pas : nouvelle connexion, en clair (autorisé par la configuration).
            let _ = e;
            match connect().await {
                Ok(s) => {
                    stream = s;
                    mse::Established { encrypt: None, decrypt: None, leftover: Vec::new() }
                }
                Err(e) => return fail(e).await,
            }
        }
        Err(e) => return fail(format!("chiffrement MSE refusé : {e}")).await,
    };
    let mse::Established { mut encrypt, mut decrypt, leftover } = est;
    if out.send(Frame::Connected { conn: id, encrypted: encrypt.is_some() }).await.is_err() {
        return;
    }
    if !leftover.is_empty() && out.send(Frame::DataIn { conn: id, data: leftover }).await.is_err() {
        return;
    }
    let debug = flag("MXT_RELAY_DEBUG");
    let trace = |dir: &str, d: &[u8]| {
        if debug {
            let head: String = d.iter().take(24).map(|b| format!("{b:02x}")).collect();
            eprintln!("relais {id} {addr} {dir} {} octets : {head}", d.len());
        }
    };
    let (mut rd, mut wr) = stream.into_split();
    // Les 68 premiers octets envoyés doivent être la poignée de main de ce torrent.
    let mut checked = false;
    let mut head: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        tokio::select! {
            r = rd.read(&mut buf) => {
                match r {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut data = buf[..n].to_vec();
                        if let Some(d) = decrypt.as_mut() { d.apply(&mut data); }
                        trace("←", &data);
                        if out.send(Frame::DataIn { conn: id, data }).await.is_err() { return; }
                    }
                }
            }
            m = rx.recv() => {
                let Some(mut data) = m else { break };
                if !checked {
                    head.extend_from_slice(&data);
                    if head.len() < 68 { continue; }
                    if head[0] != 19 || &head[1..20] != b"BitTorrent protocol" || head[28..48] != info_hash.0 {
                        break;
                    }
                    checked = true;
                    data = std::mem::take(&mut head);
                }
                trace("→", &data);
                if let Some(e) = encrypt.as_mut() { e.apply(&mut data); }
                if wr.write_all(&data).await.is_err() { break; }
            }
        }
    }
    let _ = out.send(Frame::Closed { conn: id }).await;
}
