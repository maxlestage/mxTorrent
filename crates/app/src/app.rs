//! L'interface egui. Pensée d'abord pour le téléphone : une colonne, des cibles
//! larges, des feuilles qui montent du bas ; deux colonnes sur grand écran.
//!
//! Rien n'est animé en continu : l'interface ne se redessine que lorsqu'il se
//! passe quelque chose (geste, donnée reçue), et une fois par seconde au plus
//! pendant un transfert.

use crate::engine::{DEFAULT_TRACKERS, Engine, Entry, Saved, Settings};
use crate::i18n::{bytes, duration, is_english, rate_str, set_english, tr};
use crate::icons::{self, Icon};
use crate::kit::{self, Kind};
use crate::net::{self, NetEvent};
use crate::theme::{self, AMBER, CYAN, GREEN, PINK, Palette, RED, VIOLET, font, font_semibold, mix};
use crate::web;
use egui::{Align, Align2, Color32, CornerRadius, Id, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2, vec2};
use mxtorrent_core::{InfoHash, State};
use std::cell::RefCell;
use std::rc::Rc;

/// Clé du stockage local (JSON). Les versions précédentes rangeaient un autre format sous « mxtorrent ».
const STORAGE_KEY: &str = "mxtorrent-v2";

thread_local! {
    /// Le contexte egui, pour que le moteur demande un rafraîchissement.
    static CTX: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// Demande à l'interface de se redessiner (appelé par l'horloge du moteur).
pub fn request_repaint() {
    CTX.with(|c| {
        if let Some(ctx) = c.borrow().as_ref() {
            ctx.request_repaint();
        }
    });
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Prefs {
    dark: bool,
    english: bool,
    saved: Saved,
    welcomed: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { dark: true, english: crate::i18n::browser_prefers_english(), saved: Saved::default(), welcomed: false }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sheet {
    Add,
    Share(InfoHash),
    Remove(InfoHash),
    Settings,
}

pub struct MxApp {
    engine: Rc<RefCell<Engine>>,
    selected: Option<InfoHash>,
    tab: usize,
    filter: usize,
    search: String,
    link: String,
    link_error: Option<String>,
    sheet: Option<Sheet>,
    settings_draft: Option<Settings>,
    new_tracker: String,
    log_open: bool,
    dark: bool,
    welcomed: bool,
    mobile: bool,
    style_key: Option<(bool, bool)>,
    title: String,
    saved_at: u64,
    saved_json: String,
}

impl MxApp {
    pub fn new(cc: &eframe::CreationContext<'_>, engine: Rc<RefCell<Engine>>) -> MxApp {
        theme::install_fonts(&cc.egui_ctx);
        CTX.with(|c| *c.borrow_mut() = Some(cc.egui_ctx.clone()));
        let prefs: Prefs = web::load(STORAGE_KEY).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        set_english(prefs.english);
        match web::query_param("lang").as_deref() {
            Some("en") => set_english(true),
            Some("fr") => set_english(false),
            _ => {}
        }
        {
            let mut e = engine.borrow_mut();
            e.settings = prefs.saved.settings.clone();
            if e.settings.trackers.is_empty() {
                e.settings.trackers = DEFAULT_TRACKERS.iter().map(|s| s.to_string()).collect();
            }
            e.restore(prefs.saved);
        }
        // Passerelle : celle inscrite dans la page ; celle du serveur, si c'en est un, la remplace.
        engine.borrow_mut().auto_gateway = web::configured_gateway();
        web::detect_local_tracker();
        // Lien magnet transmis par le système (gestionnaire de protocole, partage Android…).
        for key in ["magnet", "url", "text", "torrent"] {
            if let Some(v) = web::query_param(key) {
                if let Some(pos) = v.find("magnet:") {
                    net::push(NetEvent::Link(v[pos..].to_string()));
                } else if v.starts_with("http") || v.len() == 40 || v.len() == 32 {
                    net::push(NetEvent::Link(v));
                }
                web::clear_query();
            }
        }
        MxApp {
            engine,
            selected: None,
            tab: 0,
            filter: 0,
            search: String::new(),
            link: String::new(),
            link_error: None,
            sheet: None,
            settings_draft: None,
            new_tracker: String::new(),
            log_open: false,
            dark: prefs.dark,
            welcomed: prefs.welcomed,
            mobile: false,
            style_key: None,
            title: String::new(),
            saved_at: 0,
            saved_json: String::new(),
        }
    }

    /// Range les réglages et la liste des torrents dans le stockage local, s'ils ont changé.
    fn save_if_changed(&mut self) {
        let now = web::now_ms();
        if now.saturating_sub(self.saved_at) < 3_000 {
            return;
        }
        self.saved_at = now;
        let prefs = Prefs { dark: self.dark, english: is_english(), saved: self.engine.borrow().snapshot(), welcomed: self.welcomed };
        if let Ok(json) = serde_json::to_string(&prefs)
            && json != self.saved_json
        {
            web::store(STORAGE_KEY, &json);
            self.saved_json = json;
        }
    }

    fn p(&self) -> Palette {
        theme::palette(self.dark)
    }

    /// Ajoute ce que contient `self.link` ; garde l'erreur pour l'afficher.
    fn submit_link(&mut self) -> bool {
        let text = self.link.trim().to_string();
        if text.is_empty() {
            self.link_error = Some(tr("Collez d'abord un lien magnet.", "Paste a magnet link first."));
            return false;
        }
        let mut ok = true;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            if let Err(e) = self.engine.borrow_mut().add_link(line) {
                self.link_error = Some(e);
                ok = false;
            }
        }
        if ok {
            self.link.clear();
            self.link_error = None;
        }
        ok
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        // Fichiers déposés : les .torrent sont ouverts, le reste est partagé.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty() {
            wasm_bindgen_futures::spawn_local(async move {
                let mut torrents = Vec::new();
                let mut share = Vec::new();
                for f in dropped {
                    let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "fichier".into());
                    if let Ok(b) = f.bytes_async().await {
                        if name.to_ascii_lowercase().ends_with(".torrent") {
                            torrents.push((name, b));
                        } else {
                            share.push((name, b));
                        }
                    }
                }
                if !torrents.is_empty() {
                    net::push(NetEvent::TorrentFiles(torrents));
                }
                if !share.is_empty() {
                    net::push(NetEvent::Share(share));
                }
            });
        }
        // Coller un lien magnet n'importe où l'ajoute.
        if ctx.memory(|m| m.focused().is_none()) {
            let pasted: Vec<String> =
                ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None }).collect());
            for t in pasted {
                if t.trim_start().starts_with("magnet:") {
                    net::push(NetEvent::Link(t));
                }
            }
        }
        // Un torrent vient d'être créé : on le montre, avec sa feuille de partage.
        if let Some(h) = self.engine.borrow_mut().just_created.take() {
            self.selected = Some(h);
            self.sheet = Some(Sheet::Share(h));
            self.welcomed = true;
        }
    }

    // ------------------------------------------------------------ barre du haut

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.p();
        let detail_on_phone = self.mobile && self.selected.is_some();
        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 6 }))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(44.0);
                    if detail_on_phone {
                        if kit::icon_button(ui, p, Icon::Back, 44.0, &tr("Retour", "Back")).clicked() {
                            self.selected = None;
                        }
                    } else {
                        ui.add_space(4.0);
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                        icons::logo(ui.painter(), r);
                        ui.add_space(2.0);
                        ui.label(RichText::new("mx").font(font_semibold(20.0)).color(VIOLET));
                        ui.add_space(-8.0);
                        ui.label(RichText::new("Torrent").font(font_semibold(20.0)));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if kit::icon_button(ui, p, Icon::Settings, 44.0, &tr("Réglages", "Settings")).clicked() {
                            self.settings_draft = Some(self.engine.borrow().settings.clone());
                            self.sheet = Some(Sheet::Settings);
                        }
                        if !self.mobile {
                            if kit::button(ui, p, Kind::Primary, Some(Icon::Plus), &tr("Ajouter", "Add"), false).clicked() {
                                self.sheet = Some(Sheet::Add);
                                self.link_error = None;
                            }
                            let (d, u) = self.engine.borrow().total_rates();
                            if d > 0.0 || u > 0.0 {
                                ui.add_space(8.0);
                                ui.label(RichText::new(format!("↑ {}", rate_str(u))).font(font(13.5)).color(PINK));
                                ui.label(RichText::new(format!("↓ {}", rate_str(d))).font(font(13.5)).color(CYAN));
                            }
                        }
                    });
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.p();
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin::symmetric(16, 8)))
            .show_separator_line(false)
            .show(ui, |ui| {
                let (n, pending, peers) = {
                    let e = self.engine.borrow();
                    (e.connected_trackers(), e.connecting_peers(), e.entries.iter().map(|x| x.torrent.peer_count()).sum::<usize>())
                };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, if n > 0 { GREEN } else { p.faint });
                    kit::muted(ui, p, &format!("{} {n}", tr("trackers connectés :", "trackers connected:")));
                    kit::muted(ui, p, &format!("{} {peers}", tr("pairs :", "peers:")));
                    let (phase, _, _, gw_peers) = self.engine.borrow().gateway_status();
                    if phase == crate::gateway::Phase::Ready {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.0, GREEN);
                        kit::muted(
                            ui,
                            p,
                            &format!(
                                "{} 🥦 · {gw_peers} {}",
                                tr("passerelle TCP/UDP chiffrée", "encrypted TCP/UDP gateway"),
                                tr("pairs", "peers")
                            ),
                        );
                    }
                    if pending > 0 {
                        kit::muted(ui, p, &format!("{} {pending}", tr("connexions en cours :", "connecting:")));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        kit::muted(ui, p, concat!("v", env!("CARGO_PKG_VERSION")));
                        if ui.link(RichText::new(tr("Journal", "Log")).font(font(13.0))).clicked() {
                            self.log_open = !self.log_open;
                        }
                    });
                });
            });
    }

    // ------------------------------------------------------------ liste

    fn list(&mut self, ui: &mut egui::Ui) {
        let p = self.p();
        let count = self.engine.borrow().entries.len();
        if count == 0 {
            self.empty_state(ui);
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Torrents").font(font_semibold(26.0)));
            ui.label(RichText::new(count.to_string()).font(font_semibold(26.0)).color(p.faint));
        });
        if self.mobile {
            let (d, u) = self.engine.borrow().total_rates();
            if d > 0.0 || u > 0.0 {
                ui.label(RichText::new(format!("↓ {}   ↑ {}", rate_str(d), rate_str(u))).font(font(13.5)).color(p.muted));
            }
        }
        ui.add_space(4.0);
        if count >= 3 {
            let items = [tr("Tous", "All"), tr("Actifs", "Active"), tr("Terminés", "Done"), tr("En pause", "Paused")];
            kit::segmented(ui, p, &items, &mut self.filter);
            ui.add_space(4.0);
        }
        if count > 6 {
            kit::field(ui, p, &mut self.search, &tr("Rechercher", "Search"));
            ui.add_space(4.0);
        }
        let engine = self.engine.clone();
        let e = engine.borrow();
        let search = self.search.to_lowercase();
        let filter = if count >= 3 { self.filter } else { 0 };
        let items: Vec<usize> = (0..e.entries.len())
            .rev()
            .filter(|&i| {
                let t = &e.entries[i].torrent;
                let ok = match filter {
                    1 => matches!(t.state(), State::Downloading | State::FetchingMetadata),
                    2 => t.state() == State::Seeding,
                    3 => t.state() == State::Paused,
                    _ => true,
                };
                ok && (search.is_empty() || t.name().to_lowercase().contains(&search))
            })
            .collect();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if items.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| kit::muted(ui, p, &tr("Rien ici.", "Nothing here.")));
            }
            for i in items {
                let entry = &e.entries[i];
                let h = entry.info_hash();
                if torrent_card(ui, p, entry, self.selected == Some(h)).clicked() {
                    self.selected = Some(h);
                    self.tab = 0;
                }
                ui.add_space(8.0);
            }
            ui.add_space(if self.mobile { 96.0 } else { 16.0 });
        });
    }

    /// L'écran d'accueil : télécharger ou partager, sans détour.
    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let p = self.p();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let w = ui.available_width().min(460.0);
            ui.vertical_centered(|ui| {
                ui.set_max_width(w);
                ui.add_space(if self.mobile { 12.0 } else { 40.0 });
                let (r, _) = ui.allocate_exact_size(Vec2::splat(84.0), Sense::hover());
                icons::logo(ui.painter(), r);
                ui.add_space(10.0);
                ui.label(RichText::new(tr("Bienvenue sur mxTorrent", "Welcome to mxTorrent")).font(font_semibold(24.0)));
                ui.add_space(2.0);
                ui.label(
                    RichText::new(tr(
                        "Téléchargez avec un lien magnet ou partagez vos fichiers, directement de navigateur à navigateur.",
                        "Download with a magnet link or share your files, straight from browser to browser.",
                    ))
                    .font(font(15.0))
                    .color(p.muted),
                );
                ui.add_space(18.0);
            });
            ui.vertical_centered(|ui| {
                ui.set_max_width(w);
                kit::card(ui, p, |ui| {
                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                        kit::section(ui, p, &tr("Télécharger", "Download"));
                        ui.add_space(2.0);
                        let resp = kit::field(ui, p, &mut self.link, &tr("Lien magnet ou empreinte", "Magnet link or info hash"));
                        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if let Some(err) = &self.link_error {
                            ui.label(RichText::new(err).font(font(13.5)).color(RED));
                        }
                        ui.add_space(2.0);
                        if kit::button(ui, p, Kind::Primary, Some(Icon::Download), &tr("Télécharger", "Download"), true).clicked() || enter
                        {
                            self.submit_link();
                        }
                    });
                });
                ui.add_space(12.0);
                kit::card(ui, p, |ui| {
                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                        kit::section(ui, p, &tr("Partager", "Share"));
                        ui.label(
                            RichText::new(tr(
                                "Choisissez des fichiers : mxTorrent crée le torrent et vous donne un lien et un QR code.",
                                "Pick files: mxTorrent creates the torrent and gives you a link and a QR code.",
                            ))
                            .font(font(14.5))
                            .color(p.muted),
                        );
                        ui.add_space(4.0);
                        if kit::button(ui, p, Kind::Secondary, Some(Icon::Upload), &tr("Choisir des fichiers", "Choose files"), true)
                            .clicked()
                        {
                            pick_share();
                        }
                        if kit::button(
                            ui,
                            p,
                            Kind::Ghost,
                            Some(Icon::File),
                            &tr("Ouvrir un fichier .torrent", "Open a .torrent file"),
                            true,
                        )
                        .clicked()
                        {
                            pick_torrent();
                        }
                    });
                });
                ui.add_space(18.0);
                ui.label(
                    RichText::new(tr(
                        "Rien à installer, aucun compte. Les données passent directement entre navigateurs, chiffrées.",
                        "Nothing to install, no account. Data flows directly between browsers, encrypted.",
                    ))
                    .font(font(12.5))
                    .color(p.faint),
                );
                ui.add_space(24.0);
            });
        });
    }

    // ------------------------------------------------------------ détails

    fn details(&mut self, ui: &mut egui::Ui, h: InfoHash) {
        let p = self.p();
        let engine = self.engine.clone();
        let mut action_pause = None;
        let mut action_copy = false;
        let mut action_file: Option<(usize, bool)> = None;
        {
            let e = engine.borrow();
            let Some(i) = e.find(&h) else {
                self.selected = None;
                return;
            };
            let entry = &e.entries[i];
            let t = &entry.torrent;
            let state = t.state();
            let color = kit::state_color(state);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let narrow = ui.available_width() < 520.0;
                // En-tête : anneau de progression, nom, état.
                ui.horizontal(|ui| {
                    let frac = if state == State::FetchingMetadata { t.metadata_progress() } else { t.progress() };
                    kit::ring(ui, p, frac, color, if narrow { 104.0 } else { 128.0 });
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.add_space(8.0);
                        ui.add(egui::Label::new(RichText::new(t.name()).font(font_semibold(19.0))).wrap());
                        ui.add_space(2.0);
                        ui.horizontal_wrapped(|ui| {
                            kit::chip(ui, &state_label(state), color);
                            if entry.created_here {
                                kit::chip(ui, &tr("créé ici", "created here"), CYAN);
                            }
                        });
                        let size = match t.total_length() {
                            Some(total) => format!("{} {} {}", bytes(t.verified_bytes()), tr("sur", "of"), bytes(total)),
                            None => tr("Recherche des métadonnées auprès des pairs…", "Fetching metadata from peers…"),
                        };
                        kit::muted(ui, p, &size);
                    });
                });
                ui.add_space(12.0);
                // Actions.
                let paused = t.is_paused();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let w = ((ui.available_width() - 24.0) / 4.0).max(60.0);
                    let (ic, label) = if paused { (Icon::Play, tr("Reprendre", "Resume")) } else { (Icon::Pause, tr("Pause", "Pause")) };
                    if kit::tile(ui, p, ic, &label, p.text, w).clicked() {
                        action_pause = Some(!paused);
                    }
                    if kit::tile(ui, p, Icon::Qr, &tr("Partager", "Share"), p.text, w).clicked() {
                        self.sheet = Some(Sheet::Share(h));
                    }
                    if kit::tile(ui, p, Icon::Link, &tr("Copier", "Copy"), p.text, w).clicked() {
                        action_copy = true;
                    }
                    if kit::tile(ui, p, Icon::Trash, &tr("Retirer", "Remove"), RED, w).clicked() {
                        self.sheet = Some(Sheet::Remove(h));
                    }
                });
                ui.add_space(14.0);
                let tabs = [tr("Aperçu", "Overview"), tr("Fichiers", "Files"), tr("Pairs", "Peers"), "Trackers".to_string()];
                kit::segmented(ui, p, &tabs, &mut self.tab);
                ui.add_space(10.0);
                match self.tab {
                    1 => files_tab(ui, p, entry, &mut action_file),
                    2 => peers_tab(ui, p, entry, &e),
                    3 => trackers_tab(ui, p, entry, &e),
                    _ => overview_tab(ui, p, entry),
                }
                ui.add_space(24.0);
            });
        }
        let mut e = engine.borrow_mut();
        if let Some((fi, w)) = action_file
            && let Some(i) = e.find(&h)
        {
            e.entries[i].torrent.set_file_wanted(fi, w);
        }
        if let Some(pz) = action_pause {
            e.set_paused(&h, pz);
        }
        if action_copy && let Some(i) = e.find(&h) {
            let m = e.entries[i].magnet();
            ui.ctx().copy_text(m);
            e.toast(tr("Lien magnet copié", "Magnet link copied"), true);
        }
    }

    fn placeholder(&self, ui: &mut egui::Ui) {
        let p = self.p();
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            let (r, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
            icons::logo(ui.painter(), r);
            ui.add_space(8.0);
            kit::muted(ui, p, &tr("Choisissez un torrent pour voir ses détails.", "Pick a torrent to see its details."));
        });
    }

    // ------------------------------------------------------------ feuilles

    fn sheets(&mut self, ctx: &egui::Context) {
        let Some(sheet) = self.sheet else { return };
        let close = match sheet {
            Sheet::Add => self.add_sheet(ctx),
            Sheet::Share(h) => self.share_sheet(ctx, h),
            Sheet::Remove(h) => self.remove_sheet(ctx, h),
            Sheet::Settings => self.settings_sheet(ctx),
        };
        if close && self.sheet == Some(sheet) {
            self.sheet = None;
        }
    }

    fn add_sheet(&mut self, ctx: &egui::Context) -> bool {
        let p = self.p();
        let (done, close) = show_sheet(ctx, p, self.mobile, "add", |ui| {
            let mut done = false;
            ui.label(RichText::new(tr("Ajouter un torrent", "Add a torrent")).font(font_semibold(21.0)));
            ui.add_space(6.0);
            let resp =
                kit::field(ui, p, &mut self.link, &tr("Lien magnet, empreinte ou URL .torrent", "Magnet link, info hash or .torrent URL"));
            if !self.mobile && !resp.has_focus() && self.link.is_empty() {
                resp.request_focus();
            }
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Some(err) = &self.link_error {
                ui.label(RichText::new(err).font(font(13.5)).color(RED));
            }
            ui.add_space(2.0);
            if kit::button(ui, p, Kind::Primary, Some(Icon::Download), &tr("Télécharger", "Download"), true).clicked() || enter {
                done = self.submit_link();
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 8.0) / 2.0;
                ui.allocate_ui(vec2(w, 52.0), |ui| {
                    if kit::button(ui, p, Kind::Secondary, Some(Icon::Upload), &tr("Partager", "Share"), true).clicked() {
                        pick_share();
                        done = true;
                    }
                });
                ui.allocate_ui(vec2(w, 52.0), |ui| {
                    if kit::button(ui, p, Kind::Secondary, Some(Icon::File), ".torrent", true).clicked() {
                        pick_torrent();
                        done = true;
                    }
                });
            });
            done
        });
        done || close
    }

    fn share_sheet(&mut self, ctx: &egui::Context, h: InfoHash) -> bool {
        let p = self.p();
        let info = {
            let e = self.engine.borrow();
            e.find(&h).map(|i| {
                let en = &e.entries[i];
                (en.torrent.name(), en.magnet(), en.torrent.metainfo().map(|m| (m.info.name.clone(), m.to_torrent())))
            })
        };
        let Some((name, magnet, torrent)) = info else { return true };
        let engine = self.engine.clone();
        let (done, close) = show_sheet(ctx, p, self.mobile, "share", |ui| {
            let mut done = false;
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr("Partager", "Share")).font(font_semibold(21.0)));
                ui.add(egui::Label::new(RichText::new(&name).font(font(14.0)).color(p.muted)).truncate());
                ui.add_space(10.0);
                let size = ui.available_width().min(220.0).min(ui.ctx().content_rect().height() * 0.3);
                crate::qr::draw(ui, &magnet, size);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(tr(
                        "Scannez ce code avec un autre téléphone, ou envoyez le lien. Gardez cet onglet ouvert : c'est lui qui partage.",
                        "Scan this code with another phone, or send the link. Keep this tab open: it is the one sharing.",
                    ))
                    .font(font(13.0))
                    .color(p.muted),
                );
            });
            ui.add_space(10.0);
            if web::can_share() {
                if kit::button(ui, p, Kind::Primary, Some(Icon::Share), &tr("Envoyer le lien…", "Send the link…"), true).clicked() {
                    web::share_link(&name, &app_link(&magnet));
                }
                if kit::button(ui, p, Kind::Secondary, Some(Icon::Link), &tr("Copier le lien magnet", "Copy the magnet link"), true)
                    .clicked()
                {
                    ui.ctx().copy_text(magnet.clone());
                    engine.borrow_mut().toast(tr("Lien copié", "Link copied"), true);
                }
            } else if kit::button(ui, p, Kind::Primary, Some(Icon::Link), &tr("Copier le lien magnet", "Copy the magnet link"), true)
                .clicked()
            {
                ui.ctx().copy_text(magnet.clone());
                engine.borrow_mut().toast(tr("Lien copié", "Link copied"), true);
            }
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 8.0) / 2.0;
                ui.allocate_ui(vec2(w, 52.0), |ui| {
                    if kit::button(ui, p, Kind::Secondary, Some(Icon::Globe), &tr("Lien web", "Web link"), true).clicked() {
                        ui.ctx().copy_text(app_link(&magnet));
                        engine.borrow_mut().toast(tr("Lien vers l'application copié", "App link copied"), true);
                    }
                });
                ui.allocate_ui(vec2(w, 52.0), |ui| {
                    if let Some((n, bytes)) = &torrent
                        && kit::button(ui, p, Kind::Secondary, Some(Icon::Download), ".torrent", true).clicked()
                    {
                        let _ = web::save_file(&format!("{n}.torrent"), &[bytes]);
                    }
                });
            });
            if kit::button(ui, p, Kind::Ghost, None, &tr("Fermer", "Close"), true).clicked() {
                done = true;
            }
            done
        });
        done || close
    }

    fn remove_sheet(&mut self, ctx: &egui::Context, h: InfoHash) -> bool {
        let p = self.p();
        let name = {
            let e = self.engine.borrow();
            match e.find(&h) {
                Some(i) => e.entries[i].torrent.name(),
                None => return true,
            }
        };
        let (done, close) = show_sheet(ctx, p, self.mobile, "remove", |ui| {
            let mut done = false;
            ui.label(RichText::new(tr("Retirer ce torrent ?", "Remove this torrent?")).font(font_semibold(21.0)));
            ui.add(egui::Label::new(RichText::new(&name).font(font_semibold(15.0))).wrap());
            kit::muted(
                ui,
                p,
                &tr(
                    "Les données téléchargées vivent dans cet onglet : enregistrez d'abord les fichiers que vous voulez garder.",
                    "Downloaded data lives in this tab: save the files you want to keep first.",
                ),
            );
            ui.add_space(8.0);
            if kit::button(ui, p, Kind::Danger, Some(Icon::Trash), &tr("Retirer", "Remove"), true).clicked() {
                self.engine.borrow_mut().remove(&h);
                if self.selected == Some(h) {
                    self.selected = None;
                }
                done = true;
            }
            if kit::button(ui, p, Kind::Ghost, None, &tr("Annuler", "Cancel"), true).clicked() {
                done = true;
            }
            done
        });
        done || close
    }

    fn settings_sheet(&mut self, ctx: &egui::Context) -> bool {
        let p = self.p();
        let peer_id = String::from_utf8_lossy(&self.engine.borrow().peer_id).into_owned();
        let (done, close) = show_sheet(ctx, p, self.mobile, "settings", |ui| {
            let mut done = false;
            ui.label(RichText::new(tr("Réglages", "Settings")).font(font_semibold(21.0)));
            kit::section(ui, p, &tr("Apparence", "Appearance"));
            let mut theme_i = if self.dark { 0 } else { 1 };
            if kit::segmented(ui, p, &[tr("Sombre", "Dark"), tr("Clair", "Light")], &mut theme_i) {
                self.dark = theme_i == 0;
            }
            let mut lang_i = if is_english() { 1 } else { 0 };
            if kit::segmented(ui, p, &["Français".to_string(), "English".to_string()], &mut lang_i) {
                set_english(lang_i == 1);
            }
            let Some(d) = &mut self.settings_draft else { return true };
            kit::section(ui, p, &tr("Réseau", "Network"));
            kit::card(ui, p, |ui| {
                ui.spacing_mut().interact_size.y = 36.0;
                kit::stepper(ui, p, &tr("Pairs par torrent", "Peers per torrent"), &mut d.max_peers, 5, 100, 5);
                kit::stepper(ui, p, &tr("Offres par annonce", "Offers per announce"), &mut d.offers_per_announce, 1, 10, 1);
                ui.checkbox(&mut d.use_web_seeds, tr("Sources web (HTTP)", "Web seeds (HTTP)"));
            });
            kit::section(ui, p, &tr("Trackers WebSocket", "WebSocket trackers"));
            kit::card(ui, p, |ui| {
                let mut remove = None;
                for (i, t) in d.trackers.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(RichText::new(t.trim_start_matches("wss://")).font(font(14.0))).truncate());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if kit::icon_button(ui, p, Icon::Close, 32.0, &tr("Retirer", "Remove")).clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                }
                if let Some(i) = remove {
                    d.trackers.remove(i);
                }
                kit::field(ui, p, &mut self.new_tracker, "wss://…");
                let ok = self.new_tracker.starts_with("wss://") || self.new_tracker.starts_with("ws://");
                ui.horizontal(|ui| {
                    if ok && kit::button(ui, p, Kind::Secondary, Some(Icon::Plus), &tr("Ajouter", "Add"), false).clicked() {
                        d.trackers.push(self.new_tracker.trim().to_string());
                        self.new_tracker.clear();
                    }
                    if kit::button(ui, p, Kind::Ghost, None, &tr("Par défaut", "Defaults"), false).clicked() {
                        d.trackers = DEFAULT_TRACKERS.iter().map(|s| s.to_string()).collect();
                    }
                });
            });
            kit::section(ui, p, &tr("Passerelle TCP/UDP", "TCP/UDP gateway"));
            kit::card(ui, p, |ui| {
                ui.label(
                    RichText::new(tr(
                        "Un navigateur ne peut pas ouvrir de connexion TCP ou UDP : la passerelle le fait pour lui (trackers UDP et HTTP, DHT, pairs qBittorrent, Transmission…). Tout ce qui passe entre elle et vous est chiffré par Brocolis 🥦 (X25519 + ML-KEM-768), puis par MSE vers les pairs.",
                        "A browser cannot open TCP or UDP connections: the gateway does it for it (UDP and HTTP trackers, DHT, qBittorrent, Transmission peers…). Everything between it and you is encrypted by Brocolis 🥦 (X25519 + ML-KEM-768), then by MSE towards peers.",
                    ))
                    .font(font(13.0))
                    .color(p.muted),
                );
                ui.checkbox(&mut d.use_gateway, tr("Utiliser la passerelle", "Use the gateway"));
                kit::field(ui, p, &mut d.gateway, &tr("Adresse (vide : le serveur de l'application)", "Address (empty: the app's server)"));
                let (phase, detail, fp, n) = self.engine.borrow().gateway_status();
                let (color, text) = match phase {
                    crate::gateway::Phase::Ready => (
                        GREEN,
                        format!(
                            "{} · {n} {}",
                            tr("Connectée, chiffrée Brocolis v4", "Connected, Brocolis v4 encrypted"),
                            tr("pairs", "peers")
                        ),
                    ),
                    crate::gateway::Phase::Off => (
                        p.faint,
                        tr("Aucune passerelle : seuls les pairs WebRTC sont joignables", "No gateway: only WebRTC peers are reachable"),
                    ),
                    crate::gateway::Phase::Failed => (AMBER, format!("{} : {detail}", tr("Indisponible", "Unavailable"))),
                    _ => (AMBER, detail),
                };
                ui.label(RichText::new(text).font(font_semibold(13.5)).color(color));
                if let Some(fp) = fp {
                    ui.label(RichText::new(format!("{} {fp}", tr("Empreinte :", "Fingerprint:"))).font(font(12.0)).color(p.faint));
                }
            });
            kit::section(ui, p, &tr("Système", "System"));
            if kit::button(ui, p, Kind::Secondary, Some(Icon::Magnet), &tr("Ouvrir les liens magnet ici", "Open magnet links here"), true)
                .clicked()
            {
                let ok = web::register_magnet_handler();
                self.engine.borrow_mut().toast(
                    if ok {
                        tr("Demande envoyée au navigateur", "Request sent to the browser")
                    } else {
                        tr("Refusé par ce navigateur", "Refused by this browser")
                    },
                    ok,
                );
            }
            kit::section(ui, p, &tr("À propos", "About"));
            kit::card(ui, p, |ui| {
                kit::row(ui, p, "Version", env!("CARGO_PKG_VERSION"));
                kit::row(ui, p, &tr("Identifiant de pair", "Peer id"), &peer_id);
                ui.horizontal_wrapped(|ui| {
                    ui.hyperlink_to(tr("Le site", "Website"), "../");
                    ui.hyperlink_to(tr("Code source", "Source code"), "https://github.com/maxlestage/mxTorrent");
                    if ui.link(tr("Journal", "Log")).clicked() {
                        self.log_open = true;
                        done = true;
                    }
                });
            });
            ui.add_space(8.0);
            if kit::button(ui, p, Kind::Primary, None, &tr("Enregistrer", "Save"), true).clicked() {
                if let Some(d) = self.settings_draft.take() {
                    let mut e = self.engine.borrow_mut();
                    e.settings = d;
                    e.dirty = true;
                }
                done = true;
            }
            done
        });
        done || close
    }

    // ------------------------------------------------------------ couches du dessus

    fn toasts(&mut self, ctx: &egui::Context) {
        let p = self.p();
        let Some(t) = self.engine.borrow().toasts.back().cloned() else { return };
        let age = web::now_ms().saturating_sub(t.at) as f32 / 1000.0;
        if age > 3.0 {
            return;
        }
        let alpha = ((3.0 - age) * 3.0).clamp(0.0, 1.0) * (age * 8.0).min(1.0);
        let (anchor, y) = if self.mobile { (Align2::CENTER_TOP, 12.0) } else { (Align2::CENTER_BOTTOM, -48.0) };
        egui::Area::new(Id::unique("toast")).anchor(anchor, [0.0, y]).order(egui::Order::Tooltip).interactable(false).show(ctx, |ui| {
            egui::Frame::new()
                .fill(p.surface2.gamma_multiply(alpha))
                .stroke(Stroke::new(1.0, p.border.gamma_multiply(alpha)))
                .corner_radius(CornerRadius::same(255))
                .inner_margin(Margin { left: 12, right: 16, top: 8, bottom: 8 })
                .show(ui, |ui| {
                    ui.spacing_mut().interact_size.y = 18.0;
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        icons::paint(
                            ui.painter(),
                            r,
                            if t.good { Icon::Check } else { Icon::Close },
                            (if t.good { GREEN } else { RED }).gamma_multiply(alpha),
                        );
                        ui.add(
                            egui::Label::new(RichText::new(&t.text).font(font_semibold(13.5)).color(p.text.gamma_multiply(alpha)))
                                .truncate(),
                        );
                    });
                });
        });
        // Apparition et disparition fluides, seulement pendant ces trois secondes.
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }

    fn fab(&mut self, ctx: &egui::Context) {
        if !self.mobile || self.selected.is_some() || self.engine.borrow().entries.is_empty() {
            return;
        }
        egui::Area::new(Id::unique("fab")).anchor(Align2::RIGHT_BOTTOM, [-18.0, -22.0]).order(egui::Order::Foreground).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(60.0), Sense::click());
            let pa = ui.painter();
            for (d, a) in [(10.0, 20), (6.0, 35), (3.0, 50)] {
                pa.circle_filled(r.center() + vec2(0.0, d * 0.6), 30.0 + d * 0.3, Color32::from_black_alpha(a));
            }
            let fill = if resp.is_pointer_button_down_on() { theme::VIOLET_DARK } else { VIOLET };
            pa.circle_filled(r.center(), 30.0, fill);
            icons::paint(pa, Rect::from_center_size(r.center(), Vec2::splat(26.0)), Icon::Plus, Color32::WHITE);
            if resp.clicked() {
                self.sheet = Some(Sheet::Add);
                self.link_error = None;
            }
        });
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        if !ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            return;
        }
        let rect = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("drop")));
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(190));
        painter.rect_stroke(rect.shrink(16.0), CornerRadius::same(24), Stroke::new(2.0, VIOLET), egui::StrokeKind::Inside);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            tr(
                "Déposez des .torrent pour les ouvrir, ou des fichiers pour les partager",
                "Drop .torrent files to open them, or any files to share them",
            ),
            font_semibold(18.0),
            Color32::WHITE,
        );
    }

    fn log_window(&mut self, ctx: &egui::Context) {
        if !self.log_open {
            return;
        }
        let mut open = true;
        egui::Window::new(tr("Journal", "Log")).open(&mut open).default_size([420.0, 300.0]).show(ctx, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                for l in &self.engine.borrow().log {
                    ui.label(RichText::new(l).font(font(12.0)));
                }
            });
        });
        self.log_open = open;
    }
}

