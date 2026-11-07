//! L'interface egui : liste des torrents, détails, partage, réglages.
//! Une seule base de code pour l'ordinateur et le téléphone : sous 760 points
//! de large, la mise en page passe en une colonne avec un bouton flottant.

use crate::engine::{DEFAULT_TRACKERS, Engine, Saved, Settings};
use crate::i18n::{bytes, duration, is_english, rate_str, set_english, tr};
use crate::net::{self, NetEvent};
use crate::theme::{self, AMBER, CYAN, GREEN, PINK, Palette, RED, VIOLET, mix};
use crate::web;
use egui::{Align, Align2, Color32, CornerRadius, Id, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use mxtorrent_core::{InfoHash, State};
use std::cell::RefCell;
use std::rc::Rc;

const STORAGE_KEY: &str = "mxtorrent";

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Tab {
    Overview,
    Files,
    Peers,
    Trackers,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Active,
    Done,
    Paused,
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

pub struct MxApp {
    engine: Rc<RefCell<Engine>>,
    selected: Option<InfoHash>,
    tab: Tab,
    filter: Filter,
    search: String,
    add_open: bool,
    add_text: String,
    add_error: Option<String>,
    settings_open: bool,
    settings_draft: Option<Settings>,
    new_tracker: String,
    share: Option<InfoHash>,
    confirm_remove: Option<InfoHash>,
    about_open: bool,
    log_open: bool,
    dark: bool,
    welcomed: bool,
    mobile: bool,
    style_key: Option<(bool, bool)>,
}

impl MxApp {
    pub fn new(cc: &eframe::CreationContext<'_>, engine: Rc<RefCell<Engine>>) -> MxApp {
        let prefs: Prefs = cc.storage.and_then(|s| eframe::get_value(s, STORAGE_KEY)).unwrap_or_default();
        set_english(prefs.english);
        {
            let mut e = engine.borrow_mut();
            e.settings = prefs.saved.settings.clone();
            if e.settings.trackers.is_empty() {
                e.settings.trackers = DEFAULT_TRACKERS.iter().map(|s| s.to_string()).collect();
            }
            e.restore(prefs.saved);
        }
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
            tab: Tab::Overview,
            filter: Filter::All,
            search: String::new(),
            add_open: false,
            add_text: String::new(),
            add_error: None,
            settings_open: false,
            settings_draft: None,
            new_tracker: String::new(),
            share: None,
            confirm_remove: None,
            about_open: false,
            log_open: false,
            dark: prefs.dark,
            welcomed: prefs.welcomed,
            mobile: false,
            style_key: None,
        }
    }

    fn palette(&self) -> Palette {
        theme::palette(self.dark)
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
    }

    // ------------------------------------------------------------ barre du haut

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette();
        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(14, 10)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.mobile
                        && self.selected.is_some()
                        && ui
                            .add(egui::Button::new(RichText::new("⬅").size(18.0)).frame(false))
                            .on_hover_text(tr("Retour", "Back"))
                            .clicked()
                    {
                        self.selected = None;
                    }
                    let t = ui.input(|i| i.time) as f32;
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    logo(ui.painter(), r, t);
                    ui.label(RichText::new("mx").size(20.0).strong().color(VIOLET));
                    ui.add_space(-8.0);
                    ui.label(RichText::new("Torrent").size(20.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new(RichText::new("⚙").size(17.0))).on_hover_text(tr("Réglages", "Settings")).clicked() {
                            self.settings_open = true;
                            self.settings_draft = Some(self.engine.borrow().settings.clone());
                        }
                        let label = if self.mobile {
                            RichText::new("＋").size(17.0)
                        } else {
                            RichText::new(tr("＋ Ajouter", "＋ Add")).strong()
                        };
                        if ui.add(egui::Button::new(label.color(Color32::WHITE)).fill(VIOLET)).clicked() {
                            self.add_open = true;
                            self.add_error = None;
                        }
                        if !self.mobile {
                            let (d, u) = self.engine.borrow().total_rates();
                            pill(ui, &format!("↓ {}", rate_str(d)), CYAN, p);
                            pill(ui, &format!("↑ {}", rate_str(u)), PINK, p);
                        }
                    });
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette();
        egui::Panel::bottom("status").frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::symmetric(14, 6))).show(
            ui,
            |ui| {
                let e = self.engine.borrow();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    let n = e.connected_trackers();
                    let dot = if n > 0 { GREEN } else { AMBER };
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, dot);
                    small(ui, &format!("{} {n}", tr("trackers connectés :", "trackers connected:")), p);
                    small(ui, &format!("{} {}", tr("connexions en cours :", "pending connections:"), e.connecting_peers()), p);
                    small(
                        ui,
                        &format!("{} {}", tr("pairs :", "peers:"), e.entries.iter().map(|x| x.torrent.peer_count()).sum::<usize>()),
                        p,
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.link(RichText::new(tr("À propos", "About")).small()).clicked() {
                            self.about_open = true;
                        }
                        if ui.link(RichText::new(tr("Journal", "Log")).small()).clicked() {
                            self.log_open = !self.log_open;
                        }
                        small(ui, concat!("v", env!("CARGO_PKG_VERSION")), p);
                    });
                });
            },
        );
    }

    // ------------------------------------------------------------ liste

    fn list(&mut self, ui: &mut egui::Ui) {
        let p = self.palette();
        ui.horizontal(|ui| {
            for (f, fr, en) in [
                (Filter::All, "Tous", "All"),
                (Filter::Active, "Actifs", "Active"),
                (Filter::Done, "Terminés", "Done"),
                (Filter::Paused, "En pause", "Paused"),
            ] {
                let on = self.filter == f;
                let txt = RichText::new(tr(fr, en)).color(if on { Color32::WHITE } else { p.muted });
                if ui.add(egui::Button::new(txt).fill(if on { VIOLET } else { p.surface }).corner_radius(CornerRadius::same(20))).clicked()
                {
                    self.filter = f;
                }
            }
        });
        if self.engine.borrow().entries.len() > 4 {
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(tr("🔍 Rechercher", "🔍 Search")).desired_width(f32::INFINITY));
        }
        ui.add_space(4.0);
        let engine = self.engine.clone();
        let e = engine.borrow();
        let search = self.search.to_lowercase();
        let items: Vec<usize> = (0..e.entries.len())
            .filter(|&i| {
                let t = &e.entries[i].torrent;
                let ok = match self.filter {
                    Filter::All => true,
                    Filter::Active => matches!(t.state(), State::Downloading | State::FetchingMetadata),
                    Filter::Done => t.state() == State::Seeding,
                    Filter::Paused => t.state() == State::Paused,
                };
                ok && (search.is_empty() || t.name().to_lowercase().contains(&search))
            })
            .collect();
        if e.entries.is_empty() {
            drop(e);
            self.empty_state(ui);
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for i in items.into_iter().rev() {
                let entry = &e.entries[i];
                let h = entry.info_hash();
                let selected = self.selected == Some(h);
                if torrent_card(ui, entry, selected, p).clicked() {
                    self.selected = Some(h);
                }
            }
            ui.add_space(80.0);
        });
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let p = self.palette();
        ui.vertical_centered(|ui| {
            ui.add_space(30.0);
            let t = ui.input(|i| i.time) as f32;
            let (r, _) = ui.allocate_exact_size(Vec2::splat(140.0), Sense::hover());
            logo(ui.painter(), r, t);
            ui.add_space(12.0);
            ui.label(RichText::new(tr("Aucun torrent pour l'instant", "No torrents yet")).heading().strong());
            ui.label(
                RichText::new(tr(
                    "Collez un lien magnet, ouvrez un .torrent, ou déposez des fichiers pour les partager.",
                    "Paste a magnet link, open a .torrent, or drop files to share them.",
                ))
                .color(p.muted),
            );
            ui.add_space(14.0);
            if ui
                .add(
                    egui::Button::new(RichText::new(tr("＋ Ajouter un torrent", "＋ Add a torrent")).strong().color(Color32::WHITE))
                        .fill(VIOLET),
                )
                .clicked()
            {
                self.add_open = true;
            }
            if ui.button(tr("📤 Partager des fichiers", "📤 Share files")).clicked() {
                pick_share();
            }
            ui.add_space(20.0);
            ui.label(
                RichText::new(tr(
                    "Tout se passe dans votre navigateur : les pairs se connectent directement en WebRTC.",
                    "Everything happens in your browser: peers connect directly over WebRTC.",
                ))
                .small()
                .color(p.muted),
            );
        });
    }

    // ------------------------------------------------------------ détails

    fn details(&mut self, ui: &mut egui::Ui, h: InfoHash) {
        let p = self.palette();
        let engine = self.engine.clone();
        let mut e = engine.borrow_mut();
        let Some(i) = e.find(&h) else {
            self.selected = None;
            return;
        };
        let entry = &e.entries[i];
        let t = &entry.torrent;
        let state = t.state();
        let paused = t.is_paused();
        let has_meta = t.metainfo().is_some();
        let magnet = entry.magnet();

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(t.name()).heading().strong());
        });
        ui.horizontal_wrapped(|ui| {
            state_chip(ui, state);
            if entry.created_here {
                chip(ui, &tr("créé ici", "created here"), CYAN);
            }
            if t.metainfo().is_some_and(|m| m.info.private) {
                chip(ui, &tr("privé", "private"), AMBER);
            }
        });
        ui.add_space(4.0);
        progress_bar(ui, t.progress(), state, 12.0, ui.input(|i| i.time) as f32);
        ui.add_space(6.0);

        let mut action_pause = None;
        let mut action_remove = false;
        let mut action_share = false;
        let mut action_torrent = false;
        ui.horizontal_wrapped(|ui| {
            if paused {
                if ui.button(tr("▶ Reprendre", "▶ Resume")).clicked() {
                    action_pause = Some(false);
                }
            } else if ui.button(tr("⏸ Pause", "⏸ Pause")).clicked() {
                action_pause = Some(true);
            }
            if ui.button(tr("🔗 Copier le lien magnet", "🔗 Copy magnet link")).clicked() {
                ui.ctx().copy_text(magnet.clone());
            }
            if ui.button(tr("📱 Partager", "📱 Share")).clicked() {
                action_share = true;
            }
            if has_meta && ui.button(tr("💾 .torrent", "💾 .torrent")).clicked() {
                action_torrent = true;
            }
            if ui.add(egui::Button::new(RichText::new(tr("🗑 Retirer", "🗑 Remove")).color(RED))).clicked() {
                action_remove = true;
            }
        });
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            for (tab, fr, en) in [
                (Tab::Overview, "Aperçu", "Overview"),
                (Tab::Files, "Fichiers", "Files"),
                (Tab::Peers, "Pairs", "Peers"),
                (Tab::Trackers, "Trackers", "Trackers"),
            ] {
                let on = self.tab == tab;
                let r = ui.add(egui::Button::new(RichText::new(tr(fr, en)).color(if on { p.text } else { p.muted }).strong()).frame(false));
                if on {
                    let rr = r.rect;
                    ui.painter().line_segment(
                        [Pos2::new(rr.left(), rr.bottom() + 2.0), Pos2::new(rr.right(), rr.bottom() + 2.0)],
                        Stroke::new(2.5, VIOLET),
                    );
                }
                if r.clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.separator();

        let mut file_toggle: Option<(usize, bool)> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
            Tab::Overview => overview(ui, entry, p),
            Tab::Files => {
                if let Some(m) = t.metainfo() {
                    let wanted = t.file_wanted().to_vec();
                    for (fi, f) in m.info.files.iter().enumerate() {
                        egui::Frame::new().fill(p.surface).corner_radius(CornerRadius::same(12)).inner_margin(10).show(ui, |ui| {
                            ui.set_width(ui.available_width().max(1.0));
                            ui.horizontal(|ui| {
                                let mut w = wanted[fi];
                                if ui.checkbox(&mut w, "").on_hover_text(tr("Télécharger ce fichier", "Download this file")).changed() {
                                    file_toggle = Some((fi, w));
                                }
                                ui.add(egui::Label::new(RichText::new(f.path[1.min(f.path.len() - 1)..].join("/")).strong()).truncate());
                            });
                            let prog = t.file_progress(fi);
                            progress_bar(ui, prog, if prog >= 1.0 { State::Seeding } else { State::Downloading }, 6.0, 0.0);
                            ui.horizontal_wrapped(|ui| {
                                small(ui, &format!("{} · {:.0} %", bytes(f.length), prog * 100.0), p);
                                if t.file_complete(fi) {
                                    if ui.small_button(tr("💾 Enregistrer", "💾 Save")).clicked()
                                        && let Some(chunks) = t.file_chunks(fi) {
                                            let _ = web::save_file(f.file_name(), &chunks);
                                        }
                                    if web::can_preview(f.file_name()) && ui.small_button(tr("👁 Ouvrir", "👁 Open")).clicked()
                                        && let Some(chunks) = t.file_chunks(fi) {
                                            let _ = web::open_file(f.file_name(), &chunks);
                                        }
                                }
                            });
                        });
                        ui.add_space(4.0);
                    }
                } else {
                    waiting_metadata(ui, t.metadata_progress(), p);
                }
            }
            Tab::Peers => {
                let peers = t.peers();
                if peers.is_empty() {
                    ui.label(RichText::new(tr(
                        "Aucun pair pour l'instant. Les trackers mettent les navigateurs en relation ; cela peut prendre quelques dizaines de secondes.",
                        "No peers yet. Trackers introduce browsers to each other; this can take a few dozen seconds.",
                    )).color(p.muted));
                }
                for peer in peers {
                    egui::Frame::new().fill(p.surface).corner_radius(CornerRadius::same(12)).inner_margin(10).show(ui, |ui| {
                        ui.set_width(ui.available_width().max(1.0));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&peer.client).strong());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                small(ui, &format!("{:.0} %", peer.progress * 100.0), p);
                            });
                        });
                        progress_bar(ui, peer.progress, State::Downloading, 4.0, 0.0);
                        ui.horizontal_wrapped(|ui| {
                            small(ui, &format!("↓ {}  ↑ {}", rate_str(peer.down_rate), rate_str(peer.up_rate)), p);
                            small(ui, &format!("{} {} · {} {}", tr("reçu", "got"), bytes(peer.downloaded), tr("envoyé", "sent"), bytes(peer.uploaded)), p);
                            if peer.choking_us {
                                chip(ui, &tr("nous bloque", "choking us"), AMBER);
                            }
                            if peer.interested_in_us {
                                chip(ui, &tr("intéressé", "interested"), CYAN);
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            }
            Tab::Trackers => {
                let mut list: Vec<_> = entry.trackers.iter().collect();
                list.sort_by(|a, b| a.0.cmp(b.0));
                for (url, st) in list {
                    egui::Frame::new().fill(p.surface).corner_radius(CornerRadius::same(12)).inner_margin(10).show(ui, |ui| {
                        ui.set_width(ui.available_width().max(1.0));
                        ui.horizontal(|ui| {
                            let c = if e.tracker_connected(url) { GREEN } else { AMBER };
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.5, c);
                            ui.add(egui::Label::new(RichText::new(url).monospace()).truncate());
                        });
                        let s = st.seeders.map(|x| x.to_string()).unwrap_or("–".into());
                        let l = st.leechers.map(|x| x.to_string()).unwrap_or("–".into());
                        small(ui, &format!("{} {s} · {} {l}", tr("sources :", "seeders:"), tr("clients :", "leechers:")), p);
                        if let Some(m) = &st.message {
                            ui.label(RichText::new(m).small().color(AMBER));
                        }
                    });
                    ui.add_space(4.0);
                }
                let other: Vec<&String> = t.trackers.iter().filter(|u| !entry.trackers.contains_key(*u)).collect();
                if !other.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr(
                        "Ces trackers UDP/HTTP ne sont pas joignables depuis un navigateur :",
                        "These UDP/HTTP trackers can't be reached from a browser:",
                    )).small().color(p.muted));
                    for u in other {
                        ui.label(RichText::new(u).small().monospace().color(p.muted));
                    }
                }
                let ws = entry.web_seed_status();
                if !ws.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new(tr("Sources web (HTTP)", "Web seeds (HTTP)")).strong());
                    for (u, pieces, fails) in ws {
                        small(ui, &format!("{u} — {pieces} {} {}", tr("pièces", "pieces"), if fails > 0 { format!("({fails} {})", tr("échecs", "failures")) } else { String::new() }), p);
                    }
                }
            }
        });

        if let Some((fi, w)) = file_toggle {
            e.entries[i].torrent.set_file_wanted(fi, w);
            e.dirty = true;
        }
        if action_torrent && let Some(m) = e.entries[i].torrent.metainfo() {
            let _ = web::save_file(&format!("{}.torrent", m.info.name), &[&m.to_torrent()]);
        }
        if let Some(pz) = action_pause {
            e.set_paused(&h, pz);
        }
        drop(e);
        if action_share {
            self.share = Some(h);
        }
        if action_remove {
            self.confirm_remove = Some(h);
        }
    }

    // ------------------------------------------------------------ fenêtres

    fn add_dialog(&mut self, ctx: &egui::Context) {
        if !self.add_open {
            return;
        }
        let p = self.palette();
        let mut close = false;
        let r = egui::Modal::new(Id::unique("add")).show(ctx, |ui| {
            ui.set_width(ui.available_width().min(460.0));
            ui.label(RichText::new(tr("Ajouter un torrent", "Add a torrent")).heading().strong());
            ui.add_space(6.0);
            let resp = ui.add(
                egui::TextEdit::multiline(&mut self.add_text)
                    .hint_text(tr(
                        "magnet:?xt=urn:btih:…, empreinte, ou URL d'un .torrent",
                        "magnet:?xt=urn:btih:…, info hash, or .torrent URL",
                    ))
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            resp.request_focus();
            if let Some(err) = &self.add_error {
                ui.label(RichText::new(err).color(RED));
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                if ui.add(egui::Button::new(RichText::new(tr("Ajouter", "Add")).strong().color(Color32::WHITE)).fill(VIOLET)).clicked()
                    || enter
                {
                    let text = self.add_text.trim().to_string();
                    if !text.is_empty() {
                        let mut ok = true;
                        for line in text.lines().filter(|l| !l.trim().is_empty()) {
                            if let Err(e) = self.engine.borrow_mut().add_link(line) {
                                self.add_error = Some(e);
                                ok = false;
                            }
                        }
                        if ok {
                            self.add_text.clear();
                            close = true;
                        }
                    }
                }
                if ui.button(tr("📂 Fichier .torrent", "📂 .torrent file")).clicked() {
                    web::pick_files(".torrent,application/x-bittorrent", true, |files| net::push(NetEvent::TorrentFiles(files)));
                    close = true;
                }
                if ui.button(tr("📤 Partager des fichiers", "📤 Share files")).clicked() {
                    pick_share();
                    close = true;
                }
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(tr(
                    "Astuce : collez un lien magnet n'importe où dans l'application, ou déposez des fichiers sur la fenêtre.",
                    "Tip: paste a magnet link anywhere in the app, or drop files onto the window.",
                ))
                .small()
                .color(p.muted),
            );
            ui.add_space(4.0);
            if ui.button(tr("Fermer", "Close")).clicked() {
                close = true;
            }
        });
        if r.should_close() || close {
            self.add_open = false;
        }
    }

    fn share_dialog(&mut self, ctx: &egui::Context) {
        let created = self.engine.borrow_mut().just_created.take();
        if let Some(h) = created {
            self.share = Some(h);
            self.selected = Some(h);
            self.welcomed = true;
            self.about_open = false;
            self.add_open = false;
        }
        let Some(h) = self.share else { return };
        let p = self.palette();
        let (name, magnet, torrent) = {
            let e = self.engine.borrow();
            let Some(i) = e.find(&h) else {
                self.share = None;
                return;
            };
            let en = &e.entries[i];
            (en.torrent.name(), en.magnet(), en.torrent.metainfo().map(|m| (m.info.name.clone(), m.to_torrent())))
        };
        let mut close = false;
        let r = egui::Modal::new(Id::unique("share")).show(ctx, |ui| {
            ui.set_width(ui.available_width().min(420.0));
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr("Partager", "Share")).heading().strong());
                ui.label(RichText::new(&name).color(p.muted));
                ui.add_space(8.0);
                let size = ui.available_width().min(260.0);
                crate::qr::draw(ui, &magnet, size);
                ui.add_space(6.0);
                ui.label(
                    RichText::new(tr(
                        "Scannez ce code avec un téléphone, ou envoyez le lien. Gardez cet onglet ouvert : c'est lui qui partage.",
                        "Scan this code with a phone, or send the link. Keep this tab open: it is the one sharing.",
                    ))
                    .small()
                    .color(p.muted),
                );
            });
            ui.add_space(6.0);
            let mut m = magnet.clone();
            ui.add(egui::TextEdit::singleline(&mut m).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(egui::Button::new(RichText::new(tr("🔗 Copier le lien", "🔗 Copy link")).color(Color32::WHITE)).fill(VIOLET))
                    .clicked()
                {
                    ui.ctx().copy_text(magnet.clone());
                    self.engine.borrow_mut().toast(tr("Lien copié", "Link copied"), true);
                }
                if ui.button(tr("🌐 Lien vers l'application", "🌐 App link")).clicked() {
                    let base = web::window().location().href().unwrap_or_default();
                    let base = base.split(['?', '#']).next().unwrap_or("").to_string();
                    ui.ctx().copy_text(format!("{base}?magnet={}", mxtorrent_core::util::percent_encode(&magnet)));
                    self.engine.borrow_mut().toast(tr("Lien copié", "Link copied"), true);
                }
                if let Some((n, bytes)) = &torrent
                    && ui.button("💾 .torrent").clicked()
                {
                    let _ = web::save_file(&format!("{n}.torrent"), &[bytes]);
                }
                if ui.button(tr("Fermer", "Close")).clicked() {
                    close = true;
                }
            });
        });
        if r.should_close() || close {
            self.share = None;
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(h) = self.confirm_remove else { return };
        let name = {
            let e = self.engine.borrow();
            match e.find(&h) {
                Some(i) => e.entries[i].torrent.name(),
                None => {
                    self.confirm_remove = None;
                    return;
                }
            }
        };
        let mut close = false;
        let r = egui::Modal::new(Id::unique("confirm")).show(ctx, |ui| {
            ui.set_width(ui.available_width().min(380.0));
            ui.label(RichText::new(tr("Retirer ce torrent ?", "Remove this torrent?")).heading().strong());
            ui.label(&name);
            ui.label(
                RichText::new(tr(
                    "Les données téléchargées vivent dans cet onglet : enregistrez d'abord les fichiers que vous voulez garder.",
                    "Downloaded data lives in this tab: save the files you want to keep first.",
                ))
                .small(),
            );
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(tr("Retirer", "Remove")).color(Color32::WHITE)).fill(RED)).clicked() {
                    self.engine.borrow_mut().remove(&h);
                    if self.selected == Some(h) {
                        self.selected = None;
                    }
                    close = true;
                }
                if ui.button(tr("Annuler", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if r.should_close() || close {
            self.confirm_remove = None;
        }
    }

    fn settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let p = self.palette();
        let mut close = false;
        let mut apply = false;
        let r = egui::Modal::new(Id::unique("settings")).show(ctx, |ui| {
            ui.set_width(ui.available_width().min(520.0));
            ui.label(RichText::new(tr("Réglages", "Settings")).heading().strong());
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(ui.ctx().content_rect().height() * 0.65).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr("Langue", "Language"));
                    let mut en = is_english();
                    if ui.selectable_label(!en, "Français").clicked() {
                        en = false;
                    }
                    if ui.selectable_label(en, "English").clicked() {
                        en = true;
                    }
                    set_english(en);
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Thème", "Theme"));
                    if ui.selectable_label(self.dark, tr("🌙 Sombre", "🌙 Dark")).clicked() {
                        self.dark = true;
                    }
                    if ui.selectable_label(!self.dark, tr("☀ Clair", "☀ Light")).clicked() {
                        self.dark = false;
                    }
                });
                let Some(d) = &mut self.settings_draft else { return };
                ui.separator();
                ui.add(egui::Slider::new(&mut d.max_peers, 5..=100).text(tr("pairs maximum par torrent", "max peers per torrent")));
                ui.add(
                    egui::Slider::new(&mut d.offers_per_announce, 1..=10)
                        .text(tr("offres WebRTC par annonce", "WebRTC offers per announce")),
                );
                ui.checkbox(&mut d.use_web_seeds, tr("Utiliser les sources web (HTTP)", "Use web seeds (HTTP)"));
                ui.separator();
                ui.label(RichText::new(tr("Trackers WebSocket", "WebSocket trackers")).strong());
                let mut remove = None;
                for (i, t) in d.trackers.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("✖").clicked() {
                            remove = Some(i);
                        }
                        ui.label(RichText::new(t).monospace().small());
                    });
                }
                if let Some(i) = remove {
                    d.trackers.remove(i);
                }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.new_tracker).hint_text("wss://…").desired_width(260.0));
                    let ok = self.new_tracker.starts_with("wss://") || self.new_tracker.starts_with("ws://");
                    if ui.add_enabled(ok, egui::Button::new(tr("Ajouter", "Add"))).clicked() {
                        d.trackers.push(self.new_tracker.trim().to_string());
                        self.new_tracker.clear();
                    }
                });
                if ui.small_button(tr("Rétablir les trackers par défaut", "Restore default trackers")).clicked() {
                    d.trackers = DEFAULT_TRACKERS.iter().map(|s| s.to_string()).collect();
                }
                ui.separator();
                ui.label(RichText::new(tr("Serveurs STUN", "STUN servers")).strong());
                let mut stun = d.stun.join("\n");
                if ui
                    .add(egui::TextEdit::multiline(&mut stun).desired_rows(2).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace))
                    .changed()
                {
                    d.stun = stun.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                }
                ui.separator();
                if ui.button(tr("🧲 Ouvrir les liens magnet avec mxTorrent", "🧲 Open magnet links with mxTorrent")).clicked() {
                    let ok = web::register_magnet_handler();
                    self.engine.borrow_mut().toast(
                        if ok {
                            tr("Demande envoyée au navigateur", "Request sent to the browser")
                        } else {
                            tr("Refusé par le navigateur", "Refused by the browser")
                        },
                        ok,
                    );
                }
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        tr("Votre identifiant de pair :", "Your peer id:"),
                        String::from_utf8_lossy(&self.engine.borrow().peer_id)
                    ))
                    .small()
                    .monospace()
                    .color(p.muted),
                );
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(tr("Enregistrer", "Save")).color(Color32::WHITE)).fill(VIOLET)).clicked() {
                    apply = true;
                    close = true;
                }
                if ui.button(tr("Fermer", "Close")).clicked() {
                    close = true;
                }
            });
        });
        if apply && let Some(d) = self.settings_draft.take() {
            let mut e = self.engine.borrow_mut();
            e.settings = d;
            e.dirty = true;
        }
        if r.should_close() || close {
            self.settings_open = false;
        }
    }

    fn about_dialog(&mut self, ctx: &egui::Context) {
        if !self.about_open && self.welcomed {
            return;
        }
        let p = self.palette();
        let mut close = false;
        let r = egui::Modal::new(Id::unique("about")).show(ctx, |ui| {
            ui.set_width(ui.available_width().min(440.0));
            ui.vertical_centered(|ui| {
                let t = ui.input(|i| i.time) as f32;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(90.0), Sense::hover());
                logo(ui.painter(), r, t);
                ui.label(RichText::new(tr("Bienvenue dans mxTorrent", "Welcome to mxTorrent")).heading().strong());
            });
            ui.add_space(6.0);
            for (icon, fr, en) in [
                (
                    "🦀",
                    "Un client BitTorrent écrit en Rust, compilé en WebAssembly, dessiné avec egui.",
                    "A BitTorrent client written in Rust, compiled to WebAssembly, drawn with egui.",
                ),
                (
                    "🔒",
                    "Rien à installer, aucun serveur : les navigateurs échangent directement, chiffré, en WebRTC.",
                    "Nothing to install, no server: browsers exchange directly, encrypted, over WebRTC.",
                ),
                (
                    "🌍",
                    "Compatible WebTorrent : vous échangez avec webtorrent.io, Instant.io, WebTorrent Desktop…",
                    "WebTorrent compatible: you swap with webtorrent.io, Instant.io, WebTorrent Desktop…",
                ),
                (
                    "📲",
                    "Installez-le comme une application depuis le menu du navigateur.",
                    "Install it as an app from your browser's menu.",
                ),
                ("⚖", "Partagez uniquement des contenus dont vous avez les droits.", "Only share content you have the rights to."),
            ] {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(icon).size(18.0));
                    ui.label(RichText::new(tr(fr, en)).color(p.text));
                });
            }
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                if ui
                    .add(egui::Button::new(RichText::new(tr("C'est parti", "Let's go")).strong().color(Color32::WHITE)).fill(VIOLET))
                    .clicked()
                {
                    close = true;
                }
                ui.hyperlink_to(tr("Le site de mxTorrent", "The mxTorrent website"), "../");
            });
        });
        if r.should_close() || close {
            self.about_open = false;
            self.welcomed = true;
        }
    }

    fn log_window(&mut self, ctx: &egui::Context) {
        if !self.log_open {
            return;
        }
        let mut open = true;
        egui::Window::new(tr("Journal", "Log")).open(&mut open).default_size([420.0, 300.0]).show(ctx, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                for l in &self.engine.borrow().log {
                    ui.label(RichText::new(l).monospace().small());
                }
            });
        });
        self.log_open = open;
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        let p = self.palette();
        let toasts: Vec<_> = self.engine.borrow().toasts.iter().cloned().collect();
        if toasts.is_empty() {
            return;
        }
        let now = web::now_ms();
        egui::Area::new(Id::unique("toasts"))
            .anchor(Align2::CENTER_BOTTOM, [0.0, if self.mobile { -100.0 } else { -48.0 }])
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                for t in toasts {
                    let age = now.saturating_sub(t.at) as f32 / 1000.0;
                    let alpha = (1.0 - ((age - 4.0).max(0.0))).clamp(0.0, 1.0) * (age * 6.0).min(1.0);
                    let c = if t.good { GREEN } else { RED };
                    egui::Frame::new()
                        .fill(p.surface.gamma_multiply(alpha))
                        .stroke(Stroke::new(1.0, c.gamma_multiply(alpha)))
                        .corner_radius(CornerRadius::same(24))
                        .inner_margin(egui::Margin::symmetric(16, 8))
                        .show(ui, |ui| {
                            ui.label(RichText::new(&t.text).color(p.text.gamma_multiply(alpha)));
                        });
                    ui.add_space(4.0);
                }
            });
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !hovering {
            return;
        }
        let rect = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("drop")));
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(180));
        painter.rect_stroke(rect.shrink(16.0), CornerRadius::same(24), Stroke::new(3.0, VIOLET), egui::StrokeKind::Inside);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            tr(
                "Déposez des .torrent pour les ouvrir, ou des fichiers pour les partager",
                "Drop .torrent files to open them, or any files to share them",
            ),
            egui::FontId::proportional(20.0),
            Color32::WHITE,
        );
    }

    fn fab(&mut self, ctx: &egui::Context) {
        if !self.mobile || self.selected.is_some() {
            return;
        }
        egui::Area::new(Id::unique("fab")).anchor(Align2::RIGHT_BOTTOM, [-20.0, -24.0]).order(egui::Order::Foreground).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(60.0), Sense::click());
            let t = ui.input(|i| i.time) as f32;
            let pa = ui.painter();
            pa.circle_filled(r.center() + Vec2::new(0.0, 4.0), 30.0, Color32::from_black_alpha(90));
            pa.circle_filled(r.center(), 30.0, mix(VIOLET, CYAN, (t * 0.8).sin() * 0.25 + 0.25));
            let c = r.center();
            pa.line_segment([c - Vec2::new(11.0, 0.0), c + Vec2::new(11.0, 0.0)], Stroke::new(3.0, Color32::WHITE));
            pa.line_segment([c - Vec2::new(0.0, 11.0), c + Vec2::new(0.0, 11.0)], Stroke::new(3.0, Color32::WHITE));
            if resp.clicked() {
                self.add_open = true;
                self.add_error = None;
            }
        });
    }
}

