//! Une session de torrent sans entrées-sorties.
//!
//! Le transport (WebRTC dans le navigateur, mémoire dans les tests) appelle
//! [`Torrent::add_peer`], [`Torrent::on_data`], [`Torrent::poll_transmit`] et
//! [`Torrent::tick`] ; la session décide de tout le reste : poignée de main,
//! métadonnées (BEP 9), choix des pièces (la plus rare d'abord, fin de partie),
//! vérification SHA-1, envoi aux autres pairs.

use crate::bencode::{self, Value};
use crate::bitfield::Bitfield;
use crate::metainfo::{Metainfo, verify_piece};
use crate::util::{InfoHash, Rng, client_name, sha1};
use crate::wire::{BLOCK_SIZE, Frame, FrameDecoder, Handshake, Message};
use std::collections::{BTreeMap, HashMap, VecDeque};

pub type PeerKey = u64;

/// Les sources web (BEP 19) utilisent des clés avec le bit de poids fort.
pub const WEBSEED_KEY_BASE: PeerKey = 1 << 63;

const UT_METADATA_ID: u8 = 1;
const METADATA_PIECE: usize = 16 * 1024;
const MAX_METADATA: usize = 32 * 1024 * 1024;
const MAX_INCOMING_REQUESTS: usize = 512;
const MAX_UNCHOKED: usize = 12;
const REQUEST_TIMEOUT_MS: u64 = 30_000;
const METADATA_TIMEOUT_MS: u64 = 15_000;
const KEEPALIVE_MS: u64 = 60_000;
const IDLE_TIMEOUT_MS: u64 = 180_000;
const HANDSHAKE_TIMEOUT_MS: u64 = 20_000;

/// Ce que la session signale au transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Le dictionnaire `info` est connu (tout de suite pour un `.torrent`, plus tard pour un lien magnet).
    MetadataReady,
    PieceVerified(u32),
    PieceFailed(u32),
    /// Toutes les pièces voulues sont là.
    Completed,
    /// Le transport doit fermer ce pair (puis appeler [`Torrent::remove_peer`]).
    ClosePeer(PeerKey, String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    FetchingMetadata,
    Downloading,
    Seeding,
    Paused,
}

/// Débit lissé (moyenne exponentielle).
#[derive(Clone, Debug, Default)]
pub struct Rate {
    pending: u64,
    value: f64,
}

impl Rate {
    fn add(&mut self, n: usize) {
        self.pending += n as u64;
    }

    fn update(&mut self, dt_ms: u64) {
        if dt_ms == 0 {
            return;
        }
        let inst = self.pending as f64 * 1000.0 / dt_ms as f64;
        let a = (dt_ms as f64 / 4000.0).min(1.0);
        self.value += (inst - self.value) * a;
        if self.value < 1.0 {
            self.value = 0.0;
        }
        self.pending = 0;
    }