impl eframe::App for MxApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Ok(mut e) = self.engine.try_borrow_mut() {
            e.pump();
            // Pendant un transfert, une image par seconde suffit pour les chiffres.
            if e.entries.iter().any(|x| !x.torrent.is_paused() && (x.torrent.state() != State::Seeding || x.torrent.up_rate() > 0.0)) {
                ctx.request_repaint_after(std::time::Duration::from_secs(1));
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.mobile = ctx.content_rect().width() < 760.0;
        if self.style_key != Some((self.dark, self.mobile)) {
            theme::apply(&ctx, self.dark, self.mobile);
            self.style_key = Some((self.dark, self.mobile));
        }
        self.handle_input(&ctx);
        self.top_bar(ui);
        if !self.mobile {
            self.status_bar(ui);
        }
        let p = self.p();
        let pad = if self.mobile { 16 } else { 20 };
        let central = egui::Frame::new().fill(p.bg).inner_margin(Margin { left: pad, right: pad, top: 6, bottom: 0 });
        if self.mobile {
            egui::CentralPanel::default().frame(central).show(ui, |ui| match self.selected {
                Some(h) => self.details(ui, h),
                None => self.list(ui),
            });
        } else {
            let empty = self.engine.borrow().entries.is_empty();
            if empty {
                egui::CentralPanel::default().frame(central).show(ui, |ui| self.empty_state(ui));
            } else {
                egui::Panel::left("list")
                    .resizable(true)
                    .default_size(380.0)
                    .size_range(320.0..=560.0)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin { left: 20, right: 10, top: 6, bottom: 0 }))
                    .show(ui, |ui| self.list(ui));
                egui::CentralPanel::default().frame(central).show(ui, |ui| match self.selected {
                    Some(h) => self.details(ui, h),
                    None => self.placeholder(ui),
                });
            }
        }
        self.fab(&ctx);
        self.sheets(&ctx);
        self.log_window(&ctx);
        self.toasts(&ctx);
        self.drop_overlay(&ctx);
        // Titre de l'onglet : seulement quand il change.
        let title = {
            let e = self.engine.borrow();
            let (d, _) = e.total_rates();
            if e.entries.iter().any(|x| x.torrent.state() == State::Downloading) && d > 1.0 {
                format!("↓ {} · mxTorrent", rate_str(d))
            } else {
                "mxTorrent".to_string()
            }
        };
        if title != self.title {
            web::notify_title(&title);
            self.title = title;
        }
        self.save_if_changed();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.p().bg.to_normalized_gamma_f32()
    }
}