impl eframe::App for MxApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Ok(mut e) = self.engine.try_borrow_mut() {
            e.pump();
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(400));
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
        let p = self.palette();
        let central = egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(14, 8));
        if self.mobile {
            egui::CentralPanel::default().frame(central).show(ui, |ui| match self.selected {
                Some(h) => self.details(ui, h),
                None => self.list(ui),
            });
        } else {
            egui::Panel::left("list")
                .resizable(true)
                .default_size(400.0)
                .size_range(300.0..=600.0)
                .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(14, 8)))
                .show(ui, |ui| self.list(ui));
            egui::CentralPanel::default().frame(central).show(ui, |ui| match self.selected {
                Some(h) => self.details(ui, h),
                None => {
                    let has = !self.engine.borrow().entries.is_empty();
                    if has {
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                RichText::new(tr("Choisissez un torrent pour voir ses détails", "Pick a torrent to see its details"))
                                    .color(p.muted),
                            );
                        });
                    } else {
                        hero(ui, p);
                    }
                }
            });
        }
        self.fab(&ctx);
        self.add_dialog(&ctx);
        self.share_dialog(&ctx);
        self.confirm_dialog(&ctx);
        self.settings_dialog(&ctx);
        self.about_dialog(&ctx);
        self.log_window(&ctx);
        self.toasts(&ctx);
        self.drop_overlay(&ctx);
        if let Ok(mut e) = self.engine.try_borrow_mut() {
            e.pump();
            let (d, _) = e.total_rates();
            let active = e.entries.iter().filter(|x| x.torrent.state() == State::Downloading).count();
            if active > 0 {
                web::notify_title(&format!("↓ {} · mxTorrent", rate_str(d)));
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let prefs = Prefs { dark: self.dark, english: is_english(), saved: self.engine.borrow().snapshot(), welcomed: self.welcomed };
        eframe::set_value(storage, STORAGE_KEY, &prefs);
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(5)
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.palette().bg.to_normalized_gamma_f32()
    }
}

