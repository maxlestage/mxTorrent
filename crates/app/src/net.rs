//! WebRTC et WebSocket : les seules façons, pour un navigateur, de parler à d'autres pairs.
//!
//! Chaque rappel JavaScript se contente de poser un [`NetEvent`] dans une file ;
//! le moteur la vide quand il a la main. Aucun rappel ne touche le moteur
//! directement, ce qui évite toute réentrance.

use js_sys::{Array, Reflect, Uint8Array};
use mxtorrent_core::PeerKey;
use std::cell::RefCell;
use std::collections::VecDeque;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelEvent, RtcDataChannelType, RtcIceGatheringState, RtcPeerConnection,
    RtcSdpType, RtcSessionDescriptionInit, WebSocket,
};

/// Ce que le navigateur signale au moteur.
pub enum NetEvent {
    TrackerOpen(String),
    TrackerText(String, String),
    TrackerClosed(String),
    /// Notre offre est prête (candidats ICE rassemblés).
    OfferReady {
        batch: u64,
        key: PeerKey,
        sdp: Option<String>,
    },
    /// Notre réponse à l'offre d'un pair est prête.
    AnswerReady {
        key: PeerKey,
        sdp: Option<String>,
    },
    ChannelOpen(PeerKey),
    ChannelData(PeerKey, Vec<u8>),
    ChannelClosed(PeerKey),
    ChannelLow,
    WebSeed {
        key: PeerKey,
        piece: u32,
        result: Result<Vec<u8>, String>,
    },
    /// Fichiers choisis pour être partagés.
    Share(Vec<(String, Vec<u8>)>),
    /// Fichiers `.torrent` choisis, déposés ou ouverts par le système.
    TorrentFiles(Vec<(String, Vec<u8>)>),
    /// Un lien magnet ou une URL reçus de la page (paramètre, collage, JavaScript).
    Link(String),
    /// Un `.torrent` téléchargé depuis une URL.
    Fetched(String, Result<Vec<u8>, String>),
}

thread_local! {
    static QUEUE: RefCell<VecDeque<NetEvent>> = const { RefCell::new(VecDeque::new()) };
    static WAKER: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Pose un événement et réveille le moteur.
pub fn push(ev: NetEvent) {
    QUEUE.with(|q| q.borrow_mut().push_back(ev));
    WAKER.with(|w| {
        if let Ok(w) = w.try_borrow()
            && let Some(f) = w.as_ref()
        {
            f();
        }
    });
}

pub fn take_events() -> Vec<NetEvent> {
    QUEUE.with(|q| q.borrow_mut().drain(..).collect())
}

pub fn set_waker(f: impl Fn() + 'static) {
    WAKER.with(|w| *w.borrow_mut() = Some(Box::new(f)));
}

// ---------------------------------------------------------------- WebSocket

pub struct TrackerSocket {
    pub ws: WebSocket,
    _cbs: Vec<Closure<dyn FnMut(JsValue)>>,
}

impl TrackerSocket {
    pub fn connect(url: &str) -> Result<TrackerSocket, JsValue> {
        let ws = WebSocket::new(url)?;
        let mut cbs: Vec<Closure<dyn FnMut(JsValue)>> = Vec::new();
        let u = url.to_string();
        let on_open = Closure::<dyn FnMut(JsValue)>::new(move |_| push(NetEvent::TrackerOpen(u.clone())));
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        cbs.push(on_open);
        let u = url.to_string();
        let on_msg = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let e: MessageEvent = e.unchecked_into();
            if let Some(s) = e.data().as_string() {
                push(NetEvent::TrackerText(u.clone(), s));
            }
        });
        ws.set_onmessage(Some(on_msg.as_ref().unchecked_ref()));
        cbs.push(on_msg);
        let u = url.to_string();
        let on_close = Closure::<dyn FnMut(JsValue)>::new(move |_| push(NetEvent::TrackerClosed(u.clone())));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        ws.set_onerror(Some(on_close.as_ref().unchecked_ref()));
        cbs.push(on_close);
        Ok(TrackerSocket { ws, _cbs: cbs })
    }

    pub fn send(&self, text: &str) -> bool {
        self.ws.ready_state() == WebSocket::OPEN && self.ws.send_with_str(text).is_ok()
    }
}

