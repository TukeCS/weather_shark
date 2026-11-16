//! App state, data flow and the frame loop. Drawing lives in the submodules:
//! `chrome` (top bar and side panels), `pane` (one radar pane), `mapdraw`
//! (roads, labels, warnings) and `layout` (pane tiling).

mod chrome;
mod layout;
mod mapdraw;
mod pane;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui::{
    Color32, CornerRadius, FontFamily, FontId, Key, Pos2, Rect, Stroke, TextureHandle, Ui, Vec2,
    pos2, vec2,
};

use crate::alerts::Alert;
use crate::config::{Config, MAX_PANES};
use crate::level2::{Moment, Volume};
use crate::map::{Basemap, Site, sites};
use crate::net::{self, Msg, Net};
use crate::radio::{RadioPlayer, Station};
use crate::sound::Player;
use crate::tiles::Tiles;

#[cfg(target_arch = "wasm32")]
const STORE_KEY: &str = "config_lua";

struct Scan {
    vol: Arc<Volume>,
    key: String,
    generation: u64,
}

/// A pane keeps a stable id so its rectangle can animate when the layout changes.
#[derive(Clone, Copy)]
struct Pane {
    id: u64,
    moment: Moment,
}

/// Map layers the user can switch on and off at runtime (defaults come from Lua).
#[derive(Clone, Copy)]
struct Toggles {
    roads: bool,
    road_labels: bool,
    cities: bool,
    counties: bool,
    rings: bool,
    sites: bool,
    warnings: bool,
}

