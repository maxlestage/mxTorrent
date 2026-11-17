//! Trois langues : français, anglais et russe. Chaque texte est écrit dans les
//! trois à l'endroit où il sert, ce qui évite les clés orphelines.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    Fr,
    En,
    Ru,
}

static LANG: AtomicU8 = AtomicU8::new(Lang::Fr as u8);

pub fn set_lang(l: Lang) {
    LANG.store(l as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::En,
        2 => Lang::Ru,
        _ => Lang::Fr,
    }
}

/// Choisit le texte dans la langue courante.
pub fn tr(fr: &str, en: &str, ru: &str) -> String {
    match lang() {
        Lang::Fr => fr.to_string(),
        Lang::En => en.to_string(),
        Lang::Ru => ru.to_string(),
    }
}

/// Langue du navigateur au premier lancement : français et russe sont reconnus,
/// tout le reste a l'anglais.
pub fn browser_lang() -> Lang {
    #[cfg(target_arch = "wasm32")]
    {
        let lang = web_sys::window().and_then(|w| w.navigator().language()).unwrap_or_default();
        match lang.to_ascii_lowercase().get(0..2) {
            Some("fr") => Lang::Fr,
            Some("ru") => Lang::Ru,
            _ => Lang::En,
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    Lang::Fr
}

/// Nombre suivi de son unité, avec le pluriel russe correct
/// (1 пира n'existe pas : 1 пир, 2 пира, 5 пиров).
pub fn count(n: u64, fr: &str, en: &str, ru: [&str; 3]) -> String {
    match lang() {
        Lang::Fr => format!("{n} {fr}"),
        Lang::En => format!("{n} {en}"),
        Lang::Ru => {
            // 1 пир, 2 пира, 5 пиров ; 11–19 → пиров.
            let tens = n % 100 / 10;
            let form = if tens == 1 {
                2
            } else if n % 10 == 1 {
                0
            } else if (2..=4).contains(&(n % 10)) {
                1
            } else {
                2
            };
            format!("{n} {}", ru[form])
        }
    }
}

pub fn duration(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "∞".into();
    }
    let s = secs as u64;
    match lang() {
        Lang::Ru => {
            if s < 60 {
                format!("{s} с")
            } else if s < 3600 {
                format!("{} мин {:02} с", s / 60, s % 60)
            } else if s < 86_400 {
                format!("{} ч {:02} мин", s / 3600, (s % 3600) / 60)
            } else {
                format!("{} д {} ч", s / 86_400, (s % 86_400) / 3600)
            }
        }
        _ => {
            if s < 60 {
                format!("{s} s")
            } else if s < 3600 {
                format!("{} min {:02} s", s / 60, s % 60)
            } else if s < 86_400 {
                format!("{} h {:02} min", s / 3600, (s % 3600) / 60)
            } else {
                format!("{} {} {} h", s / 86_400, tr("j", "d", "д"), (s % 86_400) / 3600)
            }
        }
    }
}

pub fn rate(bps: f64) -> String {
    format!("{}/{}", mxtorrent_core::human_bytes(bps as u64), per_second())
}

/// Taille lisible, avec les unités de la langue courante.
pub fn bytes(n: u64) -> String {
    let s = mxtorrent_core::human_bytes(n);
    match lang() {
        Lang::Fr => s,
        Lang::En => s
            .replace("Kio", "KiB")
            .replace("Mio", "MiB")
            .replace("Gio", "GiB")
            .replace("Tio", "TiB")
            .replace("Pio", "PiB")
            .replace(" o", " B"),
        Lang::Ru => s
            .replace("Kio", "КиБ")
            .replace("Mio", "МиБ")
            .replace("Gio", "ГиБ")
            .replace("Tio", "ТиБ")
            .replace("Pio", "ПиБ")
            .replace(" o", " Б"),
    }
}

fn per_second() -> &'static str {
    match lang() {
        Lang::Ru => "с",
        _ => "s",
    }
}

pub fn rate_str(bps: f64) -> String {
    format!("{}/{}", bytes(bps as u64), per_second())
}
