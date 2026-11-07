//! Deux langues : français et anglais. Chaque texte est écrit dans les deux à
//! l'endroit où il sert, ce qui évite les clés orphelines.

use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn set_english(on: bool) {
    ENGLISH.store(on, Ordering::Relaxed);
}

pub fn is_english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// Choisit le texte dans la langue courante.
pub fn tr(fr: &str, en: &str) -> String {
    if is_english() { en.to_string() } else { fr.to_string() }
}

/// Langue du navigateur au premier lancement.
pub fn browser_prefers_english() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let lang = web_sys::window().and_then(|w| w.navigator().language()).unwrap_or_default();
        !lang.to_ascii_lowercase().starts_with("fr")
    }
    #[cfg(not(target_arch = "wasm32"))]
    false
}

pub fn duration(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "∞".into();
    }
    let s = secs as u64;
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min {:02} s", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{} h {:02} min", s / 3600, (s % 3600) / 60)
    } else {
        format!("{} {} {} h", s / 86_400, tr("j", "d"), (s % 86_400) / 3600)
    }
}

pub fn rate(bps: f64) -> String {
    format!("{}/s", mxtorrent_core::human_bytes(bps as u64))
}

/// Taille lisible, avec les unités de la langue courante.
pub fn bytes(n: u64) -> String {
    let s = mxtorrent_core::human_bytes(n);
    if is_english() {
        s.replace("Kio", "KiB").replace("Mio", "MiB").replace("Gio", "GiB").replace("Tio", "TiB").replace("Pio", "PiB").replace(" o", " B")
    } else {
        s
    }
}

pub fn rate_str(bps: f64) -> String {
    format!("{}/s", bytes(bps as u64))
}
