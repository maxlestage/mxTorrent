//! # mxtorrent-core
//!
//! Le moteur BitTorrent de mxTorrent, écrit sans aucune entrée-sortie : il ne
//! connaît ni le réseau, ni les fichiers, ni l'horloge. Le client web l'alimente
//! avec les octets des canaux WebRTC ; les tests l'alimentent en mémoire.
//!
//! - [`bencode`] : l'encodage des `.torrent` ;
//! - [`metainfo`] : lecture et création de torrents ;
//! - [`magnet`] : liens magnet ;
//! - [`wire`] : le protocole des pairs ;
//! - [`torrent`] : une session complète (métadonnées, pièces, envoi) ;
//! - [`tracker`] : le protocole des trackers WebSocket de WebTorrent.

pub mod bencode;
pub mod bitfield;
pub mod magnet;
pub mod metainfo;
pub mod relay;
pub mod torrent;
pub mod tracker;
pub mod util;
pub mod wire;

pub use magnet::Magnet;
pub use metainfo::{CreateOptions, FileEntry, Info, Metainfo, NewFile, create_torrent};
pub use torrent::{Event, PeerInfo, PeerKey, State, Torrent};
pub use util::{InfoHash, Rng, human_bytes, make_peer_id};
