//! # mxtorrent-tracker
//!
//! Un tracker WebSocket compatible WebTorrent : il met les navigateurs en
//! relation en relayant leurs offres et réponses WebRTC, sans jamais voir les
//! données échangées. Il peut aussi servir le site et le client (`--static dist`).
//!
//! ```text
//! mxtorrent-tracker [--port 8000] [--static dist]
//! ```
//! Le tracker répond sur `/announce` (et sur `/`, pour les clients qui
//! n'ajoutent pas de chemin).

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

const INTERVAL: u64 = 120;
const MAX_OFFERS: usize = 20;

struct Slot {
    conn: u64,
    tx: UnboundedSender<String>,
    complete: bool,
}

#[derive(Default)]
struct Swarm {
    peers: HashMap<String, Slot>,
    downloaded: u64,
}

#[derive(Default)]
struct Tracker {
    swarms: Mutex<HashMap<String, Swarm>>,
    next_conn: AtomicU64,
}

type Shared = Arc<Tracker>;

fn id20(v: &Value) -> Option<String> {
    let s = v.as_str()?;
    (s.chars().count() == 20 && s.chars().all(|c| (c as u32) < 256)).then(|| s.to_string())
}

fn hash_of(seed: u64) -> u64 {
    // Mélange simple pour choisir des pairs au hasard sans dépendance de plus.
    let mut x = seed ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

impl Tracker {
    /// Traite un message d'un client ; renvoie la réponse directe éventuelle.
    fn handle(&self, conn: u64, tx: &UnboundedSender<String>, text: &str) -> Option<String> {
        let v: Value = serde_json::from_str(text).ok()?;
        match v.get("action").and_then(|a| a.as_str()) {
            Some("announce") => self.announce(conn, tx, &v),
            Some("scrape") => Some(self.scrape(&v)),
            _ => Some(json!({"failure reason": "action inconnue"}).to_string()),
        }
    }

    fn announce(&self, conn: u64, tx: &UnboundedSender<String>, v: &Value) -> Option<String> {
        let Some(info_hash) = v.get("info_hash").and_then(id20) else {
            return Some(json!({"failure reason": "info_hash invalide"}).to_string());
        };
        let Some(peer_id) = v.get("peer_id").and_then(id20) else {
            return Some(json!({"failure reason": "peer_id invalide"}).to_string());
        };
        let mut swarms = self.swarms.lock().unwrap();

        // Réponse à une offre : on la remet au pair concerné.
        if let Some(answer) = v.get("answer") {
            let to = v.get("to_peer_id").and_then(id20)?;
            let target = swarms.get(&info_hash)?.peers.get(&to)?;
            let msg = json!({
                "action": "announce", "info_hash": info_hash, "peer_id": peer_id,
                "answer": answer, "offer_id": v.get("offer_id")?,
            });
            let _ = target.tx.send(msg.to_string());
            return None;
        }

        let event = v.get("event").and_then(|e| e.as_str()).unwrap_or("");
        if event == "stopped" {
            if let Some(s) = swarms.get_mut(&info_hash) {
                s.peers.remove(&peer_id);
                if s.peers.is_empty() {
                    swarms.remove(&info_hash);
                }
            }
            return None;
        }

        let swarm = swarms.entry(info_hash.clone()).or_default();
        let left = v.get("left").and_then(|l| l.as_f64()).unwrap_or(1.0);
        if event == "completed" {
            swarm.downloaded += 1;
        }
        let complete = left == 0.0 || event == "completed";
        swarm.peers.insert(peer_id.clone(), Slot { conn, tx: tx.clone(), complete });

        // Chaque offre part vers un autre pair, choisi au hasard.
        if let Some(offers) = v.get("offers").and_then(|o| o.as_array()) {
            let mut others: Vec<(&String, &Slot)> = swarm.peers.iter().filter(|(id, _)| **id != peer_id).collect();
            let seed = hash_of(
                conn ^ offers.len() as u64
                    ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0),
            );
            others.sort_by_key(|(id, _)| hash_of(seed ^ id.chars().map(|c| c as u64).sum::<u64>()));
            for (offer, (_, slot)) in offers.iter().take(MAX_OFFERS).zip(others) {
                let (Some(o), Some(oid)) = (offer.get("offer"), offer.get("offer_id")) else { continue };
                let msg = json!({
                    "action": "announce", "info_hash": info_hash, "peer_id": peer_id,
                    "offer": o, "offer_id": oid,
                });
                let _ = slot.tx.send(msg.to_string());
            }
        }
        let seeders = swarm.peers.values().filter(|p| p.complete).count();
        Some(
            json!({
                "action": "announce", "info_hash": info_hash, "interval": INTERVAL,
                "complete": seeders, "incomplete": swarm.peers.len() - seeders,
            })
            .to_string(),
        )
    }

    fn scrape(&self, v: &Value) -> String {
        let swarms = self.swarms.lock().unwrap();
        let hashes: Vec<String> = match v.get("info_hash") {
            Some(Value::Array(a)) => a.iter().filter_map(id20).collect(),
            Some(h) => id20(h).into_iter().collect(),
            None => swarms.keys().cloned().collect(),
        };
        let mut files = serde_json::Map::new();
        for h in hashes {
            let (c, i, d) = swarms.get(&h).map_or((0, 0, 0), |s| {
                let c = s.peers.values().filter(|p| p.complete).count();
                (c, s.peers.len() - c, s.downloaded)
            });
            files.insert(h, json!({"complete": c, "incomplete": i, "downloaded": d}));
        }
        json!({"action": "scrape", "files": files}).to_string()
    }

    fn disconnect(&self, conn: u64) {
        let mut swarms = self.swarms.lock().unwrap();
        for s in swarms.values_mut() {
            s.peers.retain(|_, p| p.conn != conn);
        }
        swarms.retain(|_, s| !s.peers.is_empty());
    }

    fn stats(&self) -> (usize, usize) {
        let swarms = self.swarms.lock().unwrap();
        (swarms.len(), swarms.values().map(|s| s.peers.len()).sum())
    }
}

