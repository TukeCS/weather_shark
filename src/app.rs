use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui::{
    Align2, Color32, CornerRadius, FontFamily, FontId, Key, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, UiBuilder, Vec2, pos2, vec2,
};

use crate::alerts::Alert;
use crate::config::{Config, DEFAULT_LUA};
use crate::level2::{Moment, Volume};
use crate::map::{Basemap, Site, sites};
use crate::net::{self, Msg, Net};
use crate::render;
use crate::sound::Player;

const STORE_KEY: &str = "config_lua";

struct Scan {
    vol: Arc<Volume>,
    key: String,
    generation: u64,
}

pub struct App {
    cfg: Config,
    cfg_error: Option<String>,
    editor: String,
    editor_dirty: bool,
    #[cfg(not(target_arch = "wasm32"))]
    cfg_mtime: Option<std::time::SystemTime>,
    #[cfg(not(target_arch = "wasm32"))]
    last_cfg_check: f64,

    net: Net,
    rx: Receiver<Msg>,
    sites: Vec<Site>,
    site: String,
    map: Basemap,

    keys: Vec<String>,
    listed: Vec<i64>,
    scan: Option<Scan>,
    loading: Option<String>,
    generation: u64,
    live: bool,
    target: Option<i64>,
    last_refresh: f64,
    error: Option<String>,

    tilt: f32,
    panes: Vec<Moment>,
    textures: HashMap<(u64, u8, Moment), (TextureHandle, f32)>,

    center: Vec2,
    zoom: f32,
    hover: Option<Pos2>,
    fitted: bool,

    alerts: Vec<(Alert, usize)>,
    seen: HashSet<String>,
    alerts_once: bool,
    alerts_error: Option<String>,
    last_alerts: f64,
    alarm_until: f64,
    player: Player,

    show_alerts: bool,
    show_settings: bool,
    site_search: String,
    jump: String,
}

fn ui_time(ui: &Ui) -> f64 {
    ui.input(|i| i.time)
}

fn today() -> i64 {
    net::now().div_euclid(86400)
}

impl App {
    /// `start` optionally opens a past scan: (site, "YYYY-MM-DD HH:MM").
    pub fn new(cc: &eframe::CreationContext<'_>, start: Option<(String, String)>) -> Self {
        let (tx, rx) = channel();
        let net = Net::new(tx, cc.egui_ctx.clone());

        // Native reads ~/.config/weather_shark/config.lua, web reads browser storage.
        #[cfg(not(target_arch = "wasm32"))]
        let src = crate::load_config_file();
        #[cfg(target_arch = "wasm32")]
        let src = cc.storage.and_then(|s| s.get_string(STORE_KEY)).unwrap_or_else(|| DEFAULT_LUA.to_string());
        let _ = STORE_KEY;

        let (cfg, cfg_error) = match Config::from_lua(&src) {
            Ok(c) => (c, None),
            Err(e) => (Config::default_config(), Some(e)),
        };
        let sites = sites();
        let site = cfg.site.clone();
        let (lat, lon) = sites.iter().find(|s| s.id == site).map_or((35.33, -97.28), |s| (s.lat, s.lon));

        let mut app = Self {
            cfg_error,
            editor: src,
            editor_dirty: false,
            #[cfg(not(target_arch = "wasm32"))]
            cfg_mtime: crate::config_mtime(),
            #[cfg(not(target_arch = "wasm32"))]
            last_cfg_check: 0.0,
            net,
            rx,
            sites,
            site: String::new(),
            map: Basemap::new(lat, lon),
            keys: Vec::new(),
            listed: Vec::new(),
            scan: None,
            loading: None,
            generation: 0,
            live: cfg.live,
            target: None,
            last_refresh: 0.0,
            error: None,
            tilt: cfg.tilt,
            panes: cfg.panes.clone(),
            textures: HashMap::new(),
            center: Vec2::ZERO,
            zoom: 2.0,
            hover: None,
            fitted: false,
            alerts: Vec::new(),
            seen: HashSet::new(),
            alerts_once: false,
            alerts_error: None,
            last_alerts: f64::NEG_INFINITY,
            alarm_until: 0.0,
            player: Player::new(),
            show_alerts: false,
            show_settings: false,
            site_search: String::new(),
            jump: String::new(),
            cfg,
        };
        app.apply_style(&cc.egui_ctx);
        match start.and_then(|(s, t)| Some((s.to_uppercase(), net::parse_time(&t)?))) {
            Some((s, t)) => app.select_site(&s, Some(t + 60)),
            None => app.select_site(&site, None),
        }
        app
    }

    // ---- config ---------------------------------------------------------------

