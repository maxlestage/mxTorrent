//! Fichiers `.torrent` (BEP 3, BEP 12, BEP 19) et création de torrents.

use crate::bencode::{self, Value};
use crate::util::{InfoHash, sha1};
use std::fmt;

/// Un fichier du torrent, avec sa position dans le flux continu des pièces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: Vec<String>,
    pub length: u64,
    pub offset: u64,
}

impl FileEntry {
    pub fn display_path(&self) -> String {
        self.path.join("/")
    }

    pub fn file_name(&self) -> &str {
        self.path.last().map(|s| s.as_str()).unwrap_or("")
    }
}

/// Le dictionnaire `info` décodé.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub name: String,
    pub piece_length: u32,
    pub pieces: Vec<[u8; 20]>,
    pub files: Vec<FileEntry>,
    pub total_length: u64,
    pub private: bool,
    /// Vrai si le torrent contient un dossier (mode multi-fichiers).
    pub multi_file: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetainfoError(pub String);

impl fmt::Display for MetainfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MetainfoError {}

impl From<bencode::DecodeError> for MetainfoError {
    fn from(e: bencode::DecodeError) -> Self {
        MetainfoError(e.to_string())
    }
}

fn bad(msg: &str) -> MetainfoError {
    MetainfoError(msg.to_string())
}

/// Un nom de fichier ne doit jamais sortir du dossier du torrent.
fn clean_component(s: &str) -> Option<String> {
    let t: String = s
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let t = t.trim().to_string();
    if t.is_empty() || t == "." || t == ".." { None } else { Some(t) }
}

impl Info {
    pub const MAX_PIECE_LENGTH: u32 = 64 * 1024 * 1024;

    pub fn from_bytes(bytes: &[u8]) -> Result<Info, MetainfoError> {
        Info::from_value(&bencode::decode(bytes)?)
    }

    pub fn from_value(v: &Value) -> Result<Info, MetainfoError> {
        let name_raw = v
            .get("name.utf-8")
            .and_then(|n| n.as_str())
            .or_else(|| v.get("name").and_then(|n| n.as_bytes()).map(|b| std::str::from_utf8(b).unwrap_or("torrent")))
            .ok_or_else(|| bad("nom manquant"))?;
        let name = clean_component(name_raw).unwrap_or_else(|| "torrent".to_string());
        let piece_length = v
            .get("piece length")
            .and_then(|p| p.as_int())
            .filter(|&p| p > 0 && p <= Info::MAX_PIECE_LENGTH as i64)
            .ok_or_else(|| bad("taille de pièce invalide"))? as u32;
        let raw = v.get("pieces").and_then(|p| p.as_bytes()).ok_or_else(|| bad("pièces manquantes"))?;
        if raw.len() % 20 != 0 {
            return Err(bad("liste de pièces mal formée"));
        }
        let pieces: Vec<[u8; 20]> = raw.chunks(20).map(|c| c.try_into().unwrap()).collect();
        let private = v.get("private").and_then(|p| p.as_int()) == Some(1);

        let mut files = Vec::new();
        let mut offset = 0u64;
        let multi_file;
        if let Some(list) = v.get("files").and_then(|f| f.as_list()) {
            multi_file = true;
            for f in list {
                let length =
                    f.get("length").and_then(|l| l.as_int()).filter(|&l| l >= 0).ok_or_else(|| bad("taille de fichier invalide"))? as u64;
                let parts = f
                    .get("path.utf-8")
                    .or_else(|| f.get("path"))
                    .and_then(|p| p.as_list())
                    .ok_or_else(|| bad("chemin de fichier manquant"))?;
                let mut path = vec![name.clone()];
                for p in parts {
                    let s = String::from_utf8_lossy(p.as_bytes().ok_or_else(|| bad("chemin invalide"))?);
                    if let Some(c) = clean_component(&s) {
                        path.push(c);
                    }
                }
                if path.len() == 1 {
                    return Err(bad("chemin de fichier vide"));
                }
                // BEP 47 : les fichiers de bourrage ne sont pas montrés mais occupent leur place.
                files.push(FileEntry { path, length, offset });
                offset = offset.checked_add(length).ok_or_else(|| bad("torrent trop grand"))?;
            }
            if files.is_empty() {
                return Err(bad("aucun fichier"));
            }
        } else {
            multi_file = false;
            let length = v.get("length").and_then(|l| l.as_int()).filter(|&l| l >= 0).ok_or_else(|| bad("taille manquante"))? as u64;
            files.push(FileEntry { path: vec![name.clone()], length, offset: 0 });
            offset = length;
        }
        let total_length = offset;
        let expected = total_length.div_ceil(piece_length as u64);
        if expected != pieces.len() as u64 {
            return Err(bad("le nombre de pièces ne correspond pas à la taille"));
        }
        Ok(Info { name, piece_length, pieces, files, total_length, private, multi_file })
    }

