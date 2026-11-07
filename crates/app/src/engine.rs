//! Le moteur du client : relie les sessions BitTorrent du cœur aux trackers
//! WebSocket, aux connexions WebRTC et aux sources HTTP.

use crate::net::{self, NetEvent, PeerConn, TrackerSocket};
use crate::web;
use mxtorrent_core::torrent::{Event, PeerKey, State, WEBSEED_KEY_BASE};
use mxtorrent_core::tracker::{Announce, AnnounceEvent, OfferOut, TrackerMessage, answer_json};
use mxtorrent_core::util::percent_encode;
use mxtorrent_core::{CreateOptions, InfoHash, Magnet, Metainfo, NewFile, Rng, Torrent, create_torrent, make_peer_id};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

pub const DEFAULT_TRACKERS: &[&str] = &[
    "wss://tracker.openwebtorrent.com",
    "wss://tracker.webtorrent.dev",
    "wss://tracker.btorrent.xyz",
    "wss://tracker.files.fm:7073/announce",
];

pub const DEFAULT_STUN: &[&str] = &["stun:stun.l.google.com:19302", "stun:global.stun.twilio.com:3478"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub trackers: Vec<String>,
    pub stun: Vec<String>,
    pub max_peers: usize,
    pub offers_per_announce: usize,
    pub use_web_seeds: bool,
    pub seed_after_download: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            trackers: DEFAULT_TRACKERS.iter().map(|s| s.to_string()).collect(),
            stun: DEFAULT_STUN.iter().map(|s| s.to_string()).collect(),
            max_peers: 40,
            offers_per_announce: 5,
            use_web_seeds: true,
            seed_after_download: true,
        }
    }
}

/// État d'un tracker pour un torrent, pour l'interface.
#[derive(Clone, Debug, Default)]
pub struct TrackerStatus {
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    pub message: Option<String>,
    pub last_announce: u64,
    pub next_announce: u64,
    pub interval: u64,
    pub announced_started: bool,
}

struct WebSeed {
    url: String,
    key: PeerKey,
    in_flight: Option<u32>,
    failures: u32,
    retry_at: u64,
    pieces: u64,
}

pub struct Entry {
    pub torrent: Torrent,
    pub trackers: HashMap<String, TrackerStatus>,
    pub completed_at: Option<u64>,
    pub created_here: bool,
    web_seeds: Vec<WebSeed>,
}

impl Entry {
    pub fn info_hash(&self) -> InfoHash {
        self.torrent.info_hash()
    }

    pub fn magnet(&self) -> String {
        match self.torrent.metainfo() {
            Some(m) => Magnet::from_metainfo(m).to_uri(),
            None => Magnet {
                info_hash: self.torrent.info_hash(),
                name: Some(self.torrent.name()),
                trackers: self.torrent.trackers.clone(),
                web_seeds: self.torrent.web_seeds.clone(),
                length: None,
            }
            .to_uri(),
        }
    }

    pub fn web_seed_status(&self) -> Vec<(String, u64, u32)> {
        self.web_seeds.iter().map(|w| (w.url.clone(), w.pieces, w.failures)).collect()
    }

    pub fn swarm_counts(&self) -> (Option<u32>, Option<u32>) {
        let s = self.trackers.values().filter_map(|t| t.seeders).max();
        let l = self.trackers.values().filter_map(|t| t.leechers).max();
        (s, l)
    }
}

struct Tracker {
    sock: Option<TrackerSocket>,
    open: bool,
    retry_at: u64,
    backoff: u64,
}

struct Pending {
    conn: PeerConn,
    info_hash: InfoHash,
    tracker: String,
    created: u64,
    /// Pour une offre : son identifiant ; pour une réponse : (pair distant, identifiant).
    offer_id: [u8; 20],
    remote_peer: Option<[u8; 20]>,
}

struct Batch {
    info_hash: InfoHash,
    tracker: String,
    waiting: usize,
    offers: Vec<OfferOut>,
    event: AnnounceEvent,
}

/// Une notification pour l'interface (bandeau éphémère).
#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub at: u64,
    pub good: bool,
}

