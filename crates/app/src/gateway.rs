//! Côté navigateur, la passerelle TCP/UDP : elle donne accès à tout le réseau
//! BitTorrent (trackers UDP et HTTP, DHT, pairs TCP), ce qu'un navigateur ne
//! peut pas faire seul.
//!
//! Tout ce qui passe entre le navigateur et elle est chiffré par une session
//! Brocolis 🥦 v4 : identité éphémère par onglet, établissement hybride
//! X25519 + ML-KEM-768 (résistant aux ordinateurs quantiques), triple cliquet,
//! en-têtes chiffrés. Le paquet de clés de la passerelle est lu en HTTPS sur
//! le même serveur. Les pairs relayés deviennent des pairs comme les autres
//! pour le moteur.

use crate::engine::Engine;
use crate::net::{self, GatewaySocket, NetEvent};
use crate::web;
use base64::Engine as _;
use brocolis_proto::{Content, Device, KeyBundle, Session4, Token4, hasard};
use mxtorrent_core::InfoHash;
use mxtorrent_core::relay::{FLAG_DHT, Frame, MAX_BATCH};
use mxtorrent_core::torrent::{PeerKey, State};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
/// Au-delà, on attend que la WebSocket se vide avant de servir d'autres blocs.
const HIGH_WATER: usize = 4 * 1024 * 1024;
const MAX_PENDING_CONNECTS: usize = 12;
const MAX_GATEWAY_PEERS: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Off,
    Fetching,
    Connecting,
    Ready,
    Failed,
}

struct PeerLink {
    info_hash: InfoHash,
    key: PeerKey,
    connected: bool,
    encrypted: bool,
}

pub struct GatewayClient {
    pub base: String,
    pub phase: Phase,
    pub fingerprint: Option<String>,
    pub detail: String,
    sock: Option<GatewaySocket>,
    device: Device,
    session: Option<Session4>,
    sent_initial: bool,
    out: Vec<u8>,
    retry_at: u64,
    backoff: u64,
    next_req: u32,
    next_conn: u32,
    lookups: HashMap<u32, InfoHash>,
    links: HashMap<u32, PeerLink>,
    by_key: HashMap<PeerKey, u32>,
    queue: HashMap<InfoHash, VecDeque<SocketAddr>>,
    tried: HashMap<InfoHash, HashSet<SocketAddr>>,
    last_lookup: HashMap<InfoHash, u64>,
    pub found: HashMap<InfoHash, usize>,
}

fn now_secs() -> u64 {
    web::now_ms() / 1000
}

impl GatewayClient {
    pub fn new(base: String) -> GatewayClient {
        GatewayClient {
            base,
            phase: Phase::Off,
            fingerprint: None,
            detail: String::new(),
            sock: None,
            device: Device::generate(&mut hasard::system(), now_secs(), false),
            session: None,
            sent_initial: false,
            out: Vec::new(),
            retry_at: 0,
            backoff: 5_000,
            next_req: 1,
            next_conn: 1,
            lookups: HashMap::new(),
            links: HashMap::new(),
            by_key: HashMap::new(),
            queue: HashMap::new(),
            tried: HashMap::new(),
            last_lookup: HashMap::new(),
            found: HashMap::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base.trim_end_matches('/'))
    }

    fn ws_url(&self) -> String {
        let u = self.url("relay");
        if let Some(r) = u.strip_prefix("https://") {
            format!("wss://{r}")
        } else if let Some(r) = u.strip_prefix("http://") {
            format!("ws://{r}")
        } else {
            u
        }
    }

    pub fn is_gateway_peer(&self, key: PeerKey) -> bool {
        self.by_key.contains_key(&key)
    }

    pub fn peer_encrypted(&self, key: PeerKey) -> bool {
        self.by_key.get(&key).and_then(|c| self.links.get(c)).is_some_and(|l| l.encrypted)
    }

    pub fn connected_peers(&self) -> usize {
        self.links.values().filter(|l| l.connected).count()
    }

    fn push(&mut self, f: Frame) {
        f.encode_into(&mut self.out);
    }

    /// Chiffre et envoie ce qui attend, par lots.
    fn send_out(&mut self) {
        // Rien à dire : on n'envoie qu'une fois le préambule qui ouvre la session.
        if self.out.is_empty() && (self.sent_initial || self.session.as_ref().is_some_and(|s| s.is_confirmed())) {
            return;
        }
        let (Some(sock), Some(session)) = (&self.sock, &mut self.session) else { return };
        if !sock.is_open() {
            return;
        }
        let all = std::mem::take(&mut self.out);
        let frames = Frame::decode_batch(&all).unwrap_or_default();
        let mut batch = Vec::new();
        let mut batches = Vec::new();
        for f in frames {
            if !batch.is_empty() && batch.len() + f.encoded_len() > MAX_BATCH {
                batches.push(std::mem::take(&mut batch));
            }
            f.encode_into(&mut batch);
        }
        if !batch.is_empty() || batches.is_empty() {
            batches.push(batch);
        }
        for b in batches {
            match session.encrypt_token(&B64.encode(&b)) {
                Ok(t) => sock.send(&t.to_bytes()),
                Err(_) => return,
            }
        }
        self.sent_initial = true;
    }