    pub fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    /// Taille réelle d'une pièce (la dernière est souvent plus courte).
    pub fn piece_size(&self, index: usize) -> u32 {
        let start = index as u64 * self.piece_length as u64;
        (self.total_length - start).min(self.piece_length as u64) as u32
    }

    /// Pièces qui touchent le fichier `file` (intervalle demi-ouvert).
    pub fn file_pieces(&self, file: usize) -> std::ops::Range<usize> {
        let f = &self.files[file];
        if f.length == 0 {
            return 0..0;
        }
        let pl = self.piece_length as u64;
        let first = (f.offset / pl) as usize;
        let last = ((f.offset + f.length - 1) / pl) as usize;
        first..last + 1
    }
}

/// Un fichier `.torrent` complet.
#[derive(Clone, Debug)]
pub struct Metainfo {
    pub info: Info,
    pub info_hash: InfoHash,
    /// Octets d'origine du dictionnaire `info` (servis aux pairs par `ut_metadata`).
    pub info_bytes: Vec<u8>,
    pub trackers: Vec<String>,
    pub web_seeds: Vec<String>,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub creation_date: Option<i64>,
}

impl Metainfo {
    pub fn from_torrent(bytes: &[u8]) -> Result<Metainfo, MetainfoError> {
        let root = bencode::decode(bytes)?;
        let (s, e) = bencode::dict_value_span(bytes, b"info")?.ok_or_else(|| bad("dictionnaire info manquant"))?;
        let info_bytes = bytes[s..e].to_vec();
        let info = Info::from_value(root.get("info").unwrap())?;
        let mut trackers = Vec::new();
        let mut push = |t: &str| {
            let t = t.trim();
            if !t.is_empty() && !trackers.iter().any(|x| x == t) {
                trackers.push(t.to_string());
            }
        };
        if let Some(tiers) = root.get("announce-list").and_then(|a| a.as_list()) {
            for tier in tiers {
                for t in tier.as_list().unwrap_or(&[]) {
                    if let Some(s) = t.as_str() {
                        push(s);
                    }
                }
            }
        }
        if let Some(a) = root.get("announce").and_then(|a| a.as_str()) {
            push(a);
        }
        let mut web_seeds = Vec::new();
        match root.get("url-list") {
            Some(Value::Bytes(b)) => web_seeds.extend(std::str::from_utf8(b).ok().map(String::from)),
            Some(Value::List(l)) => web_seeds.extend(l.iter().filter_map(|u| u.as_str().map(String::from))),
            _ => {}
        }
        web_seeds.retain(|u| u.starts_with("http://") || u.starts_with("https://"));
        Ok(Metainfo {
            info_hash: InfoHash::of(&info_bytes),
            info,
            info_bytes,
            trackers,
            web_seeds,
            comment: root.get("comment").and_then(|c| c.as_str()).map(String::from),
            created_by: root.get("created by").and_then(|c| c.as_str()).map(String::from),
            creation_date: root.get("creation date").and_then(|c| c.as_int()),
        })
    }

    /// Construit un `Metainfo` à partir d'un dictionnaire `info` reçu d'un pair.
    pub fn from_info_bytes(info_bytes: Vec<u8>, trackers: Vec<String>, web_seeds: Vec<String>) -> Result<Metainfo, MetainfoError> {
        let info = Info::from_bytes(&info_bytes)?;
        Ok(Metainfo {
            info_hash: InfoHash::of(&info_bytes),
            info,
            info_bytes,
            trackers,
            web_seeds,
            comment: None,
            created_by: None,
            creation_date: None,
        })
    }

