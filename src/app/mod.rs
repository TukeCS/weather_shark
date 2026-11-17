//! App state, data flow and the frame loop. Drawing lives in the submodules:
//! `chrome` (top bar and side panels), `pane` (one radar pane), `mapdraw`
//! (roads, labels, warnings) and `layout` (pane tiling).

mod chrome;
mod layout;
mod luahl;
mod mapdraw;
mod pane;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui::{
    Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Stroke, TextureHandle, Ui, Vec2, pos2,
    vec2,
};

use crate::alerts::Alert;
use crate::config::{Action, Config, MAX_PANES};
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
    watches: bool,
    /// SPC outlook on the map
    outlook: Option<crate::spc::Sel>,
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
            watches: c.map.watches,
            outlook: c.outlook.show,
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

/// `weather_shark screenshot`: render once everything has loaded, save, quit.
pub struct ShotOpts {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub path: std::path::PathBuf,
    pub panes: Option<Vec<Moment>>,
    pub view_km: Option<f32>,
    pub layout: Option<crate::config::LayoutMode>,
    /// centre the view on (lat, lon)
    pub at: Option<(f64, f64)>,
    /// open a side panel: "lua", "alerts" or "radio"
    pub panel: Option<String>,
    /// show a simulated tornado warning
    pub simulate: bool,
}

pub struct App {
    shot: Option<ShotOpts>,
    shot_ready_frames: u32,
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
    /// bytes downloaded / total for the scan being loaded
    progress: Option<(usize, Option<usize>)>,
    /// last few fully loaded scans
    cache: Vec<(String, Arc<Volume>)>,
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
    /// split ratios from dragging the borders between panes (NaN = the default)
    split_ratios: Vec<f32>,
    /// (pane count, layout) the ratios were dragged in
    split_shape: Option<(usize, crate::config::LayoutMode)>,
    /// a divider is being dragged: panes follow it without animating
    resizing: bool,
    hovered_pane: Option<usize>,
    /// radar images by (scan generation, sweep number, product, previous volume's)
    textures: HashMap<(u64, u8, Moment, bool), (TextureHandle, f32)>,

    center: Vec2,
    zoom: f32,
    hover: Option<Pos2>,
    fitted: bool,
    /// Once the user pans or zooms, stop auto-fitting the view to the window.
    user_moved: bool,

    /// real-time volume coming in from the chunks bucket
    feed: Option<crate::chunks::Stream>,
    /// site whose live chunk folder is being looked up
    feed_finding: Option<String>,
    last_feed_poll: f64,
    feed_polls: u32,
    /// new chunks decoded this frame: rebuild the picture once, after the messages
    feed_dirty: bool,
    /// tilts that just finished, being swept in over the previous scan
    reveals: Vec<Reveal>,

    alerts: Vec<(Alert, usize)>,
    seen: HashSet<String>,
    alerts_once: bool,
    alerts_error: Option<String>,
    last_alerts: f64,
    alarm_until: f64,
    /// test warnings from the "simulate" button, with their expiry time
    simulated: Vec<(Alert, f64)>,
    /// warnings the user hid from the map (they stay listed under HIDDEN)
    hidden: HashSet<String>,
    /// alert events switched off in the map menu (kept across config reloads)
    off_events: HashSet<String>,
    player: Player,

    outlook: Option<crate::spc::Outlook>,
    outlook_error: Option<String>,
    /// what was last requested, and when
    outlook_asked: Option<(crate::spc::Sel, f64)>,
    /// what the outlook key brings back after turning it off
    outlook_last: crate::spc::Sel,