fn pick_share() {
    web::pick_files("", true, |files| net::push(NetEvent::Share(files)));
}

// ---------------------------------------------------------------- petits composants

fn small(ui: &mut egui::Ui, text: &str, p: Palette) {
    ui.label(RichText::new(text).small().color(p.muted));
}

fn pill(ui: &mut egui::Ui, text: &str, color: Color32, p: Palette) {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).monospace());
        });
}

fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(color).strong());
        });
}

fn state_color(s: State) -> Color32 {
    match s {
        State::FetchingMetadata => AMBER,
        State::Downloading => CYAN,
        State::Seeding => GREEN,
        State::Paused => Color32::GRAY,
    }
}

fn state_label(s: State) -> String {
    match s {
        State::FetchingMetadata => tr("métadonnées…", "metadata…"),
        State::Downloading => tr("téléchargement", "downloading"),
        State::Seeding => tr("partage", "seeding"),
        State::Paused => tr("en pause", "paused"),
    }
}

fn state_chip(ui: &mut egui::Ui, s: State) {
    chip(ui, &state_label(s), state_color(s));
}

/// Barre de progression en dégradé, avec un reflet qui glisse pendant le téléchargement.
fn progress_bar(ui: &mut egui::Ui, progress: f32, state: State, height: f32, time: f32) {
    let w = ui.available_width().max(height * 2.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, height), Sense::hover());
    let pa = ui.painter();
    let r = CornerRadius::same((height / 2.0) as u8);
    let bg = if ui.visuals().dark_mode { Color32::from_rgb(30, 30, 50) } else { Color32::from_rgb(225, 222, 245) };
    pa.rect_filled(rect, r, bg);
    let prog = progress.clamp(0.0, 1.0);
    if prog > 0.0 {
        let fill = Rect::from_min_size(rect.min, Vec2::new((w * prog).max(height), height));
        let (a, b) = match state {
            State::Seeding => (GREEN, CYAN),
            State::Paused => (Color32::GRAY, Color32::DARK_GRAY),
            State::FetchingMetadata => (AMBER, PINK),
            State::Downloading => (VIOLET, CYAN),
        };
        // Dégradé horizontal en bandes.
        let steps = 24;
        pa.rect_filled(fill, r, a);
        for k in 0..steps {
            let x0 = fill.left() + fill.width() * k as f32 / steps as f32;
            let x1 = fill.left() + fill.width() * (k + 1) as f32 / steps as f32;
            let c = mix(a, b, k as f32 / steps as f32);
            let seg = Rect::from_min_max(Pos2::new(x0, fill.top()), Pos2::new(x1, fill.bottom()));
            let rr = if k == 0 {
                CornerRadius { nw: r.nw, sw: r.sw, ne: 0, se: 0 }
            } else if k == steps - 1 {
                CornerRadius { ne: r.ne, se: r.se, nw: 0, sw: 0 }
            } else {
                CornerRadius::ZERO
            };
            pa.rect_filled(seg, rr, c);
        }
        if state == State::Downloading && time > 0.0 {
            let x = fill.left() + (time * 0.6).fract() * (fill.width() + 60.0) - 30.0;
            let shine = Rect::from_center_size(Pos2::new(x, fill.center().y), Vec2::new(30.0, height)).intersect(fill);
            if shine.width() > 0.0 {
                pa.rect_filled(shine, r, Color32::from_white_alpha(50));
            }
        }
    }
}