pub struct Engine {
    pub peer_id: [u8; 20],
    pub settings: Settings,
    pub entries: Vec<Entry>,
    trackers: HashMap<String, Tracker>,
    pending: HashMap<PeerKey, Pending>,
    offer_index: HashMap<[u8; 20], PeerKey>,
    conns: HashMap<PeerKey, (InfoHash, PeerConn)>,
    batches: HashMap<u64, Batch>,
    next_key: PeerKey,
    next_batch: u64,
    rng: Rng,
    pub toasts: VecDeque<Toast>,
    pub log: VecDeque<String>,
    /// Torrent créé à l'instant, à montrer dans la fenêtre de partage.
    pub just_created: Option<InfoHash>,
    pub dirty: bool,
}

const OFFER_TTL_MS: u64 = 50_000;
const MIN_ANNOUNCE_MS: u64 = 30_000;
const DEFAULT_INTERVAL_MS: u64 = 120_000;

impl Engine {
    pub fn new(settings: Settings) -> Engine {
        let mut rng = Rng::new(web::random_u64());
        Engine {
            peer_id: make_peer_id(&mut rng),
            settings,
            entries: Vec::new(),
            trackers: HashMap::new(),
            pending: HashMap::new(),
            offer_index: HashMap::new(),
            conns: HashMap::new(),
            batches: HashMap::new(),
            next_key: 1,
            next_batch: 1,
            rng,
            toasts: VecDeque::new(),
            log: VecDeque::new(),
            just_created: None,
            dirty: false,
        }
    }

    fn key(&mut self) -> PeerKey {
        self.next_key += 1;
        self.next_key
    }

    pub fn toast(&mut self, text: impl Into<String>, good: bool) {
        let text = text.into();
        self.logline(text.clone());
        self.toasts.push_back(Toast { text, at: web::now_ms(), good });
        while self.toasts.len() > 4 {
            self.toasts.pop_front();
        }
    }

    pub fn logline(&mut self, text: impl Into<String>) {
        let d = js_sys::Date::new_0();
        self.log.push_back(format!("{:02}:{:02}:{:02}  {}", d.get_hours(), d.get_minutes(), d.get_seconds(), text.into()));
        while self.log.len() > 300 {
            self.log.pop_front();
        }
    }

    pub fn find(&self, h: &InfoHash) -> Option<usize> {
        self.entries.iter().position(|e| e.info_hash() == *h)
    }

    // ------------------------------------------------------------ ajout

    /// Lien magnet, empreinte, ou URL d'un fichier `.torrent`.
    pub fn add_link(&mut self, input: &str) -> Result<(), String> {
        let input = input.trim();
        if input.starts_with("http://") || input.starts_with("https://") {
            let url = input.to_string();
            self.toast(crate::i18n::tr("Téléchargement du .torrent…", "Downloading the .torrent…"), true);
            wasm_bindgen_futures::spawn_local(async move {
                let r = web::fetch_bytes(&url, None).await.map(|(_, b)| b);
                net::push(NetEvent::Fetched(url, r));
            });
            return Ok(());
        }
        let m =
            Magnet::parse(input).ok_or_else(|| crate::i18n::tr("Lien magnet ou empreinte invalide", "Invalid magnet link or info hash"))?;
        if let Some(i) = self.find(&m.info_hash) {
            for t in m.trackers {
                if !self.entries[i].torrent.trackers.contains(&t) {
                    self.entries[i].torrent.trackers.push(t);
                }
            }
            return Err(crate::i18n::tr("Ce torrent est déjà dans la liste", "This torrent is already in the list"));
        }
        let t = Torrent::from_magnet(&m, self.peer_id, self.rng.next_u64(), web::now_ms());
        self.insert(t, false);
        Ok(())
    }

