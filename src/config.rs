//! Lua config: run the script with piccolo (a pure-Rust Lua VM, so it works
//! in wasm too), turn the returned table into a plain Rust tree, then read
//! typed settings out of that.

use egui::Color32;
use piccolo::{Closure, Executor, Fuel, Lua, Table, Value};

use crate::level2::Moment;

pub const DEFAULT_LUA: &str = include_str!("../assets/default_config.lua");

// ---- Lua value tree ---------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub enum L {
    #[default]
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    Table { arr: Vec<L>, map: Vec<(String, L)> },
}

impl L {
    pub fn get(&self, key: &str) -> &L {
        match self {
            L::Table { map, .. } => map.iter().find(|(k, _)| k == key).map(|(_, v)| v).unwrap_or(&L::Nil),
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
            let q = |i: usize| u8::from_str_radix(h.get(i..i + 1)?, 16).ok().map(|v| v * 17);
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
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
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
}

#[derive(Clone)]
pub struct AlertCfg {
    pub enabled: bool,
    pub radius_km: f64,
    pub volume: f32,
    pub on_startup: bool,
    pub rules: Vec<AlertRule>,
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
    pub tilt: f32,
    pub vel_units: VelUnits,
    pub panes: Vec<Moment>,
    pub smooth: bool,
    pub image_size: usize,
    pub theme: Theme,
    pub map: MapStyle,
    pub palettes: Vec<(Moment, Palette)>,
    pub alerts: AlertCfg,
    pub bookmarks: Vec<Bookmark>,
}

impl Config {
    pub fn palette(&self, m: Moment) -> &Palette {
        &self.palettes.iter().find(|(k, _)| *k == m).expect("every moment has a palette").1
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
        let platform = if cfg!(target_arch = "wasm32") { "web" } else { "native" };
        let root = run_lua(src, platform)?;
        // Anything the user leaves out falls back to the built-in config.
        let base = if src == DEFAULT_LUA { L::Nil } else { run_lua(DEFAULT_LUA, platform)? };
        Self::build(&root, &base)
    }

    fn build(r: &L, d: &L) -> Result<Self, String> {
        let pick = |k: &str| if r.get(k).is_nil() { d.get(k) } else { r.get(k) };
        let sub = |sect: &str, k: &str| {
            let v = r.get(sect).get(k);
            if v.is_nil() { d.get(sect).get(k) } else { v }
        };
        let col = |v: &L, what: &str| -> Result<Option<Color32>, String> {
            match v {
                L::Nil | L::Bool(false) => Ok(None),
                L::Str(s) => color(s).map(Some).ok_or_else(|| format!("{what}: bad colour {s:?}")),
                _ => Err(format!("{what}: expected a \"#rrggbb\" string")),
            }
        };
        let need = |v: &L, what: &str| col(v, what)?.ok_or_else(|| format!("{what}: colour missing"));

        let theme = Theme {
            bg: need(sub("theme", "bg"), "theme.bg")?,
            panel: need(sub("theme", "panel"), "theme.panel")?,
            line: need(sub("theme", "line"), "theme.line")?,
            fg: need(sub("theme", "fg"), "theme.fg")?,
            muted: need(sub("theme", "muted"), "theme.muted")?,
            accent: need(sub("theme", "accent"), "theme.accent")?,
            radius: sub("theme", "radius").num().unwrap_or(10.0) as f32,
            font_size: sub("theme", "font_size").num().unwrap_or(13.0).clamp(8.0, 32.0) as f32,
        };
        let map = MapStyle {
            states: col(sub("map", "states"), "map.states")?,
            counties: col(sub("map", "counties"), "map.counties")?,
            range_rings: col(sub("map", "range_rings"), "map.range_rings")?,
            sites: col(sub("map", "sites"), "map.sites")?.unwrap_or(theme.accent),
            site_labels: sub("map", "site_labels").bool().unwrap_or(true),
            crosshair: col(sub("map", "crosshair"), "map.crosshair")?.unwrap_or(Color32::WHITE),
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
                let v = s.arr().first().and_then(L::num).ok_or_else(|| format!("{what}: expected {{ value, \"#colour\" }}"))?;
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
                    range_folded: col(p.get("range_folded"), "range_folded")?.unwrap_or(Color32::from_rgb(0x5b, 0x3a, 0x8a)),
                    mask_ref: p.get("mask_ref").num().map(|v| v as f32),
                },
            ));
        }

        let a = |k: &str| sub("alerts", k);
        let mut rules = Vec::new();
        let rule_list = if r.get("alerts").get("rules").is_nil() { d.get("alerts").get("rules") } else { r.get("alerts").get("rules") };
        for (i, x) in rule_list.arr().iter().enumerate() {
            let what = format!("alerts.rules[{}]", i + 1);
            rules.push(AlertRule {
                event: x.get("event").str().ok_or_else(|| format!("{what}: needs event = \"...\""))?.to_string(),
                tornado: x.get("tornado").str().map(str::to_lowercase),
                damage: x.get("damage").str().map(str::to_lowercase),
                hail: x.get("hail").num().map(|v| v as f32),
                sound: x.get("sound").str().unwrap_or("beep").to_lowercase(),
                file: x.get("file").str().map(String::from),
                repeats: x.get("repeats").num().unwrap_or(1.0).clamp(0.0, 20.0) as u32,
                volume: x.get("volume").num().unwrap_or(1.0).clamp(0.0, 1.0) as f32,
                color: col(x.get("color"), &what)?.unwrap_or(theme.accent),
            });
        }
        let alerts = AlertCfg {
            enabled: a("enabled").bool().unwrap_or(true),
            radius_km: a("radius_km").num().unwrap_or(300.0),
            volume: a("volume").num().unwrap_or(0.8).clamp(0.0, 1.0) as f32,
            on_startup: a("on_startup").bool().unwrap_or(false),
            rules,
        };

        let panes: Vec<Moment> = pick("panes")
            .arr()
            .iter()
            .filter_map(|p| p.str())
            .map(|s| Moment::from_key(s).ok_or_else(|| format!("panes: unknown product {s:?} (try REF, VEL, CC, ZDR, SW, PHI)")))
            .collect::<Result<_, _>>()?;
        if panes.is_empty() || panes.len() > 6 {
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

        Ok(Config {
            site: pick("site").str().unwrap_or("KTLX").to_uppercase(),
            live: pick("live").bool().unwrap_or(true),
            refresh_seconds: pick("refresh_seconds").num().unwrap_or(60.0).max(15.0),
            tilt: pick("tilt").num().unwrap_or(0.5) as f32,
            vel_units: match pick("velocity_units").str().unwrap_or("kt") {
                "mph" => VelUnits::Mph,
                "ms" | "m/s" => VelUnits::Ms,
                _ => VelUnits::Kt,
            },
            panes,
            smooth: pick("smooth").bool().unwrap_or(false),
            image_size: pick("image_size").num().unwrap_or(2048.0).clamp(256.0, 4096.0) as usize,
            theme,
            map,
            palettes,
            alerts,
            bookmarks,
        })
    }
}