/// Une feuille modale : montée du bas sur téléphone, centrée sur ordinateur.
fn show_sheet<R>(ctx: &egui::Context, p: Palette, mobile: bool, id: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> (R, bool) {
    let screen = ctx.content_rect();
    let id = Id::unique(("sheet", id));
    let mut modal = egui::Modal::new(id).backdrop_color(Color32::from_black_alpha(if p.dark { 170 } else { 90 }));
    let max_h = screen.height() * if mobile { 0.82 } else { 0.86 };
    if mobile {
        modal = modal.area(egui::Modal::default_area(id).anchor(Align2::CENTER_BOTTOM, [0.0, 0.0])).frame(
            egui::Frame::new().fill(p.surface).corner_radius(CornerRadius { nw: 26, ne: 26, sw: 0, se: 0 }).inner_margin(Margin {
                left: 18,
                right: 18,
                top: 8,
                bottom: 28,
            }),
        );
    } else {
        modal = modal.frame(
            egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).corner_radius(CornerRadius::same(22)).inner_margin(24),
        );
    }
    let r = modal.show(ctx, |ui| {
        let w = if mobile { screen.width() - 36.0 } else { 440.0 };
        ui.set_width(w);
        if mobile {
            let (r, _) = ui.allocate_exact_size(vec2(w, 16.0), Sense::hover());
            ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(40.0, 5.0)), CornerRadius::same(3), p.border);
        }
        // Une zone ancrée en bas ne grandit pas d'elle-même : on lui redonne la hauteur
        // de son contenu mesurée à l'image précédente.
        let mem = id.with("content_h");
        let wanted = ui.ctx().data(|d| d.get_temp::<f32>(mem)).unwrap_or(0.0).min(max_h);
        let out = egui::ScrollArea::vertical().max_height(max_h).min_scrolled_height(wanted).auto_shrink([false, true]).show(ui, |ui| {
            ui.set_width(w);
            ui.spacing_mut().item_spacing.y = 10.0;
            add(ui)
        });
        let h = out.content_size.y;
        if (h - wanted).abs() > 1.0 && wanted < max_h {
            ui.ctx().data_mut(|d| d.insert_temp(mem, h));
            ui.ctx().request_repaint();
        }
        out.inner
    });
    let close = r.should_close();
    (r.inner, close)
}