    pub fn add_torrent_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
        let meta = Metainfo::from_torrent(bytes).map_err(|e| e.to_string())?;
        if self.find(&meta.info_hash).is_some() {
            return Err(crate::i18n::tr("Ce torrent est déjà dans la liste", "This torrent is already in the list"));
        }
        let t = Torrent::from_metainfo(meta, self.peer_id, self.rng.next_u64(), web::now_ms());
        self.insert(t, false);
        Ok(())
    }

    /// Crée un torrent à partir de fichiers locaux et le partage aussitôt.
    pub fn share_files(&mut self, files: Vec<(String, Vec<u8>)>) -> Result<InfoHash, String> {
        if files.is_empty() {
            return Err(crate::i18n::tr("Aucun fichier", "No file"));
        }
        let new: Vec<NewFile> = files
            .iter()
            .map(|(p, d)| NewFile { path: p.split('/').filter(|s| !s.is_empty()).map(String::from).collect(), data: d })
            .collect();
        let name = if files.len() == 1 { None } else { Some(format!("mxTorrent-{}", files.len())) };
        let name = match new.first() {
            Some(f) if files.len() > 1 && f.path.len() > 1 && new.iter().all(|x| x.path.len() > 1 && x.path[0] == f.path[0]) => {
                Some(f.path[0].clone())
            }
            _ => name,
        };
        let opts = CreateOptions {
            name,
            trackers: self.settings.trackers.clone(),
            creation_date: Some((web::now_ms() / 1000) as i64),
            ..Default::default()
        };
        let meta = create_torrent(&new, &opts).map_err(|e| e.to_string())?;
        let h = meta.info_hash;
        if let Some(i) = self.find(&h) {
            self.just_created = Some(h);
            let _ = i;
            return Ok(h);
        }
        let datas: Vec<&[u8]> = files.iter().map(|(_, d)| d.as_slice()).collect();
        let t = Torrent::seed(meta, &datas, self.peer_id, self.rng.next_u64(), web::now_ms())?;
        drop(datas);
        drop(files);
        self.insert(t, true);
        self.just_created = Some(h);
        Ok(h)
    }

    fn insert(&mut self, mut torrent: Torrent, created_here: bool) {
        // Les trackers du réglage complètent ceux du torrent (pas pour un torrent privé).
        let private = torrent.metainfo().is_some_and(|m| m.info.private);
        if !private {
            for t in &self.settings.trackers {
                if !torrent.trackers.contains(t) {
                    torrent.trackers.push(t.clone());
                }
            }
        }
        let now = web::now_ms();
        let name = torrent.name();
        let web_seeds = torrent
            .web_seeds
            .iter()
            .enumerate()
            .map(|(i, u)| WebSeed {
                url: u.clone(),
                key: WEBSEED_KEY_BASE + i as u64,
                in_flight: None,
                failures: 0,
                retry_at: 0,
                pieces: 0,
            })
            .collect();
        let trackers = torrent
            .trackers
            .iter()
            .filter(|u| u.starts_with("wss://") || u.starts_with("ws://"))
            .map(|u| (u.clone(), TrackerStatus::default()))
            .collect();
        self.entries.push(Entry { torrent, trackers, completed_at: if created_here { Some(now) } else { None }, created_here, web_seeds });
        self.dirty = true;
        self.toast(format!("{} {name}", crate::i18n::tr("Ajouté :", "Added:")), true);
        self.ensure_trackers();
    }

    pub fn remove(&mut self, h: &InfoHash) {
        let Some(i) = self.find(h) else { return };
        self.announce_stop(i);
        let e = self.entries.remove(i);
        let keys: Vec<PeerKey> = self.conns.iter().filter(|(_, (ih, _))| ih == h).map(|(k, _)| *k).collect();
        for k in keys {
            self.conns.remove(&k);
        }
        self.pending.retain(|_, p| p.info_hash != *h);
        let pending = &self.pending;
        self.offer_index.retain(|_, k| pending.contains_key(k));
        self.dirty = true;
        self.toast(format!("{} {}", crate::i18n::tr("Retiré :", "Removed:"), e.torrent.name()), true);
    }

    pub fn set_paused(&mut self, h: &InfoHash, paused: bool) {
        let Some(i) = self.find(h) else { return };
        if paused {
            self.announce_stop(i);
        }
        self.entries[i].torrent.set_paused(paused);
        if !paused {
            for st in self.entries[i].trackers.values_mut() {
                st.next_announce = 0;
                st.announced_started = false;
            }
        }
        self.dirty = true;
        self.handle_core_events();
    }

    fn announce_stop(&mut self, i: usize) {
        let e = &self.entries[i];
        let a = Announce {
            info_hash: e.info_hash(),
            peer_id: self.peer_id,
            uploaded: e.torrent.uploaded,
            downloaded: e.torrent.downloaded,
            left: e.torrent.left(),
            event: AnnounceEvent::Stopped,
            numwant: 0,
            offers: vec![],
        };
        let text = a.to_json();
        for url in e.trackers.keys() {
            if let Some(t) = self.trackers.get(url)
                && let Some(s) = &t.sock
            {
                s.send(&text);
            }
        }
    }

    // ------------------------------------------------------------ boucle

    /// Traite les événements du navigateur puis fait avancer les sessions.
    pub fn pump(&mut self) {
        for ev in net::take_events() {
            self.on_net(ev);
        }
        self.handle_core_events();
        self.flush();
    }

    /// Appelé quatre fois par seconde.
    pub fn tick(&mut self) {
        let now = web::now_ms();
        for e in &mut self.entries {
            e.torrent.tick(now);
        }
        self.handle_core_events();
        self.ensure_trackers();
        self.schedule_announces(now);
        self.expire_pending(now);
        self.run_web_seeds(now);
        self.flush();
        let before = self.toasts.len();
        self.toasts.retain(|t| now.saturating_sub(t.at) < 5_000);
        if self.toasts.len() != before {
            self.dirty = true;
        }
    }

    fn on_net(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::TrackerOpen(url) => {
                if let Some(t) = self.trackers.get_mut(&url) {
                    t.open = true;
                    t.backoff = 5_000;
                }
                self.logline(format!("tracker {url} : {}", crate::i18n::tr("connecté", "connected")));
                for e in &mut self.entries {
                    if let Some(st) = e.trackers.get_mut(&url) {
                        st.next_announce = 0;
                        st.message = None;
                    }
                }
                self.schedule_announces(web::now_ms());
            }
            NetEvent::TrackerClosed(url) => {
                let now = web::now_ms();
                if let Some(t) = self.trackers.get_mut(&url)
                    && t.sock.is_some()
                {
                    t.sock = None;
                    t.open = false;
                    t.retry_at = now + t.backoff;
                    t.backoff = (t.backoff * 2).min(300_000);
                    for e in &mut self.entries {
                        if let Some(st) = e.trackers.get_mut(&url) {
                            st.message = Some(crate::i18n::tr("injoignable, nouvel essai bientôt", "unreachable, retrying soon"));
                            st.announced_started = false;
                        }
                    }
                }
            }
            NetEvent::TrackerText(url, text) => self.on_tracker(url, TrackerMessage::parse(&text)),
            NetEvent::OfferReady { batch, key, sdp } => {
                let Some(b) = self.batches.get_mut(&batch) else { return };
                b.waiting = b.waiting.saturating_sub(1);
                match (sdp, self.pending.get(&key)) {
                    (Some(sdp), Some(p)) => b.offers.push(OfferOut { offer_id: p.offer_id, sdp }),
                    _ => {
                        self.pending.remove(&key);
                    }
                }
                if b.waiting == 0 {
                    let b = self.batches.remove(&batch).unwrap();
                    self.send_announce(b);
                }
            }
            NetEvent::AnswerReady { key, sdp } => {
                let Some(p) = self.pending.get(&key) else { return };
                match (sdp, p.remote_peer) {
                    (Some(sdp), Some(to)) => {
                        let text = answer_json(&p.info_hash, &self.peer_id, &to, &p.offer_id, &sdp);
                        let sent = self.trackers.get(&p.tracker).and_then(|t| t.sock.as_ref()).is_some_and(|s| s.send(&text));
                        if !sent {
                            self.pending.remove(&key);
                        }
                    }
                    _ => {
                        self.pending.remove(&key);
                    }
                }
            }
            NetEvent::ChannelOpen(key) => {
                if let Some(mut p) = self.pending.remove(&key) {
                    if !p.conn.adopt_channel(key) || !p.conn.is_channel_open() {
                        self.pending.insert(key, p);
                        return;
                    }
                    self.offer_index.remove(&p.offer_id);
                    let Some(i) = self.find(&p.info_hash) else { return };
                    if self.entries[i].torrent.is_paused() || self.entries[i].torrent.peer_count() >= self.settings.max_peers {
                        return;
                    }
                    self.entries[i].torrent.add_peer(key);
                    p.conn.open = true;
                    self.conns.insert(key, (p.info_hash, p.conn));
                }
            }
            NetEvent::ChannelData(key, data) => {
                if let Some((h, _)) = self.conns.get(&key) {
                    let h = *h;
                    if let Some(i) = self.find(&h) {
                        self.entries[i].torrent.on_data(key, &data);
                    }
                }
            }
            NetEvent::ChannelClosed(key) => {
                self.pending.remove(&key);
                if let Some((h, _)) = self.conns.remove(&key)
                    && let Some(i) = self.find(&h)
                {
                    self.entries[i].torrent.remove_peer(key);
                }
            }
            NetEvent::ChannelLow => {}
            NetEvent::WebSeed { key, piece, result } => self.on_web_seed(key, piece, result),
            NetEvent::Share(files) => {
                if files.is_empty() {
                    return;
                }
                match self.share_files(files) {
                    Ok(_) => {}
                    Err(e) => self.toast(e, false),
                }
            }
            NetEvent::TorrentFiles(files) => {
                for (name, bytes) in files {
                    if let Err(e) = self.add_torrent_bytes(&bytes) {
                        self.toast(format!("{name} : {e}"), false);
                    }
                }
            }
            NetEvent::Link(l) => {
                if let Err(e) = self.add_link(&l) {
                    self.toast(e, false);
                }
            }
            NetEvent::Fetched(url, r) => match r.and_then(|b| self.add_torrent_bytes(&b)) {
                Ok(()) => {}
                Err(e) => self.toast(format!("{url} : {e}"), false),
            },
        }
        self.dirty = true;
    }

    fn on_tracker(&mut self, url: String, msg: TrackerMessage) {
        let now = web::now_ms();
        match msg {
            TrackerMessage::Stats { info_hash, interval, complete, incomplete } => {
                let Some(i) = self.find(&info_hash) else { return };
                if let Some(st) = self.entries[i].trackers.get_mut(&url) {
                    if complete.is_some() {
                        st.seeders = complete;
                    }
                    if incomplete.is_some() {
                        st.leechers = incomplete;
                    }
                    if let Some(iv) = interval {
                        st.interval = (iv as u64 * 1000).clamp(MIN_ANNOUNCE_MS, 30 * 60_000);
                    }
                    st.message = None;
                }
            }
            TrackerMessage::Offer { info_hash, peer_id, offer_id, sdp } => {
                let Some(i) = self.find(&info_hash) else { return };
                let t = &self.entries[i].torrent;
                if t.is_paused() || peer_id == self.peer_id || t.is_connected_to(&peer_id) || t.peer_count() >= self.settings.max_peers {
                    return;
                }
                let key = self.key();
                match PeerConn::answer(key, sdp, &self.settings.stun) {
                    Ok(conn) => {
                        self.pending
                            .insert(key, Pending { conn, info_hash, tracker: url, created: now, offer_id, remote_peer: Some(peer_id) });
                    }
                    Err(e) => self.logline(format!("WebRTC : {e:?}")),
                }
            }
            TrackerMessage::Answer { info_hash, peer_id, offer_id, sdp } => {
                let Some(&key) = self.offer_index.get(&offer_id) else { return };
                if let Some(p) = self.pending.get_mut(&key)
                    && p.info_hash == info_hash
                    && p.remote_peer.is_none()
                {
                    p.remote_peer = Some(peer_id);
                    p.conn.accept_answer(sdp);
                }
            }
            TrackerMessage::Failure(reason) | TrackerMessage::Warning(reason) => {
                for e in &mut self.entries {
                    if let Some(st) = e.trackers.get_mut(&url) {
                        st.message = Some(reason.clone());
                    }
                }
            }
            TrackerMessage::Other => {}
        }
    }

    fn handle_core_events(&mut self) {
        let now = web::now_ms();
        let mut closes = Vec::new();
        let mut toasts = Vec::new();
        for e in &mut self.entries {
            for ev in e.torrent.take_events() {
                match ev {
                    Event::ClosePeer(k, _reason) => closes.push(k),
                    Event::Completed => {
                        if e.completed_at.is_none() {
                            e.completed_at = Some(now);
                            toasts.push(format!("{} {}", crate::i18n::tr("Terminé :", "Done:"), e.torrent.name()));
                            for st in e.trackers.values_mut() {
                                st.next_announce = 0; // annonce « completed »
                            }
                        }
                    }
                    Event::MetadataReady => {
                        self.dirty = true;
                        // Les sources web peuvent démarrer maintenant que l'on connaît les fichiers.
                    }
                    Event::PieceFailed(_) | Event::PieceVerified(_) => {}
                }
            }
        }
        for t in toasts {
            self.toast(t, true);
            web::notify_title("mxTorrent ✓");
        }
        for k in closes {
            if let Some((h, _conn)) = self.conns.remove(&k)
                && let Some(i) = self.find(&h)
            {
                self.entries[i].torrent.remove_peer(k);
            }
        }
    }

    /// Envoie ce que les sessions ont à dire, sans déborder les tampons WebRTC.
    fn flush(&mut self) {
        let mut dead = Vec::new();
        for (key, (h, conn)) in &self.conns {
            let Some(e) = self.entries.iter_mut().find(|e| e.torrent.info_hash() == *h) else {
                dead.push(*key);
                continue;
            };
            if !e.torrent.wants_to_send(*key) {
                continue;
            }
            let budget = conn.budget();
            if budget == 0 {
                continue;
            }
            let out = e.torrent.poll_transmit(*key, budget);
            if !out.is_empty() && !conn.send(&out) {
                dead.push(*key);
            }
        }
        for k in dead {
            if let Some((h, _)) = self.conns.remove(&k)
                && let Some(i) = self.find(&h)
            {
                self.entries[i].torrent.remove_peer(k);
            }
        }
    }

    // ------------------------------------------------------------ trackers

    fn ensure_trackers(&mut self) {
        let now = web::now_ms();
        let mut wanted: Vec<String> = Vec::new();
        for e in &self.entries {
            if !e.torrent.is_paused() {
                for u in e.trackers.keys() {
                    if !wanted.contains(u) {
                        wanted.push(u.clone());
                    }
                }
            }
        }
        for url in &wanted {
            let t = self.trackers.entry(url.clone()).or_insert(Tracker { sock: None, open: false, retry_at: 0, backoff: 5_000 });
            if t.sock.is_none() && now >= t.retry_at {
                match TrackerSocket::connect(url) {
                    Ok(s) => t.sock = Some(s),
                    Err(_) => {
                        t.retry_at = now + t.backoff;
                        t.backoff = (t.backoff * 2).min(300_000);
                    }
                }
            }
        }
        // Les trackers qui ne servent plus sont fermés.
        self.trackers.retain(|u, _| wanted.contains(u));
    }

    pub fn tracker_connected(&self, url: &str) -> bool {
        self.trackers.get(url).is_some_and(|t| t.open)
    }

    fn schedule_announces(&mut self, now: u64) {
        let mut jobs = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.torrent.is_paused() {
                continue;
            }
            for (url, st) in &e.trackers {
                if !self.trackers.get(url).is_some_and(|t| t.open) || now < st.next_announce {
                    continue;
                }
                jobs.push((i, url.clone()));
            }
        }
        for (i, url) in jobs {
            let max_peers = self.settings.max_peers;
            let e = &mut self.entries[i];
            let st = e.trackers.get_mut(&url).unwrap();
            let interval = if st.interval == 0 { DEFAULT_INTERVAL_MS } else { st.interval };
            let peers = e.torrent.peer_count();
            // Peu de pairs : on réannonce plus souvent.
            let wait = if peers < 4 && e.torrent.state() != State::Seeding { MIN_ANNOUNCE_MS.max(interval / 4) } else { interval };
            st.last_announce = now;
            st.next_announce = now + wait;
            let event = if !st.announced_started {
                st.announced_started = true;
                AnnounceEvent::Started
            } else if e.completed_at.is_some_and(|c| c >= now.saturating_sub(1000)) {
                AnnounceEvent::Completed
            } else {
                AnnounceEvent::Update
            };
            let h = e.info_hash();
            let n_offers = if peers >= max_peers { 0 } else { self.settings.offers_per_announce.min(max_peers - peers) };
            self.start_batch(h, url, n_offers, event);
        }
    }

    fn start_batch(&mut self, info_hash: InfoHash, tracker: String, n: usize, event: AnnounceEvent) {
        let batch = self.next_batch;
        self.next_batch += 1;
        let mut waiting = 0;
        for _ in 0..n {
            let key = self.key();
            let mut offer_id = [0u8; 20];
            self.rng.fill(&mut offer_id);
            match PeerConn::offer(key, batch, &self.settings.stun) {
                Ok(conn) => {
                    self.pending.insert(
                        key,
                        Pending { conn, info_hash, tracker: tracker.clone(), created: web::now_ms(), offer_id, remote_peer: None },
                    );
                    self.offer_index.insert(offer_id, key);
                    waiting += 1;
                }
                Err(e) => self.logline(format!("WebRTC : {e:?}")),
            }
        }
        let b = Batch { info_hash, tracker, waiting, offers: Vec::new(), event };
        if waiting == 0 {
            self.send_announce(b);
        } else {
            self.batches.insert(batch, b);
        }
    }

    fn send_announce(&mut self, b: Batch) {
        let Some(i) = self.find(&b.info_hash) else { return };
        let e = &self.entries[i];
        let a = Announce {
            info_hash: b.info_hash,
            peer_id: self.peer_id,
            uploaded: e.torrent.uploaded,
            downloaded: e.torrent.downloaded,
            left: e.torrent.left(),
            event: b.event,
            numwant: b.offers.len() as u32,
            offers: b.offers,
        };
        if let Some(s) = self.trackers.get(&b.tracker).and_then(|t| t.sock.as_ref()) {
            s.send(&a.to_json());
        }
    }

    fn expire_pending(&mut self, now: u64) {
        let before = self.pending.len();
        self.pending.retain(|_, p| now.saturating_sub(p.created) < OFFER_TTL_MS);
        if self.pending.len() != before {
            let pending = &self.pending;
            self.offer_index.retain(|_, k| pending.contains_key(k));
        }
    }

    // ------------------------------------------------------------ sources web

    fn run_web_seeds(&mut self, now: u64) {
        if !self.settings.use_web_seeds {
            return;
        }
        for e in &mut self.entries {
            if e.torrent.metainfo().is_none() || e.torrent.is_complete() || e.torrent.is_paused() {
                continue;
            }
            for ws in &mut e.web_seeds {
                if ws.in_flight.is_some() || now < ws.retry_at || ws.failures >= 8 {
                    continue;
                }
                let Some(piece) = e.torrent.webseed_pick(ws.key) else { break };
                ws.in_flight = Some(piece);
                let meta = e.torrent.metainfo().unwrap();
                let ranges: Vec<(String, u64, u64)> = e
                    .torrent
                    .piece_file_ranges(piece)
                    .into_iter()
                    .map(|(fi, off, len)| (web_seed_url(&ws.url, meta, fi), off, len))
                    .collect();
                let key = ws.key;
                wasm_bindgen_futures::spawn_local(async move {
                    let mut data = Vec::new();
                    let mut err = None;
                    for (url, off, len) in ranges {
                        match web::fetch_bytes(&url, Some((off, off + len - 1))).await {
                            Ok((206, b)) if b.len() as u64 == len => data.extend_from_slice(&b),
                            Ok((200, b)) if b.len() as u64 >= off + len => data.extend_from_slice(&b[off as usize..(off + len) as usize]),
                            Ok((s, _)) => {
                                err = Some(format!("HTTP {s}"));
                                break;
                            }
                            Err(e) => {
                                err = Some(e);
                                break;
                            }
                        }
                    }
                    net::push(NetEvent::WebSeed {
                        key,
                        piece,
                        result: match err {
                            Some(e) => Err(e),
                            None => Ok(data),
                        },
                    });
                });
            }
        }
    }

    fn on_web_seed(&mut self, key: PeerKey, piece: u32, result: Result<Vec<u8>, String>) {
        let now = web::now_ms();
        for e in &mut self.entries {
            let Some(ws) = e.web_seeds.iter_mut().find(|w| w.key == key && w.in_flight == Some(piece)) else { continue };
            ws.in_flight = None;
            match result {
                Ok(data) => {
                    if e.torrent.webseed_done(key, piece, data) {
                        ws.failures = 0;
                        ws.pieces += 1;
                    } else {
                        ws.failures += 1;
                    }
                }
                Err(_) => {
                    e.torrent.webseed_failed(key, piece);
                    ws.failures += 1;
                    ws.retry_at = now + 2_000 * (1 << ws.failures.min(6)) as u64;
                }
            }
            break;
        }
        self.handle_core_events();
    }

    // ------------------------------------------------------------ chiffres globaux

    pub fn total_rates(&self) -> (f64, f64) {
        self.entries.iter().fold((0.0, 0.0), |(d, u), e| (d + e.torrent.down_rate(), u + e.torrent.up_rate()))
    }

    pub fn connected_trackers(&self) -> usize {
        self.trackers.values().filter(|t| t.open).count()
    }

    pub fn connecting_peers(&self) -> usize {
        self.pending.len()
    }

    // ------------------------------------------------------------ sauvegarde

    pub fn snapshot(&self) -> Saved {
        Saved {
            settings: self.settings.clone(),
            torrents: self
                .entries
                .iter()
                .map(|e| SavedTorrent {
                    torrent: e.torrent.metainfo().map(|m| crate::b64::encode(&m.to_torrent())),
                    magnet: e.magnet(),
                    paused: e.torrent.is_paused(),
                    unwanted: e.torrent.file_wanted().iter().enumerate().filter(|(_, w)| !**w).map(|(i, _)| i).collect(),
                })
                .collect(),
        }
    }

    pub fn restore(&mut self, saved: Saved) {
        for s in saved.torrents {
            let r = match s.torrent.as_deref().and_then(crate::b64::decode) {
                Some(bytes) => self.add_torrent_bytes(&bytes),
                None => self.add_link(&s.magnet),
            };
            if r.is_ok()
                && let Some(e) = self.entries.last_mut()
            {
                for f in &s.unwanted {
                    e.torrent.set_file_wanted(*f, false);
                }
                if s.paused {
                    e.torrent.set_paused(true);
                }
            }
        }
        self.toasts.clear();
    }
}

/// Adresse d'un fichier sur une source web (BEP 19).
fn web_seed_url(base: &str, meta: &Metainfo, file: usize) -> String {
    if !meta.info.multi_file {
        return if base.ends_with('/') { format!("{base}{}", percent_encode(&meta.info.name)) } else { base.to_string() };
    }
    let mut u = base.to_string();
    if !u.ends_with('/') {
        u.push('/');
    }
    let parts: Vec<String> = meta.info.files[file].path.iter().map(|p| percent_encode(p)).collect();
    u.push_str(&parts.join("/"));
    u
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub settings: Settings,
    pub torrents: Vec<SavedTorrent>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedTorrent {
    pub torrent: Option<String>,
    pub magnet: String,
    pub paused: bool,
    pub unwanted: Vec<usize>,
}
