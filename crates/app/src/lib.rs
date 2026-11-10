//! # mxTorrent
//!
//! Un client BitTorrent pour le navigateur, sur ordinateur comme sur téléphone :
//! Rust compilé en WebAssembly, interface egui, échanges en WebRTC
//! (compatible avec le réseau WebTorrent). Aucune application à installer :
//! la page s'installe comme une PWA.

pub mod b64;
pub mod i18n;

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod engine;
#[cfg(target_arch = "wasm32")]
mod gateway;
#[cfg(target_arch = "wasm32")]
mod icons;
#[cfg(target_arch = "wasm32")]
mod kit;
#[cfg(target_arch = "wasm32")]
mod net;
#[cfg(target_arch = "wasm32")]
mod qr;
#[cfg(target_arch = "wasm32")]
mod theme;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
pub use wasm::WebHandle;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use crate::app::MxApp;
    use crate::engine::{Engine, Settings};
    use crate::net::{self, NetEvent};
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    thread_local! {
        static ENGINE: RefCell<Option<Rc<RefCell<Engine>>>> = const { RefCell::new(None) };
    }

    fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
        let e = ENGINE.with(|e| e.borrow().clone())?;
        let mut g = e.try_borrow_mut().ok()?;
        Some(f(&mut g))
    }

    /// La poignée que la page JavaScript garde sur l'application.
    #[wasm_bindgen]
    pub struct WebHandle {
        runner: eframe::WebRunner,
    }

    #[wasm_bindgen]
    impl WebHandle {
        #[wasm_bindgen(constructor)]
        #[allow(clippy::new_without_default)]
        pub fn new() -> WebHandle {
            eframe::WebLogger::init(log::LevelFilter::Warn).ok();
            WebHandle { runner: eframe::WebRunner::new() }
        }

        /// Démarre l'application dans le canevas donné.
        pub async fn start(&self, canvas: web_sys::HtmlCanvasElement) -> Result<(), JsValue> {
            let engine = Rc::new(RefCell::new(Engine::new(Settings::default())));
            ENGINE.with(|e| *e.borrow_mut() = Some(engine.clone()));
            // Les rappels du navigateur réveillent le moteur aussitôt…
            let e = engine.clone();
            net::set_waker(move || {
                if let Ok(mut e) = e.try_borrow_mut() {
                    e.pump();
                }
            });
            // … et une horloge le fait avancer, même onglet en arrière-plan.
            let e = engine.clone();
            let tick = Closure::<dyn FnMut()>::new(move || {
                if let Ok(mut e) = e.try_borrow_mut() {
                    e.pump();
                    e.tick();
                    // Quelque chose a changé (torrent ajouté, pair connecté…) : on redessine.
                    if std::mem::take(&mut e.dirty) {
                        crate::app::request_repaint();
                    }
                }
            });
            crate::web::window().set_interval_with_callback_and_timeout_and_arguments_0(tick.as_ref().unchecked_ref(), 250)?;
            tick.forget();

            let e = engine.clone();
            self.runner.start(canvas, eframe::WebOptions::default(), Box::new(move |cc| Ok(Box::new(MxApp::new(cc, e))))).await
        }

        /// Ajoute un lien magnet, une empreinte ou une URL de `.torrent`.
        pub fn add_link(&self, link: String) {
            net::push(NetEvent::Link(link));
        }

        /// Ajoute un fichier `.torrent` (ouvert par le système, par exemple).
        pub fn add_torrent_file(&self, name: String, bytes: Vec<u8>) {
            net::push(NetEvent::TorrentFiles(vec![(name, bytes)]));
        }

        /// Partage un fichier (reçu du menu « Partager » du système).
        pub fn share_file(&self, name: String, bytes: Vec<u8>) {
            net::push(NetEvent::Share(vec![(name, bytes)]));
        }

        /// Remplace la liste des trackers (tracker auto-hébergé, tests de bout en bout).
        pub fn set_trackers(&self, trackers: Vec<String>) {
            with_engine(|e| {
                e.settings.trackers = trackers;
                e.dirty = true;
            });
        }

        /// Choisit la passerelle TCP/UDP (adresse HTTP d'un serveur mxtorrent-tracker).
        pub fn set_gateway(&self, base: String) {
            with_engine(|e| {
                e.settings.gateway = base;
                e.settings.use_gateway = true;
                e.dirty = true;
            });
        }

        /// État des torrents en JSON (pour les scripts et les tests).
        pub fn status(&self) -> String {
            with_engine(|e| {
                let list: Vec<serde_json::Value> = e
                    .entries
                    .iter()
                    .map(|x| {
                        let t = &x.torrent;
                        serde_json::json!({
                            "name": t.name(),
                            "info_hash": t.info_hash().to_hex(),
                            "magnet": x.magnet(),
                            "state": format!("{:?}", t.state()),
                            "progress": t.progress(),
                            "peers": t.peer_count(),
                            "downloaded": t.downloaded,
                            "uploaded": t.uploaded,
                            "down_rate": t.down_rate(),
                            "up_rate": t.up_rate(),
                        })
                    })
                    .collect();
                let (phase, detail, fp, gw_peers) = e.gateway_status();
                serde_json::json!({
                    "peer_id": String::from_utf8_lossy(&e.peer_id),
                    "trackers": e.connected_trackers(),
                    "gateway": { "phase": format!("{phase:?}"), "detail": detail, "fingerprint": fp, "peers": gw_peers },
                    "torrents": list,
                })
                .to_string()
            })
            .unwrap_or_else(|| "{}".into())
        }

        /// Le journal du moteur, une ligne par événement (diagnostic).
        pub fn log(&self) -> String {
            with_engine(|e| e.log.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default()
        }

        /// Les pairs d'un torrent, avec leur état (diagnostic).
        pub fn peers(&self, info_hash: String) -> String {
            with_engine(|e| {
                let h = mxtorrent_core::InfoHash::parse(&info_hash)?;
                let i = e.find(&h)?;
                let list: Vec<serde_json::Value> = e.entries[i]
                    .torrent
                    .peers()
                    .iter()
                    .map(|p| {
                        serde_json::json!({
                            "client": p.client, "gateway": e.is_gateway_peer(p.key), "progress": p.progress,
                            "downloaded": p.downloaded, "uploaded": p.uploaded, "choking_us": p.choking_us,
                            "we_interested": p.we_interested, "requests": p.pending_requests,
                        })
                    })
                    .collect();
                Some(serde_json::json!({ "metadata": e.entries[i].torrent.metadata_progress(), "peers": list }).to_string())
            })
            .flatten()
            .unwrap_or_else(|| "{}".into())
        }

        /// Contenu d'un fichier terminé (pour les tests de bout en bout).
        pub fn file_bytes(&self, info_hash: String, file: usize) -> Option<Vec<u8>> {
            with_engine(|e| {
                let h = mxtorrent_core::InfoHash::parse(&info_hash)?;
                let i = e.find(&h)?;
                e.entries[i].torrent.file_bytes(file)
            })
            .flatten()
        }

        pub fn has_panicked(&self) -> bool {
            self.runner.has_panicked()
        }

        pub fn panic_message(&self) -> Option<String> {
            self.runner.panic_summary().map(|s| s.message())
        }
    }
}