    fn apply_config(&mut self, ctx: &egui::Context, src: &str) {
        match Config::from_lua(src) {
            Ok(c) => {
                let site_changed = c.site != self.cfg.site;
                if c.panes != self.cfg.panes {
                    self.panes = c.panes.clone();
                }
                if c.tilt != self.cfg.tilt {
                    self.tilt = c.tilt;
                }
                self.cfg = c;
                self.cfg_error = None;
                self.textures.clear();
                self.apply_style(ctx);
                if site_changed {
                    let s = self.cfg.site.clone();
                    self.select_site(&s, None);
                }
            }
            Err(e) => self.cfg_error = Some(e),
        }
    }

    fn apply_style(&self, ctx: &egui::Context) {
        let t = &self.cfg.theme;
        let mut v = egui::Visuals::dark();
        v.panel_fill = t.bg;
        v.window_fill = t.panel;
        v.window_stroke = Stroke::new(1.0, t.line);
        v.extreme_bg_color = t.bg;
        v.faint_bg_color = t.panel;
        v.override_text_color = Some(t.fg);
        v.selection.bg_fill = t.accent.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, t.accent);
        v.hyperlink_color = t.accent;
        let r = CornerRadius::same(6);
        for (w, fill, stroke) in [
            (&mut v.widgets.noninteractive, t.bg, t.line),
            (&mut v.widgets.inactive, t.panel, t.line),
            (&mut v.widgets.hovered, t.panel, t.accent),
            (&mut v.widgets.active, t.line, t.accent),
            (&mut v.widgets.open, t.panel, t.accent),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.bg_stroke = Stroke::new(1.0, stroke);
            w.corner_radius = r;
            w.fg_stroke.color = t.fg;
        }
        v.widgets.noninteractive.fg_stroke.color = t.muted;
        v.widgets.hovered.fg_stroke.color = t.accent;
        let size = t.font_size;
        ctx.all_styles_mut(|s| {
            s.visuals = v.clone();
            for (style, font) in s.text_styles.iter_mut() {
                let k = match style {
                    egui::TextStyle::Heading => 1.25,
                    egui::TextStyle::Small => 0.8,
                    _ => 1.0,
                };
                *font = FontId::new(size * k, FontFamily::Monospace);
            }
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(8.0, 3.0);
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn watch_config(&mut self, ctx: &egui::Context, now: f64) {
        if now - self.last_cfg_check < 1.0 {
            return;
        }
        self.last_cfg_check = now;
        let m = crate::config_mtime();
        if m != self.cfg_mtime {
            self.cfg_mtime = m;
            let src = crate::load_config_file();
            if !self.editor_dirty {
                self.editor = src.clone();
            }
            self.apply_config(ctx, &src);
        }
    }

    fn save_editor(&mut self, ctx: &egui::Context) {
        let src = self.editor.clone();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let p = crate::config_path();
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if let Err(e) = std::fs::write(&p, &src) {
                self.cfg_error = Some(format!("couldn't save {}: {e}", p.display()));
                return;
            }
            self.cfg_mtime = crate::config_mtime();
        }
        self.editor_dirty = false;
        self.apply_config(ctx, &src);
    }

    // ---- data -----------------------------------------------------------------

    fn select_site(&mut self, id: &str, target: Option<i64>) {
        let id = id.to_uppercase();
        if let Some(s) = self.sites.iter().find(|s| s.id == id) {
            self.map.recenter(s.lat, s.lon);
        }
        if id != self.site {
            self.center = Vec2::ZERO;
            self.zoom = self.zoom.max(1.2);
            self.scan = None;
            self.textures.clear();
        }
        self.site = id;
        self.keys.clear();
        self.listed.clear();
        self.loading = None;
        self.error = None;
        self.target = target;
        match target {
            Some(t) => {
                self.live = false;
                self.list(t.div_euclid(86400));
            }
            None => {
                self.list(today());
                self.list(today() - 1);
            }
        }
    }

    fn list(&mut self, day: i64) {
        if !self.listed.contains(&day) {
            self.listed.push(day);
            self.net.list(&self.site, day);
        }
    }

    fn load(&mut self, key: &str) {
        if self.loading.as_deref() == Some(key) || self.scan.as_ref().is_some_and(|s| s.key == key) {
            return;
        }
        self.loading = Some(key.to_string());
        self.net.volume(key);
    }

    fn pick_scan(&mut self) {
        if let Some(t) = self.target {
            let best = self.keys.iter().rev().find(|k| net::key_time(k).is_some_and(|kt| kt <= t)).cloned();
            match best {
                Some(k) => {
                    self.target = None;
                    self.load(&k);
                }
                None => {
                    let day = t.div_euclid(86400);
                    if !self.listed.contains(&(day - 1)) {
                        self.list(day - 1);
                    } else if self.listed.len() >= 2 {
                        self.error = Some(format!("no scans for {} near {}", self.site, net::fmt_time(t)));
                        self.target = None;
                    }
                }
            }
        } else if self.live {
            if let Some(k) = self.keys.last().cloned() {
                self.load(&k);
            }
        }
    }

    fn step(&mut self, dir: i32) {
        let Some(cur) = self.scan.as_ref().map(|s| s.key.clone()) else { return };
        self.live = false;
        let i = self.keys.iter().position(|k| *k == cur).unwrap_or(0) as i32 + dir;
        if i >= 0 && (i as usize) < self.keys.len() {
            let k = self.keys[i as usize].clone();
            self.load(&k);
        } else if i < 0 {
            // Reached the start of the listing: go fetch the day before.
            if let Some(t) = net::key_time(&cur) {
                self.target = Some(t - 1);
                self.list(t.div_euclid(86400) - 1);
            }
        }
    }

    fn poll_alerts(&mut self) {
        if !self.cfg.alerts.enabled {
            self.alerts.clear();
            return;
        }
        let events: Vec<String> = self.cfg.alerts.rules.iter().map(|r| r.event.clone()).collect();
        if !events.is_empty() {
            self.net.alerts(&events);
        }
    }

    fn receive(&mut self, now: f64) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Listing { site, keys, .. } => {
                    if site != self.site {
                        continue;
                    }
                    match keys {
                        Ok(k) => {
                            self.keys.extend(k);
                            self.keys.sort();
                            self.keys.dedup();
                            self.pick_scan();
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Msg::Volume { key, vol } => {
                    if self.loading.as_deref() != Some(key.as_str()) {
                        continue;
                    }
                    self.loading = None;
                    match vol {
                        Ok(vol) => {
                            self.generation += 1;
                            self.textures.clear();
                            self.error = None;
                            self.scan = Some(Scan { vol, key, generation: self.generation });
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Msg::Alerts(res) => match res {
                    Ok(list) => self.update_alerts(list, now),
                    Err(e) => self.alerts_error = Some(e),
                },
            }
        }
    }

    fn update_alerts(&mut self, list: Vec<Alert>, now: f64) {
        self.alerts_error = None;
        let a = &self.cfg.alerts;
        let radius = a.radius_km;
        let proj = self.map.proj;
        let mut kept: Vec<(Alert, usize)> = list
            .into_iter()
            .filter(|al| {
                radius <= 0.0
                    || al.polygons.iter().flatten().any(|&(lon, lat)| proj.fwd(lon, lat).to_vec2().length() as f64 <= radius)
            })
            .filter_map(|al| {
                let rule = a.rules.iter().position(|r| al.matches(r))?;
                Some((al, rule))
            })
            .collect();
        kept.sort_by_key(|(_, r)| *r);

        // Sound the most important new alert (rules earlier in the list win).
        let fresh = kept.iter().filter(|(al, _)| !self.seen.contains(&al.id)).map(|(_, r)| *r).min();
        if let Some(r) = fresh {
            if self.alerts_once || a.on_startup {
                let rule = &a.rules[r];
                if rule.sound != "none" || rule.file.is_some() {
                    self.player.play(&rule.sound, rule.file.as_deref(), rule.repeats, rule.volume * a.volume);
                }
                self.alarm_until = now + 30.0;
            }
        }
        self.seen.extend(kept.iter().map(|(al, _)| al.id.clone()));
        self.alerts_once = true;
        self.alerts = kept;
    }

    // ---- view helpers -------------------------------------------------------

    fn to_screen(&self, rect: Rect, km: Pos2) -> Pos2 {
        let c = rect.center();
        pos2(c.x + (km.x - self.center.x) * self.zoom, c.y - (km.y - self.center.y) * self.zoom)
    }

    fn to_km(&self, rect: Rect, p: Pos2) -> Pos2 {
        let c = rect.center();
        pos2(self.center.x + (p.x - c.x) / self.zoom, self.center.y - (p.y - c.y) / self.zoom)
    }

    fn tilts(&self) -> Vec<f32> {
        self.scan.as_ref().map(|s| s.vol.tilts()).unwrap_or_default()
    }

    fn shift_tilt(&mut self, d: i32) {
        let t = self.tilts();
        if t.is_empty() {
            return;
        }
        let cur = t.iter().position(|x| (x - self.tilt).abs() < 0.15).unwrap_or(0) as i32;
        self.tilt = t[(cur + d).clamp(0, t.len() as i32 - 1) as usize];
    }

    fn set_pane_count(&mut self, n: usize) {
        let order = [Moment::Ref, Moment::Vel, Moment::Rho, Moment::Zdr, Moment::Sw, Moment::Phi];
        while self.panes.len() < n {
            let next = order.iter().find(|m| !self.panes.contains(m)).copied().unwrap_or(Moment::Ref);
            self.panes.push(next);
        }
        self.panes.truncate(n.max(1));
    }

    // ---- UI -----------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut Ui, now: f64) {
        let t = self.cfg.theme.clone();
        ui.horizontal_centered(|ui| {
            // logo dot: pulses red while a new alert is fresh
            let alarm = now < self.alarm_until;
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            let c = if alarm {
                let k = (0.5 + 0.5 * (now * 6.0).sin()) as f32;
                Color32::from_rgb(255, (55.0 * k) as u8, (95.0 * k) as u8)
            } else {
                t.accent
            };
            ui.painter().circle_filled(r.center(), 4.0, c);
            ui.label(egui::RichText::new("weather_shark").strong());
            ui.separator();

            // site picker
            let site_btn = ui.button(egui::RichText::new(format!("{} ▾", self.site)).color(t.accent));
            egui::Popup::menu(&site_btn).show(|ui| {
                ui.set_min_width(240.0);
                ui.add(egui::TextEdit::singleline(&mut self.site_search).hint_text("search sites…"));
                let q = self.site_search.to_lowercase();
                let mut pick = None;
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for s in &self.sites {
                        if q.is_empty() || s.id.to_lowercase().contains(&q) || s.name.to_lowercase().contains(&q) {
                            if ui.selectable_label(s.id == self.site, format!("{}  {}", s.id, s.name)).clicked() {
                                pick = Some(s.id.clone());
                            }
                        }
                    }
                });
                if let Some(p) = pick {
                    self.live = true;
                    self.select_site(&p, None);
                    ui.close();
                }
            });

            // scan time
            match &self.scan {
                Some(s) => {
                    let age = net::now() - s.vol.time;
                    ui.label(net::fmt_time(s.vol.time));
                    let tag = if age < 172_800 { net::fmt_age(age) } else { "archive".into() };
                    ui.label(egui::RichText::new(tag).color(t.muted));
                }
                None => {
                    ui.label(egui::RichText::new("no scan").color(t.muted));
                }
            }
            if self.loading.is_some() || (self.scan.is_none() && self.error.is_none()) {
                ui.spinner();
            }

            if ui.button("◀").on_hover_text("previous scan (←)").clicked() {
                self.step(-1);
            }
            if ui.button("▶").on_hover_text("next scan (→)").clicked() {
                self.step(1);
            }
            let live = egui::RichText::new("● live").color(if self.live { Color32::from_rgb(255, 69, 95) } else { t.muted });
            if ui.selectable_label(self.live, live).on_hover_text("follow the newest scan (L)").clicked() {
                self.live = !self.live;
                if self.live {
                    let s = self.site.clone();
                    self.select_site(&s, None);
                }
            }
            ui.separator();

            // tilt
            let tilts = self.tilts();
            egui::ComboBox::from_id_salt("tilt")
                .selected_text(format!("{:.1}°", self.tilt))
                .width(64.0)
                .show_ui(ui, |ui| {
                    for tl in &tilts {
                        ui.selectable_value(&mut self.tilt, *tl, format!("{tl:.1}°"));
                    }
                })
                .response
                .on_hover_text("tilt (↑/↓)");
            ui.separator();

            // pane count
            for n in 1..=4 {
                if ui.selectable_label(self.panes.len() == n, n.to_string()).on_hover_text("panes (1–4)").clicked() {
                    self.set_pane_count(n);
                }
            }
            ui.separator();

            let bm = ui.button("events");
            egui::Popup::menu(&bm).show(|ui| {
                ui.set_min_width(280.0);
                ui.label(egui::RichText::new("bookmarks").color(t.muted).small());
                let mut go = None;
                for b in &self.cfg.bookmarks {
                    if ui.button(format!("{:<14} {} {}", b.name, b.site, b.time)).clicked() {
                        go = Some((b.site.clone(), b.time.clone()));
                    }
                }
                ui.separator();
                ui.label(egui::RichText::new("go to  SITE YYYY-MM-DD HH:MM (UTC)").color(t.muted).small());
                let r = ui.add(egui::TextEdit::singleline(&mut self.jump).hint_text("KTLX 2013-05-20 20:08"));
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    let s = self.jump.trim().to_string();
                    let (site, time) = s.split_once(' ').unwrap_or((&s, ""));
                    go = Some((site.to_uppercase(), time.to_string()));
                }
                if let Some((site, time)) = go {
                    match net::parse_time(&time) {
                        Some(t) => self.select_site(&site, Some(t + 60)),
                        None => self.error = Some(format!("can't read time {time:?}")),
                    }
                    ui.close();
                }
            });
            let n = self.alerts.len();
            let warn_col = self.alerts.first().map(|(_, r)| self.cfg.alerts.rules[*r].color).unwrap_or(t.muted);
            let txt = egui::RichText::new(format!("alerts {n}")).color(if n > 0 { warn_col } else { t.fg });
            if ui.selectable_label(self.show_alerts, txt).on_hover_text("active warnings near this radar").clicked() {
                self.show_alerts = !self.show_alerts;
            }
            if ui.selectable_label(self.show_settings, "lua").on_hover_text("edit config.lua").clicked() {
                self.show_settings = !self.show_settings;
            }
        });
    }