fn torrent_card(ui: &mut egui::Ui, entry: &crate::engine::Entry, selected: bool, p: Palette) -> egui::Response {
    let t = &entry.torrent;
    let state = t.state();
    let time = ui.input(|i| i.time) as f32;
    let frame = egui::Frame::new()
        .fill(if selected { p.surface2 } else { p.surface })
        .stroke(Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { VIOLET } else { p.border }))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(12);
    let resp = frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width().max(1.0));
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(RichText::new(t.name()).strong()).truncate().selectable(false));
            });
            progress_bar(ui, t.progress(), state, 8.0, time);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                state_chip(ui, state);
                let size = match t.total_length() {
                    Some(total) => format!("{} / {}", bytes(t.verified_bytes()), bytes(total)),
                    None => format!("{} {:.0} %", tr("métadonnées", "metadata"), t.metadata_progress() * 100.0),
                };
                small(ui, &size, p);
                if state != State::Paused {
                    small(ui, &format!("↓ {}  ↑ {}", rate_str(t.down_rate()), rate_str(t.up_rate())), p);
                    small(ui, &format!("{} {}", t.peer_count(), tr("pairs", "peers")), p);
                }
                if state == State::Downloading
                    && t.down_rate() > 1.0
                    && let Some(left) = t.left()
                {
                    small(ui, &format!("⏱ {}", duration(left as f64 / t.down_rate())), p);
                }
            });
        })
        .response;
    ui.add_space(6.0);
    resp.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn waiting_metadata(ui: &mut egui::Ui, prog: f32, p: Palette) {
    ui.vertical_centered(|ui| {
        ui.add_space(20.0);
        ui.spinner();
        ui.label(RichText::new(tr("Récupération des métadonnées auprès des pairs…", "Fetching metadata from peers…")).color(p.muted));
        if prog > 0.0 {
            ui.label(RichText::new(format!("{:.0} %", prog * 100.0)).small().color(p.muted));
        }
    });
}