impl Drop for TrackerSocket {
    fn drop(&mut self) {
        self.ws.set_onopen(None);
        self.ws.set_onmessage(None);
        self.ws.set_onclose(None);
        self.ws.set_onerror(None);
        let _ = self.ws.close();
    }
}

// ---------------------------------------------------------------- WebRTC

/// Une connexion WebRTC et son canal de données.
pub struct PeerConn {
    pub pc: RtcPeerConnection,
    pub dc: Option<RtcDataChannel>,
    pub open: bool,
    cbs: Vec<Closure<dyn FnMut(JsValue)>>,
}

const HIGH_WATER: u32 = 1024 * 1024;
const LOW_WATER: u32 = 256 * 1024;
/// Taille des morceaux envoyés : compatible avec tous les navigateurs et WebTorrent.
const CHUNK: usize = 16 * 1024;

fn rtc_config(stun: &[String]) -> RtcConfiguration {
    let cfg = RtcConfiguration::new();
    let servers = Array::new();
    for s in stun {
        let o = js_sys::Object::new();
        let _ = Reflect::set(&o, &"urls".into(), &JsValue::from_str(s));
        servers.push(&o);
    }
    cfg.set_ice_servers(&servers);
    cfg
}

impl PeerConn {
    fn new(stun: &[String]) -> Result<PeerConn, JsValue> {
        let pc = RtcPeerConnection::new_with_configuration(&rtc_config(stun))?;
        Ok(PeerConn { pc, dc: None, open: false, cbs: Vec::new() })
    }