    /// Réécrit un fichier `.torrent`.
    pub fn to_torrent(&self) -> Vec<u8> {
        let mut root = Value::dict();
        if let Some(first) = self.trackers.first() {
            root = root.with("announce", Value::str(first));
            root = root.with("announce-list", Value::List(self.trackers.iter().map(|t| Value::List(vec![Value::str(t)])).collect()));
        }
        if let Some(c) = &self.comment {
            root = root.with("comment", Value::str(c));
        }
        if let Some(c) = &self.created_by {
            root = root.with("created by", Value::str(c));
        }
        if let Some(d) = self.creation_date {
            root = root.with("creation date", Value::Int(d));
        }
        if !self.web_seeds.is_empty() {
            root = root.with("url-list", Value::List(self.web_seeds.iter().map(|u| Value::str(u)).collect()));
        }
        // Le dictionnaire info est recopié octet pour octet pour garder l'empreinte,
        // inséré à sa place pour que les clés restent triées.
        let enc = bencode::encode(&root);
        let insert_at = info_insert_position(&enc);
        let mut res = Vec::with_capacity(enc.len() + self.info_bytes.len() + 6);
        res.extend_from_slice(&enc[..insert_at]);
        res.extend_from_slice(b"4:info");
        res.extend_from_slice(&self.info_bytes);
        res.extend_from_slice(&enc[insert_at..]);
        res
    }
}

/// Position, dans un dictionnaire encodé, où insérer la clé `info` en gardant l'ordre.
fn info_insert_position(enc: &[u8]) -> usize {
    let mut pos = 1;
    while pos < enc.len() && enc[pos] != b'e' {
        let (key_start, key_end, value_end) = match entry_bounds(enc, pos) {
            Some(b) => b,
            None => break,
        };
        if &enc[key_start..key_end] > b"info".as_slice() {
            return pos;
        }
        pos = value_end;
    }
    pos
}

fn entry_bounds(enc: &[u8], pos: usize) -> Option<(usize, usize, usize)> {
    let colon = pos + enc[pos..].iter().position(|&b| b == b':')?;
    let len: usize = std::str::from_utf8(&enc[pos..colon]).ok()?.parse().ok()?;
    let key_start = colon + 1;
    let key_end = key_start + len;
    let (_, used) = bencode::decode_prefix(&enc[key_end..]).ok()?;
    Some((key_start, key_end, key_end + used))
}

/// Choisit une taille de pièce : environ 1 000 à 2 000 pièces, entre 16 Kio et 8 Mio.
pub fn auto_piece_length(total: u64) -> u32 {
    let mut pl: u64 = 16 * 1024;
    while pl < 8 * 1024 * 1024 && total / pl > 1500 {
        pl *= 2;
    }
    pl as u32
}

/// Source d'un fichier à partager : son chemin dans le torrent et ses octets.
pub struct NewFile<'a> {
    pub path: Vec<String>,
    pub data: &'a [u8],
}

/// Options de création.
#[derive(Clone, Debug, Default)]
pub struct CreateOptions {
    pub name: Option<String>,
    pub piece_length: Option<u32>,
    pub trackers: Vec<String>,
    pub web_seeds: Vec<String>,
    pub comment: Option<String>,
    pub private: bool,
    pub creation_date: Option<i64>,
}

