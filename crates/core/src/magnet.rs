//! Liens magnet (BEP 9).

use crate::metainfo::Metainfo;
use crate::util::{InfoHash, percent_decode, percent_encode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Magnet {
    pub info_hash: InfoHash,
    pub name: Option<String>,
    pub trackers: Vec<String>,
    pub web_seeds: Vec<String>,
    pub length: Option<u64>,
}

impl Magnet {
    /// Accepte un lien `magnet:?xt=urn:btih:…` ou une empreinte seule (hex ou base32).
    pub fn parse(input: &str) -> Option<Magnet> {
        let input = input.trim();
        if let Some(h) = InfoHash::parse(input) {
            return Some(Magnet { info_hash: h, name: None, trackers: vec![], web_seeds: vec![], length: None });
        }
        let query = input.strip_prefix("magnet:?").or_else(|| input.strip_prefix("magnet:"))?;
        let mut hash = None;
        let mut m = Magnet { info_hash: InfoHash::default(), name: None, trackers: vec![], web_seeds: vec![], length: None };
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            // Les clés peuvent être numérotées : tr.1, xt.2…
            let key = k.split('.').next().unwrap_or(k);
            let v = percent_decode(v);
            match key {
                "xt" => {
                    if let Some(h) = v.strip_prefix("urn:btih:").and_then(InfoHash::parse) {
                        hash.get_or_insert(h);
                    }
                }
                "dn" => m.name = Some(v).filter(|s| !s.is_empty()),
                "tr" => {
                    if !v.is_empty() && !m.trackers.contains(&v) {
                        m.trackers.push(v);
                    }
                }
                "ws" | "as" => {
                    if (v.starts_with("http://") || v.starts_with("https://")) && !m.web_seeds.contains(&v) {
                        m.web_seeds.push(v);
                    }
                }
                "xl" => m.length = v.parse().ok(),
                _ => {}
            }
        }
        m.info_hash = hash?;
        Some(m)
    }

    pub fn from_metainfo(meta: &Metainfo) -> Magnet {
        Magnet {
            info_hash: meta.info_hash,
            name: Some(meta.info.name.clone()),
            trackers: meta.trackers.clone(),
            web_seeds: meta.web_seeds.clone(),
            length: Some(meta.info.total_length),
        }
    }

    pub fn to_uri(&self) -> String {
        let mut s = format!("magnet:?xt=urn:btih:{}", self.info_hash.to_hex());
        if let Some(n) = &self.name {
            s.push_str("&dn=");
            s.push_str(&percent_encode(n));
        }
        if let Some(l) = self.length {
            s.push_str(&format!("&xl={l}"));
        }
        for t in &self.trackers {
            s.push_str("&tr=");
            s.push_str(&percent_encode(t));
        }
        for w in &self.web_seeds {
            s.push_str("&ws=");
            s.push_str(&percent_encode(w));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lien_complet() {
        let uri = "magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel&tr=udp%3A%2F%2Fexplodie.org%3A6969&tr=wss%3A%2F%2Ftracker.webtorrent.dev&ws=https%3A%2F%2Fwebtorrent.io%2Ftorrents%2F&xs=x";
        let m = Magnet::parse(uri).unwrap();
        assert_eq!(m.info_hash.to_hex(), "08ada5a7a6183aae1e09d831df6748d566095a10");
        assert_eq!(m.name.as_deref(), Some("Sintel"));
        assert_eq!(m.trackers, vec!["udp://explodie.org:6969", "wss://tracker.webtorrent.dev"]);
        assert_eq!(m.web_seeds, vec!["https://webtorrent.io/torrents/"]);
        assert_eq!(Magnet::parse(&m.to_uri()).unwrap(), m);
    }

    #[test]
    fn empreinte_seule_et_invalide() {
        assert!(Magnet::parse("08ada5a7a6183aae1e09d831df6748d566095a10").is_some());
        assert!(Magnet::parse("magnet:?dn=rien").is_none());
        assert!(Magnet::parse("https://example.com").is_none());
    }
}