fn overview(ui: &mut egui::Ui, entry: &crate::engine::Entry, p: Palette) {
    let t = &entry.torrent;
    let cols = if ui.available_width() > 520.0 { 4 } else { 2 };
    let (seeds, leech) = entry.swarm_counts();
    let stats: Vec<(String, String, Color32)> = vec![
        (tr("Réception", "Download"), rate_str(t.down_rate()), CYAN),
        (tr("Envoi", "Upload"), rate_str(t.up_rate()), PINK),
        (tr("Pairs", "Peers"), format!("{} ({} {})", t.peer_count(), t.seed_count(), tr("complets", "seeds")), VIOLET),
        (
            tr("Essaim", "Swarm"),
            format!("{} / {}", seeds.map(|x| x.to_string()).unwrap_or("–".into()), leech.map(|x| x.to_string()).unwrap_or("–".into())),
            GREEN,
        ),
        (tr("Reçu", "Downloaded"), bytes(t.downloaded), CYAN),
        (tr("Envoyé", "Uploaded"), bytes(t.uploaded), PINK),
        (
            tr("Ratio", "Ratio"),
            if t.downloaded > 0 { format!("{:.2}", t.uploaded as f64 / t.downloaded as f64) } else { "–".into() },
            AMBER,
        ),
        (tr("Taille", "Size"), t.total_length().map(bytes).unwrap_or("–".into()), VIOLET),
    ];
    let w = ((ui.available_width() - 8.0 * (cols as f32 - 1.0)) / cols as f32).max(60.0);
    egui::Grid::new("stats").num_columns(cols).spacing([8.0, 8.0]).show(ui, |ui| {
        for (i, (label, value, color)) in stats.iter().enumerate() {
            egui::Frame::new().fill(p.surface).corner_radius(CornerRadius::same(14)).inner_margin(10).show(ui, |ui| {
                ui.set_width((w - 20.0).max(40.0));
                ui.label(RichText::new(label).small().color(p.muted));
                ui.label(RichText::new(value).strong().color(*color));
            });
            if (i + 1) % cols == 0 {
                ui.end_row();
            }
        }
    });
    ui.add_space(10.0);
    if let Some(m) = t.metainfo() {
        ui.label(
            RichText::new(format!(
                "{} — {} × {}",
                tr("Carte des pièces", "Piece map"),
                m.info.piece_count(),
                bytes(m.info.piece_length as u64)
            ))
            .strong(),
        );
        piece_map(ui, t, p);
        ui.add_space(8.0);
        egui::Grid::new("meta").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new(tr("Empreinte", "Info hash")).color(p.muted));
            ui.label(RichText::new(t.info_hash().to_hex()).monospace().small());
            ui.end_row();
            ui.label(RichText::new(tr("Fichiers", "Files")).color(p.muted));
            ui.label(m.info.files.len().to_string());
            ui.end_row();
            if let Some(c) = &m.comment {
                ui.label(RichText::new(tr("Commentaire", "Comment")).color(p.muted));
                ui.label(c);
                ui.end_row();
            }
            if let Some(c) = &m.created_by {
                ui.label(RichText::new(tr("Créé avec", "Created by")).color(p.muted));
                ui.label(c);
                ui.end_row();
            }
            if t.hash_failures > 0 {
                ui.label(RichText::new(tr("Pièces rejetées", "Rejected pieces")).color(p.muted));
                ui.label(RichText::new(t.hash_failures.to_string()).color(RED));
                ui.end_row();
            }
        });
    } else {
        waiting_metadata(ui, t.metadata_progress(), p);
        ui.label(RichText::new(t.info_hash().to_hex()).monospace().small().color(p.muted));
    }
}