    /// Errors float over the bottom-right of the map.
    fn error_toast(&self, ui: &Ui, area: Rect) {
        let Some(e) = self.cfg_error.as_ref().or(self.error.as_ref()) else { return };
        let t = &self.cfg.theme;
        let font = FontId::monospace(t.font_size * 0.9);
        let galley = ui.painter().layout(e.clone(), font, Color32::from_rgb(255, 130, 130), (area.width() * 0.6).max(200.0));
        let r = Rect::from_min_size(area.right_bottom() - galley.size() - vec2(28.0, 44.0), galley.size()).expand(8.0);
        let p = ui.painter();
        p.rect_filled(r, 8, t.panel);
        p.rect_stroke(r, 8, Stroke::new(1.0, Color32::from_rgb(160, 60, 70)), StrokeKind::Inside);
        p.galley(r.min + vec2(8.0, 8.0), galley, t.fg);
    }

    fn settings_panel(&mut self, ui: &mut Ui) {
        let t = self.cfg.theme.clone();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("config.lua").strong());
            #[cfg(not(target_arch = "wasm32"))]
            ui.label(egui::RichText::new(crate::config_path().display().to_string()).color(t.muted).small());
        });
        ui.horizontal(|ui| {
            if ui.button("apply").on_hover_text("ctrl+s").clicked() {
                self.save_editor(ui.ctx());
            }
            if ui.button("reset to default").clicked() {
                self.editor = DEFAULT_LUA.to_string();
                self.editor_dirty = true;
            }
            ui.label(egui::RichText::new("test:").color(t.muted));
            for s in ["siren", "alarm", "beep", "chime"] {
                if ui.small_button(s).clicked() {
                    self.player.play(s, None, 1, self.cfg.alerts.volume);
                }
            }
        });
        match &self.cfg_error {
            Some(e) => ui.colored_label(Color32::from_rgb(255, 120, 120), e),
            None => ui.label(egui::RichText::new("ok").color(t.muted).small()),
        };
        ui.add_space(4.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            let r = ui.add(
                egui::TextEdit::multiline(&mut self.editor)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(40)
                    .lock_focus(true),
            );
            if r.changed() {
                self.editor_dirty = true;
            }
        });
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::S)) {
            self.save_editor(ui.ctx());
        }
    }

    fn alerts_panel(&mut self, ui: &mut Ui) {
        let t = self.cfg.theme.clone();
        ui.add_space(8.0);
        ui.label(egui::RichText::new("ACTIVE WARNINGS").color(t.muted).small());
        if let Some(e) = &self.alerts_error {
            ui.colored_label(Color32::from_rgb(255, 120, 120), e);
        }
        if self.alerts.is_empty() {
            ui.label(egui::RichText::new(format!("nothing within {:.0} km of {}", self.cfg.alerts.radius_km, self.site)).color(t.muted));
        }
        let mut focus = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (al, r) in &self.alerts {
                let col = self.cfg.alerts.rules[*r].color;
                let resp = egui::Frame::new()
                    .fill(t.panel)
                    .stroke(Stroke::new(1.0, t.line))
                    .corner_radius(8)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (b, _) = ui.allocate_exact_size(vec2(4.0, 16.0), Sense::hover());
                            ui.painter().rect_filled(b, 2, col);
                            ui.label(egui::RichText::new(&al.event).color(col).strong());
                            if let Some(tag) = al.tag() {
                                ui.label(egui::RichText::new(tag).small().background_color(col.gamma_multiply(0.25)).color(col));
                            }
                        });
                        ui.label(egui::RichText::new(&al.area).small());
                        let mut extra = String::new();
                        if let Some(h) = al.hail {
                            extra += &format!("hail {h:.2}\"  ");
                        }
                        if let Some(e) = al.expires.get(11..16) {
                            extra += &format!("until {e} local");
                        }
                        ui.label(egui::RichText::new(extra).color(t.muted).small());
                    })
                    .response
                    .interact(Sense::click());
                if resp.on_hover_text(&al.headline).clicked() {
                    focus = al.polygons.first().cloned();
                }
                ui.add_space(4.0);
            }
        });
        if let Some(poly) = focus {
            let pts: Vec<Pos2> = poly.iter().map(|&(x, y)| self.map.proj.fwd(x, y)).collect();
            if !pts.is_empty() {
                let r = Rect::from_points(&pts);
                self.center = r.center().to_vec2();
                self.zoom = self.zoom.max(3.0);
            }
        }
    }

    fn pane_rects(&self, area: Rect) -> Vec<Rect> {
        let n = self.panes.len();
        let gap = 8.0;
        let (cols, rows) = match n {
            0..=3 => (n.max(1), 1),
            4 => (2, 2),
            _ => (3, 2),
        };
        let w = (area.width() - gap * (cols as f32 - 1.0)) / cols as f32;
        let h = (area.height() - gap * (rows as f32 - 1.0)) / rows as f32;
        (0..n)
            .map(|i| {
                let (c, r) = (i % cols, i / cols);
                Rect::from_min_size(area.min + vec2(c as f32 * (w + gap), r as f32 * (h + gap)), vec2(w, h))
            })
            .collect()
    }

    fn texture(&mut self, ctx: &egui::Context, m: Moment) -> Option<(TextureHandle, f32)> {
        let scan = self.scan.as_ref()?;
        let sweep = scan.vol.sweep_for(m, self.tilt)?;
        let key = (scan.generation, sweep.number, m);
        if let Some(t) = self.textures.get(&key) {
            return Some(t.clone());
        }
        let range = render::range_km(sweep, m).clamp(50.0, 470.0);
        let t0 = web_time::Instant::now();
        let img = render::rasterize(sweep, m, self.cfg.palette(m), self.cfg.unit_factor(m), self.cfg.image_size, range);
        log::debug!("rendered {m:?} in {:?}", t0.elapsed());
        let opts = if self.cfg.smooth { TextureOptions::LINEAR } else { TextureOptions::NEAREST };
        let tex = ctx.load_texture(format!("{}-{}-{:?}", scan.key, sweep.number, m), img, opts);
        self.textures.insert(key, (tex.clone(), range));
        Some((tex, range))
    }

    fn pane(&mut self, ui: &mut Ui, rect: Rect, idx: usize) {
        let t = self.cfg.theme.clone();
        let ms = self.cfg.map.clone();
        let m = self.panes[idx];
        let id = ui.scope_id().with(("pane", idx));
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let rad = t.radius;

        // input: drag to pan, scroll/pinch to zoom around the cursor
        if resp.dragged() {
            let d = resp.drag_delta();
            self.center += vec2(-d.x, d.y) / self.zoom;
        }
        if let Some(p) = resp.hover_pos() {
            self.hover = Some(self.to_km(rect, p));
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = (scroll * 0.0018).exp() * pinch;
            if factor != 1.0 {
                let before = self.to_km(rect, p);
                self.zoom = (self.zoom * factor).clamp(0.05, 60.0);
                let after = self.to_km(rect, p);
                self.center += before - after;
            }
        }
        if resp.double_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let k = self.to_km(rect, p);
                self.center = k.to_vec2();
                self.zoom = (self.zoom * 2.0).min(60.0);
            }
        }

        painter.rect_filled(rect, rad, t.panel);

        // radar image
        if let Some((tex, range)) = self.texture(ui.ctx(), m) {
            let a = self.to_screen(rect, pos2(-range, range));
            let b = self.to_screen(rect, pos2(range, -range));
            painter.image(tex.id(), Rect::from_two_pos(a, b), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        }

        // map
        let view = Rect::from_two_pos(self.to_km(rect, rect.min), self.to_km(rect, rect.max)).expand(5.0);
        let draw_layer = |layer: &crate::map::Layer, col: Color32, width: f32| {
            for (bb, pts) in &layer.lines {
                if !bb.intersects(view) {
                    continue;
                }
                let mut out: Vec<Pos2> = Vec::with_capacity(pts.len());
                for p in pts {
                    let s = self.to_screen(rect, *p);
                    if out.last().is_none_or(|l: &Pos2| (l.x - s.x).abs() + (l.y - s.y).abs() > 1.5) {
                        out.push(s);
                    }
                }
                if out.len() >= 2 {
                    painter.add(Shape::line(out, Stroke::new(width, col)));
                }
            }
        };
        if let Some(c) = ms.counties.filter(|_| self.zoom > 0.6) {
            draw_layer(&self.map.counties_km, c, 0.8);
        }
        if let Some(c) = ms.states {
            draw_layer(&self.map.states_km, c, 1.2);
        }
        if let Some(c) = ms.range_rings {
            let o = self.to_screen(rect, Pos2::ZERO);
            for r in (50..=450).step_by(50) {
                painter.circle_stroke(o, r as f32 * self.zoom, Stroke::new(1.0, c));
            }
        }

        // warning polygons
        for (al, r) in &self.alerts {
            let col = self.cfg.alerts.rules[*r].color;
            for poly in &al.polygons {
                let pts: Vec<Pos2> = poly.iter().map(|&(x, y)| self.to_screen(rect, self.map.proj.fwd(x, y))).collect();
                painter.add(Shape::closed_line(pts, Stroke::new(2.0, col)));
            }
        }

        // radar sites (click to switch)
        let pointer = resp.hover_pos();
        let mut hovered_site = None;
        for s in &self.sites {
            let p = self.to_screen(rect, self.map.proj.fwd(s.lon, s.lat));
            if !rect.expand(20.0).contains(p) {
                continue;
            }
            let cur = s.id == self.site;
            let near = pointer.is_some_and(|q| q.distance(p) < 10.0);
            if near {
                hovered_site = Some(s.id.clone());
            }
            let r = if near { 5.0 } else { 3.5 };
            painter.circle_filled(p, r, if cur { t.fg } else { ms.sites });
            if cur {
                painter.circle_stroke(p, 7.0, Stroke::new(1.5, ms.sites));
            }
            if ms.site_labels && (self.zoom > 0.35 || near || cur) {
                painter.text(p + vec2(7.0, -7.0), Align2::LEFT_BOTTOM, &s.id, FontId::monospace(10.5), if near { t.fg } else { t.muted });
            }
        }
        if let Some(s) = &hovered_site {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            if resp.clicked() && *s != self.site {
                self.live = true;
                let s = s.clone();
                self.select_site(&s, None);
            }
        }

        // crosshair (shared by all panes)
        let mut readout = None;
        if let Some(h) = self.hover {
            let p = self.to_screen(rect, h);
            if rect.contains(p) {
                let c = ms.crosshair;
                painter.line_segment([p - vec2(9.0, 0.0), p - vec2(3.0, 0.0)], Stroke::new(1.5, c));
                painter.line_segment([p + vec2(3.0, 0.0), p + vec2(9.0, 0.0)], Stroke::new(1.5, c));
                painter.line_segment([p - vec2(0.0, 9.0), p - vec2(0.0, 3.0)], Stroke::new(1.5, c));
                painter.line_segment([p + vec2(0.0, 3.0), p + vec2(0.0, 9.0)], Stroke::new(1.5, c));
            }
            if let Some(scan) = &self.scan {
                if let Some(sw) = scan.vol.sweep_for(m, self.tilt) {
                    readout = Some(match render::sample(sw, m, self.cfg.unit_factor(m), self.cfg.palette(m).mask_ref, h.x, h.y) {
                        Some(v) if m == Moment::Rho => format!("{v:.3}"),
                        Some(v) => format!("{v:.1} {}", self.cfg.unit_label(m)),
                        None => "–".into(),
                    });
                }
            }
        }

        if self.scan.is_none() {
            let msg = if let Some(e) = &self.error { e.clone() } else { format!("loading {}…", self.site) };
            painter.text(rect.center() + vec2(0.0, 40.0), Align2::CENTER_CENTER, msg, FontId::monospace(13.0), t.muted);
        }

        // header: product picker + readout
        let head = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(rect.width() - 20.0, 24.0));
        ui.scope_builder(UiBuilder::new().max_rect(head).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            let mut sel = m;
            egui::ComboBox::from_id_salt(("product", idx))
                .selected_text(egui::RichText::new(m.key()).color(t.accent).strong())
                .width(64.0)
                .show_ui(ui, |ui| {
                    for mm in Moment::ALL {
                        ui.selectable_value(&mut sel, mm, format!("{:<4} {}", mm.key(), mm.title()));
                    }
                });
            self.panes[idx] = sel;
            let elev = self.scan.as_ref().and_then(|s| s.vol.sweep_for(m, self.tilt)).map(|s| s.elevation);
            let sub = match elev {
                Some(e) => format!("{} · {e:.1}°", m.title()),
                None => m.title().to_string(),
            };
            ui.label(egui::RichText::new(sub).color(t.muted).small());
            if let Some(r) = readout {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(r).color(t.fg));
                });
            }
        });

        // legend
        let pal = self.cfg.palette(m);
        let lw = (rect.width() - 24.0).min(220.0);
        let lr = Rect::from_min_size(pos2(rect.min.x + 12.0, rect.max.y - 22.0), vec2(lw, 6.0));
        let (lo, hi) = (pal.min(), pal.max());
        let steps = 64;
        for i in 0..steps {
            let v = lo + (hi - lo) * (i as f32 + 0.5) / steps as f32;
            let x0 = lr.min.x + lr.width() * i as f32 / steps as f32;
            let x1 = lr.min.x + lr.width() * (i + 1) as f32 / steps as f32 + 0.5;
            painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, lr.y_range()), 0, pal.color(v));
        }
        let fmt = |v: f32| if m == Moment::Rho { format!("{v:.2}") } else { format!("{v:.0}") };
        let small = FontId::monospace(10.0);
        painter.text(lr.left_top() - vec2(0.0, 3.0), Align2::LEFT_BOTTOM, fmt(lo), small.clone(), t.muted);
        painter.text(lr.right_top() - vec2(0.0, 3.0), Align2::RIGHT_BOTTOM, format!("{} {}", fmt(hi), self.cfg.unit_label(m)), small, t.muted);

        painter.rect_stroke(rect, rad, Stroke::new(1.0, t.line), StrokeKind::Inside);
    }

    fn keys_input(&mut self, ui: &Ui) {
        if ui.ctx().memory(|m| m.focused().is_some()) {
            return;
        }
        let (l, r, u, d, live, home, n) = ui.input(|i| {
            let n = [Key::Num1, Key::Num2, Key::Num3, Key::Num4].iter().position(|k| i.key_pressed(*k));
            (
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::L),
                i.key_pressed(Key::R),
                n,
            )
        });
        if l {
            self.step(-1);
        }
        if r {
            self.step(1);
        }
        if u {
            self.shift_tilt(1);
        }
        if d {
            self.shift_tilt(-1);
        }
        if live {
            self.live = !self.live;
            if self.live {
                let s = self.site.clone();
                self.select_site(&s, None);
            }
        }
        if home {
            self.center = Vec2::ZERO;
            self.fitted = false;
        }
        if let Some(n) = n {
            self.set_pane_count(n + 1);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let now = ui_time(ui);
        let frame_start = web_time::Instant::now();
        let ctx = ui.ctx().clone();
        self.receive(now);
        #[cfg(not(target_arch = "wasm32"))]
        self.watch_config(&ctx, now);

        if ui.input(|i| i.pointer.any_pressed()) {
            self.player.unlock();
        }

        // periodic refresh
        if now - self.last_refresh > self.cfg.refresh_seconds {
            self.last_refresh = now;
            if self.live && self.loading.is_none() {
                self.listed.retain(|d| *d != today());
                self.list(today());
            }
        }
        if now - self.last_alerts > self.cfg.refresh_seconds {
            self.last_alerts = now;
            self.poll_alerts();
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(if now < self.alarm_until { 0 } else { 5 }));

        // first time we know the pane size, fit ~230 km around the radar
        if !self.fitted {
            let r = ui.available_rect_before_wrap();
            let w = r.width() / self.panes.len().clamp(1, 3) as f32;
            self.zoom = (w.min(r.height() - 50.0) / 320.0).max(0.3);
            self.fitted = true;
        }

        let t = self.cfg.theme.clone();
        self.keys_input(ui);

        egui::Panel::top("top")
            .exact_size(40.0)
            .frame(egui::Frame::new().fill(t.panel).stroke(Stroke::new(1.0, t.line)).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| {
                // scrolls sideways instead of overlapping when the window is narrow
                egui::ScrollArea::horizontal()
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.set_height(40.0);
                        self.top_bar(ui, now)
                    });
            });

        if self.show_settings {
            egui::Panel::right("settings")
                .default_size(480.0)
                .resizable(true)
                .frame(egui::Frame::new().fill(t.bg).stroke(Stroke::new(1.0, t.line)).inner_margin(egui::Margin::symmetric(12, 4)))
                .show(ui, |ui| self.settings_panel(ui));
        }
        if self.show_alerts {
            egui::Panel::right("alerts")
                .default_size(300.0)
                .resizable(true)
                .frame(egui::Frame::new().fill(t.bg).stroke(Stroke::new(1.0, t.line)).inner_margin(egui::Margin::symmetric(12, 4)))
                .show(ui, |ui| self.alerts_panel(ui));
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg).inner_margin(8)).show(ui, |ui| {
            let area = ui.available_rect_before_wrap();
            if !ui.rect_contains_pointer(area) {
                self.hover = None;
            }
            for (i, r) in self.pane_rects(area).into_iter().enumerate() {
                self.pane(ui, r, i);
            }
            self.error_toast(ui, area);
        });
        log::trace!("frame {:?}", frame_start.elapsed());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if cfg!(target_arch = "wasm32") && !self.editor_dirty {
            storage.set_string(STORE_KEY, self.editor.clone());
        }
    }
}