/// Crée un torrent à partir de fichiers en mémoire. Un seul fichier donne un
/// torrent mono-fichier ; plusieurs fichiers sont rangés dans un dossier.
pub fn create_torrent(files: &[NewFile<'_>], opts: &CreateOptions) -> Result<Metainfo, MetainfoError> {
    if files.is_empty() {
        return Err(bad("aucun fichier à partager"));
    }
    let total: u64 = files.iter().map(|f| f.data.len() as u64).sum();
    let pl = opts.piece_length.unwrap_or_else(|| auto_piece_length(total));
    let mut pieces = Vec::new();
    let mut hasher = sha1_smol::Sha1::new();
    let mut filled = 0usize;
    for f in files {
        let mut data = f.data;
        while !data.is_empty() {
            let take = (pl as usize - filled).min(data.len());
            hasher.update(&data[..take]);
            filled += take;
            data = &data[take..];
            if filled == pl as usize {
                pieces.extend_from_slice(&hasher.digest().bytes());
                hasher.reset();
                filled = 0;
            }
        }
    }
    if filled > 0 {
        pieces.extend_from_slice(&hasher.digest().bytes());
    }
    let single = files.len() == 1 && files[0].path.len() == 1;
    let name = opts
        .name
        .clone()
        .or_else(|| {
            if single {
                files[0].path.last().cloned()
            } else {
                files[0].path.first().cloned().filter(|_| files.iter().all(|f| f.path.len() > 1 && f.path[0] == files[0].path[0]))
            }
        })
        .unwrap_or_else(|| "partage".to_string());
    let mut info =
        Value::dict().with("name", Value::str(&name)).with("piece length", Value::Int(pl as i64)).with("pieces", Value::Bytes(pieces));
    if opts.private {
        info = info.with("private", Value::Int(1));
    }
    if single {
        info = info.with("length", Value::Int(total as i64));
    } else {
        // Si tous les chemins commencent par le nom du dossier, on le retire.
        let strip = files.iter().all(|f| f.path.len() > 1 && f.path[0] == name);
        let list = files
            .iter()
            .map(|f| {
                let parts = if strip { &f.path[1..] } else { &f.path[..] };
                Value::dict()
                    .with("length", Value::Int(f.data.len() as i64))
                    .with("path", Value::List(parts.iter().map(|p| Value::str(p)).collect()))
            })
            .collect();
        info = info.with("files", Value::List(list));
    }
    let info_bytes = bencode::encode(&info);
    let mut m = Metainfo::from_info_bytes(info_bytes, opts.trackers.clone(), opts.web_seeds.clone())?;
    m.comment = opts.comment.clone();
    m.created_by = Some(format!("mxTorrent {}", env!("CARGO_PKG_VERSION")));
    m.creation_date = opts.creation_date;
    Ok(m)
}

/// Vérifie une pièce.
pub fn verify_piece(info: &Info, index: usize, data: &[u8]) -> bool {
    data.len() == info.piece_size(index) as usize && sha1(data) == info.pieces[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Vec<u8>, Vec<u8>) {
        let a: Vec<u8> = (0..70_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let b: Vec<u8> = (0..5_000u32).map(|i| (i % 13) as u8).collect();
        (a, b)
    }

    #[test]
    fn creation_puis_relecture() {
        let (a, b) = sample();
        let files = [
            NewFile { path: vec!["dossier".into(), "a.bin".into()], data: &a },
            NewFile { path: vec!["dossier".into(), "sous".into(), "b.txt".into()], data: &b },
        ];
        let opts = CreateOptions { piece_length: Some(16384), trackers: vec!["wss://t.example".into()], ..Default::default() };
        let m = create_torrent(&files, &opts).unwrap();
        assert_eq!(m.info.name, "dossier");
        assert_eq!(m.info.total_length, 75_000);
        assert_eq!(m.info.piece_count(), 5);
        assert_eq!(m.info.piece_size(4), 75_000 - 4 * 16384);
        assert_eq!(m.info.files[1].path, vec!["dossier", "sous", "b.txt"]);
        assert_eq!(m.info.files[1].offset, 70_000);
        assert_eq!(m.info.file_pieces(1), 4..5);

        let bytes = m.to_torrent();
        let back = Metainfo::from_torrent(&bytes).unwrap();
        assert_eq!(back.info_hash, m.info_hash);
        assert_eq!(back.trackers, vec!["wss://t.example"]);
        assert_eq!(back.info, m.info);
        assert_eq!(back.created_by.as_deref(), Some(concat!("mxTorrent ", env!("CARGO_PKG_VERSION"))));

        let mut all = a.clone();
        all.extend_from_slice(&b);
        for i in 0..m.info.piece_count() {
            let s = i * 16384;
            let e = (s + 16384).min(all.len());
            assert!(verify_piece(&m.info, i, &all[s..e]));
        }
        assert!(!verify_piece(&m.info, 0, &all[1..16385]));
    }

    #[test]
    fn mono_fichier() {
        let (a, _) = sample();
        let m = create_torrent(&[NewFile { path: vec!["video.mp4".into()], data: &a }], &CreateOptions::default()).unwrap();
        assert!(!m.info.multi_file);
        assert_eq!(m.info.name, "video.mp4");
        assert_eq!(m.info.files.len(), 1);
    }

    #[test]
    fn chemins_assainis() {
        let mut info = Value::dict()
            .with("name", Value::str("../x"))
            .with("piece length", Value::Int(16384))
            .with("pieces", Value::Bytes(vec![0; 20]));
        info = info.with(
            "files",
            Value::List(vec![
                Value::dict()
                    .with("length", Value::Int(3))
                    .with("path", Value::List(vec![Value::str(".."), Value::str("etc"), Value::str("passwd")])),
            ]),
        );
        let i = Info::from_value(&info).unwrap();
        assert_eq!(i.name, ".._x");
        assert_eq!(i.files[0].path, vec![".._x", "etc", "passwd"]);
    }

    #[test]
    fn taille_auto() {
        assert_eq!(auto_piece_length(1000), 16384);
        assert!(auto_piece_length(4 << 30) >= 2 << 20);
    }
}