    fn drop_all_links(&mut self) -> Vec<(InfoHash, PeerKey)> {
        let out = self.links.drain().map(|(_, l)| (l.info_hash, l.key)).collect();
        self.by_key.clear();
        self.lookups.clear();
        out
    }

    pub fn close_peer(&mut self, key: PeerKey) {
        if let Some(c) = self.by_key.remove(&key) {
            self.links.remove(&c);
            self.push(Frame::Close { conn: c });
        }
    }

    /// Ce qui attend d'être envoyé dans la WebSocket (pour ralentir la lecture des pièces).
    pub fn budget(&self) -> usize {
        match &self.sock {
            Some(s) if self.phase == Phase::Ready => HIGH_WATER.saturating_sub(s.buffered() + self.out.len() * 4 / 3),
            _ => 0,
        }
    }
}

impl Engine {
    /// La passerelle à utiliser : celle des réglages, sinon celle du serveur qui sert l'application.
    pub fn gateway_base(&self) -> Option<String> {
        let s = self.settings.gateway.trim();
        if !self.settings.use_gateway {
            return None;
        }
        if !s.is_empty() {
            return Some(s.to_string());
        }
        self.auto_gateway.clone()
    }

    /// Appelé à chaque tour d'horloge.
    pub(crate) fn gateway_tick(&mut self) {
        let wanted = self.gateway_base();
        // Changement d'adresse (ou désactivation) : on repart de zéro.
        if self.gateway.as_ref().map(|g| g.base.clone()) != wanted {
            if let Some(mut g) = self.gateway.take() {
                for (h, k) in g.drop_all_links() {
                    if let Some(i) = self.find(&h) {
                        self.entries[i].torrent.remove_peer(k);
                    }
                }
            }
            self.gateway = wanted.map(GatewayClient::new);
        }
        let now = web::now_ms();
        let Some(g) = self.gateway.as_mut() else { return };
        match g.phase {
            Phase::Off | Phase::Failed if now >= g.retry_at && !self.entries.is_empty() => {
                g.phase = Phase::Fetching;
                g.detail = crate::i18n::tr("lecture du paquet de clés…", "reading the key bundle…");
                let url = g.url("relay/bundle");
                wasm_bindgen_futures::spawn_local(async move {
                    let r = web::fetch_bytes(&url, None).await.map(|(_, b)| b);
                    net::push(NetEvent::GatewayInfo(r));
                });
            }
            Phase::Ready => self.gateway_schedule(now),
            _ => {}
        }
        if let Some(g) = self.gateway.as_mut() {
            g.send_out();
        }
    }

    fn gateway_fail(&mut self, why: String) {
        let Some(g) = self.gateway.as_mut() else { return };
        g.phase = Phase::Failed;
        g.detail = why;
        g.session = None;
        g.sock = None;
        g.retry_at = web::now_ms() + g.backoff;
        g.backoff = (g.backoff * 2).min(300_000);
        let dropped = g.drop_all_links();
        for (h, k) in dropped {
            if let Some(i) = self.find(&h) {
                self.entries[i].torrent.remove_peer(k);
            }
        }
        self.dirty = true;
    }

    pub(crate) fn on_gateway_info(&mut self, r: Result<Vec<u8>, String>) {
        let parsed = r.and_then(|b| {
            let v: serde_json::Value =
                serde_json::from_slice(&b).map_err(|_| crate::i18n::tr("pas une passerelle mxTorrent", "not an mxTorrent gateway"))?;
            if v["protocol"] != "brocolis-v4" {
                return Err(crate::i18n::tr("protocole inconnu", "unknown protocol"));
            }
            let bundle = KeyBundle::from_base64(v["bundle"].as_str().unwrap_or("")).map_err(|e| format!("{e:?}"))?;
            bundle.verify().map_err(|e| format!("{e:?}"))?;
            Ok(bundle)
        });
        let bundle = match parsed {
            Ok(b) => b,
            Err(e) => return self.gateway_fail(e),
        };
        let Some(g) = self.gateway.as_mut() else { return };
        match g.device.initiate4(&bundle, &mut hasard::system()) {
            Ok(s) => {
                g.fingerprint = Some(bundle.identity.fingerprint());
                g.session = Some(s);
                g.sent_initial = false;
                match GatewaySocket::connect(&g.ws_url()) {
                    Ok(sock) => {
                        g.sock = Some(sock);
                        g.phase = Phase::Connecting;
                        g.detail = crate::i18n::tr("connexion…", "connecting…");
                    }
                    Err(e) => self.gateway_fail(format!("{e:?}")),
                }
            }
            Err(e) => self.gateway_fail(format!("{e:?}")),
        }
    }

