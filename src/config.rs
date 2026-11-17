//! Lua config: run the script with piccolo (a pure-Rust Lua VM, so it works
//! in wasm too), turn the returned table into a plain Rust tree, then read
//! typed settings out of that.

use egui::Color32;
use piccolo::{Closure, Executor, Fuel, Lua, Table, Value};

use crate::level2::Moment;

pub const MAX_PANES: usize = 6;

pub const DEFAULT_LUA: &str = include_str!("../assets/default_config.lua");

// ---- Lua value tree ---------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub enum L {
    #[default]
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    Table {
        arr: Vec<L>,
        map: Vec<(String, L)>,
    },
}

impl L {
    pub fn get(&self, key: &str) -> &L {
        match self {
            L::Table { map, .. } => map
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v)
                .unwrap_or(&L::Nil),
            _ => &L::Nil,
        }
    }
    pub fn arr(&self) -> &[L] {
        match self {
            L::Table { arr, .. } => arr,
            _ => &[],
        }
    }
    pub fn num(&self) -> Option<f64> {
        match self {
            L::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            L::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn bool(&self) -> Option<bool> {
        match self {
            L::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn is_nil(&self) -> bool {
        matches!(self, L::Nil)
    }
}

fn convert<'gc>(v: Value<'gc>, depth: u32) -> L {
    match v {
        Value::Nil => L::Nil,
        Value::Boolean(b) => L::Bool(b),
        Value::Integer(i) => L::Num(i as f64),
        Value::Number(n) => L::Num(n),
        Value::String(s) => L::Str(s.to_str_lossy().into_owned()),
        Value::Table(t) if depth < 16 => convert_table(t, depth),
        _ => L::Nil,
    }
}

fn convert_table<'gc>(t: Table<'gc>, depth: u32) -> L {
    let mut arr = Vec::new();
    let mut map = Vec::new();
    let len = t.length();
    for i in 1..=len {
        arr.push(convert(t.get_value(Value::Integer(i)), depth + 1));
    }
    for (k, v) in t.iter() {
        if let Value::String(s) = k {
            map.push((s.to_str_lossy().into_owned(), convert(v, depth + 1)));
        }
    }
    L::Table { arr, map }
}

/// Run a Lua script and return the table it returns.
pub fn run_lua(src: &str, platform: &str) -> Result<L, String> {
    let mut lua = Lua::core();
    let ex = lua
        .try_enter(|ctx| {
            let _ = ctx.set_global("platform", ctx.intern(platform.as_bytes()));
            let closure = Closure::load(ctx, Some("config.lua"), src.as_bytes())?;
            Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
        })
        .map_err(|e| e.to_string())?;

    // Cap the work so `while true do end` can't freeze the app.
    for _ in 0..20_000 {
        let mut fuel = Fuel::with(4096);
        if lua.enter(|ctx| ctx.fetch(&ex).step(ctx, &mut fuel)) {
            let v = lua.enter(|ctx| match ctx.fetch(&ex).take_result::<Value>(ctx) {
                Ok(Ok(v)) => Ok(convert(v, 0)),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            });
            return v.and_then(|v| match v {
                L::Table { .. } => Ok(v),
                _ => Err("config.lua must `return { ... }` a table".into()),
            });
        }
    }
    Err("config.lua took too long to run (infinite loop?)".into())
}

// ---- typed config -------------------------------------------------------------

pub fn color(s: &str) -> Option<Color32> {
    let h = s.strip_prefix('#')?;
    let p = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some(Color32::from_rgb(p(0)?, p(2)?, p(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(p(0)?, p(2)?, p(4)?, p(6)?)),
        3 => {
            let q = |i: usize| {
                u8::from_str_radix(h.get(i..i + 1)?, 16)
                    .ok()
                    .map(|v| v * 17)
            };
            Some(Color32::from_rgb(q(0)?, q(1)?, q(2)?))
        }
        _ => None,
    }
}

#[derive(Clone)]
pub struct Palette {
    pub stops: Vec<(f32, Color32)>,
    pub step: bool,
    pub range_folded: Color32,
    pub mask_ref: Option<f32>,
}

impl Palette {
    pub fn color(&self, v: f32) -> Color32 {
        let s = &self.stops;
        if s.is_empty() || v < s[0].0 {
            return Color32::TRANSPARENT;
        }
        let i = s.partition_point(|(x, _)| *x <= v);
        if i >= s.len() {
            return s[s.len() - 1].1;
        }
        let (a, ca) = s[i - 1];
        if self.step {
            return ca;
        }
        let (b, cb) = s[i];
        let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
        lerp(ca, cb, t)
    }
    pub fn min(&self) -> f32 {
        self.stops.first().map_or(0.0, |s| s.0)
    }
    pub fn max(&self) -> f32 {
        self.stops.last().map_or(1.0, |s| s.0)
    }
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

#[derive(Clone)]
pub struct Theme {
    pub bg: Color32,
    pub panel: Color32,
    pub line: Color32,
    pub fg: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub radius: f32,
    pub font_size: f32,
}

#[derive(Clone)]
pub struct MapStyle {
    pub states: Option<Color32>,
    pub counties: Option<Color32>,
    pub range_rings: Option<Color32>,
    pub sites: Color32,
    pub site_labels: bool,
    pub crosshair: Color32,
    /// Vector tiles (OpenMapTiles schema) for roads and towns: a TileJSON URL.
    pub tiles: String,
    pub roads: bool,
    pub road_labels: bool,
    pub cities: bool,
    /// motorway, trunk, primary, secondary, tertiary, minor
    pub road_colors: [Color32; 6],
    pub city_color: Color32,
    pub warnings: bool,
    /// county outlines of tornado / severe thunderstorm watches
    pub watches: bool,
}

/// How one outlook level is drawn. `None` = not drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelStyle {
    pub line: Option<Color32>,
    pub fill: Option<Color32>,
}

/// `outlook = { ... }`: the SPC convective outlook layer.
#[derive(Clone)]
pub struct OutlookCfg {
    /// what to show on start (None = off)
    pub show: Option<crate::spc::Sel>,
    /// outline width in px (0 = no outlines)
    pub width: f32,
    /// opacity of the fill when a level has no fill colour of its own (0 = no fill)
    pub fill: f32,
    /// draw fills under the radar instead of over it
    pub fill_under_radar: bool,
    pub labels: bool,
    pub label_size: f32,
    /// per-level overrides keyed like "MRGL", "10%", "SIG" (upper case, no "%")
    pub levels: Vec<(String, Option<LevelStyle>)>,
}

impl OutlookCfg {
    fn norm(k: &str) -> String {
        k.trim().trim_end_matches('%').to_ascii_uppercase()
    }

    /// How to draw an area with SPC label `key` and SPC colour `spc`; None hides it.
    pub fn style(&self, key: &str, spc: Color32) -> Option<LevelStyle> {
        let k = Self::norm(key);
        let over = self.levels.iter().find(|(n, _)| *n == k).map(|(_, s)| *s);
        let base = LevelStyle {
            line: (self.width > 0.0).then_some(spc),
            fill: (self.fill > 0.0).then(|| spc.gamma_multiply(self.fill)),
        };
        match over {
            None => Some(base),
            Some(None) => None,
            Some(Some(s)) => Some(s),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LayoutMode {
    Spiral,
    Dwindle,
    Columns,
    Grid,
}

#[derive(Clone)]
pub struct Layout {
    pub mode: LayoutMode,
    pub ratio: f32,
    pub gap: f32,
}

#[derive(Clone)]
pub struct RadioCfg {
    pub enabled: bool,
    pub volume: f32,
}

#[derive(Clone, Default)]
pub struct AlertRule {
    pub event: String,
    pub tornado: Option<String>,
    pub damage: Option<String>,
    pub hail: Option<f32>,
    pub sound: String,
    pub file: Option<String>,
    pub repeats: u32,
    pub volume: f32,
    pub color: Color32,
    /// draw the polygon / county outline on the map
    pub map: bool,
    /// show it in the alerts list (and count it on the alerts button)
    pub list: bool,
}

#[derive(Clone)]
pub struct AlertCfg {
    pub enabled: bool,
    pub radius_km: f64,
    pub volume: f32,
    pub on_startup: bool,
    pub rules: Vec<AlertRule>,
    /// play sounds at all
    pub sounds: bool,
}

/// Something you can bind a key to in `keys = { ... }`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    PrevScan,
    NextScan,
    TiltUp,
    TiltDown,
    AddPane,
    ClosePane,
    Live,
    Recenter,
    Roads,
    Cities,
    Warnings,
    Watches,
    Outlook,
    Settings,
    Open,
    Radio,
    Play,
}

impl Action {
    pub const ALL: [(Action, &'static str, &'static str); 17] = [
        (Action::PrevScan, "prev_scan", "ArrowLeft"),
        (Action::NextScan, "next_scan", "ArrowRight"),
        (Action::TiltUp, "tilt_up", "ArrowUp"),
        (Action::TiltDown, "tilt_down", "ArrowDown"),
        (Action::AddPane, "add_pane", "N"),
        (Action::ClosePane, "close_pane", "X"),
        (Action::Live, "live", "L"),
        (Action::Recenter, "recenter", "R"),
        (Action::Roads, "roads", "M"),
        (Action::Cities, "cities", "C"),
        (Action::Warnings, "warnings", "W"),
        (Action::Watches, "watches", "A"),
        (Action::Outlook, "outlook", "S"),
        (Action::Settings, "settings", "Comma"),
        (Action::Open, "open", "O"),
        (Action::Radio, "radio", "Space"),
        (Action::Play, "play", "P"),
    ];
}

pub const TOOLBAR_ITEMS: [&str; 9] = [
    "site", "time", "tilt", "panes", "map", "events", "alerts", "radio", "lua",
];

#[derive(Clone)]
pub struct UiCfg {
    /// top bar items, left to right (see TOOLBAR_ITEMS)
    pub toolbar: Vec<String>,
    pub header: bool,
    pub legend: bool,
    pub readout: bool,
    pub crosshair: bool,
    pub toasts: bool,
    /// width of the alerts / radio panel (the Lua editor gets 1.5×)
    pub panel_width: f32,
}

/// Colours for the Lua editor.
#[derive(Clone)]
pub struct Syntax {
    pub keyword: Color32,
    pub string: Color32,
    pub number: Color32,
    pub comment: Color32,
    pub field: Color32,
    pub punct: Color32,
}

#[derive(Clone)]
pub struct Bookmark {
    pub name: String,
    pub site: String,
    pub time: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum VelUnits {
    Kt,
    Mph,
    Ms,
}

impl VelUnits {
    pub fn factor(self) -> f32 {
        match self {
            VelUnits::Kt => 1.943_844,
            VelUnits::Mph => 2.236_936,
            VelUnits::Ms => 1.0,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            VelUnits::Kt => "kt",
            VelUnits::Mph => "mph",
            VelUnits::Ms => "m/s",
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub site: String,
    pub live: bool,
    pub refresh_seconds: f64,
    /// follow the radar in real time through the chunks bucket
    pub realtime: bool,
    /// how far back the loop goes (minutes), and how long each frame shows (s)
    pub loop_minutes: u32,
    pub loop_frame: f64,
    /// draw the rotating sweep line on live data
    pub sweep_line: bool,
    pub tilt: f32,
    pub vel_units: VelUnits,
    pub panes: Vec<Moment>,
    pub smooth: bool,
    pub image_size: usize,
    pub view_km: f32,
    pub theme: Theme,
    pub map: MapStyle,
    pub layout: Layout,
    pub radio: RadioCfg,
    pub ui: UiCfg,
    pub keys: Vec<(Action, egui::Key)>,
    pub syntax: Syntax,
    pub palettes: Vec<(Moment, Palette)>,
    pub alerts: AlertCfg,
    pub outlook: OutlookCfg,
    pub bookmarks: Vec<Bookmark>,
}

impl Config {
    pub fn palette(&self, m: Moment) -> &Palette {
        &self
            .palettes
            .iter()
            .find(|(k, _)| *k == m)
            .expect("every moment has a palette")
            .1
    }

    /// Multiplier from the radar's native units to display units.
    pub fn unit_factor(&self, m: Moment) -> f32 {
        match m {
            Moment::Vel | Moment::Sw => self.vel_units.factor(),
            _ => 1.0,
        }
    }

    pub fn unit_label(&self, m: Moment) -> &'static str {
        match m {
            Moment::Ref => "dBZ",
            Moment::Vel | Moment::Sw => self.vel_units.label(),
            Moment::Zdr => "dB",
            Moment::Phi => "°",
            Moment::Rho => "",
        }
    }

    pub fn default_config() -> Self {
        Self::from_lua(DEFAULT_LUA).expect("built-in config is valid")
    }

    pub fn from_lua(src: &str) -> Result<Self, String> {
        let platform = if cfg!(target_arch = "wasm32") {
            "web"
        } else {
            "native"
        };
        let root = run_lua(src, platform)?;
        // Anything the user leaves out falls back to the built-in config.
        let base = if src == DEFAULT_LUA {
            L::Nil
        } else {
            run_lua(DEFAULT_LUA, platform)?
        };
        Self::build(&root, &base)
    }

    fn build(r: &L, d: &L) -> Result<Self, String> {
        let pick = |k: &str| {
            if r.get(k).is_nil() {
                d.get(k)
            } else {
                r.get(k)
            }
        };
        let sub = |sect: &str, k: &str| {
            let v = r.get(sect).get(k);
            if v.is_nil() { d.get(sect).get(k) } else { v }
        };
        let col = |v: &L, what: &str| -> Result<Option<Color32>, String> {
            match v {
                L::Nil | L::Bool(false) => Ok(None),
                L::Str(s) => color(s)
                    .map(Some)
                    .ok_or_else(|| format!("{what}: bad colour {s:?}")),
                _ => Err(format!("{what}: expected a \"#rrggbb\" string")),
            }
        };
        let need =
            |v: &L, what: &str| col(v, what)?.ok_or_else(|| format!("{what}: colour missing"));

        let theme = Theme {
            bg: need(sub("theme", "bg"), "theme.bg")?,
            panel: need(sub("theme", "panel"), "theme.panel")?,
            line: need(sub("theme", "line"), "theme.line")?,
            fg: need(sub("theme", "fg"), "theme.fg")?,
            muted: need(sub("theme", "muted"), "theme.muted")?,
            accent: need(sub("theme", "accent"), "theme.accent")?,
            radius: sub("theme", "radius").num().unwrap_or(10.0) as f32,
            font_size: sub("theme", "font_size")
                .num()
                .unwrap_or(13.0)
                .clamp(8.0, 32.0) as f32,
        };
        let map = MapStyle {
            states: col(sub("map", "states"), "map.states")?,
            counties: col(sub("map", "counties"), "map.counties")?,
            range_rings: col(sub("map", "range_rings"), "map.range_rings")?,
            sites: col(sub("map", "sites"), "map.sites")?.unwrap_or(theme.accent),
            site_labels: sub("map", "site_labels").bool().unwrap_or(true),
            crosshair: col(sub("map", "crosshair"), "map.crosshair")?.unwrap_or(Color32::WHITE),
            tiles: sub("map", "tiles")
                .str()
                .unwrap_or("https://tiles.openfreemap.org/planet")
                .to_string(),
            roads: sub("map", "roads").bool().unwrap_or(true),
            road_labels: sub("map", "road_labels").bool().unwrap_or(true),
            cities: sub("map", "cities").bool().unwrap_or(true),
            road_colors: {
                let rc = |k: &str| -> Result<Color32, String> {
                    let v = r.get("map").get("road_colors").get(k);
                    let v = if v.is_nil() {
                        d.get("map").get("road_colors").get(k)
                    } else {
                        v
                    };
                    Ok(col(v, &format!("map.road_colors.{k}"))?.unwrap_or(Color32::TRANSPARENT))
                };
                [
                    rc("motorway")?,
                    rc("trunk")?,
                    rc("primary")?,
                    rc("secondary")?,
                    rc("tertiary")?,
                    rc("minor")?,
                ]
            },
            city_color: col(sub("map", "city_color"), "map.city_color")?.unwrap_or(theme.fg),
            warnings: sub("map", "warnings").bool().unwrap_or(true),
            watches: sub("map", "watches").bool().unwrap_or(true),
        };
        let layout = Layout {
            mode: match sub("layout", "mode").str().unwrap_or("spiral") {
                "dwindle" => LayoutMode::Dwindle,
                "columns" => LayoutMode::Columns,
                "grid" => LayoutMode::Grid,
                "spiral" | "fibonacci" => LayoutMode::Spiral,
                m => {
                    return Err(format!(
                        "layout.mode: unknown {m:?} (spiral, dwindle, columns, grid)"
                    ));
                }
            },
            ratio: sub("layout", "ratio").num().unwrap_or(0.5).clamp(0.2, 0.8) as f32,
            gap: sub("layout", "gap").num().unwrap_or(8.0).clamp(0.0, 40.0) as f32,
        };
        let radio = RadioCfg {
            enabled: sub("radio", "enabled").bool().unwrap_or(true),
            volume: sub("radio", "volume").num().unwrap_or(0.8).clamp(0.0, 1.0) as f32,
        };

        let mut palettes = Vec::new();
        for m in Moment::ALL {
            let mut p = r.get("palettes").get(m.key());
            if p.arr().is_empty() {
                p = d.get("palettes").get(m.key());
            }
            let mut stops = Vec::new();
            for (i, s) in p.arr().iter().enumerate() {
                let what = format!("palettes.{}[{}]", m.key(), i + 1);
                let v = s
                    .arr()
                    .first()
                    .and_then(L::num)
                    .ok_or_else(|| format!("{what}: expected {{ value, \"#colour\" }}"))?;
                let c = need(s.arr().get(1).unwrap_or(&L::Nil), &what)?;
                stops.push((v as f32, c));
            }
            stops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            if stops.is_empty() {
                return Err(format!("palettes.{} is empty", m.key()));
            }
            palettes.push((
                m,
                Palette {
                    stops,
                    step: p.get("step").bool().unwrap_or(false),
                    range_folded: col(p.get("range_folded"), "range_folded")?
                        .unwrap_or(Color32::from_rgb(0x5b, 0x3a, 0x8a)),
                    mask_ref: p.get("mask_ref").num().map(|v| v as f32),
                },
            ));
        }

        let a = |k: &str| sub("alerts", k);
        let mut rules = Vec::new();
        let rule_list = if r.get("alerts").get("rules").is_nil() {
            d.get("alerts").get("rules")
        } else {
            r.get("alerts").get("rules")
        };
        for (i, x) in rule_list.arr().iter().enumerate() {
            let what = format!("alerts.rules[{}]", i + 1);
            rules.push(AlertRule {
                event: x
                    .get("event")
                    .str()
                    .ok_or_else(|| format!("{what}: needs event = \"...\""))?
                    .to_string(),
                tornado: x.get("tornado").str().map(str::to_lowercase),
                damage: x.get("damage").str().map(str::to_lowercase),
                hail: x.get("hail").num().map(|v| v as f32),
                sound: {
                    // anything to do with tornadoes gets the loudest sound unless told otherwise
                    let tornado = x.get("tornado").str().is_some()
                        || x.get("event")
                            .str()
                            .is_some_and(|e| e.to_ascii_lowercase().contains("tornado"));
                    let fallback = if tornado { "eas" } else { "beep" };
                    let s = x.get("sound").str().unwrap_or(fallback).to_lowercase();
                    if s != "none" && !crate::sound::SOUNDS.contains(&s.as_str()) {
                        return Err(format!(
                            "{what}: unknown sound {s:?} ({}, or \"none\")",
                            crate::sound::SOUNDS.join(", ")
                        ));
                    }
                    s
                },
                file: x.get("file").str().map(String::from),
                repeats: x.get("repeats").num().unwrap_or(1.0).clamp(0.0, 20.0) as u32,
                volume: x.get("volume").num().unwrap_or(1.0).clamp(0.0, 1.0) as f32,
                color: col(x.get("color"), &what)?.unwrap_or(theme.accent),
                map: x.get("map").bool().unwrap_or(true),
                list: x.get("list").bool().unwrap_or(true),
            });
        }
        let alerts = AlertCfg {
            enabled: a("enabled").bool().unwrap_or(true),
            radius_km: a("radius_km").num().unwrap_or(300.0),
            volume: a("volume").num().unwrap_or(0.8).clamp(0.0, 1.0) as f32,
            on_startup: a("on_startup").bool().unwrap_or(false),
            rules,
            sounds: a("sounds").bool().unwrap_or(true),
        };

        let toolbar: Vec<String> = {
            let v = if r.get("ui").get("toolbar").is_nil() {
                d.get("ui").get("toolbar")
            } else {
                r.get("ui").get("toolbar")
            };
            v.arr()
                .iter()
                .filter_map(|x| x.str())
                .map(str::to_lowercase)
                .collect()
        };
        if let Some(bad) = toolbar
            .iter()
            .find(|t| !TOOLBAR_ITEMS.contains(&t.as_str()))
        {
            return Err(format!(
                "ui.toolbar: unknown item {bad:?} (try {})",
                TOOLBAR_ITEMS.join(", ")
            ));
        }
        let ui = UiCfg {
            toolbar,
            header: sub("ui", "header").bool().unwrap_or(true),
            legend: sub("ui", "legend").bool().unwrap_or(true),
            readout: sub("ui", "readout").bool().unwrap_or(true),
            crosshair: sub("ui", "crosshair").bool().unwrap_or(true),
            toasts: sub("ui", "toasts").bool().unwrap_or(true),
            panel_width: sub("ui", "panel_width")
                .num()
                .unwrap_or(320.0)
                .clamp(220.0, 900.0) as f32,
        };

        let mut keys = Vec::new();
        for (action, name, default) in Action::ALL {
            let v = r.get("keys").get(name);
            let k = match v {
                L::Bool(false) => continue, // keys = { radio = false } unbinds it
                L::Str(s) => s.as_str(),
                _ => d.get("keys").get(name).str().unwrap_or(default),
            };
            let key = egui::Key::from_name(k)
                .or_else(|| egui::Key::from_name(&k.to_uppercase()))
                .ok_or_else(|| format!("keys.{name}: unknown key {k:?} (e.g. \"N\", \"F2\", \"ArrowLeft\", \"Space\")"))?;
            keys.push((action, key));
        }

        let syn = |k: &str, fallback: Color32| -> Result<Color32, String> {
            Ok(
                col(sub("theme", "syntax").get(k), &format!("theme.syntax.{k}"))?
                    .or(col(d.get("theme").get("syntax").get(k), "")?)
                    .unwrap_or(fallback),
            )
        };
        let syntax = Syntax {
            keyword: syn("keyword", theme.accent)?,
            string: syn("string", Color32::from_rgb(0x9e, 0xce, 0x6a))?,
            number: syn("number", Color32::from_rgb(0xff, 0x9e, 0x64))?,
            comment: syn("comment", theme.muted)?,
            field: syn("field", Color32::from_rgb(0x7d, 0xcf, 0xff))?,
            punct: syn("punct", theme.muted)?,
        };

        let panes: Vec<Moment> = pick("panes")
            .arr()
            .iter()
            .filter_map(|p| p.str())
            .map(|s| {
                Moment::from_key(s).ok_or_else(|| {
                    format!("panes: unknown product {s:?} (try REF, VEL, CC, ZDR, SW, PHI)")
                })
            })
            .collect::<Result<_, _>>()?;
        if panes.is_empty() || panes.len() > MAX_PANES {
            return Err("panes: give between 1 and 6 products".into());
        }

        let bookmarks = pick("bookmarks")
            .arr()
            .iter()
            .filter_map(|b| {
                Some(Bookmark {
                    name: b.get("name").str()?.to_string(),
                    site: b.get("site").str()?.to_uppercase(),
                    time: b.get("time").str()?.to_string(),
                })
            })
            .collect();

        let outlook = {
            let day = sub("outlook", "day").num().unwrap_or(1.0);
            if !(1.0..=8.0).contains(&day) || day.fract() != 0.0 {
                return Err(format!("outlook.day: {day} (1 to 8)"));
            }
            let day = day as u8;
            let show = match sub("outlook", "show") {
                L::Nil | L::Bool(false) => None,
                L::Bool(true) => Some(crate::spc::Sel {
                    day,
                    kind: crate::spc::Kind::Categorical,
                }),
                L::Str(s) => {
                    let kind = crate::spc::Kind::from_name(s).ok_or_else(|| {
                        format!(
                            "outlook.show: unknown {s:?} (categorical, tornado, wind, hail, severe)"
                        )
                    })?;
                    if !crate::spc::Kind::for_day(day).contains(&kind) {
                        return Err(format!(
                            "outlook.show: SPC has no {s:?} outlook for day {day} (day 3 has categorical and severe, days 4-8 only severe)"
                        ));
                    }
                    Some(crate::spc::Sel { day, kind })
                }
                _ => return Err("outlook.show: expected false or a name like \"tornado\"".into()),
            };
            let fill = sub("outlook", "fill").num().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
            let width = sub("outlook", "width")
                .num()
                .unwrap_or(1.6)
                .clamp(0.0, 10.0) as f32;
            // defaults first, then the user's table on top
            let mut levels: Vec<(String, Option<LevelStyle>)> = Vec::new();
            for src in [
                d.get("outlook").get("colors"),
                r.get("outlook").get("colors"),
            ] {
                let L::Table { map, .. } = src else { continue };
                for (k, v) in map {
                    let what = format!("outlook.colors[\"{k}\"]");
                    let st = match v {
                        L::Bool(false) => None,
                        L::Str(_) => {
                            let c = col(v, &what)?;
                            Some(LevelStyle {
                                line: c.filter(|_| width > 0.0),
                                fill: c.filter(|_| fill > 0.0).map(|c| c.gamma_multiply(fill)),
                            })
                        }
                        L::Table { .. } => Some(LevelStyle {
                            line: col(v.get("line"), &format!("{what}.line"))?,
                            fill: col(v.get("fill"), &format!("{what}.fill"))?,
                        }),
                        _ => {
                            return Err(format!(
                                "{what}: expected a colour, false, or {{ line = ..., fill = ... }}"
                            ));
                        }
                    };
                    let k = OutlookCfg::norm(k);
                    levels.retain(|(n, _)| *n != k);
                    levels.push((k, st));
                }
            }
            OutlookCfg {
                show,
                width,
                fill,
                fill_under_radar: sub("outlook", "fill_under_radar").bool().unwrap_or(true),
                labels: sub("outlook", "labels").bool().unwrap_or(true),
                label_size: sub("outlook", "label_size")
                    .num()
                    .unwrap_or(11.0)
                    .clamp(6.0, 30.0) as f32,
                levels,
            }
        };

        Ok(Config {
            site: pick("site").str().unwrap_or("KTLX").to_uppercase(),
            live: pick("live").bool().unwrap_or(true),
            refresh_seconds: pick("refresh_seconds").num().unwrap_or(60.0).max(15.0),
            realtime: pick("realtime").bool().unwrap_or(true),
            loop_minutes: pick("loop_minutes").num().unwrap_or(15.0).clamp(5.0, 180.0) as u32,
            loop_frame: pick("loop_frame_ms").num().unwrap_or(400.0).clamp(50.0, 5000.0) / 1000.0,
            sweep_line: pick("sweep_line").bool().unwrap_or(true),
            tilt: pick("tilt").num().unwrap_or(0.5) as f32,
            vel_units: match pick("velocity_units").str().unwrap_or("kt") {
                "mph" => VelUnits::Mph,
                "ms" | "m/s" => VelUnits::Ms,
                _ => VelUnits::Kt,
            },
            panes,
            smooth: pick("smooth").bool().unwrap_or(false),
            view_km: pick("view_km").num().unwrap_or(160.0).clamp(10.0, 2000.0) as f32,
            image_size: pick("image_size")
                .num()
                .unwrap_or(2048.0)
                .clamp(256.0, 4096.0) as usize,
            theme,
            map,
            layout,
            radio,
            ui,
            keys,
            syntax,
            palettes,
            alerts,
            outlook,
            bookmarks,
        })
    }
}