    pub fn bytes_per_sec(&self) -> f64 {
        self.value
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Block {
    Missing,
    Requested(Vec<PeerKey>),
    Received,
}

#[derive(Debug)]
struct Partial {
    data: Vec<u8>,
    blocks: Vec<Block>,
    received: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BlockReq {
    piece: u32,
    begin: u32,
    length: u32,
}

#[derive(Debug)]
struct Peer {
    decoder: FrameDecoder,
    out: Vec<u8>,
    sent_handshake: bool,
    remote: Option<Handshake>,
    ext_handshake_done: bool,
    am_choking: bool,
    am_interested: bool,
    peer_choking: bool,
    peer_interested: bool,
    bitfield: Option<Bitfield>,
    raw_bitfield: Option<Vec<u8>>,
    early_haves: Vec<u32>,
    ut_metadata: Option<u8>,
    metadata_size: Option<usize>,
    client: Option<String>,
    reqq: usize,
    requested: Vec<(BlockReq, u64)>,
    incoming: VecDeque<BlockReq>,
    down: Rate,
    up: Rate,
    downloaded: u64,
    uploaded: u64,
    connected_at: u64,
    last_recv: u64,
    last_sent: u64,
    snubbed: bool,
    closing: bool,
}

impl Peer {
    fn new(now: u64) -> Peer {
        Peer {
            decoder: FrameDecoder::default(),
            out: Vec::new(),
            sent_handshake: false,
            remote: None,
            ext_handshake_done: false,
            am_choking: true,
            am_interested: false,
            peer_choking: true,
            peer_interested: false,
            bitfield: None,
            raw_bitfield: None,
            early_haves: Vec::new(),
            ut_metadata: None,
            metadata_size: None,
            client: None,
            reqq: 250,
            requested: Vec::new(),
            incoming: VecDeque::new(),
            down: Rate::default(),
            up: Rate::default(),
            downloaded: 0,
            uploaded: 0,
            connected_at: now,
            last_recv: now,
            last_sent: now,
            snubbed: false,
            closing: false,
        }
    }

    fn send(&mut self, m: &Message, now: u64) {
        m.encode_into(&mut self.out);
        self.last_sent = now;
    }

    fn has(&self, piece: usize) -> bool {
        self.bitfield.as_ref().is_some_and(|b| b.get(piece))
    }

    fn pipeline(&self) -> usize {
        if self.snubbed {
            return 2;
        }
        let per_sec = self.down.bytes_per_sec() / BLOCK_SIZE as f64;
        ((4.0 + per_sec * 2.0) as usize).clamp(8, 128).min(self.reqq.max(2))
    }
}

/// Informations sur un pair, pour l'interface.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub key: PeerKey,
    pub client: String,
    pub progress: f32,
    pub down_rate: f64,
    pub up_rate: f64,
    pub downloaded: u64,
    pub uploaded: u64,
    pub choking_us: bool,
    pub interested_in_us: bool,
    pub we_choke: bool,
    pub we_interested: bool,
    pub pending_requests: usize,
}

#[derive(Debug, Default)]
struct MetadataFetch {
    size: usize,
    pieces: Vec<Option<Vec<u8>>>,
    asked: Vec<Option<(PeerKey, u64)>>,
}

/// Une session de torrent.
pub struct Torrent {
    info_hash: InfoHash,
    peer_id: [u8; 20],
    meta: Option<Metainfo>,
    pub trackers: Vec<String>,
    pub web_seeds: Vec<String>,
    name_hint: Option<String>,
    fetch: Option<MetadataFetch>,
    have: Bitfield,
    store: Vec<Option<Box<[u8]>>>,
    partial: HashMap<u32, Partial>,
    wanted_files: Vec<bool>,
    wanted: Vec<bool>,
    availability: Vec<u32>,
    peers: BTreeMap<PeerKey, Peer>,
    events: Vec<Event>,
    pending_haves: Vec<u32>,
    rng: Rng,
    now: u64,
    last_tick: u64,
    paused: bool,
    completed_signaled: bool,
    down: Rate,
    up: Rate,
    pub downloaded: u64,
    pub uploaded: u64,
    pub wasted: u64,
    pub hash_failures: u32,
}

impl Torrent {
    fn blank(info_hash: InfoHash, peer_id: [u8; 20], seed: u64, now: u64) -> Torrent {
        Torrent {
            info_hash,
            peer_id,
            meta: None,
            trackers: Vec::new(),
            web_seeds: Vec::new(),
            name_hint: None,
            fetch: None,
            have: Bitfield::default(),
            store: Vec::new(),
            partial: HashMap::new(),
            wanted_files: Vec::new(),
            wanted: Vec::new(),
            availability: Vec::new(),
            peers: BTreeMap::new(),
            events: Vec::new(),
            pending_haves: Vec::new(),
            rng: Rng::new(seed),
            now,
            last_tick: now,
            paused: false,
            completed_signaled: false,
            down: Rate::default(),
            up: Rate::default(),
            downloaded: 0,
            uploaded: 0,
            wasted: 0,
            hash_failures: 0,
        }
    }

    /// Démarre depuis un `.torrent`.
    pub fn from_metainfo(meta: Metainfo, peer_id: [u8; 20], seed: u64, now: u64) -> Torrent {
        let mut t = Torrent::blank(meta.info_hash, peer_id, seed, now);
        t.trackers = meta.trackers.clone();
        t.web_seeds = meta.web_seeds.clone();
        t.install_metadata(meta);
        t
    }

    /// Démarre depuis un lien magnet : les métadonnées viendront des pairs.
    pub fn from_magnet(m: &crate::magnet::Magnet, peer_id: [u8; 20], seed: u64, now: u64) -> Torrent {
        let mut t = Torrent::blank(m.info_hash, peer_id, seed, now);
        t.trackers = m.trackers.clone();
        t.web_seeds = m.web_seeds.clone();
        t.name_hint = m.name.clone();
        t
    }

    /// Partage des fichiers que l'on possède déjà, dans l'ordre du torrent.
    pub fn seed(meta: Metainfo, files: &[&[u8]], peer_id: [u8; 20], seed: u64, now: u64) -> Result<Torrent, String> {
        let pl = meta.info.piece_length as usize;
        let total: usize = files.iter().map(|f| f.len()).sum();
        if total as u64 != meta.info.total_length {
            return Err("la taille des fichiers ne correspond pas au torrent".into());
        }
        let mut t = Torrent::from_metainfo(meta, peer_id, seed, now);
        let mut piece = Vec::with_capacity(pl);
        let mut index = 0usize;
        for f in files {
            let mut data: &[u8] = f;
            while !data.is_empty() {
                let take = (pl - piece.len()).min(data.len());
                piece.extend_from_slice(&data[..take]);
                data = &data[take..];
                if piece.len() == pl {
                    t.accept_full_piece(index, std::mem::replace(&mut piece, Vec::with_capacity(pl)))?;
                    index += 1;
                }
            }
        }
        if !piece.is_empty() {
            t.accept_full_piece(index, piece)?;
        }
        t.events.clear();
        t.events.push(Event::MetadataReady);
        t.completed_signaled = true;
        Ok(t)
    }

    fn accept_full_piece(&mut self, index: usize, data: Vec<u8>) -> Result<(), String> {
        let info = &self.meta.as_ref().unwrap().info;
        if !verify_piece(info, index, &data) {
            return Err(format!("la pièce {index} ne correspond pas"));
        }
        self.store[index] = Some(data.into_boxed_slice());
        self.have.set(index);
        Ok(())
    }

    fn install_metadata(&mut self, meta: Metainfo) {
        let n = meta.info.piece_count();
        self.have = Bitfield::new(n);
        self.store = (0..n).map(|_| None).collect();
        self.wanted_files = vec![true; meta.info.files.len()];
        self.wanted = vec![true; n];
        self.availability = vec![0; n];
        self.fetch = None;
        self.meta = Some(meta);
        self.events.push(Event::MetadataReady);
        // Les champs de bits reçus avant les métadonnées peuvent enfin être lus.
        let keys: Vec<PeerKey> = self.peers.keys().copied().collect();
        for k in keys {
            let mut p = self.peers.remove(&k).unwrap();
            if let Some(raw) = p.raw_bitfield.take() {
                match Bitfield::from_bytes(&raw, n) {
                    Some(b) => p.bitfield = Some(b),
                    None => self.close(&mut p, k, "champ de bits invalide"),
                }
            }
            let bf = p.bitfield.get_or_insert_with(|| Bitfield::new(n));
            for h in std::mem::take(&mut p.early_haves) {
                bf.set(h as usize);
            }
            if let Some(b) = &p.bitfield {
                for i in b.ones() {
                    self.availability[i] += 1;
                }
            }
            self.update_interest(&mut p);
            self.peers.insert(k, p);
        }
    }

    // ----- accès -----

    pub fn info_hash(&self) -> InfoHash {
        self.info_hash
    }

    pub fn peer_id(&self) -> [u8; 20] {
        self.peer_id
    }

    pub fn metainfo(&self) -> Option<&Metainfo> {
        self.meta.as_ref()
    }

    pub fn name(&self) -> String {
        match (&self.meta, &self.name_hint) {
            (Some(m), _) => m.info.name.clone(),
            (None, Some(n)) => n.clone(),
            (None, None) => self.info_hash.to_hex(),
        }
    }

    pub fn state(&self) -> State {
        if self.paused {
            State::Paused
        } else if self.meta.is_none() {
            State::FetchingMetadata
        } else if self.is_complete() {
            State::Seeding
        } else {
            State::Downloading
        }
    }

    pub fn have(&self) -> &Bitfield {
        &self.have
    }

    /// Toutes les pièces voulues sont-elles là ?
    pub fn is_complete(&self) -> bool {
        self.meta.is_some() && (0..self.have.len()).all(|i| !self.wanted[i] || self.have.get(i))
    }

    pub fn total_length(&self) -> Option<u64> {
        self.meta.as_ref().map(|m| m.info.total_length)
    }

    /// Octets vérifiés.
    pub fn verified_bytes(&self) -> u64 {
        let Some(m) = &self.meta else { return 0 };
        self.have.ones().map(|i| m.info.piece_size(i) as u64).sum()
    }

    /// Octets voulus (selon les fichiers choisis).
    pub fn wanted_bytes(&self) -> u64 {
        let Some(m) = &self.meta else { return 0 };
        (0..self.wanted.len()).filter(|&i| self.wanted[i]).map(|i| m.info.piece_size(i) as u64).sum()
    }

    /// Progression entre 0 et 1, sur les pièces voulues.
    pub fn progress(&self) -> f32 {
        let Some(m) = &self.meta else { return 0.0 };
        let mut want = 0u64;
        let mut got = 0u64;
        for i in 0..self.wanted.len() {
            if self.wanted[i] {
                let s = m.info.piece_size(i) as u64;
                want += s;
                if self.have.get(i) {
                    got += s;
                } else if let Some(p) = self.partial.get(&(i as u32)) {
                    got += (p.received as u64 * BLOCK_SIZE as u64).min(s);
                }
            }
        }
        if want == 0 { 1.0 } else { (got as f64 / want as f64) as f32 }
    }

    /// Octets restants (inconnu tant que les métadonnées manquent).
    pub fn left(&self) -> Option<u64> {
        self.meta.as_ref()?;
        Some(self.wanted_bytes().saturating_sub(self.verified_bytes_wanted()))
    }

    fn verified_bytes_wanted(&self) -> u64 {
        let Some(m) = &self.meta else { return 0 };
        self.have.ones().filter(|&i| self.wanted[i]).map(|i| m.info.piece_size(i) as u64).sum()
    }

    pub fn metadata_progress(&self) -> f32 {
        match &self.fetch {
            Some(f) if !f.pieces.is_empty() => f.pieces.iter().filter(|p| p.is_some()).count() as f32 / f.pieces.len() as f32,
            _ => {
                if self.meta.is_some() {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    pub fn down_rate(&self) -> f64 {
        self.down.bytes_per_sec()
    }

    pub fn up_rate(&self) -> f64 {
        self.up.bytes_per_sec()
    }

    pub fn peer_count(&self) -> usize {
        self.peers.values().filter(|p| p.remote.is_some()).count()
    }

    pub fn seed_count(&self) -> usize {
        self.peers.values().filter(|p| p.bitfield.as_ref().is_some_and(|b| b.is_full())).count()
    }

    pub fn has_peer(&self, key: PeerKey) -> bool {
        self.peers.contains_key(&key)
    }

    pub fn is_connected_to(&self, peer_id: &[u8; 20]) -> bool {
        self.peers.values().any(|p| p.remote.is_some_and(|h| &h.peer_id == peer_id))
    }

    pub fn peers(&self) -> Vec<PeerInfo> {
        let n = self.have.len().max(1);
        self.peers
            .iter()
            .filter(|(_, p)| p.remote.is_some())
            .map(|(&key, p)| PeerInfo {
                key,
                client: p.client.clone().unwrap_or_else(|| client_name(&p.remote.unwrap().peer_id)),
                progress: p.bitfield.as_ref().map(|b| b.count() as f32 / n as f32).unwrap_or(0.0),
                down_rate: p.down.bytes_per_sec(),
                up_rate: p.up.bytes_per_sec(),
                downloaded: p.downloaded,
                uploaded: p.uploaded,
                choking_us: p.peer_choking,
                interested_in_us: p.peer_interested,
                we_choke: p.am_choking,
                we_interested: p.am_interested,
                pending_requests: p.requested.len(),
            })
            .collect()
    }

    /// État de chaque pièce pour la carte : 0 = absente, 1 à 254 = en cours, 255 = vérifiée.
    pub fn piece_states(&self) -> Vec<u8> {
        (0..self.have.len())
            .map(|i| {
                if self.have.get(i) {
                    255
                } else if let Some(p) = self.partial.get(&(i as u32)) {
                    1 + (p.received * 253 / p.blocks.len().max(1)) as u8
                } else {
                    0
                }
            })
            .collect()
    }

    /// Nombre de pairs connectés qui possèdent chaque pièce.
    pub fn availability(&self) -> &[u32] {
        &self.availability
    }

    /// Pièces voulues (selon les fichiers choisis).
    pub fn wanted_pieces(&self) -> &[bool] {
        &self.wanted
    }

    pub fn file_wanted(&self) -> &[bool] {
        &self.wanted_files
    }

    /// Choisit les fichiers à télécharger.
    pub fn set_file_wanted(&mut self, file: usize, wanted: bool) {
        let Some(m) = &self.meta else { return };
        if file >= self.wanted_files.len() || self.wanted_files[file] == wanted {
            return;
        }
        self.wanted_files[file] = wanted;
        let info = &m.info;
        let mut w = vec![false; info.piece_count()];
        for (fi, &on) in self.wanted_files.iter().enumerate() {
            if on {
                for p in info.file_pieces(fi) {
                    w[p] = true;
                }
            }
        }
        self.wanted = w;
        self.completed_signaled = self.is_complete();
        let keys: Vec<PeerKey> = self.peers.keys().copied().collect();
        for k in keys {
            let mut p = self.peers.remove(&k).unwrap();
            self.update_interest(&mut p);
            self.peers.insert(k, p);
        }
    }

    /// Progression d'un fichier (0 à 1).
    pub fn file_progress(&self, file: usize) -> f32 {
        let Some(m) = &self.meta else { return 0.0 };
        let f = &m.info.files[file];
        if f.length == 0 {
            return 1.0;
        }
        let pl = m.info.piece_length as u64;
        let mut got = 0u64;
        for p in m.info.file_pieces(file) {
            if self.have.get(p) {
                let ps = p as u64 * pl;
                let pe = ps + m.info.piece_size(p) as u64;
                got += pe.min(f.offset + f.length) - ps.max(f.offset);
            }
        }
        got as f32 / f.length as f32
    }

    pub fn file_complete(&self, file: usize) -> bool {
        let Some(m) = &self.meta else { return false };
        m.info.file_pieces(file).all(|p| self.have.get(p))
    }

    /// Lit des octets du flux continu (toutes les pièces touchées doivent être là).
    pub fn read(&self, offset: u64, len: usize) -> Option<Vec<u8>> {
        let m = self.meta.as_ref()?;
        if offset + len as u64 > m.info.total_length {
            return None;
        }
        let pl = m.info.piece_length as u64;
        let mut out = Vec::with_capacity(len);
        let mut pos = offset;
        let end = offset + len as u64;
        while pos < end {
            let piece = (pos / pl) as usize;
            let data = self.store[piece].as_ref()?;
            let in_piece = (pos - piece as u64 * pl) as usize;
            let take = (data.len() - in_piece).min((end - pos) as usize);
            out.extend_from_slice(&data[in_piece..in_piece + take]);
            pos += take as u64;
        }
        Some(out)
    }

    /// Contenu complet d'un fichier, s'il est téléchargé.
    pub fn file_bytes(&self, file: usize) -> Option<Vec<u8>> {
        let m = self.meta.as_ref()?;
        let f = m.info.files.get(file)?;
        self.read(f.offset, f.length as usize)
    }

    /// Morceaux d'un fichier, pièce par pièce, sans tout recopier d'un coup.
    pub fn file_chunks(&self, file: usize) -> Option<Vec<&[u8]>> {
        let m = self.meta.as_ref()?;
        let f = m.info.files.get(file)?;
        let pl = m.info.piece_length as u64;
        let mut out = Vec::new();
        for p in m.info.file_pieces(file) {
            let data = self.store[p].as_ref()?;
            let ps = p as u64 * pl;
            let s = f.offset.max(ps) - ps;
            let e = (f.offset + f.length).min(ps + data.len() as u64) - ps;
            out.push(&data[s as usize..e as usize]);
        }
        Some(out)
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if paused {
            let keys: Vec<PeerKey> = self.peers.keys().copied().collect();
            for k in keys {
                let mut p = self.peers.remove(&k).unwrap();
                self.close(&mut p, k, "pause");
                self.peers.insert(k, p);
            }
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    // ----- transport -----

    /// Un canal vient de s'ouvrir : on envoie notre poignée de main.
    pub fn add_peer(&mut self, key: PeerKey) {
        let mut p = Peer::new(self.now);
        self.send_handshake(&mut p);
        self.peers.insert(key, p);
    }

    /// Le canal est fermé.
    pub fn remove_peer(&mut self, key: PeerKey) {
        if let Some(mut p) = self.peers.remove(&key) {
            self.release_requests(&mut p, key);
            if let Some(b) = &p.bitfield {
                for i in b.ones() {
                    if let Some(a) = self.availability.get_mut(i) {
                        *a = a.saturating_sub(1);
                    }
                }
            }
            if let Some(f) = &mut self.fetch {
                for a in &mut f.asked {
                    if a.is_some_and(|(k, _)| k == key) {
                        *a = None;
                    }
                }
            }
        }
    }

    /// Octets reçus d'un pair.
    pub fn on_data(&mut self, key: PeerKey, data: &[u8]) {
        let Some(mut p) = self.peers.remove(&key) else { return };
        if p.closing {
            self.peers.insert(key, p);
            return;
        }
        p.last_recv = self.now;
        p.decoder.push(data);
        loop {
            match p.decoder.next_frame() {
                Ok(Some(Frame::Handshake(h))) => self.on_handshake(&mut p, key, h),
                Ok(Some(Frame::Message(m))) => self.on_message(&mut p, key, m),
                Ok(None) => break,
                Err(e) => {
                    self.close(&mut p, key, e.0);
                    break;
                }
            }
            if p.closing {
                break;
            }
        }
        if !p.closing {
            self.request_more(&mut p, key);
        }
        self.peers.insert(key, p);
        self.flush_haves();
    }

    /// Octets à envoyer à ce pair. `budget` limite les blocs servis
    /// (le transport passe la place libre de son tampon d'envoi).
    pub fn poll_transmit(&mut self, key: PeerKey, budget: usize) -> Vec<u8> {
        let Some(mut p) = self.peers.remove(&key) else { return Vec::new() };
        while p.out.len() < budget && !p.am_choking {
            let Some(r) = p.incoming.pop_front() else { break };
            let Some(m) = &self.meta else { break };
            let offset = r.piece as u64 * m.info.piece_length as u64 + r.begin as u64;
            match self.read(offset, r.length as usize) {
                Some(data) => {
                    let n = data.len();
                    p.send(&Message::Piece { index: r.piece, begin: r.begin, data }, self.now);
                    p.up.add(n);
                    p.uploaded += n as u64;
                    self.up.add(n);
                    self.uploaded += n as u64;
                }
                None => continue,
            }
        }
        let out = std::mem::take(&mut p.out);
        self.peers.insert(key, p);
        out
    }

    /// Y a-t-il quelque chose à envoyer (messages ou blocs demandés) ?
    pub fn wants_to_send(&self, key: PeerKey) -> bool {
        self.peers.get(&key).is_some_and(|p| !p.out.is_empty() || (!p.am_choking && !p.incoming.is_empty()))
    }

    /// Horloge : débits, délais, choix des pairs servis, nouvelles demandes.
    pub fn tick(&mut self, now: u64) {
        self.now = now;
        let dt = now.saturating_sub(self.last_tick);
        if dt >= 250 {
            self.last_tick = now;
            self.down.update(dt);
            self.up.update(dt);
            for p in self.peers.values_mut() {
                p.down.update(dt);
                p.up.update(dt);
            }
        }
        let keys: Vec<PeerKey> = self.peers.keys().copied().collect();
        for &k in &keys {
            let mut p = self.peers.remove(&k).unwrap();
            if !p.closing {
                if p.remote.is_none() && now.saturating_sub(p.connected_at) > HANDSHAKE_TIMEOUT_MS {
                    self.close(&mut p, k, "pas de poignée de main");
                } else if now.saturating_sub(p.last_recv) > IDLE_TIMEOUT_MS {
                    self.close(&mut p, k, "silencieux depuis trop longtemps");
                } else {
                    // Demandes sans réponse : on les redistribue.
                    let expired: Vec<BlockReq> =
                        p.requested.iter().filter(|(_, t)| now.saturating_sub(*t) > REQUEST_TIMEOUT_MS).map(|(r, _)| *r).collect();
                    if !expired.is_empty() {
                        p.snubbed = true;
                        for r in expired {
                            p.requested.retain(|(x, _)| *x != r);
                            self.unassign(r, k);
                        }
                    }
                    if p.remote.is_some() && now.saturating_sub(p.last_sent) > KEEPALIVE_MS {
                        p.send(&Message::KeepAlive, now);
                    }
                }
            }
            self.peers.insert(k, p);
        }
        self.choke_algorithm();
        self.request_metadata();
        for &k in &keys {
            if let Some(mut p) = self.peers.remove(&k) {
                if !p.closing {
                    self.request_more(&mut p, k);
                }
                self.peers.insert(k, p);
            }
        }
        self.flush_haves();
    }

    // ----- sources web (BEP 19) -----

    /// Choisit une pièce entière à demander à une source HTTP.
    pub fn webseed_pick(&mut self, key: PeerKey) -> Option<u32> {
        if self.paused {
            return None;
        }
        let m = self.meta.as_ref()?;
        let n = m.info.piece_count();
        let mut best: Option<usize> = None;
        // On préfère les pièces que les pairs ont le moins : les sources web complètent l'essaim.
        for i in 0..n {
            if self.wanted[i]
                && !self.have.get(i)
                && !self.partial.contains_key(&(i as u32))
                && best.is_none_or(|b| self.availability[i] < self.availability[b])
            {
                best = Some(i);
                if self.availability[i] == 0 {
                    break;
                }
            }
        }
        let i = best?;
        let size = m.info.piece_size(i);
        let nb = size.div_ceil(BLOCK_SIZE) as usize;
        self.partial.insert(i as u32, Partial { data: vec![0; size as usize], blocks: vec![Block::Requested(vec![key]); nb], received: 0 });
        Some(i as u32)
    }

    /// Fichiers et plages qui composent une pièce : (fichier, décalage dans le fichier, longueur).
    pub fn piece_file_ranges(&self, piece: u32) -> Vec<(usize, u64, u64)> {
        let Some(m) = &self.meta else { return vec![] };
        let pl = m.info.piece_length as u64;
        let start = piece as u64 * pl;
        let end = start + m.info.piece_size(piece as usize) as u64;
        m.info
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.length > 0 && f.offset < end && f.offset + f.length > start)
            .map(|(i, f)| {
                let s = start.max(f.offset);
                let e = end.min(f.offset + f.length);
                (i, s - f.offset, e - s)
            })
            .collect()
    }

    /// Une source web a livré une pièce.
    pub fn webseed_done(&mut self, key: PeerKey, piece: u32, data: Vec<u8>) -> bool {
        if self.have.get(piece as usize) {
            self.partial.remove(&piece);
            return true;
        }
        let n = data.len();
        self.down.add(n);
        self.downloaded += n as u64;
        let ok = self.meta.as_ref().is_some_and(|m| verify_piece(&m.info, piece as usize, &data));
        if ok {
            self.cancel_others(piece, key);
            self.partial.remove(&piece);
            self.complete_piece(piece, data);
        } else {
            self.webseed_failed(key, piece);
            self.hash_failures += 1;
            self.wasted += n as u64;
            self.events.push(Event::PieceFailed(piece));
        }
        self.flush_haves();
        ok
    }

    /// La demande à une source web a échoué : la pièce redevient libre.
    pub fn webseed_failed(&mut self, key: PeerKey, piece: u32) {
        if let Some(p) = self.partial.get_mut(&piece) {
            for b in &mut p.blocks {
                if let Block::Requested(owners) = b {
                    owners.retain(|&o| o != key);
                    if owners.is_empty() {
                        *b = Block::Missing;
                    }
                }
            }
            if p.received == 0 && p.blocks.iter().all(|b| *b == Block::Missing) {
                self.partial.remove(&piece);
            }
        }
    }

    // ----- interne -----

    fn close(&mut self, p: &mut Peer, key: PeerKey, reason: &str) {
        if !p.closing {
            p.closing = true;
            self.events.push(Event::ClosePeer(key, reason.to_string()));
        }
    }

    fn send_handshake(&mut self, p: &mut Peer) {
        if p.sent_handshake {
            return;
        }
        p.sent_handshake = true;
        p.out.extend_from_slice(&Handshake::new(self.info_hash, self.peer_id).encode());
    }

    fn on_handshake(&mut self, p: &mut Peer, key: PeerKey, h: Handshake) {
        if h.info_hash != self.info_hash {
            return self.close(p, key, "autre torrent");
        }
        if h.peer_id == self.peer_id {
            return self.close(p, key, "connexion à soi-même");
        }
        if self.is_connected_to(&h.peer_id) {
            return self.close(p, key, "pair déjà connecté");
        }
        self.send_handshake(p);
        p.remote = Some(h);
        if let Some(m) = &self.meta {
            if self.have.count() > 0 {
                p.send(&Message::Bitfield(self.have.as_bytes().to_vec()), self.now);
            }
            p.bitfield = Some(Bitfield::new(m.info.piece_count()));
        }
        if h.supports_extensions() {
            let mut m = Value::dict().with("ut_metadata", Value::Int(UT_METADATA_ID as i64));
            let _ = &mut m;
            let mut hs = Value::dict()
                .with("m", m)
                .with("v", Value::str(concat!("mxTorrent ", env!("CARGO_PKG_VERSION"))))
                .with("reqq", Value::Int(MAX_INCOMING_REQUESTS as i64));
            if let Some(meta) = &self.meta {
                hs = hs.with("metadata_size", Value::Int(meta.info_bytes.len() as i64));
            }
            p.send(&Message::Extended { id: 0, payload: bencode::encode(&hs) }, self.now);
        }
    }

    fn on_message(&mut self, p: &mut Peer, key: PeerKey, m: Message) {
        if p.remote.is_none() {
            return self.close(p, key, "message avant la poignée de main");
        }
        let n = self.have.len();
        match m {
            Message::KeepAlive | Message::Port(_) | Message::Unknown { .. } => {}
            Message::Choke => {
                p.peer_choking = true;
                self.release_requests(p, key);
            }
            Message::Unchoke => p.peer_choking = false,
            Message::Interested => p.peer_interested = true,
            Message::NotInterested => p.peer_interested = false,
            Message::Have(i) => {
                if self.meta.is_none() {
                    if p.early_haves.len() < 100_000 {
                        p.early_haves.push(i);
                    }
                } else if (i as usize) < n {
                    let bf = p.bitfield.get_or_insert_with(|| Bitfield::new(n));
                    if bf.set(i as usize) {
                        self.availability[i as usize] += 1;
                        if !p.am_interested && self.wanted[i as usize] && !self.have.get(i as usize) {
                            p.am_interested = true;
                            p.send(&Message::Interested, self.now);
                        }
                    }
                } else {
                    self.close(p, key, "pièce inexistante");
                }
            }
            Message::Bitfield(raw) => {
                if self.meta.is_none() {
                    p.raw_bitfield = Some(raw);
                } else {
                    match Bitfield::from_bytes(&raw, n) {
                        Some(b) => {
                            if let Some(old) = &p.bitfield {
                                for i in old.ones() {
                                    self.availability[i] = self.availability[i].saturating_sub(1);
                                }
                            }
                            for i in b.ones() {
                                self.availability[i] += 1;
                            }
                            p.bitfield = Some(b);
                            self.update_interest(p);
                        }
                        None => self.close(p, key, "champ de bits invalide"),
                    }
                }
            }
            Message::Request { index, begin, length } => {
                if p.am_choking || self.meta.is_none() {
                    return;
                }
                let valid = (index as usize) < n
                    && self.have.get(index as usize)
                    && length > 0
                    && length <= 128 * 1024
                    && begin as u64 + length as u64 <= self.meta.as_ref().unwrap().info.piece_size(index as usize) as u64;
                if !valid {
                    return self.close(p, key, "demande invalide");
                }
                if p.incoming.len() < MAX_INCOMING_REQUESTS {
                    p.incoming.push_back(BlockReq { piece: index, begin, length });
                }
            }
            Message::Cancel { index, begin, length } => {
                p.incoming.retain(|r| *r != BlockReq { piece: index, begin, length });
            }
            Message::Piece { index, begin, data } => self.on_block(p, key, index, begin, data),
            Message::Extended { id, payload } => self.on_extended(p, key, id, &payload),
        }
    }

    fn on_block(&mut self, p: &mut Peer, key: PeerKey, index: u32, begin: u32, data: Vec<u8>) {
        let len = data.len();
        p.down.add(len);
        p.downloaded += len as u64;
        self.down.add(len);
        self.downloaded += len as u64;
        let req = BlockReq { piece: index, begin, length: len as u32 };
        let was_requested = p.requested.iter().any(|(r, _)| *r == req);
        p.requested.retain(|(r, _)| *r != req);
        if was_requested {
            p.snubbed = false;
        }
        let Some(part) = self.partial.get_mut(&index) else {
            self.wasted += len as u64;
            return;
        };
        if !begin.is_multiple_of(BLOCK_SIZE) || begin as usize + len > part.data.len() {
            self.wasted += len as u64;
            return;
        }
        let b = (begin / BLOCK_SIZE) as usize;
        let expected = (part.data.len() - begin as usize).min(BLOCK_SIZE as usize);
        if len != expected || part.blocks[b] == Block::Received {
            self.wasted += len as u64;
            return;
        }
        let others: Vec<PeerKey> = match &part.blocks[b] {
            Block::Requested(o) => o.iter().copied().filter(|&o| o != key).collect(),
            _ => vec![],
        };
        part.data[begin as usize..begin as usize + len].copy_from_slice(&data);
        part.blocks[b] = Block::Received;
        part.received += 1;
        // Fin de partie : le même bloc avait été demandé ailleurs, on annule.
        for o in others {
            if let Some(op) = self.peers.get_mut(&o) {
                op.requested.retain(|(r, _)| *r != req);
                op.send(&Message::Cancel { index, begin, length: len as u32 }, self.now);
            }
        }
        if part.received == part.blocks.len() {
            let part = self.partial.remove(&index).unwrap();
            let ok = verify_piece(&self.meta.as_ref().unwrap().info, index as usize, &part.data);
            if ok {
                self.complete_piece(index, part.data);
                self.update_interest(p);
            } else {
                self.hash_failures += 1;
                self.wasted += part.data.len() as u64;
                self.events.push(Event::PieceFailed(index));
            }
        }
    }

    fn complete_piece(&mut self, index: u32, data: Vec<u8>) {
        self.store[index as usize] = Some(data.into_boxed_slice());
        self.have.set(index as usize);
        self.pending_haves.push(index);
        self.events.push(Event::PieceVerified(index));
        if !self.completed_signaled && self.is_complete() {
            self.completed_signaled = true;
            self.events.push(Event::Completed);
        }
    }

    fn cancel_others(&mut self, piece: u32, except: PeerKey) {
        let Some(part) = self.partial.get(&piece) else { return };
        let size = part.data.len() as u32;
        let mut cancels = Vec::new();
        for (b, st) in part.blocks.iter().enumerate() {
            if let Block::Requested(owners) = st {
                for &o in owners {
                    if o != except {
                        let begin = b as u32 * BLOCK_SIZE;
                        cancels.push((o, BlockReq { piece, begin, length: (size - begin).min(BLOCK_SIZE) }));
                    }
                }
            }
        }
        for (o, r) in cancels {
            if let Some(op) = self.peers.get_mut(&o) {
                op.requested.retain(|(x, _)| *x != r);
                op.send(&Message::Cancel { index: r.piece, begin: r.begin, length: r.length }, self.now);
            }
        }
    }

    fn flush_haves(&mut self) {
        if self.pending_haves.is_empty() {
            return;
        }
        let haves = std::mem::take(&mut self.pending_haves);
        let keys: Vec<PeerKey> = self.peers.keys().copied().collect();
        for k in keys {
            let mut p = self.peers.remove(&k).unwrap();
            if p.remote.is_some() && !p.closing {
                for &h in &haves {
                    if !p.has(h as usize) {
                        p.send(&Message::Have(h), self.now);
                    }
                }
                self.update_interest(&mut p);
            }
            self.peers.insert(k, p);
        }
    }

    fn update_interest(&mut self, p: &mut Peer) {
        if p.remote.is_none() || self.meta.is_none() {
            return;
        }
        let interested = p.bitfield.as_ref().is_some_and(|b| b.ones().any(|i| self.wanted[i] && !self.have.get(i)));
        if interested != p.am_interested {
            p.am_interested = interested;
            p.send(if interested { &Message::Interested } else { &Message::NotInterested }, self.now);
        }
    }

    fn choke_algorithm(&mut self) {
        let have_any = self.have.count() > 0;
        let mut unchoked = self.peers.values().filter(|p| !p.am_choking).count();
        let now = self.now;
        // On sert ceux qui le demandent, dans la limite des places.
        for p in self.peers.values_mut() {
            if p.closing || p.remote.is_none() {
                continue;
            }
            if !p.am_choking && !p.peer_interested {
                p.am_choking = true;
                p.incoming.clear();
                p.send(&Message::Choke, now);
                unchoked -= 1;
            }
        }
        if !have_any || self.paused {
            return;
        }
        // Les meilleurs fournisseurs d'abord (donnant-donnant), puis les autres.
        let mut candidates: Vec<(PeerKey, f64)> = self
            .peers
            .iter()
            .filter(|(_, p)| !p.closing && p.remote.is_some() && p.am_choking && p.peer_interested)
            .map(|(&k, p)| (k, p.down.bytes_per_sec()))
            .collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (k, _) in candidates {
            if unchoked >= MAX_UNCHOKED {
                break;
            }
            let p = self.peers.get_mut(&k).unwrap();
            p.am_choking = false;
            p.send(&Message::Unchoke, now);
            unchoked += 1;
        }
    }

    fn release_requests(&mut self, p: &mut Peer, key: PeerKey) {
        for (r, _) in std::mem::take(&mut p.requested) {
            self.unassign(r, key);
        }
    }

    fn unassign(&mut self, r: BlockReq, key: PeerKey) {
        if let Some(part) = self.partial.get_mut(&r.piece) {
            let b = (r.begin / BLOCK_SIZE) as usize;
            if let Some(Block::Requested(owners)) = part.blocks.get_mut(b) {
                owners.retain(|&o| o != key);
                if owners.is_empty() {
                    part.blocks[b] = Block::Missing;
                }
            }
        }
    }

    fn request_more(&mut self, p: &mut Peer, key: PeerKey) {
        if self.paused || self.meta.is_none() || p.peer_choking || !p.am_interested || p.remote.is_none() {
            return;
        }
        let target = p.pipeline();
        if p.requested.len() >= target {
            return;
        }
        let mut need = target - p.requested.len();
        let now = self.now;
        let mut new_reqs = Vec::new();

        // 1. Terminer les pièces commencées que ce pair possède (les plus avancées d'abord).
        let mut started: Vec<(u32, usize)> = self
            .partial
            .iter()
            .filter(|(i, part)| p.has(**i as usize) && part.blocks.contains(&Block::Missing))
            .map(|(i, part)| (*i, part.received))
            .collect();
        started.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (i, _) in started {
            if need == 0 {
                break;
            }
            let part = self.partial.get_mut(&i).unwrap();
            take_blocks(part, i, key, &mut need, &mut new_reqs, false);
        }

        // 2. Commencer de nouvelles pièces, la plus rare d'abord.
        while need > 0 {
            let Some(i) = self.pick_new_piece(p) else { break };
            let size = self.meta.as_ref().unwrap().info.piece_size(i);
            let nb = size.div_ceil(BLOCK_SIZE) as usize;
            let mut part = Partial { data: vec![0; size as usize], blocks: vec![Block::Missing; nb], received: 0 };
            take_blocks(&mut part, i as u32, key, &mut need, &mut new_reqs, false);
            self.partial.insert(i as u32, part);
        }

        // 3. Fin de partie : tout est demandé, on double les demandes en cours.
        if need > 0 && self.endgame() {
            let mut ids: Vec<u32> = self.partial.keys().copied().filter(|i| p.has(*i as usize)).collect();
            ids.sort();
            for i in ids {
                if need == 0 {
                    break;
                }
                let part = self.partial.get_mut(&i).unwrap();
                take_blocks(part, i, key, &mut need, &mut new_reqs, true);
            }
        }

        for r in new_reqs {
            p.send(&Message::Request { index: r.piece, begin: r.begin, length: r.length }, now);
            p.requested.push((r, now));
        }
    }

    fn endgame(&self) -> bool {
        (0..self.have.len()).all(|i| !self.wanted[i] || self.have.get(i) || self.partial.contains_key(&(i as u32)))
            && self.partial.values().all(|p| !p.blocks.contains(&Block::Missing))
    }

    fn pick_new_piece(&mut self, p: &Peer) -> Option<usize> {
        let bf = p.bitfield.as_ref()?;
        let mut best: Option<(u32, u64)> = None;
        let mut best_i = None;
        for i in bf.ones() {
            if !self.wanted[i] || self.have.get(i) || self.partial.contains_key(&(i as u32)) {
                continue;
            }
            // Rareté d'abord, puis un peu de hasard pour ne pas tous choisir la même.
            let score = (self.availability[i], self.rng.next_u64());
            if best.is_none_or(|b| score < b) {
                best = Some(score);
                best_i = Some(i);
            }
        }
        best_i
    }

    fn on_extended(&mut self, p: &mut Peer, key: PeerKey, id: u8, payload: &[u8]) {
        if id == 0 {
            let Ok(v) = bencode::decode(payload) else {
                return self.close(p, key, "poignée de main d'extension invalide");
            };
            p.ext_handshake_done = true;
            if let Some(m) = v.get("m")
                && let Some(ut) = m.get("ut_metadata").and_then(|x| x.as_int())
            {
                p.ut_metadata = u8::try_from(ut).ok().filter(|&x| x != 0);
            }
            if let Some(s) = v.get("metadata_size").and_then(|x| x.as_int())
                && s > 0
                && (s as usize) <= MAX_METADATA
            {
                p.metadata_size = Some(s as usize);
            }
            if let Some(c) = v.get("v").and_then(|x| x.as_str()) {
                p.client = Some(c.chars().take(40).collect());
            }
            if let Some(q) = v.get("reqq").and_then(|x| x.as_int()) {
                p.reqq = q.clamp(1, 2000) as usize;
            }
            return;
        }
        if id != UT_METADATA_ID {
            return; // extension que nous n'avons pas annoncée
        }
        let Ok((v, used)) = bencode::decode_prefix(payload) else {
            return self.close(p, key, "message ut_metadata invalide");
        };
        let msg_type = v.get("msg_type").and_then(|x| x.as_int()).unwrap_or(-1);
        let piece = v.get("piece").and_then(|x| x.as_int()).unwrap_or(-1);
        if piece < 0 {
            return;
        }
        let piece = piece as usize;
        match msg_type {
            0 => {
                let Some(their) = p.ut_metadata else { return };
                let reply = match &self.meta {
                    Some(m) if piece * METADATA_PIECE < m.info_bytes.len() => {
                        let s = piece * METADATA_PIECE;
                        let e = (s + METADATA_PIECE).min(m.info_bytes.len());
                        let mut out = bencode::encode(
                            &Value::dict()
                                .with("msg_type", Value::Int(1))
                                .with("piece", Value::Int(piece as i64))
                                .with("total_size", Value::Int(m.info_bytes.len() as i64)),
                        );
                        out.extend_from_slice(&m.info_bytes[s..e]);
                        out
                    }
                    _ => bencode::encode(&Value::dict().with("msg_type", Value::Int(2)).with("piece", Value::Int(piece as i64))),
                };
                p.send(&Message::Extended { id: their, payload: reply }, self.now);
            }
            1 => {
                if self.meta.is_some() {
                    return;
                }
                let data = &payload[used..];
                let Some(f) = &mut self.fetch else { return };
                if piece >= f.pieces.len() {
                    return;
                }
                let expected = (f.size - piece * METADATA_PIECE).min(METADATA_PIECE);
                if data.len() != expected {
                    return;
                }
                f.pieces[piece] = Some(data.to_vec());
                f.asked[piece] = None;
                if f.pieces.iter().all(|x| x.is_some()) {
                    let bytes: Vec<u8> = f.pieces.iter().flat_map(|x| x.as_ref().unwrap().iter().copied()).collect();
                    if InfoHash(sha1(&bytes)) == self.info_hash {
                        match Metainfo::from_info_bytes(bytes, self.trackers.clone(), self.web_seeds.clone()) {
                            Ok(meta) => {
                                // Le pair courant est hors de la table : on le remet le temps de l'installation.
                                let taken = std::mem::replace(p, Peer::new(self.now));
                                self.peers.insert(key, taken);
                                self.install_metadata(meta);
                                *p = self.peers.remove(&key).unwrap();
                            }
                            Err(_) => self.fetch = None,
                        }
                    } else {
                        self.fetch = None;
                    }
                }
            }
            2 => {
                if let Some(f) = &mut self.fetch
                    && piece < f.asked.len()
                {
                    f.asked[piece] = None;
                }
                p.metadata_size = None; // ce pair ne nous aidera pas
            }
            _ => {}
        }
    }

    fn request_metadata(&mut self) {
        if self.meta.is_some() || self.paused {
            return;
        }
        let now = self.now;
        if self.fetch.is_none() {
            let size = self.peers.values().filter(|p| p.ut_metadata.is_some()).find_map(|p| p.metadata_size);
            let Some(size) = size else { return };
            let n = size.div_ceil(METADATA_PIECE);
            self.fetch = Some(MetadataFetch { size, pieces: vec![None; n], asked: vec![None; n] });
        }
        let f = self.fetch.as_mut().unwrap();
        for a in &mut f.asked {
            if a.is_some_and(|(_, t)| now.saturating_sub(t) > METADATA_TIMEOUT_MS) {
                *a = None;
            }
        }
        let size = f.size;
        let helpers: Vec<PeerKey> = self
            .peers
            .iter()
            .filter(|(_, p)| !p.closing && p.ut_metadata.is_some() && p.metadata_size == Some(size))
            .map(|(&k, _)| k)
            .collect();
        if helpers.is_empty() {
            return;
        }
        let mut h = self.rng.below(helpers.len());
        for i in 0..f.pieces.len() {
            if f.pieces[i].is_some() || f.asked[i].is_some() {
                continue;
            }
            let k = helpers[h % helpers.len()];
            h += 1;
            let p = self.peers.get_mut(&k).unwrap();
            let msg = bencode::encode(&Value::dict().with("msg_type", Value::Int(0)).with("piece", Value::Int(i as i64)));
            p.send(&Message::Extended { id: p.ut_metadata.unwrap(), payload: msg }, now);
            f.asked[i] = Some((k, now));
        }
    }
}

fn take_blocks(part: &mut Partial, piece: u32, key: PeerKey, need: &mut usize, out: &mut Vec<BlockReq>, endgame: bool) {
    let size = part.data.len() as u32;
    for (b, st) in part.blocks.iter_mut().enumerate() {
        if *need == 0 {
            return;
        }
        let begin = b as u32 * BLOCK_SIZE;
        let r = BlockReq { piece, begin, length: (size - begin).min(BLOCK_SIZE) };
        match st {
            Block::Missing => {
                *st = Block::Requested(vec![key]);
            }
            Block::Requested(owners) if endgame && !owners.contains(&key) && owners.len() < 3 => {
                owners.push(key);
            }
            _ => continue,
        }
        out.push(r);
        *need -= 1;
    }
}