/// Lien vers l'application qui ouvre directement ce torrent.
fn app_link(magnet: &str) -> String {
    let base = web::window().location().href().unwrap_or_default();
    let base = base.split(['?', '#']).next().unwrap_or("").to_string();
    format!("{base}?magnet={}", mxtorrent_core::util::percent_encode(magnet))
}

fn pick_share() {
    web::pick_files("", true, |files| net::push(NetEvent::Share(files)));
}

fn pick_torrent() {
    web::pick_files(".torrent,application/x-bittorrent", true, |files| net::push(NetEvent::TorrentFiles(files)));
}

fn state_label(s: State) -> String {
    match s {
        State::FetchingMetadata => tr("Métadonnées", "Metadata"),
        State::Downloading => tr("Téléchargement", "Downloading"),
        State::Seeding => tr("Partage", "Seeding"),
        State::Paused => tr("En pause", "Paused"),
    }
}

fn torrent_card(ui: &mut egui::Ui, p: Palette, entry: &Entry, selected: bool) -> egui::Response {
    let t = &entry.torrent;
    let state = t.state();
    let color = kit::state_color(state);
    let resp = egui::Frame::new()
        .fill(if selected { p.surface2 } else { p.surface })
        .stroke(Stroke::new(1.0, if selected { VIOLET } else { p.border }))
        .corner_radius(CornerRadius::same(18))
        .inner_margin(Margin { left: 16, right: 16, top: 14, bottom: 14 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width().max(1.0));
            ui.spacing_mut().interact_size.y = 22.0;
            ui.add(egui::Label::new(RichText::new(t.name()).font(font_semibold(15.5))).truncate().selectable(false));
            ui.add_space(2.0);
            let frac = if state == State::FetchingMetadata { t.metadata_progress() } else { t.progress() };
            kit::bar(ui, p, frac, color, 6.0);
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                kit::chip(ui, &state_label(state), color);
                let detail = match t.total_length() {
                    Some(total) => format!("{:.0} % · {}", (t.progress() * 100.0).floor(), bytes(total)),
                    None => format!("{} {}", t.peer_count(), tr("pairs", "peers")),
                };
                ui.label(RichText::new(detail).font(font(13.0)).color(p.muted));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if state == State::Downloading && t.down_rate() > 1.0 {
                        ui.label(RichText::new(format!("↓ {}", rate_str(t.down_rate()))).font(font_semibold(13.0)).color(CYAN));
                    } else if state == State::Seeding && t.up_rate() > 1.0 {
                        ui.label(RichText::new(format!("↑ {}", rate_str(t.up_rate()))).font(font_semibold(13.0)).color(PINK));
                    } else if state != State::Paused && t.total_length().is_some() {
                        ui.label(RichText::new(format!("{} {}", t.peer_count(), tr("pairs", "peers"))).font(font(13.0)).color(p.faint));
                    }
                });
            });
        })
        .response;
    resp.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn overview_tab(ui: &mut egui::Ui, p: Palette, entry: &Entry) {
    let t = &entry.torrent;
    let (seeds, leech) = entry.swarm_counts();
    kit::card(ui, p, |ui| {
        kit::row(ui, p, &tr("Réception", "Download"), &rate_str(t.down_rate()));
        kit::row(ui, p, &tr("Envoi", "Upload"), &rate_str(t.up_rate()));
        if t.state() == State::Downloading
            && t.down_rate() > 1.0
            && let Some(left) = t.left()
        {
            kit::row(ui, p, &tr("Temps restant", "Time left"), &duration(left as f64 / t.down_rate()));
        }
        kit::row(
            ui,
            p,
            &tr("Pairs connectés", "Connected peers"),
            &format!("{} ({} {})", t.peer_count(), t.seed_count(), tr("complets", "seeds")),
        );
        let sw = |x: Option<u32>| x.map(|v| v.to_string()).unwrap_or_else(|| "–".into());
        kit::row(ui, p, &tr("Essaim (sources / clients)", "Swarm (seeds / leechers)"), &format!("{} / {}", sw(seeds), sw(leech)));
        kit::row(ui, p, &tr("Reçu", "Downloaded"), &bytes(t.downloaded));
        kit::row(ui, p, &tr("Envoyé", "Uploaded"), &bytes(t.uploaded));
        if t.downloaded > 0 {
            kit::row(ui, p, "Ratio", &format!("{:.2}", t.uploaded as f64 / t.downloaded as f64));
        }
    });
    if let Some(m) = t.metainfo() {
        ui.add_space(10.0);
        kit::section(ui, p, &format!("{} · {} × {}", tr("Pièces", "Pieces"), m.info.piece_count(), bytes(m.info.piece_length as u64)));
        kit::card(ui, p, |ui| piece_map(ui, p, t));
        ui.add_space(10.0);
        kit::card(ui, p, |ui| {
            kit::row(ui, p, &tr("Taille", "Size"), &bytes(m.info.total_length));
            kit::row(ui, p, &tr("Fichiers", "Files"), &m.info.files.len().to_string());
            if let Some(c) = &m.created_by {
                kit::row(ui, p, &tr("Créé avec", "Created by"), c);
            }
            if t.hash_failures > 0 {
                kit::row(ui, p, &tr("Pièces rejetées", "Rejected pieces"), &t.hash_failures.to_string());
            }
            ui.add_space(4.0);
            ui.label(RichText::new(tr("Empreinte", "Info hash")).color(p.muted));
            ui.add(egui::Label::new(RichText::new(t.info_hash().to_hex()).font(font(12.5)).color(p.faint)).wrap());
            if let Some(c) = &m.comment {
                ui.add_space(4.0);
                ui.label(RichText::new(c).font(font(13.0)).color(p.muted));
            }
        });
    }
}