    stations: Vec<Station>,
    radio_error: Option<String>,
    radio_requested: f64,
    radio: RadioPlayer,
    playing: Option<String>,
    radio_search: String,
    /// play the nearest station once the list arrives (radio key pressed before it loaded)
    radio_autoplay: bool,

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
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        start: Option<(String, String)>,
        shot: Option<ShotOpts>,
    ) -> Self {
        // Layout comes from config.lua, not from egui's saved state (an old saved
        // panel width would otherwise override it on every start).
        cc.egui_ctx.memory_mut(|m| *m = egui::Memory::default());
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

        let (mut cfg, cfg_error) = match Config::from_lua(&src) {
            Ok(c) => (c, None),
            Err(e) => (Config::default_config(), Some(e)),
        };
        if let Some(s) = &shot {
            if let Some(p) = &s.panes {
                cfg.panes = p.clone();
            }
            if let Some(v) = s.view_km {
                cfg.view_km = v;
            }
            if let Some(l) = s.layout {
                cfg.layout.mode = l;
            }
        }
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
            shot,
            shot_ready_frames: 0,
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
            progress: None,
            cache: Vec::new(),
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
            split_ratios: Vec::new(),
            split_shape: None,
            resizing: false,
            hovered_pane: None,
            textures: HashMap::new(),
            center: Vec2::ZERO,
            zoom: 2.0,
            hover: None,
            fitted: false,
            user_moved: false,
            feed: None,
            feed_finding: None,
            last_feed_poll: f64::NEG_INFINITY,
            feed_polls: 0,
            feed_dirty: false,
            reveals: Vec::new(),
            alerts: Vec::new(),
            seen: HashSet::new(),
            alerts_once: false,
            alerts_error: None,
            last_alerts: f64::NEG_INFINITY,
            alarm_until: 0.0,
            simulated: Vec::new(),
            hidden: HashSet::new(),
            off_events: HashSet::new(),
            outlook: None,
            outlook_error: None,
            outlook_asked: None,
            outlook_last: cfg.outlook.show.unwrap_or(crate::spc::Sel {
                day: 1,
                kind: crate::spc::Kind::Categorical,
            }),
            player: Player::new(),
            stations: Vec::new(),
            radio_error: None,
            radio_requested: f64::NEG_INFINITY,
            radio: RadioPlayer::new(),
            playing: None,
            radio_search: String::new(),
            radio_autoplay: false,
            side: Side::None,
            site_search: String::new(),
            jump: String::new(),
            cfg,
        };
        app.radio.set_volume(app.cfg.radio.volume);
        app.apply_style(&cc.egui_ctx);
        // `weather_shark FILE` opens a local radar file instead
        #[cfg(not(target_arch = "wasm32"))]
        let start = match start {
            Some((s, _)) if std::path::Path::new(&s).is_file() => {
                app.select_site(&site, None);
                app.live = false;
                app.net.open_path(s.into());
                None
            }
            other => other,
        };
        match start.and_then(|(s, t)| Some((s.to_uppercase(), net::parse_time(&t)?))) {
            Some((s, t)) => app.select_site(&s, Some(t + 60)),
            None if app.scan.is_none() && app.live => app.select_site(&site, None),
            None => {}
        }
        if let Some((lat, lon)) = app.shot.as_ref().and_then(|s| s.at) {
            app.center = app.map.proj.fwd(lon, lat).to_vec2();
        }
        app.side = match app.shot.as_ref().and_then(|s| s.panel.as_deref()) {
            Some("lua") => Side::Settings,
            Some("alerts") => Side::Alerts,
            Some("radio") => Side::Radio,
            _ => app.side,
        };
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
        self.progress = None;
        self.error = None;
        self.target = target;
        self.feed = None;
        self.feed_finding = None;
        match target {
            Some(t) => {
                self.live = false;
                self.list(t.div_euclid(86400));
            }
            None => {
                self.list(today());
                self.list(today() - 1);
                if self.cfg.realtime {
                    self.feed_finding = Some(self.site.clone());
                    self.net.chunk_find(&self.site);
                }
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
        // recently viewed scans are kept, so stepping back and forth is instant
        if let Some(vol) = self
            .cache
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
        {
            self.loading = None;
            self.progress = None;
            self.show_scan(key.to_string(), vol);
            return;
        }
        self.loading = Some(key.to_string());
        self.progress = Some((0, None));
        self.net.volume(key);
    }

    /// Show a scan (or a newer snapshot of the one on screen). Sweeps never change
    /// once decoded, so images already made for this scan are kept.
    fn show_scan(&mut self, key: String, vol: Arc<Volume>) {
        match &mut self.scan {
            Some(s) if s.key == key => s.vol = vol,
            _ => {
                self.generation += 1;
                self.textures.clear();
                self.scan = Some(Scan {
                    vol,
                    key,
                    generation: self.generation,
                });
            }
        }
    }

    /// 0..1 while a scan downloads (None if the size isn't known yet).
    fn load_fraction(&self) -> Option<f32> {
        match self.progress {
            Some((got, Some(total))) if total > 0 => {
                Some((got as f32 / total as f32).clamp(0.0, 1.0))
            }
            _ => None,
        }
    }

    fn progress_text(&self) -> String {
        let mb = |b: usize| b as f32 / 1_048_576.0;
        match self.progress {
            Some((got, Some(total))) => format!("{:.1} / {:.1} MB", mb(got), mb(total)),
            Some((got, None)) => format!("{:.1} MB", mb(got)),
            None => String::new(),
        }
    }

    fn cache_put(&mut self, key: &str, vol: Arc<Volume>) {
        self.cache.retain(|(k, _)| k != key);
        self.cache.push((key.to_string(), vol));
        if self.cache.len() > 6 {
            self.cache.remove(0);
        }
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
            && !net::key_time(&k).is_some_and(|t| self.live_newer(t))
        {
            self.load(&k);
        }
    }

    /// The live feed is on screen with data at least as new as `t`.
    fn live_newer(&self, t: i64) -> bool {
        self.scan
            .as_ref()
            .is_some_and(|s| s.key.starts_with("live:") && s.vol.time >= t)
    }

    /// A tilt being revealed on a pane showing `number`: (start azimuth, how far
    /// round 0..1), while live data is on screen.
    fn reveal(&self, number: u8, now: f64) -> Option<(f32, f32)> {
        if !self.cfg.sweep_line || !self.live {
            return None;
        }
        self.scan.as_ref().filter(|s| s.key.starts_with("live:"))?;
        let r = self.reveals.iter().find(|r| r.number == number)?;
        let k = ((now - r.start) / r.dur).clamp(0.0, 1.0) as f32;
        (k < 1.0).then_some((r.from_az, k))
    }

    // ---- real-time chunks ---------------------------------------------------------

    /// The newest complete archive scan of this radar, to fill in what the live
    /// volume hasn't scanned yet.
    fn archive_base(&self) -> Option<Arc<Volume>> {
        self.scan
            .as_ref()
            .filter(|s| !s.key.starts_with("live:") && s.vol.site == self.site)
            .map(|s| s.vol.clone())
            .or_else(|| {
                self.cache
                    .iter()
                    .filter(|(_, v)| v.site == self.site)
                    .max_by_key(|(_, v)| v.time)
                    .map(|(_, v)| v.clone())
            })
    }

    /// Look for new chunks every few seconds (and for the next volume).
    fn poll_feed(&mut self, now: f64, force: bool) {
        let Some(f) = &self.feed else { return };
        if !self.live || (!force && now - self.last_feed_poll < 2.5) {
            return;
        }
        self.last_feed_poll = now;
        self.feed_polls += 1;
        if !f.ended {
            self.net.chunk_list(&f.site, f.num, &f.last_key);
        }
        // the next folder too, in case this volume's last chunk never shows up
        if f.ended || self.feed_polls.is_multiple_of(6) {
            let (n, after) = f.next_after();
            self.net.chunk_list(&f.site, n, &after);
        }
    }

    fn feed_listed(&mut self, site: String, num: u16, keys: Result<Vec<String>, String>) {
        use crate::chunks::{Stream, next_num, parse_key};
        let Some(f) = self.feed.as_mut().filter(|f| f.site == site) else {
            return;
        };
        let keys = match keys {
            Ok(k) => k,
            Err(e) => {
                log::warn!("{e}");
                return;
            }
        };
        let get = if num == f.num {
            f.listed(&keys)
        } else if num == next_num(f.num) {
            // the radar started a new volume: what we have becomes its background
            let Some(stamp) = keys
                .iter()
                .filter_map(|k| parse_key(k))
                .map(|c| c.stamp)
                .filter(|s| *s > f.stamp)
                .min()
            else {
                return;
            };
            let base = f.composite().map(Arc::new).or_else(|| f.base.clone());
            let mut nf = Stream::new(&site, num, &stamp, base);
            let get = nf.listed(&keys);
            *f = nf;
            get
        } else {
            return;
        };
        for k in get {
            self.net.chunk(&k);
        }
    }

    fn feed_chunk(&mut self, key: String, bytes: Result<Vec<u8>, String>) {
        // (the picture is rebuilt once per frame in show_feed)
        let Some(f) = self.feed.as_mut() else { return };
        let changed = match bytes {
            Ok(b) => f.arrived(&key, b),
            Err(e) => {
                log::warn!("{key}: {e}");
                f.failed(&key);
                false
            }
        };
        if let Some(e) = f.error.take() {
            log::warn!("live data {}: {e}", f.key());
        }
        if changed && self.live {
            self.feed_dirty = true;
        }
    }

    /// Put the live volume on screen. Only the images of sweeps that changed are redrawn.
    fn show_feed(&mut self, now: f64) {
        let Some(f) = self.feed.as_mut() else { return };
        let dirty = std::mem::take(&mut f.dirty);
        let Some(vol) = f.composite() else { return };
        // never step back to something older than the archive scan on screen
        if self
            .scan
            .as_ref()
            .is_some_and(|s| !s.key.starts_with("live:") && s.vol.time > vol.time)
        {
            return;
        }
        let key = f.key();
        let new_volume = self.scan.as_ref().is_none_or(|s| s.key != key);
        // each tilt that just finished turns in once, at the speed the radar scanned it
        for s in vol.sweeps.iter().filter(|s| dirty.contains(&s.number)) {
            if let Some((from_az, took)) = s.scan_path() {
                self.reveals.retain(|r| r.number != s.number);
                self.reveals.push(Reveal {
                    number: s.number,
                    from_az,
                    start: now,
                    dur: took.clamp(2.0, 12.0),
                });
            }
        }
        self.show_scan(key, Arc::new(vol));
        if !new_volume {
            // the old images (under a reveal) stay; the finished tilts are redrawn
            self.textures
                .retain(|(_, n, _, old), _| *old || !dirty.contains(n));
        }
    }

    fn step(&mut self, dir: i32) {
        let Some(cur) = self.scan.as_ref().map(|s| s.key.clone()) else {
            return;
        };
        self.live = false;
        // the live volume comes after every archived one
        let pos = if cur.starts_with("live:") {
            self.keys.len()
        } else {
            self.keys.iter().position(|k| *k == cur).unwrap_or(0)
        };
        let i = pos as i32 + dir;
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

    /// Show a radar file the user opened, centred on its radar.
    fn show_local(&mut self, name: String, vol: Arc<Volume>) {
        self.live = false;
        self.feed = None;
        self.feed_finding = None;
        self.target = None;
        self.loading = None;
        self.progress = None;
        self.keys.clear();
        self.listed.clear();
        self.error = None;
        if vol.lat != 0.0 {
            self.map.recenter(vol.lat, vol.lon);
        } else if let Some(s) = self.sites.iter().find(|s| s.id == vol.site) {
            self.map.recenter(s.lat, s.lon);
        }
        if vol.site != self.site {
            self.center = Vec2::ZERO;
            self.last_alerts = f64::NEG_INFINITY;
        }
        self.site = vol.site.clone();
        log::info!("opened {name} ({} sweeps)", vol.sweeps.len());
        self.show_scan(format!("file:{name}"), vol);
    }

    /// Files dropped on the window open like any other scan.
    fn handle_drops(&mut self, ui: &Ui) {
        let dropped = ui.input(|i| i.raw.dropped_files.clone());
        let Some(f) = dropped.into_iter().next() else {
            return;
        };
        #[cfg(not(target_arch = "wasm32"))]
        self.net.open_path(f.path().to_path_buf());
        #[cfg(target_arch = "wasm32")]
        {
            // browsers only read files asynchronously
            let net = self.net.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let name = f.path().to_string_lossy().into_owned();
                match f.bytes_async().await {
                    Ok(b) => net.open_bytes(name, b),
                    Err(e) => net.send(Msg::Local { name, vol: Err(e) }),
                }
            });
        }
    }

    fn go_live(&mut self) {
        self.live = true;
        let s = self.site.clone();
        self.select_site(&s, None);
    }

    // ---- SPC outlook ---------------------------------------------------------------

    /// Fetch the selected outlook when it changes, and every 10 minutes (SPC updates
    /// day 1 five times a day).
    fn poll_outlook(&mut self, now: f64) {
        let Some(sel) = self.show.outlook else {
            return;
        };
        self.outlook_last = sel;
        let stale = match self.outlook_asked {
            Some((s, t)) => s != sel || now - t > 600.0,
            None => true,
        };
        if stale {
            if self.outlook.as_ref().is_some_and(|o| o.sel != sel) {
                self.outlook = None;
            }
            self.outlook_error = None;
            self.outlook_asked = Some((sel, now));
            self.net.outlook(sel);
        }
    }

    /// The outlook to draw: loaded, switched on, and in effect at the shown scan's time.
    fn outlook_shown(&self) -> Option<&crate::spc::Outlook> {
        let o = self.outlook.as_ref()?;
        let on = self.show.outlook == Some(o.sel);
        let t = self.scan.as_ref().map_or(net::now(), |s| s.vol.time);
        (on && o.covers(t)).then_some(o)
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

    fn update_alerts(&mut self, mut list: Vec<Alert>, now: f64) {
        self.alerts_error = None;
        // a simulated warning (alerts panel) stays until it expires
        self.simulated.retain(|(_, until)| *until > now);
        list.extend(self.simulated.iter().map(|(al, _)| al.clone()));
        let a = &self.cfg.alerts;
        let kept = crate::alerts::select(list, &a.rules, a.radius_km, &self.map.proj, |fips| {
            self.map.county(fips)
        });

        // Sound the most important new alert near the radar (rules earlier in the list win).
        let fresh = kept
            .iter()
            .filter(|(al, r)| {
                al.near
                    && !self.seen.contains(&al.id)
                    && !self.off_events.contains(&a.rules[*r].event)
            })
            .map(|(_, r)| *r)
            .min();
        if let Some(r) = fresh
            && (self.alerts_once || a.on_startup)
        {
            let rule = &a.rules[r];
            if a.sounds && (rule.sound != "none" || rule.file.is_some()) {
                self.player.play(
                    &rule.sound,
                    rule.file.as_deref(),
                    rule.repeats,
                    rule.volume * a.volume,
                );
            }
            self.alarm_until = now + 30.0;
        }
        // forget hidden warnings once the NWS has cancelled / expired them
        self.hidden
            .retain(|id| kept.iter().any(|(al, _)| &al.id == id));
        self.seen.extend(kept.iter().map(|(al, _)| al.id.clone()));
        self.alerts_once = true;
        self.alerts = kept;
    }

    /// Remove simulated warnings (all, or just one by id) and stop their sound.
    fn clear_simulated(&mut self, id: Option<&str>) {
        let gone = |a: &Alert| a.id.starts_with("simulated-") && id.is_none_or(|i| a.id == i);
        self.simulated.retain(|(a, _)| !gone(a));
        self.hidden
            .retain(|h| !(h.starts_with("simulated-") && id.is_none_or(|i| h == i)));
        self.alerts.retain(|(a, _)| !gone(a));
        if !self
            .alerts
            .iter()
            .any(|(a, _)| a.id.starts_with("simulated-"))
        {
            self.player.stop();
            self.alarm_until = 0.0;
        }
    }

    /// Put a fake confirmed tornado warning just west of the map centre so you can
    /// see (and hear) what your alert rules look like. It goes away after 3 minutes.
    fn simulate_warning(&mut self, now: f64) {
        let c = self.center;
        // a typical warning box: ~30 km long, angled SW→NE ahead of the storm
        let corners = [
            (-18.0, -6.0),
            (-14.0, 4.0),
            (10.0, 14.0),
            (14.0, 2.0),
            (-18.0, -6.0),
        ];
        let ring: Vec<(f64, f64)> = corners
            .iter()
            .map(|(x, y)| self.map.proj.inv(pos2(c.x + x - 12.0, c.y + y)))
            .collect();
        let al = Alert {
            id: format!("simulated-{now}"),
            event: "Tornado Warning".into(),
            headline: "SIMULATED: a test warning from weather_shark, not a real alert".into(),
            area: "Simulated County (test)".into(),
            expires: String::new(),
            tornado: Some("OBSERVED".into()),
            damage: None,
            hail: None,
            polygons: vec![ring],
            counties: Vec::new(),
            county_based: false,
            near: true,
        };
        self.simulated.push((al, now + 180.0));
        self.alerts_once = true; // so it sounds even right after startup
        let real: Vec<Alert> = self
            .alerts
            .iter()
            .filter(|(a, _)| !a.id.starts_with("simulated-"))
            .map(|(a, _)| a.clone())
            .collect();
        self.update_alerts(real, now);
    }

    /// Whether an alert's event type is switched on in the map menu.
    fn event_on(&self, rule: usize) -> bool {
        !self.off_events.contains(&self.cfg.alerts.rules[rule].event)
    }

    /// Alerts for the list and the toolbar count: near the radar, switched on, list = true.
    fn listed_alerts(&self) -> impl Iterator<Item = (usize, &Alert)> {
        self.alerts.iter().enumerate().filter_map(|(i, (a, r))| {
            (a.near && self.event_on(*r) && self.cfg.alerts.rules[*r].list).then_some((i, a))
        })
    }

    /// Real tornado warnings in effect anywhere in the country.
    fn tornado_total(&self) -> usize {
        self.alerts
            .iter()
            .filter(|(a, _)| is_real_tornado(a))
            .count()
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
                Msg::Volume { key, vol, done } => {
                    if self.loading.as_deref() != Some(key.as_str()) {
                        continue;
                    }
                    if done {
                        self.loading = None;
                        self.progress = None;
                    }
                    match vol {
                        Ok(vol) => {
                            self.error = None;
                            if done {
                                log::info!("loaded {key} ({} sweeps)", vol.sweeps.len());
                                self.cache_put(&key, vol.clone());
                                // the live feed's first volume uses it for the tilts it hasn't reached
                                if let Some(f) = self.feed.as_mut()
                                    && f.base.is_none()
                                    && f.site == vol.site
                                {
                                    f.base = Some(vol.clone());
                                    // the tilts it fills in change
                                    self.textures.clear();
                                    self.feed_dirty = true;
                                }
                            } else {
                                log::debug!("early view of {key}: {} sweeps", vol.sweeps.len());
                            }
                            if !self.live_newer(vol.time) {
                                self.show_scan(key, vol);
                            }
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Msg::Progress { key, got, total } => {
                    if self.loading.as_deref() == Some(key.as_str()) {
                        self.progress = Some((got, total));
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
                Msg::Outlook(res) => match res {
                    // a reply for a selection the user has since changed is dropped
                    Ok(o) if self.outlook_asked.is_some_and(|(s, _)| s == o.sel) => {
                        self.outlook_error = None;
                        self.outlook = Some(o);
                    }
                    Ok(_) => {}
                    Err(e) => self.outlook_error = Some(e),
                },
                Msg::Tile { key, tile } => self.tiles.done(key, tile),
                Msg::ChunkFound { site, found } => {
                    if self.feed_finding.as_deref() != Some(site.as_str()) {
                        continue;
                    }
                    self.feed_finding = None;
                    match found {
                        Ok((num, stamp)) => {
                            log::info!("live data: {site} volume {num} ({stamp})");
                            let base = self.archive_base();
                            self.feed = Some(crate::chunks::Stream::new(&site, num, &stamp, base));
                            self.poll_feed(now, true);
                        }
                        // the archive keeps working without it
                        Err(e) => log::warn!("live data for {site}: {e}"),
                    }
                }
                Msg::ChunkList { site, num, keys } => self.feed_listed(site, num, keys),
                Msg::Chunk { key, bytes } => self.feed_chunk(key, bytes),
                Msg::Local { name, vol } => match vol {
                    Ok(vol) => self.show_local(name, vol),
                    Err(e) => self.error = Some(e),
                },
                Msg::Radio(res) => match res {
                    Ok(s) => {
                        self.stations = s;
                        self.radio_error = None;
                        if std::mem::take(&mut self.radio_autoplay)
                            && let Some(st) = self
                                .stations_by_distance()
                                .first()
                                .map(|(_, s)| (*s).clone())
                        {
                            let ctx = self.net.ctx();
                            self.play_radio(&ctx, &st);
                        }
                    }
                    Err(e) => self.radio_error = Some(e),
                },
            }
        }
        if std::mem::take(&mut self.feed_dirty) {
            self.show_feed(now);
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

    /// Drag the gaps between panes to resize them; double-click one to reset it.
    fn dividers(&mut self, ui: &mut Ui, dividers: &[layout::Divider]) {
        let mut resizing = false;
        for d in dividers {
            let id = egui::Id::NULL.with(("divider", d.idx));
            let resp = ui.interact(d.strip, id, egui::Sense::click_and_drag());
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(if d.vertical {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::ResizeVertical
                });
                let c = d.strip.center();
                let line = if d.vertical {
                    [pos2(c.x, d.strip.min.y), pos2(c.x, d.strip.max.y)]
                } else {
                    [pos2(d.strip.min.x, c.y), pos2(d.strip.max.x, c.y)]
                };
                ui.painter()
                    .line_segment(line, Stroke::new(2.0, self.cfg.theme.accent));
            }
            if resp.double_clicked() {
                if let Some(r) = self.split_ratios.get_mut(d.idx) {
                    *r = f32::NAN; // back to the default
                }
            } else if resp.dragged()
                && let Some(p) = resp.interact_pointer_pos()
            {
                resizing = true;
                if self.split_ratios.len() <= d.idx {
                    self.split_ratios.resize(d.idx + 1, f32::NAN);
                }
                self.split_ratios[d.idx] = d.ratio_at(p, self.cfg.layout.gap);
            }
        }
        self.resizing = resizing;
    }

    /// Animate a pane towards its slot in the layout.
    fn animated_rect(&mut self, ctx: &egui::Context, id: u64, target: Rect) -> Rect {
        // follow the pointer exactly while a divider is dragged
        let t = if self.resizing { 0.0 } else { 0.22 };
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
        let a = |i: usize, v: f32| ctx.animate_value_with_time(base.with(i), v, t);
        Rect::from_min_max(
            pos2(a(0, target.min.x), a(1, target.min.y)),
            pos2(a(2, target.max.x), a(3, target.max.y)),
        )
    }

    /// Screenshot mode: once the scan, map tiles and images are in and the
    /// layout has stopped animating, grab the frame, save it and quit.
    fn take_screenshot_when_ready(&mut self, ctx: &egui::Context) {
        let Some(shot) = &self.shot else { return };
        ctx.request_repaint();
        let ready = self.scan.is_some()
            && self.loading.is_none()
            && self.tiles.template.is_some()
            && self.tiles.inflight() == 0
            && !self.textures.is_empty()
            // live: wait for the real-time data once it's been found
            && self.feed_finding.is_none()
            && self
                .feed
                .as_ref()
                .is_none_or(|f| self.live_newer(0) && f.caught_up());
        self.shot_ready_frames = if ready { self.shot_ready_frames + 1 } else { 0 };
        if self.shot_ready_frames == 1 && shot.simulate && self.simulated.is_empty() {
            let now = ctx.input(|i| i.time);
            self.simulate_warning(now);
        }
        let Some(shot) = &self.shot else { return };
        if self.shot_ready_frames != 45 {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = shot.path.clone();
            let ctx2 = ctx.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                egui::ScreenshotCallback::new(move |img| {
                    match crate::png::write(&path, &img) {
                        Ok(()) => {
                            println!("saved {} ({}x{})", path.display(), img.size[0], img.size[1])
                        }
                        Err(e) => eprintln!("couldn't save {}: {e}", path.display()),
                    }
                    ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
                }),
            ));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = shot;
    }

    /// Key bindings come from `keys = { ... }` in the Lua config.
    fn keys_input(&mut self, ui: &Ui) {
        if ui.ctx().memory(|m| m.focused().is_some()) {
            return;
        }
        let actions: Vec<Action> = self
            .cfg
            .keys
            .iter()
            .filter(|(_, k)| ui.input(|i| i.key_pressed(*k) && !i.modifiers.command))
            .map(|(a, _)| *a)
            .collect();
        for a in actions {
            match a {
                Action::PrevScan => self.step(-1),
                Action::NextScan => self.step(1),
                Action::TiltUp => self.shift_tilt(1),
                Action::TiltDown => self.shift_tilt(-1),
                Action::Live => {
                    if self.live {
                        self.live = false;
                    } else {
                        self.go_live();
                    }
                }
                Action::Recenter => {
                    self.center = Vec2::ZERO;
                    self.fitted = false;
                    self.user_moved = false;
                }
                Action::AddPane => self.add_pane(),
                Action::ClosePane => {
                    if let Some(i) = self.hovered_pane {
                        self.close_pane(i);
                    }
                }
                Action::Cities => self.show.cities = !self.show.cities,
                Action::Roads => self.show.roads = !self.show.roads,
                Action::Warnings => self.show.warnings = !self.show.warnings,
                Action::Watches => self.show.watches = !self.show.watches,
                Action::Outlook => {
                    self.show.outlook = match self.show.outlook {
                        Some(_) => None,
                        None => Some(self.outlook_last),
                    }
                }
                Action::Settings => {
                    self.side = if self.side == Side::Settings {
                        Side::None
                    } else {
                        Side::Settings
                    };
                }
                Action::Open => {
                    if let Err(e) = self.net.pick_file() {
                        self.error = Some(e);
                    }
                }
                Action::Radio if self.cfg.radio.enabled => {
                    if self.playing.is_some() {
                        self.stop_radio();
                    } else if let Some((_, st)) = self
                        .stations_by_distance()
                        .first()
                        .map(|(d, s)| (*d, (*s).clone()))
                    {
                        self.play_radio(ui.ctx(), &st);
                    } else {
                        // no station list yet: fetch it and play the nearest when it arrives
                        self.radio_autoplay = true;
                        self.radio_requested = ui.input(|i| i.time);
                        self.net.radio_list();
                    }
                }
                Action::Radio => {}
            }
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
        // simulated warnings disappear on time, not at the next alert refresh
        if let Some(id) = self
            .simulated
            .iter()
            .find(|(_, until)| *until <= now)
            .map(|(a, _)| a.id.clone())
        {
            self.clear_simulated(Some(&id));
        }

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
        self.poll_feed(now, false);
        self.poll_outlook(now);
        if !self.tiles.json_requested && (self.show.roads || self.show.cities) {
            self.tiles.json_requested = true;
            self.net.tilejson(&self.cfg.map.tiles);
        }
        let repaint = if now < self.alarm_until { 0 } else { 5 };
        ctx.request_repaint_after(std::time::Duration::from_secs(repaint));
        if self.feed.is_some() && self.live {
            // keep polling for chunks; ~30 fps while a tilt is being swept in
            self.reveals.retain(|r| now - r.start < r.dur);
            let ms = if self.reveals.is_empty() { 1000 } else { 33 };
            ctx.request_repaint_after(std::time::Duration::from_millis(ms));
        }

        // fit `view_km` around the radar until the user moves the map (the window
        // may still be settling into its size on the first frames)
        if !self.fitted || !self.user_moved {
            let r = ui.available_rect_before_wrap();
            let rects = layout::tile(r, self.panes.len(), &self.cfg.layout, &self.split_ratios).0;
            let w = rects
                .iter()
                .map(|r| r.width().min(r.height()))
                .fold(f32::INFINITY, f32::min);
            self.zoom = (w / (2.0 * self.cfg.view_km)).max(0.05);
            self.fitted = true;
        }

        let t = self.cfg.theme.clone();
        self.keys_input(ui);
        self.handle_drops(ui);

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
                // download progress: a thin line along the bottom edge of the bar
                if self.loading.is_some() {
                    let full = ui.max_rect().expand2(vec2(12.0, 0.0));
                    let y = full.max.y;
                    let f = self.load_fraction().unwrap_or(0.04);
                    let line = Rect::from_min_max(
                        pos2(full.min.x, y - 2.0),
                        pos2(full.min.x + full.width() * f, y),
                    );
                    ui.painter()
                        .with_clip_rect(full.expand(1.0))
                        .rect_filled(line, 0, t.accent);
                }
            });

        if self.side != Side::None {
            let width = if self.side == Side::Settings {
                self.cfg.ui.panel_width * 1.5
            } else {
                self.cfg.ui.panel_width
            };
            egui::Panel::right("side")
                .default_size(width)
                .size_range(220.0..=(ui.available_width() * 0.6).max(240.0))
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
                // dragged split ratios only make sense for the layout they were made in
                let shape = (self.panes.len(), self.cfg.layout.mode);
                if self.split_shape != Some(shape) {
                    self.split_shape = Some(shape);
                    self.split_ratios.clear();
                }
                let (targets, dividers) =
                    layout::tile(area, self.panes.len(), &self.cfg.layout, &self.split_ratios);
                let mut rects = Vec::new();
                for (i, target) in targets.into_iter().enumerate() {
                    let id = self.panes[i].id;
                    let r = self.animated_rect(&ctx, id, target);
                    rects.push((id, r));
                    self.pane(ui, r, i);
                }
                self.last_rects = rects;
                self.dividers(ui, &dividers);
                self.request_tiles();
                self.error_toast(ui, area);
                if ui.input(|i| !i.raw.hovered_files.is_empty()) {
                    let p = ui.painter();
                    p.rect_filled(area, t.radius, t.bg.gamma_multiply(0.85));
                    p.rect_stroke(
                        area.shrink(6.0),
                        t.radius,
                        Stroke::new(2.0, t.accent),
                        egui::StrokeKind::Inside,
                    );
                    p.text(
                        area.center(),
                        egui::Align2::CENTER_CENTER,
                        "drop a NEXRAD Level II file to open it",
                        FontId::monospace(16.0),
                        t.fg,
                    );
                }
            });
        self.take_screenshot_when_ready(&ctx);
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        #[cfg(target_arch = "wasm32")]
        if !self.editor_dirty && self.shot.is_none() {
            _storage.set_string(STORE_KEY, self.editor.clone());
        }
    }
}

fn is_real_tornado(a: &Alert) -> bool {
    a.event.eq_ignore_ascii_case("Tornado Warning") && !a.id.starts_with("simulated-")
}

/// A freshly finished tilt being swept onto the map: one turn from where the radar
/// started it, as long as the radar took (capped), starting at `start` (UI clock).
struct Reveal {
    number: u8,
    from_az: f32,
    start: f64,
    dur: f64,
}

/// Small helper for muted section labels like "ACTIVE WARNINGS".
fn heading(ui: &mut Ui, text: &str, muted: Color32) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text).color(muted).small());
}
