//! Petits ponts vers le navigateur : horloge, aléa, fichiers, téléchargements, minuteries.

use js_sys::{Array, Promise, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

pub fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

pub fn window() -> web_sys::Window {
    web_sys::window().expect("pas de fenêtre")
}

/// Aléa du navigateur (crypto.getRandomValues).
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    if let Ok(c) = window().crypto() {
        let _ = c.get_random_values_with_u8_array(&mut out);
    } else {
        for b in &mut out {
            *b = (js_sys::Math::random() * 256.0) as u8;
        }
    }
    out
}

pub fn random_u64() -> u64 {
    u64::from_le_bytes(random_bytes::<8>())
}

/// Attend `ms` millisecondes.
pub async fn sleep(ms: i32) {
    let p = Promise::new(&mut |resolve, _| {
        let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
    });
    let _ = JsFuture::from(p).await;
}

/// Type MIME d'après l'extension (pour l'aperçu et l'enregistrement).
pub fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "ogv" => "video/ogg",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "md" | "srt" | "vtt" | "log" | "nfo" => "text/plain;charset=utf-8",
        "json" => "application/json",
        "html" | "htm" => "text/plain;charset=utf-8", // jamais exécuté
        "torrent" => "application/x-bittorrent",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

pub fn can_preview(name: &str) -> bool {
    let m = mime_for(name);
    m.starts_with("video/") || m.starts_with("audio/") || m.starts_with("image/") || m.starts_with("text/") || m == "application/pdf"
}

fn make_blob(chunks: &[&[u8]], mime: &str) -> Result<web_sys::Blob, JsValue> {
    let parts = Array::new();
    for c in chunks {
        // Copie dans la mémoire JavaScript : la mémoire wasm peut bouger.
        parts.push(&Uint8Array::from(*c));
    }
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
    web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
}

/// Propose d'enregistrer des octets sous un nom de fichier.
pub fn save_file(name: &str, chunks: &[&[u8]]) -> Result<(), JsValue> {
    let blob = make_blob(chunks, mime_for(name))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let doc = window().document().unwrap();
    let a: web_sys::HtmlAnchorElement = doc.create_element("a")?.dyn_into()?;
    a.set_href(&url);
    a.set_download(name);
    a.style().set_property("display", "none")?;
    doc.body().unwrap().append_child(&a)?;
    a.click();
    a.remove();
    revoke_later(url, 60_000);
    Ok(())
}

/// Ouvre un fichier dans un nouvel onglet (vidéo, image, PDF…).
pub fn open_file(name: &str, chunks: &[&[u8]]) -> Result<(), JsValue> {
    let blob = make_blob(chunks, mime_for(name))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let _ = window().open_with_url_and_target(&url, "_blank")?;
    revoke_later(url, 10 * 60_000);
    Ok(())
}

fn revoke_later(url: String, ms: i32) {
    let cb = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), ms);
}

/// Ouvre le sélecteur de fichiers et rend les fichiers choisis (chemin, octets).
pub fn pick_files(accept: &str, multiple: bool, on_done: impl FnOnce(Vec<(String, Vec<u8>)>) + 'static) {
    let doc = window().document().unwrap();
    let Ok(el) = doc.create_element("input") else { return };
    let input: web_sys::HtmlInputElement = el.unchecked_into();
    input.set_type("file");
    input.set_multiple(multiple);
    if !accept.is_empty() {
        input.set_accept(accept);
    }
    let _ = input.style().set_property("display", "none");
    let _ = doc.body().unwrap().append_child(&input);
    let input2 = input.clone();
    let cb = Closure::once_into_js(move || {
        let files = input2.files();
        input2.remove();
        wasm_bindgen_futures::spawn_local(async move {
            let mut out = Vec::new();
            if let Some(list) = files {
                for i in 0..list.length() {
                    if let Some(f) = list.get(i)
                        && let Ok(buf) = JsFuture::from(f.array_buffer()).await
                    {
                        out.push((f.name(), Uint8Array::new(&buf).to_vec()));
                    }
                }
            }
            on_done(out);
        });
    });
    input.set_onchange(Some(cb.unchecked_ref()));
    input.click();
}

/// Télécharge une URL (fichier `.torrent` ou source web), éventuellement une plage d'octets.
pub async fn fetch_bytes(url: &str, range: Option<(u64, u64)>) -> Result<(u16, Vec<u8>), String> {
    let init = web_sys::RequestInit::new();
    init.set_method("GET");
    init.set_mode(web_sys::RequestMode::Cors);
    if let Some((s, e)) = range {
        let h = web_sys::Headers::new().map_err(|e| format!("{e:?}"))?;
        h.set("Range", &format!("bytes={s}-{e}")).map_err(|e| format!("{e:?}"))?;
        init.set_headers(&h);
    }
    let req = web_sys::Request::new_with_str_and_init(url, &init).map_err(|e| format!("{e:?}"))?;
    let resp = JsFuture::from(window().fetch_with_request(&req)).await.map_err(|_| "réseau ou CORS refusé".to_string())?;
    let resp: web_sys::Response = resp.dyn_into().map_err(|_| "réponse invalide")?;
    let status = resp.status();
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    let buf = JsFuture::from(resp.array_buffer().map_err(|e| format!("{e:?}"))?).await.map_err(|e| format!("{e:?}"))?;
    Ok((status, Uint8Array::new(&buf).to_vec()))
}

/// Paramètre de l'URL de la page (`?magnet=…`), utilisé par le gestionnaire de liens magnet.
pub fn query_param(name: &str) -> Option<String> {
    let search = window().location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get(name).filter(|s| !s.is_empty())
}

pub fn clear_query() {
    if let Ok(h) = window().history() {
        let path = window().location().pathname().unwrap_or_default();
        let _ = h.replace_state_with_url(&JsValue::NULL, "", Some(&path));
    }
}

/// Propose au navigateur d'ouvrir les liens magnet avec mxTorrent.
pub fn register_magnet_handler() -> bool {
    let nav = window().navigator();
    let base = window().location().href().unwrap_or_default();
    let base = base.split(['?', '#']).next().unwrap_or("").to_string();
    nav.register_protocol_handler("magnet", &format!("{base}?magnet=%s"), "mxTorrent").is_ok()
}

pub fn notify_title(text: &str) {
    if let Some(doc) = window().document() {
        doc.set_title(text);
    }
}

/// Si la page est servie par mxtorrent-tracker (Heroku, serveur perso…), ce serveur
/// est aussi un tracker : on le détecte en lisant `../announce`.
pub fn detect_local_tracker() {
    let loc = window().location();
    let (Ok(proto), Ok(host), Ok(path)) = (loc.protocol(), loc.host(), loc.pathname()) else { return };
    // /app/ → / ; /mxTorrent/app/ → /mxTorrent/
    let base = path.trim_end_matches('/').trim_end_matches("/app").trim_end_matches('/').to_string();
    let http = format!("{proto}//{host}{base}/announce");
    let ws = format!("{}//{host}{base}/announce", if proto == "https:" { "wss:" } else { "ws:" });
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok((_, body)) = fetch_bytes(&http, None).await
            && body.starts_with(b"mxtorrent-tracker")
        {
            crate::net::push(crate::net::NetEvent::LocalTracker(ws));
        }
    });
}