    pub(crate) fn on_gateway_open(&mut self) {
        let Some(g) = self.gateway.as_mut() else { return };
        // Premier message : le préambule Brocolis (un lot vide suffit).
        g.send_out();
    }

    pub(crate) fn on_gateway_closed(&mut self) {
        if self.gateway.as_ref().is_some_and(|g| g.phase != Phase::Off) {
            self.gateway_fail(crate::i18n::tr("connexion perdue, nouvel essai bientôt", "connection lost, retrying soon"));
        }
    }

    pub(crate) fn on_gateway_data(&mut self, bytes: Vec<u8>) {
        let frames = {
            let Some(g) = self.gateway.as_mut() else { return };
            let Some(session) = g.session.as_mut() else { return };
            let opened = Token4::from_bytes(&bytes).and_then(|t| session.decrypt(&t, now_secs(), &mut hasard::system()));
            let text = match opened {
                Ok(Content::Text(t)) => t,
                _ => return self.gateway_fail(crate::i18n::tr("message de la passerelle illisible", "unreadable gateway message")),
            };
            match B64.decode(text.as_bytes()).ok().and_then(|b| Frame::decode_batch(&b)) {
                Some(f) => f,
                None => return self.gateway_fail(crate::i18n::tr("lot mal formé", "malformed batch")),
            }
        };
        if let Some(g) = self.gateway.as_mut()
            && g.phase != Phase::Ready
        {
            g.phase = Phase::Ready;
            g.backoff = 5_000;
            g.detail = String::new();
            let fp = g.fingerprint.clone().unwrap_or_default();
            self.logline(format!("passerelle : session Brocolis v4 établie ({fp})"));
            self.dirty = true;
        }
        for f in frames {
            self.on_gateway_frame(f);
        }
    }

    fn on_gateway_frame(&mut self, f: Frame) {
        match f {
            Frame::Peers { req, peers, .. } => {
                let Some(g) = self.gateway.as_mut() else { return };
                let Some(h) = g.lookups.get(&req).copied() else { return };
                let tried = g.tried.entry(h).or_default();
                let q = g.queue.entry(h).or_default();
                let mut n = 0;
                for p in peers {
                    if !tried.contains(&p) && !q.contains(&p) {
                        q.push_back(p);
                        n += 1;
                    }
                }
                *g.found.entry(h).or_default() += n;
                self.dirty = true;
            }
            Frame::LookupDone { req, found, summary } => {
                if let Some(g) = self.gateway.as_mut() {
                    g.lookups.remove(&req);
                }
                self.logline(format!("passerelle : {found} pairs trouvés ({summary})"));
            }
            Frame::Connected { conn, encrypted } => {
                let Some(g) = self.gateway.as_mut() else { return };
                let Some(l) = g.links.get_mut(&conn) else { return };
                l.connected = true;
                l.encrypted = encrypted;
                let (h, k) = (l.info_hash, l.key);
                if let Some(i) = self.find(&h) {
                    self.entries[i].torrent.add_peer(k);
                }
                self.dirty = true;
            }
            Frame::DataIn { conn, data } => {
                let Some(g) = self.gateway.as_ref() else { return };
                let Some(l) = g.links.get(&conn) else { return };
                let (h, k) = (l.info_hash, l.key);
                if let Some(i) = self.find(&h) {
                    self.entries[i].torrent.on_data(k, &data);
                }
            }
            Frame::Failed { conn, .. } | Frame::Closed { conn } => {
                let Some(g) = self.gateway.as_mut() else { return };
                if let Some(l) = g.links.remove(&conn) {
                    g.by_key.remove(&l.key);
                    if let Some(i) = self.find(&l.info_hash) {
                        self.entries[i].torrent.remove_peer(l.key);
                    }
                }
            }
            _ => {}
        }
    }

