//! Protocole des trackers WebSocket de WebTorrent.
//!
//! Le tracker ne transporte aucune donnée : il met des navigateurs en relation
//! en relayant les offres et réponses WebRTC (SDP) entre pairs d'un même torrent.

use crate::util::{InfoHash, binary_to_latin1, latin1_to_binary};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sdp {
    pub kind: String,
    pub sdp: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferOut {
    pub offer_id: [u8; 20],
    pub sdp: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnounceEvent {
    Started,
    Completed,
    Stopped,
    Update,
}

/// Une annonce : « je participe à ce torrent, voici des offres de connexion ».
#[derive(Clone, Debug)]
pub struct Announce {
    pub info_hash: InfoHash,
    pub peer_id: [u8; 20],
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: Option<u64>,
    pub event: AnnounceEvent,
    pub numwant: u32,
    pub offers: Vec<OfferOut>,
}

impl Announce {
    pub fn to_json(&self) -> String {
        let mut v = json!({
            "action": "announce",
            "info_hash": binary_to_latin1(&self.info_hash.0),
            "peer_id": binary_to_latin1(&self.peer_id),
            "uploaded": self.uploaded,
            "downloaded": self.downloaded,
            // Taille inconnue (lien magnet) : on annonce un grand nombre, comme WebTorrent.
            "left": self.left.unwrap_or(1 << 53),
            "numwant": self.numwant,
            "offers": self.offers.iter().map(|o| json!({
                "offer": { "type": "offer", "sdp": o.sdp },
                "offer_id": binary_to_latin1(&o.offer_id),
            })).collect::<Vec<_>>(),
        });
        let ev = match self.event {
            AnnounceEvent::Started => Some("started"),
            AnnounceEvent::Completed => Some("completed"),
            AnnounceEvent::Stopped => Some("stopped"),
            AnnounceEvent::Update => None,
        };
        if let Some(e) = ev {
            v["event"] = json!(e);
        }
        v.to_string()
    }
}

/// Notre réponse à l'offre d'un autre pair.
pub fn answer_json(info_hash: &InfoHash, peer_id: &[u8; 20], to_peer: &[u8; 20], offer_id: &[u8; 20], sdp: &str) -> String {
    json!({
        "action": "announce",
        "info_hash": binary_to_latin1(&info_hash.0),
        "peer_id": binary_to_latin1(peer_id),
        "to_peer_id": binary_to_latin1(to_peer),
        "offer_id": binary_to_latin1(offer_id),
        "answer": { "type": "answer", "sdp": sdp },
    })
    .to_string()
}

pub fn scrape_json(hashes: &[InfoHash]) -> String {
    json!({
        "action": "scrape",
        "info_hash": hashes.iter().map(|h| binary_to_latin1(&h.0)).collect::<Vec<_>>(),
    })
    .to_string()
}

/// Ce que le tracker peut nous envoyer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackerMessage {
    /// Réponse à notre annonce.
    Stats {
        info_hash: InfoHash,
        interval: Option<u32>,
        complete: Option<u32>,
        incomplete: Option<u32>,
    },
    /// Un pair veut se connecter : il faut lui répondre.
    Offer {
        info_hash: InfoHash,
        peer_id: [u8; 20],
        offer_id: [u8; 20],
        sdp: String,
    },
    /// Un pair a répondu à l'une de nos offres.
    Answer {
        info_hash: InfoHash,
        peer_id: [u8; 20],
        offer_id: [u8; 20],
        sdp: String,
    },
    Failure(String),
    Warning(String),
    Other,
}

fn id20(v: &Value) -> Option<[u8; 20]> {
    latin1_to_binary(v.as_str()?)?.try_into().ok()
}

fn sdp_of(v: &Value) -> Option<String> {
    v.get("sdp").and_then(|s| s.as_str()).map(String::from)
}

impl TrackerMessage {
    pub fn parse(text: &str) -> TrackerMessage {
        let Ok(v) = serde_json::from_str::<Value>(text) else { return TrackerMessage::Other };
        if let Some(f) = v.get("failure reason").and_then(|f| f.as_str()) {
            return TrackerMessage::Failure(f.to_string());
        }
        if v.get("action").and_then(|a| a.as_str()) != Some("announce") {
            if let Some(w) = v.get("warning message").and_then(|w| w.as_str()) {
                return TrackerMessage::Warning(w.to_string());
            }
            return TrackerMessage::Other;
        }
        let Some(info_hash) = v.get("info_hash").and_then(id20).map(InfoHash) else { return TrackerMessage::Other };
        let peer_id = v.get("peer_id").and_then(id20);
        let offer_id = v.get("offer_id").and_then(id20);
        if let (Some(o), Some(p), Some(id)) = (v.get("offer"), peer_id, offer_id)
            && let Some(sdp) = sdp_of(o)
        {
            return TrackerMessage::Offer { info_hash, peer_id: p, offer_id: id, sdp };
        }
        if let (Some(a), Some(p), Some(id)) = (v.get("answer"), peer_id, offer_id)
            && let Some(sdp) = sdp_of(a)
        {
            return TrackerMessage::Answer { info_hash, peer_id: p, offer_id: id, sdp };
        }
        if let Some(w) = v.get("warning message").and_then(|w| w.as_str()) {
            return TrackerMessage::Warning(w.to_string());
        }
        let num = |k: &str| v.get(k).and_then(|x| x.as_u64()).map(|x| x.min(u32::MAX as u64) as u32);
        TrackerMessage::Stats { info_hash, interval: num("interval"), complete: num("complete"), incomplete: num("incomplete") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annonce_et_messages() {
        let a = Announce {
            info_hash: InfoHash([0xff; 20]),
            peer_id: *b"-MX0100-abcdefghijkl",
            uploaded: 1,
            downloaded: 2,
            left: None,
            event: AnnounceEvent::Started,
            numwant: 5,
            offers: vec![OfferOut { offer_id: [7; 20], sdp: "v=0".into() }],
        };
        let v: Value = serde_json::from_str(&a.to_json()).unwrap();
        assert_eq!(v["event"], "started");
        assert_eq!(v["info_hash"].as_str().unwrap().chars().count(), 20);
        assert_eq!(v["offers"][0]["offer"]["sdp"], "v=0");

        let offer = json!({
            "action": "announce", "info_hash": binary_to_latin1(&[0xff; 20]),
            "peer_id": binary_to_latin1(&[1; 20]), "offer_id": binary_to_latin1(&[2; 20]),
            "offer": {"type": "offer", "sdp": "x"}
        });
        assert_eq!(
            TrackerMessage::parse(&offer.to_string()),
            TrackerMessage::Offer { info_hash: InfoHash([0xff; 20]), peer_id: [1; 20], offer_id: [2; 20], sdp: "x".into() }
        );
        let stats = json!({"action": "announce", "info_hash": binary_to_latin1(&[0; 20]), "interval": 120, "complete": 3, "incomplete": 1});
        assert!(matches!(TrackerMessage::parse(&stats.to_string()), TrackerMessage::Stats { complete: Some(3), .. }));
        assert_eq!(TrackerMessage::parse(r#"{"failure reason":"non"}"#), TrackerMessage::Failure("non".into()));
        let ans = answer_json(&InfoHash([0; 20]), &[1; 20], &[2; 20], &[3; 20], "y");
        assert!(matches!(TrackerMessage::parse(&ans), TrackerMessage::Answer { peer_id, .. } if peer_id == [1; 20]));
    }
}