impl Toggles {
    fn from_cfg(c: &Config) -> Self {
        Self {
            roads: c.map.roads,
            road_labels: c.map.road_labels,
            cities: c.map.cities,
            counties: c.map.counties.is_some(),
            rings: c.map.range_rings.is_some(),
            sites: true,
            warnings: c.map.warnings,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    None,
    Alerts,
    Radio,
    Settings,
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
    tiles: Tiles,
    /// Tiles the panes asked for this frame; fetched after drawing.
    want_tiles: Vec<crate::tiles::Key>,
    show: Toggles,

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
    panes: Vec<Pane>,
    next_pane_id: u64,
    /// Where a newly added pane grows out from (the pane it split).
    spawn: Option<(u64, Rect)>,
    last_rects: Vec<(u64, Rect)>,
    hovered_pane: Option<usize>,
    textures: HashMap<(u64, u8, Moment), (TextureHandle, f32)>,

    center: Vec2,
    zoom: f32,
    hover: Option<Pos2>,
    fitted: bool,
    /// Once the user pans or zooms, stop auto-fitting the view to the window.
    user_moved: bool,

    alerts: Vec<(Alert, usize)>,
    seen: HashSet<String>,
    alerts_once: bool,
    alerts_error: Option<String>,
    last_alerts: f64,
    alarm_until: f64,
    player: Player,

    stations: Vec<Station>,
    radio_error: Option<String>,
    radio_requested: f64,
    radio: RadioPlayer,
    playing: Option<String>,
    radio_search: String,

    side: Side,
    site_search: String,
    jump: String,
}

fn today() -> i64 {
    net::now().div_euclid(86400)
}

const PRODUCT_ORDER: [Moment; 6] = [
    Moment::Ref,
    Moment::Vel,
    Moment::Rho,
    Moment::Zdr,
    Moment::Sw,
    Moment::Phi,
];

impl App {
    /// `start` optionally opens a past scan: (site, "YYYY-MM-DD HH:MM").
    pub fn new(cc: &eframe::CreationContext<'_>, start: Option<(String, String)>) -> Self {
        let (tx, rx) = channel();
        let net = Net::new(tx, cc.egui_ctx.clone());

        // Native reads ~/.config/weather_shark/config.lua, web reads browser storage.
        #[cfg(not(target_arch = "wasm32"))]
        let src = crate::load_config_file();
        #[cfg(target_arch = "wasm32")]
        let src = cc
            .storage
            .and_then(|s| s.get_string(STORE_KEY))
            .unwrap_or_else(|| crate::config::DEFAULT_LUA.to_string());

        let (cfg, cfg_error) = match Config::from_lua(&src) {
            Ok(c) => (c, None),
            Err(e) => (Config::default_config(), Some(e)),
        };
        let sites = sites();
        let site = cfg.site.clone();
        let (lat, lon) = sites
            .iter()
            .find(|s| s.id == site)
            .map_or((35.33, -97.28), |s| (s.lat, s.lon));
        let panes: Vec<Pane> = cfg
            .panes
            .iter()
            .enumerate()
            .map(|(i, m)| Pane {
                id: i as u64,
                moment: *m,
            })
            .collect();

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
            tiles: Tiles::default(),
            want_tiles: Vec::new(),
            show: Toggles::from_cfg(&cfg),
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
            next_pane_id: panes.len() as u64,
            panes,
            spawn: None,
            last_rects: Vec::new(),
            hovered_pane: None,
            textures: HashMap::new(),
            center: Vec2::ZERO,
            zoom: 2.0,
            hover: None,
            fitted: false,
            user_moved: false,
            alerts: Vec::new(),
            seen: HashSet::new(),
            alerts_once: false,
            alerts_error: None,
            last_alerts: f64::NEG_INFINITY,
            alarm_until: 0.0,
            player: Player::new(),
            stations: Vec::new(),
            radio_error: None,
            radio_requested: f64::NEG_INFINITY,
            radio: RadioPlayer::new(),
            playing: None,
            radio_search: String::new(),
            side: Side::None,
            site_search: String::new(),
            jump: String::new(),
            cfg,
        };
        app.radio.set_volume(app.cfg.radio.volume);
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
                    self.panes = c
                        .panes
                        .iter()
                        .map(|m| Pane {
                            id: self.bump_id(),
                            moment: *m,
                        })
                        .collect();
                }
                if c.tilt != self.cfg.tilt {
                    self.tilt = c.tilt;
                }
                if c.map.tiles != self.cfg.map.tiles {
                    self.tiles = Tiles::default();
                }
                self.show = Toggles::from_cfg(&c);
                self.radio.set_volume(c.radio.volume);
                if !c.radio.enabled {
                    self.stop_radio();
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
        v.slider_trailing_fill = true;
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

    // ---- radar data -------------------------------------------------------------

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
            self.seen.clear();
            self.alerts_once = false;
            self.last_alerts = f64::NEG_INFINITY; // re-filter warnings around the new radar
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
        if self.loading.as_deref() == Some(key) || self.scan.as_ref().is_some_and(|s| s.key == key)
        {
            return;
        }
        self.loading = Some(key.to_string());
        self.net.volume(key);
    }

    fn pick_scan(&mut self) {
        if let Some(t) = self.target {
            let best = self
                .keys
                .iter()
                .rev()
                .find(|k| net::key_time(k).is_some_and(|kt| kt <= t))
                .cloned();
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
                        self.error = Some(format!(
                            "no scans for {} near {}",
                            self.site,
                            net::fmt_time(t)
                        ));
                        self.target = None;
                    }
                }
            }
        } else if self.live
            && let Some(k) = self.keys.last().cloned()
        {
            self.load(&k);
        }
    }

    fn step(&mut self, dir: i32) {
        let Some(cur) = self.scan.as_ref().map(|s| s.key.clone()) else {
            return;
        };
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

    fn go_live(&mut self) {
        self.live = true;
        let s = self.site.clone();
        self.select_site(&s, None);
    }

    // ---- alerts ------------------------------------------------------------------

    fn poll_alerts(&mut self) {
        if !self.cfg.alerts.enabled {
            self.alerts.clear();
            return;
        }
        let events: Vec<String> = self
            .cfg
            .alerts
            .rules
            .iter()
            .map(|r| r.event.clone())
            .collect();
        if !events.is_empty() {
            self.net.alerts(&events);
        }
    }

    fn update_alerts(&mut self, list: Vec<Alert>, now: f64) {
        self.alerts_error = None;
        let a = &self.cfg.alerts;
        let radius = a.radius_km;
        let proj = self.map.proj;
        let mut kept: Vec<(Alert, usize)> =
            list.into_iter()
                .filter_map(|mut al| {
                    let rule = a.rules.iter().position(|r| al.matches(r))?;
                    // Watches have no polygon: outline the counties they cover instead.
                    if al.polygons.is_empty() && !al.counties.is_empty() {
                        al.polygons = al
                            .counties
                            .iter()
                            .flat_map(|c| self.map.county(*c))
                            .collect();
                        al.county_based = true;
                    }
                    let near = radius <= 0.0
                        || al.polygons.iter().flatten().any(|&(lon, lat)| {
                            proj.fwd(lon, lat).to_vec2().length() as f64 <= radius
                        });
                    near.then_some((al, rule))
                })
                .collect();
        kept.sort_by_key(|(_, r)| *r);

        // Sound the most important new alert (rules earlier in the list win).
        let fresh = kept
            .iter()
            .filter(|(al, _)| !self.seen.contains(&al.id))
            .map(|(_, r)| *r)
            .min();
        if let Some(r) = fresh
            && (self.alerts_once || a.on_startup)
        {
            let rule = &a.rules[r];
            if rule.sound != "none" || rule.file.is_some() {
                self.player.play(
                    &rule.sound,
                    rule.file.as_deref(),
                    rule.repeats,
                    rule.volume * a.volume,
                );
            }
            self.alarm_until = now + 30.0;
        }
        self.seen.extend(kept.iter().map(|(al, _)| al.id.clone()));
        self.alerts_once = true;
        self.alerts = kept;
    }

    // ---- radio ------------------------------------------------------------------

    fn play_radio(&mut self, ctx: &egui::Context, st: &Station) {
        self.radio.play(&st.url, ctx.clone());
        self.playing = Some(st.call.clone());
    }

    fn stop_radio(&mut self) {
        self.radio.stop();
        self.playing = None;
    }

    /// Stations sorted by distance (km) from the current radar.
    fn stations_by_distance(&self) -> Vec<(f32, &Station)> {
        let mut v: Vec<(f32, &Station)> = self
            .stations
            .iter()
            .map(|s| (self.map.proj.fwd(s.lon, s.lat).to_vec2().length(), s))
            .collect();
        v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        v
    }

    // ---- messages -----------------------------------------------------------------

    fn receive(&mut self, now: f64) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Listing { site, keys } => {
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
                            self.scan = Some(Scan {
                                vol,
                                key,
                                generation: self.generation,
                            });
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Msg::Alerts(res) => match res {
                    Ok(list) => self.update_alerts(list, now),
                    Err(e) => self.alerts_error = Some(e),
                },
                Msg::TileJson(res) => match res {
                    Ok(t) => self.tiles.template = Some(t),
                    Err(e) => self.tiles.error = Some(e),
                },
                Msg::Tile { key, tile } => self.tiles.done(key, tile),
                Msg::Radio(res) => match res {
                    Ok(s) => {
                        self.stations = s;
                        self.radio_error = None;
                    }
                    Err(e) => self.radio_error = Some(e),
                },
            }
        }
    }

    // ---- view ---------------------------------------------------------------------

    fn to_screen(&self, rect: Rect, km: Pos2) -> Pos2 {
        let c = rect.center();
        pos2(
            c.x + (km.x - self.center.x) * self.zoom,
            c.y - (km.y - self.center.y) * self.zoom,
        )
    }

    fn to_km(&self, rect: Rect, p: Pos2) -> Pos2 {
        let c = rect.center();
        pos2(
            self.center.x + (p.x - c.x) / self.zoom,
            self.center.y - (p.y - c.y) / self.zoom,
        )
    }

    fn ll_to_screen(&self, rect: Rect, lon: f64, lat: f64) -> Pos2 {
        self.to_screen(rect, self.map.proj.fwd(lon, lat))
    }

    fn tilts(&self) -> Vec<f32> {
        self.scan
            .as_ref()
            .map(|s| s.vol.tilts())
            .unwrap_or_default()
    }

    fn shift_tilt(&mut self, d: i32) {
        let t = self.tilts();
        if t.is_empty() {
            return;
        }
        let cur = t
            .iter()
            .position(|x| (x - self.tilt).abs() < 0.15)
            .unwrap_or(0) as i32;
        self.tilt = t[(cur + d).clamp(0, t.len() as i32 - 1) as usize];
    }

    // ---- panes --------------------------------------------------------------------

    fn bump_id(&mut self) -> u64 {
        self.next_pane_id += 1;
        self.next_pane_id
    }

    fn add_pane(&mut self) {
        if self.panes.len() >= MAX_PANES {
            return;
        }
        let moment = PRODUCT_ORDER
            .iter()
            .find(|m| !self.panes.iter().any(|p| p.moment == **m))
            .copied()
            .unwrap_or(Moment::Ref);
        let id = self.bump_id();
        // The new pane grows out of the one it splits (the current last pane).
        self.spawn = self
            .panes
            .last()
            .and_then(|p| self.last_rects.iter().find(|(i, _)| *i == p.id))
            .map(|(_, r)| (id, *r));
        self.panes.push(Pane { id, moment });
    }

    fn close_pane(&mut self, idx: usize) {
        if self.panes.len() > 1 && idx < self.panes.len() {
            self.panes.remove(idx);
        }
    }

    /// Animate a pane towards its slot in the layout.
    fn animated_rect(&mut self, ctx: &egui::Context, id: u64, target: Rect) -> Rect {
        let base = egui::Id::NULL.with(("pane-rect", id));
        if let Some((sid, from)) = self.spawn
            && sid == id
        {
            // Seed the animation with the rect it splits from.
            for (i, v) in [from.min.x, from.min.y, from.max.x, from.max.y]
                .into_iter()
                .enumerate()
            {
                ctx.animate_value_with_time(base.with(i), v, 0.0);
            }
            self.spawn = None;
        }
        let t = 0.22;
        let a = |i: usize, v: f32| ctx.animate_value_with_time(base.with(i), v, t);
        Rect::from_min_max(
            pos2(a(0, target.min.x), a(1, target.min.y)),
            pos2(a(2, target.max.x), a(3, target.max.y)),
        )
    }

    fn keys_input(&mut self, ui: &Ui) {
        if ui.ctx().memory(|m| m.focused().is_some()) {
            return;
        }
        let pressed = |k: Key| ui.input(|i| i.key_pressed(k));
        if pressed(Key::ArrowLeft) {
            self.step(-1);
        }
        if pressed(Key::ArrowRight) {
            self.step(1);
        }
        if pressed(Key::ArrowUp) {
            self.shift_tilt(1);
        }
        if pressed(Key::ArrowDown) {
            self.shift_tilt(-1);
        }
        if pressed(Key::L) {
            if self.live {
                self.live = false;
            } else {
                self.go_live();
            }
        }
        if pressed(Key::R) {
            self.center = Vec2::ZERO;
            self.fitted = false;
            self.user_moved = false;
        }
        if pressed(Key::N) || pressed(Key::Plus) || pressed(Key::Equals) {
            self.add_pane();
        }
        if pressed(Key::X)
            && let Some(i) = self.hovered_pane
        {
            self.close_pane(i);
        }
        if pressed(Key::C) {
            self.show.cities = !self.show.cities;
        }
        if pressed(Key::M) {
            self.show.roads = !self.show.roads;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let now = ui.input(|i| i.time);
        let ctx = ui.ctx().clone();
        self.receive(now);
        #[cfg(not(target_arch = "wasm32"))]
        self.watch_config(&ctx, now);
        self.tiles.next_frame();

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
        if !self.tiles.json_requested && (self.show.roads || self.show.cities) {
            self.tiles.json_requested = true;
            self.net.tilejson(&self.cfg.map.tiles);
        }
        let repaint = if now < self.alarm_until { 0 } else { 5 };
        ctx.request_repaint_after(std::time::Duration::from_secs(repaint));

        // fit ~160 km around the radar until the user moves the map (the window
        // may still be settling into its size on the first frames)
        if !self.fitted || !self.user_moved {
            let r = ui.available_rect_before_wrap();
            let rects = layout::rects(r, self.panes.len(), &self.cfg.layout);
            let w = rects
                .iter()
                .map(|r| r.width().min(r.height()))
                .fold(f32::INFINITY, f32::min);
            self.zoom = (w / 320.0).max(0.3);
            self.fitted = true;
        }

        let t = self.cfg.theme.clone();
        self.keys_input(ui);

        egui::Panel::top("top")
            .exact_size(40.0)
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .stroke(Stroke::new(1.0, t.line))
                    .inner_margin(egui::Margin::symmetric(12, 0)),
            )
            .show(ui, |ui| {
                // scrolls sideways instead of overlapping when the window is narrow
                egui::ScrollArea::horizontal()
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.set_height(40.0);
                        self.top_bar(ui, now)
                    });
            });

        if self.side != Side::None {
            let width = if self.side == Side::Settings {
                480.0
            } else {
                320.0
            };
            egui::Panel::right("side")
                .default_size(width)
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(t.bg)
                        .stroke(Stroke::new(1.0, t.line))
                        .inner_margin(egui::Margin::symmetric(12, 4)),
                )
                .show(ui, |ui| match self.side {
                    Side::Settings => self.settings_panel(ui),
                    Side::Alerts => self.alerts_panel(ui),
                    Side::Radio => self.radio_panel(ui, now),
                    Side::None => {}
                });
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(8))
            .show(ui, |ui| {
                let area = ui.available_rect_before_wrap();
                if !ui.rect_contains_pointer(area) {
                    self.hover = None;
                }
                self.hovered_pane = None;
                let targets = layout::rects(area, self.panes.len(), &self.cfg.layout);
                let mut rects = Vec::new();
                for (i, target) in targets.into_iter().enumerate() {
                    let id = self.panes[i].id;
                    let r = self.animated_rect(&ctx, id, target);
                    rects.push((id, r));
                    self.pane(ui, r, i);
                }
                self.last_rects = rects;
                self.request_tiles();
                self.error_toast(ui, area);
            });
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        #[cfg(target_arch = "wasm32")]
        if !self.editor_dirty {
            _storage.set_string(STORE_KEY, self.editor.clone());
        }
    }
}

/// Small helper for muted section labels like "ACTIVE WARNINGS".
fn heading(ui: &mut Ui, text: &str, muted: Color32) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text).color(muted).small());
}