fn files_tab(ui: &mut egui::Ui, p: Palette, entry: &Entry, toggle: &mut Option<(usize, bool)>) {
    let t = &entry.torrent;
    let Some(m) = t.metainfo() else {
        kit::card(ui, p, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                kit::muted(
                    ui,
                    p,
                    &tr("Les fichiers apparaîtront dès que les métadonnées seront reçues.", "Files will appear once metadata is received."),
                );
            });
        });
        return;
    };
    let wanted = t.file_wanted().to_vec();
    for (fi, f) in m.info.files.iter().enumerate() {
        kit::card(ui, p, |ui| {
            ui.horizontal(|ui| {
                let mut w = wanted[fi];
                if m.info.files.len() > 1 && ui.checkbox(&mut w, "").changed() {
                    *toggle = Some((fi, w));
                }
                ui.add(egui::Label::new(RichText::new(f.path[1.min(f.path.len() - 1)..].join("/")).font(font_semibold(14.5))).truncate());
            });
            let prog = t.file_progress(fi);
            kit::bar(ui, p, prog, if prog >= 1.0 { GREEN } else { VIOLET }, 5.0);
            ui.horizontal(|ui| {
                kit::muted(ui, p, &format!("{} · {:.0} %", bytes(f.length), (prog * 100.0).floor()));
            });
            if t.file_complete(fi) {
                ui.horizontal_wrapped(|ui| {
                    if kit::button(ui, p, Kind::Secondary, Some(Icon::Download), &tr("Enregistrer", "Save"), false).clicked()
                        && let Some(chunks) = t.file_chunks(fi)
                    {
                        let _ = web::save_file(f.file_name(), &chunks);
                    }
                    if web::can_preview(f.file_name())
                        && kit::button(ui, p, Kind::Ghost, Some(Icon::Eye), &tr("Ouvrir", "Open"), false).clicked()
                        && let Some(chunks) = t.file_chunks(fi)
                    {
                        let _ = web::open_file(f.file_name(), &chunks);
                    }
                });
            }
        });
        ui.add_space(8.0);
    }
}