    /// Recherches périodiques et nouvelles connexions.
    fn gateway_schedule(&mut self, now: u64) {
        let max_peers = self.settings.max_peers;
        let jobs: Vec<(InfoHash, Vec<String>, bool, bool, usize)> = self
            .entries
            .iter()
            .filter(|e| !e.torrent.is_paused())
            .map(|e| {
                let t = &e.torrent;
                let classic: Vec<String> = t
                    .trackers
                    .iter()
                    .filter(|u| u.starts_with("udp://") || u.starts_with("http://") || u.starts_with("https://"))
                    .cloned()
                    .collect();
                let private = t.metainfo().is_some_and(|m| m.info.private);
                (e.info_hash(), classic, private, t.state() == State::Seeding, t.peer_count())
            })
            .collect();
        for (h, classic, private, seeding, peers) in jobs {
            let room = {
                let Some(g) = self.gateway.as_mut() else { return };
                let last = g.last_lookup.get(&h).copied().unwrap_or(0);
                let every = if seeding {
                    900_000
                } else if peers < 5 {
                    60_000
                } else {
                    300_000
                };
                if (last == 0 || now.saturating_sub(last) >= every) && (!classic.is_empty() || !private) {
                    g.last_lookup.insert(h, now);
                    let req = g.next_req;
                    g.next_req += 1;
                    g.lookups.insert(req, h);
                    g.push(Frame::Lookup { req, info_hash: h, flags: if private { 0 } else { FLAG_DHT }, trackers: classic });
                }
                // Connexions : dans la limite des pairs voulus.
                let mine = g.links.values().filter(|l| l.info_hash == h).count();
                let pending = g.links.values().filter(|l| l.info_hash == h && !l.connected).count();
                let queued = g.queue.get(&h).map(|q| q.len()).unwrap_or(0);
                max_peers
                    .saturating_sub(peers.max(mine))
                    .min(MAX_GATEWAY_PEERS.saturating_sub(mine))
                    .min(MAX_PENDING_CONNECTS.saturating_sub(pending))
                    .min(queued)
            };
            let keys: Vec<PeerKey> = (0..room).map(|_| self.new_key()).collect();
            let Some(g) = self.gateway.as_mut() else { return };
            for key in keys {
                let Some(addr) = g.queue.get_mut(&h).and_then(|q| q.pop_front()) else { break };
                g.tried.entry(h).or_default().insert(addr);
                let conn = g.next_conn;
                g.next_conn += 1;
                g.links.insert(conn, PeerLink { info_hash: h, key, connected: false, encrypted: false });
                g.by_key.insert(key, conn);
                g.push(Frame::Connect { conn, info_hash: h, addr });
            }
        }
    }

    /// Envoie les octets que les sessions ont pour les pairs relayés.
    pub(crate) fn gateway_flush(&mut self) {
        let Some(g) = self.gateway.as_ref() else { return };
        if g.phase != Phase::Ready {
            return;
        }
        let mut budget = g.budget();
        let keys: Vec<(u32, InfoHash, PeerKey)> =
            g.links.iter().filter(|(_, l)| l.connected).map(|(c, l)| (*c, l.info_hash, l.key)).collect();
        let mut frames = Vec::new();
        for (conn, h, key) in keys {
            if budget == 0 {
                break;
            }
            let Some(i) = self.find(&h) else { continue };
            let t = &mut self.entries[i].torrent;
            if !t.wants_to_send(key) {
                continue;
            }
            let data = t.poll_transmit(key, budget.min(512 * 1024));
            budget = budget.saturating_sub(data.len());
            if !data.is_empty() {
                frames.push(Frame::Data { conn, data });
            }
        }
        let g = self.gateway.as_mut().unwrap();
        for f in frames {
            g.push(f);
        }
        g.send_out();
    }

    /// Le moteur ferme un pair relayé.
    pub(crate) fn gateway_close(&mut self, key: PeerKey) -> bool {
        match self.gateway.as_mut() {
            Some(g) if g.is_gateway_peer(key) => {
                g.close_peer(key);
                true
            }
            _ => false,
        }
    }

    /// Un torrent retiré ou mis en pause : ses pairs relayés sont fermés.
    pub(crate) fn gateway_forget(&mut self, h: &InfoHash) {
        let Some(g) = self.gateway.as_mut() else { return };
        let keys: Vec<PeerKey> = g.links.values().filter(|l| l.info_hash == *h).map(|l| l.key).collect();
        for k in keys {
            g.close_peer(k);
        }
        g.queue.remove(h);
        g.last_lookup.remove(h);
        g.send_out();
    }

    pub fn gateway_status(&self) -> (Phase, String, Option<String>, usize) {
        match &self.gateway {
            Some(g) => (g.phase, g.detail.clone(), g.fingerprint.clone(), g.connected_peers()),
            None => (Phase::Off, String::new(), None, 0),
        }
    }

    pub fn is_gateway_peer(&self, key: PeerKey) -> bool {
        self.gateway.as_ref().is_some_and(|g| g.is_gateway_peer(key))
    }

    pub fn gateway_peer_encrypted(&self, key: PeerKey) -> bool {
        self.gateway.as_ref().is_some_and(|g| g.peer_encrypted(key))
    }
}