    fn wire_channel(&mut self, key: PeerKey, dc: &RtcDataChannel) {
        dc.set_binary_type(RtcDataChannelType::Arraybuffer);
        dc.set_buffered_amount_low_threshold(LOW_WATER);
        let on_open = Closure::<dyn FnMut(JsValue)>::new(move |_| push(NetEvent::ChannelOpen(key)));
        dc.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        let on_msg = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let e: MessageEvent = e.unchecked_into();
            let data = e.data();
            if data.is_instance_of::<js_sys::ArrayBuffer>() {
                push(NetEvent::ChannelData(key, Uint8Array::new(&data).to_vec()));
            } else if let Some(s) = data.as_string() {
                push(NetEvent::ChannelData(key, s.into_bytes()));
            }
        });
        dc.set_onmessage(Some(on_msg.as_ref().unchecked_ref()));
        let on_close = Closure::<dyn FnMut(JsValue)>::new(move |_| push(NetEvent::ChannelClosed(key)));
        dc.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        dc.set_onerror(Some(on_close.as_ref().unchecked_ref()));
        let on_low = Closure::<dyn FnMut(JsValue)>::new(move |_| push(NetEvent::ChannelLow));
        dc.set_onbufferedamountlow(Some(on_low.as_ref().unchecked_ref()));
        self.cbs.extend([on_open, on_msg, on_close, on_low]);
        self.dc = Some(dc.clone());
    }

    fn watch_state(&mut self, key: PeerKey) {
        let pc = self.pc.clone();
        let on_state = Closure::<dyn FnMut(JsValue)>::new(move |_| {
            use web_sys::RtcPeerConnectionState as S;
            if matches!(pc.connection_state(), S::Failed | S::Closed) {
                push(NetEvent::ChannelClosed(key));
            }
        });
        self.pc.set_onconnectionstatechange(Some(on_state.as_ref().unchecked_ref()));
        self.cbs.push(on_state);
    }

    /// Prépare une offre : le canal est créé de notre côté.
    pub fn offer(key: PeerKey, batch: u64, stun: &[String]) -> Result<PeerConn, JsValue> {
        let mut c = PeerConn::new(stun)?;
        let dc = c.pc.create_data_channel(&hex_label());
        c.wire_channel(key, &dc);
        c.watch_state(key);
        let pc = c.pc.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let sdp = async {
                let offer = JsFuture::from(pc.create_offer()).await?;
                let sdp = Reflect::get(&offer, &"sdp".into())?.as_string().unwrap_or_default();
                let init = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
                init.set_sdp(&sdp);
                JsFuture::from(pc.set_local_description(&init)).await?;
                Ok::<_, JsValue>(gathered_sdp(&pc).await)
            }
            .await
            .ok()
            .flatten();
            push(NetEvent::OfferReady { batch, key, sdp });
        });
        Ok(c)
    }

    /// Répond à l'offre d'un pair : le canal viendra de lui.
    pub fn answer(key: PeerKey, remote_sdp: String, stun: &[String]) -> Result<PeerConn, JsValue> {
        let mut c = PeerConn::new(stun)?;
        c.watch_state(key);
        let on_dc = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let e: RtcDataChannelEvent = e.unchecked_into();
            CHANNELS.with(|m| m.borrow_mut().push((key, e.channel())));
            push(NetEvent::ChannelOpen(key));
        });
        c.pc.set_ondatachannel(Some(on_dc.as_ref().unchecked_ref()));
        c.cbs.push(on_dc);
        let pc = c.pc.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let sdp = async {
                let init = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
                init.set_sdp(&remote_sdp);
                JsFuture::from(pc.set_remote_description(&init)).await?;
                let answer = JsFuture::from(pc.create_answer()).await?;
                let sdp = Reflect::get(&answer, &"sdp".into())?.as_string().unwrap_or_default();
                let init = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
                init.set_sdp(&sdp);
                JsFuture::from(pc.set_local_description(&init)).await?;
                Ok::<_, JsValue>(gathered_sdp(&pc).await)
            }
            .await
            .ok()
            .flatten();
            push(NetEvent::AnswerReady { key, sdp });
        });
        Ok(c)
    }

    /// La réponse du pair à notre offre est arrivée.
    pub fn accept_answer(&self, sdp: String) {
        let pc = self.pc.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let init = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
            init.set_sdp(&sdp);
            let _ = JsFuture::from(pc.set_remote_description(&init)).await;
        });
    }

    /// Côté réponse : le canal annoncé par `ondatachannel` est rangé ici.
    pub fn adopt_channel(&mut self, key: PeerKey) -> bool {
        if self.dc.is_some() {
            return true;
        }
        let ch = CHANNELS.with(|m| {
            let mut m = m.borrow_mut();
            m.iter().position(|(k, _)| *k == key).map(|i| m.remove(i).1)
        });
        match ch {
            Some(dc) => {
                self.wire_channel(key, &dc);
                true
            }
            None => false,
        }
    }

    pub fn is_channel_open(&self) -> bool {
        self.dc.as_ref().is_some_and(|d| d.ready_state() == web_sys::RtcDataChannelState::Open)
    }

    /// Place libre dans le tampon d'envoi.
    pub fn budget(&self) -> usize {
        match &self.dc {
            Some(dc) => HIGH_WATER.saturating_sub(dc.buffered_amount()) as usize,
            None => 0,
        }
    }

    pub fn send(&self, data: &[u8]) -> bool {
        let Some(dc) = &self.dc else { return false };
        for c in data.chunks(CHUNK) {
            if dc.send_with_u8_array(c).is_err() {
                return false;
            }
        }
        true
    }
}

impl Drop for PeerConn {
    fn drop(&mut self) {
        if let Some(dc) = &self.dc {
            dc.set_onopen(None);
            dc.set_onmessage(None);
            dc.set_onclose(None);
            dc.set_onerror(None);
            dc.set_onbufferedamountlow(None);
            dc.close();
        }
        self.pc.set_ondatachannel(None);
        self.pc.set_onconnectionstatechange(None);
        self.pc.close();
        self.cbs.clear();
    }
}

thread_local! {
    static CHANNELS: RefCell<Vec<(PeerKey, RtcDataChannel)>> = const { RefCell::new(Vec::new()) };
}

fn hex_label() -> String {
    mxtorrent_core::util::to_hex(&crate::web::random_bytes::<10>())
}

/// Attend la fin du rassemblement des candidats ICE (pas d'ICE « au fil de l'eau »,
/// comme WebTorrent), avec une limite de temps : on part avec ce qu'on a.
async fn gathered_sdp(pc: &RtcPeerConnection) -> Option<String> {
    for _ in 0..40 {
        if pc.ice_gathering_state() == RtcIceGatheringState::Complete {
            break;
        }
        crate::web::sleep(100).await;
    }
    pc.local_description().map(|d| d.sdp())
}