fn peers_tab(ui: &mut egui::Ui, p: Palette, entry: &Entry, e: &Engine) {
    let peers = entry.torrent.peers();
    if peers.is_empty() {
        kit::card(ui, p, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                kit::muted(
                    ui,
                    p,
                    &tr(
                        "Aucun pair pour l'instant. Les trackers mettent les navigateurs en relation ; cela peut prendre quelques dizaines de secondes.",
                        "No peers yet. Trackers introduce browsers to each other; this can take a few dozen seconds.",
                    ),
                );
            });
        });
        return;
    }
    kit::card(ui, p, |ui| {
        for (k, peer) in peers.iter().enumerate() {
            if k > 0 {
                ui.separator();
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(&peer.client).font(font_semibold(14.5)));
                // D'où vient le pair : navigateur (WebRTC) ou réseau classique via la passerelle.
                if e.is_gateway_peer(peer.key) {
                    let tag = if e.gateway_peer_encrypted(peer.key) { "TCP · MSE · 🥦" } else { "TCP · 🥦" };
                    kit::chip(ui, tag, GREEN);
                } else {
                    kit::chip(ui, "WebRTC", CYAN);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    kit::muted(ui, p, &format!("{:.0} %", peer.progress * 100.0));
                });
            });
            kit::bar(ui, p, peer.progress, CYAN, 4.0);
            kit::muted(ui, p, &format!("↓ {}   ↑ {}", rate_str(peer.down_rate), rate_str(peer.up_rate)));
        }
    });
}