async fn client(socket: WebSocket, tracker: Shared) {
    let conn = tracker.next_conn.fetch_add(1, Ordering::Relaxed);
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(Message::Text(m.into())).await.is_err() {
                break;
            }
        }
    });
    while let Some(Ok(msg)) = stream.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
            Message::Close(_) => break,
            _ => continue,
        };
        if text.len() > 512 * 1024 {
            break;
        }
        if let Some(reply) = tracker.handle(conn, &tx, &text) {
            let _ = tx.send(reply);
        }
    }
    tracker.disconnect(conn);
    writer.abort();
}

/// Une requête WebSocket, ou rien (pour laisser la main aux fichiers statiques).
struct MaybeWs(Option<WebSocketUpgrade>);

impl<S: Send + Sync> FromRequestParts<S> for MaybeWs {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Ok(MaybeWs(WebSocketUpgrade::from_request_parts(parts, state).await.ok()))
    }
}

async fn announce(MaybeWs(ws): MaybeWs, State(t): State<Shared>) -> Response {
    match ws {
        Some(ws) => ws.max_message_size(1 << 20).on_upgrade(move |s| client(s, t)).into_response(),
        None => {
            let (swarms, peers) = t.stats();
            format!("mxtorrent-tracker : {swarms} torrents, {peers} pairs. Utilisez wss://…/announce dans votre client WebTorrent.\n")
                .into_response()
        }
    }
}

#[tokio::main]
async fn main() {
    let mut port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8000);
    let mut static_dir: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" | "-p" => port = args.next().and_then(|p| p.parse().ok()).expect("--port <nombre>"),
            "--static" | "-s" => static_dir = args.next(),
            "--help" | "-h" => {
                println!("mxtorrent-tracker [--port 8000] [--static dist]");
                return;
            }
            other => {
                eprintln!("argument inconnu : {other}");
                std::process::exit(2);
            }
        }
    }
    let tracker: Shared = Arc::new(Tracker::default());
    let mut app = Router::new().route("/announce", get(announce));
    app = match &static_dir {
        Some(dir) => {
            // `/` reste un tracker pour les clients WebSocket, et sert le site sinon.
            let files = tower_http::services::ServeDir::new(dir).append_index_html_on_directories(true);
            let t2 = tracker.clone();
            app.route_service(
                "/",
                axum::routing::get(move |ws: MaybeWs, req: axum::extract::Request| {
                    let t = t2.clone();
                    let files = files.clone();
                    async move {
                        match ws.0 {
                            Some(ws) => ws.on_upgrade(move |s| client(s, t)).into_response(),
                            None => tower::ServiceExt::oneshot(files, req).await.into_response(),
                        }
                    }
                }),
            )
            .fallback_service(tower_http::services::ServeDir::new(dir).append_index_html_on_directories(true))
        }
        None => app.route("/", get(announce)),
    };
    let app = app.with_state(tracker);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await.expect("port occupé");
    println!("mxtorrent-tracker : ws://localhost:{port}/announce");
    if let Some(d) = &static_dir {
        println!("site et client : http://localhost:{port}/ (dossier {d})");
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin(n: u8) -> String {
        std::iter::repeat_n(n as char, 20).collect()
    }

    #[test]
    fn relaie_offres_et_reponses() {
        let t = Tracker::default();
        let (ta, mut ra) = unbounded_channel();
        let (tb, mut rb) = unbounded_channel();
        let ih = bin(0xAA);
        let r = t.handle(1, &ta, &json!({"action":"announce","info_hash":ih,"peer_id":bin(1),"left":0,"offers":[]}).to_string()).unwrap();
        assert!(r.contains("\"complete\":1"));
        let off = json!({"action":"announce","info_hash":ih,"peer_id":bin(2),"left":10,"event":"started",
            "offers":[{"offer":{"type":"offer","sdp":"S"},"offer_id":bin(9)}]});
        let r = t.handle(2, &tb, &off.to_string()).unwrap();
        assert!(r.contains("\"incomplete\":1"));
        let got: Value = serde_json::from_str(&ra.try_recv().unwrap()).unwrap();
        assert_eq!(got["offer"]["sdp"], "S");
        assert_eq!(got["peer_id"], bin(2));
        let ans = json!({"action":"announce","info_hash":ih,"peer_id":bin(1),"to_peer_id":bin(2),"offer_id":bin(9),"answer":{"type":"answer","sdp":"A"}});
        assert!(t.handle(1, &ta, &ans.to_string()).is_none());
        let got: Value = serde_json::from_str(&rb.try_recv().unwrap()).unwrap();
        assert_eq!(got["answer"]["sdp"], "A");
        assert_eq!(t.stats(), (1, 2));
        t.disconnect(1);
        assert_eq!(t.stats(), (1, 1));
        let s: Value = serde_json::from_str(&t.scrape(&json!({"action":"scrape"}))).unwrap();
        assert_eq!(s["files"][&ih]["incomplete"], 1);
    }
}