/// Une case par pièce : vérifiée, en cours, absente, ou non voulue.
fn piece_map(ui: &mut egui::Ui, t: &mxtorrent_core::Torrent, p: Palette) {
    let states = t.piece_states();
    let wanted = t.wanted_pieces();
    let n = states.len().max(1);
    let w = ui.available_width().max(40.0);
    // Au plus ~2 000 cases : au-delà, plusieurs pièces par case.
    let per = n.div_ceil(2000).max(1);
    let cells = n.div_ceil(per);
    let cell = ((w * 140.0 / cells as f32).sqrt()).clamp(4.0, 14.0);
    let cols = ((w / cell).floor() as usize).max(1);
    let rows = cells.div_ceil(cols);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, rows as f32 * cell), Sense::hover());
    let pa = ui.painter_at(rect);
    let empty = if p.dark { Color32::from_rgb(28, 28, 46) } else { Color32::from_rgb(226, 223, 246) };
    for c in 0..cells {
        let range = c * per..((c + 1) * per).min(n);
        let len = range.len().max(1) as f32;
        let have = range.clone().filter(|&i| states[i] == 255).count() as f32 / len;
        let part: f32 =
            range.clone().map(|i| if states[i] > 0 && states[i] < 255 { states[i] as f32 / 255.0 } else { 0.0 }).sum::<f32>() / len;
        let any_wanted = range.clone().any(|i| wanted.get(i).copied().unwrap_or(true));
        let color = if have >= 1.0 {
            mix(VIOLET, CYAN, (c % cols) as f32 / cols as f32)
        } else if part > 0.0 {
            mix(empty, AMBER, 0.35 + part * 0.65)
        } else if have > 0.0 {
            mix(empty, VIOLET, have)
        } else if !any_wanted {
            empty.gamma_multiply(0.4)
        } else {
            empty
        };
        let x = rect.left() + (c % cols) as f32 * cell;
        let y = rect.top() + (c / cols) as f32 * cell;
        pa.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::splat(cell - 1.5)), CornerRadius::same(2), color);
    }
}