fn trackers_tab(ui: &mut egui::Ui, p: Palette, entry: &Entry, e: &Engine) {
    let mut list: Vec<_> = entry.trackers.iter().collect();
    list.sort_by(|a, b| a.0.cmp(b.0));
    kit::card(ui, p, |ui| {
        for (k, (url, st)) in list.iter().enumerate() {
            if k > 0 {
                ui.separator();
            }
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, if e.tracker_connected(url) { GREEN } else { AMBER });
                ui.add(egui::Label::new(RichText::new(url.trim_start_matches("wss://")).font(font_semibold(14.0))).truncate());
            });
            let s = st.seeders.map(|x| x.to_string()).unwrap_or("–".into());
            let l = st.leechers.map(|x| x.to_string()).unwrap_or("–".into());
            kit::muted(ui, p, &format!("{} {s} · {} {l}", tr("sources", "seeds"), tr("clients", "leechers")));
            if let Some(m) = &st.message {
                ui.label(RichText::new(m).font(font(12.5)).color(AMBER));
            }
        }
    });
    let ws = entry.web_seed_status();
    if !ws.is_empty() {
        ui.add_space(10.0);
        kit::section(ui, p, &tr("Sources web", "Web seeds"));
        kit::card(ui, p, |ui| {
            for (u, pieces, _) in ws {
                kit::row(ui, p, &u, &format!("{pieces} {}", tr("pièces", "pieces")));
            }
        });
    }
}

/// Une case par pièce : vérifiée, en cours, absente, ou non voulue.
fn piece_map(ui: &mut egui::Ui, p: Palette, t: &mxtorrent_core::Torrent) {
    let states = t.piece_states();
    let wanted = t.wanted_pieces();
    let n = states.len().max(1);
    let w = ui.available_width().max(40.0);
    let per = n.div_ceil(1200).max(1);
    let cells = n.div_ceil(per);
    let cell = ((w * 110.0 / cells as f32).sqrt()).clamp(5.0, 14.0);
    let cols = ((w / cell).floor() as usize).max(1);
    let rows = cells.div_ceil(cols);
    let (rect, _) = ui.allocate_exact_size(vec2(w, rows as f32 * cell), Sense::hover());
    let pa = ui.painter_at(rect);
    for c in 0..cells {
        let range = c * per..((c + 1) * per).min(n);
        let len = range.len().max(1) as f32;
        let have = range.clone().filter(|&i| states[i] == 255).count() as f32 / len;
        let part: f32 =
            range.clone().map(|i| if states[i] > 0 && states[i] < 255 { states[i] as f32 / 255.0 } else { 0.0 }).sum::<f32>() / len;
        let any_wanted = range.clone().any(|i| wanted.get(i).copied().unwrap_or(true));
        let color = if have >= 1.0 {
            VIOLET
        } else if part > 0.0 {
            mix(p.surface2, AMBER, 0.4 + part * 0.6)
        } else if have > 0.0 {
            mix(p.surface2, VIOLET, have)
        } else if !any_wanted {
            p.surface
        } else {
            p.surface2
        };
        let x = rect.left() + (c % cols) as f32 * cell;
        let y = rect.top() + (c / cols) as f32 * cell;
        pa.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::splat(cell - 2.0)), CornerRadius::same(2), color);
    }
}