fn hero(ui: &mut egui::Ui, p: Palette) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.12);
        let t = ui.input(|i| i.time) as f32;
        let (r, _) = ui.allocate_exact_size(Vec2::splat(220.0), Sense::hover());
        logo(ui.painter(), r, t);
        ui.add_space(16.0);
        ui.label(RichText::new(tr("Le torrent, directement dans le navigateur", "Torrents, right in your browser")).size(28.0).strong());
        ui.label(
            RichText::new(tr(
                "Rust + WebAssembly + WebRTC. Aucune installation, aucun serveur entre vous et vos pairs.",
                "Rust + WebAssembly + WebRTC. No install, no server between you and your peers.",
            ))
            .color(p.muted),
        );
        ui.add_space(12.0);
        if ui.button(tr("📤 Partager des fichiers", "📤 Share files")).clicked() {
            pick_share();
        }
    });
}

/// Le logo animé : un noyau et trois orbites inclinées, comme sur le site.
pub fn logo(pa: &egui::Painter, r: Rect, t: f32) {
    let c = r.center();
    let s = r.width() / 2.0;
    pa.circle_filled(c, s * 0.95, VIOLET.gamma_multiply(0.07));
    for (k, col) in [VIOLET, CYAN, PINK].into_iter().enumerate() {
        let tilt = k as f32 * std::f32::consts::TAU / 3.0 + t * 0.15;
        let pts: Vec<Pos2> = (0..=48)
            .map(|i| {
                let a = i as f32 / 48.0 * std::f32::consts::TAU;
                let (x, y) = (a.cos() * s * 0.85, a.sin() * s * 0.32);
                Pos2::new(c.x + x * tilt.cos() - y * tilt.sin(), c.y + x * tilt.sin() + y * tilt.cos())
            })
            .collect();
        pa.add(egui::Shape::line(pts, Stroke::new((s * 0.06).max(1.2), col.gamma_multiply(0.85))));
        let a = t * (1.2 + k as f32 * 0.35) + k as f32 * 2.0;
        let (x, y) = (a.cos() * s * 0.85, a.sin() * s * 0.32);
        let dot = Pos2::new(c.x + x * tilt.cos() - y * tilt.sin(), c.y + x * tilt.sin() + y * tilt.cos());
        pa.circle_filled(dot, (s * 0.1).max(2.0), col);
    }
    pa.circle_filled(c, s * 0.22, mix(VIOLET, CYAN, (t * 0.7).sin() * 0.5 + 0.5));
    pa.circle_filled(c - Vec2::splat(s * 0.06), s * 0.08, Color32::from_white_alpha(120));
}
