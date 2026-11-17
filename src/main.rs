// No console window behind the app in Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod alerts;
mod app;
mod config;
mod level2;
mod map;
mod mvt;
mod net;
#[cfg(not(target_arch = "wasm32"))]
mod png;
mod radio;
mod render;
mod sound;
mod spc;
mod tiles;

/// Where config.lua lives:
/// - `$XDG_CONFIG_HOME/weather_shark/` if that's set (any OS)
/// - Linux: `~/.config/weather_shark/`
/// - macOS: `~/.config/weather_shark/` if it exists, else `~/Library/Application Support/weather_shark/`
/// - Windows: `%APPDATA%\weather_shark\`
#[cfg(not(target_arch = "wasm32"))]
pub fn config_path() -> std::path::PathBuf {
    use std::path::PathBuf;
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let home = env("HOME").or_else(|| env("USERPROFILE"));
    let dir = if let Some(x) = env("XDG_CONFIG_HOME") {
        x.join("weather_shark")
    } else if cfg!(target_os = "windows") {
        env("APPDATA")
            .or_else(|| home.clone().map(|h| h.join("AppData").join("Roaming")))
            .unwrap_or_else(|| ".".into())
            .join("weather_shark")
    } else if cfg!(target_os = "macos") {
        let h = home.unwrap_or_else(|| ".".into());
        let dot = h.join(".config").join("weather_shark");
        if dot.exists() {
            dot
        } else {
            h.join("Library")
                .join("Application Support")
                .join("weather_shark")
        }
    } else {
        home.unwrap_or_else(|| ".".into())
            .join(".config")
            .join("weather_shark")
    };
    dir.join("config.lua")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn config_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(config_path())
        .and_then(|m| m.modified())
        .ok()
}

/// The user's config, or the built-in one (written out on first run so it's easy to edit).
#[cfg(not(target_arch = "wasm32"))]
pub fn load_config_file() -> String {
    let p = config_path();
    match std::fs::read_to_string(&p) {
        Ok(s) => s,
        Err(_) => {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&p, config::DEFAULT_LUA);
            config::DEFAULT_LUA.to_string()
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    match std::env::args().nth(1).as_deref() {
        Some("default-config") => {
            print!("{}", config::DEFAULT_LUA);
            return Ok(());
        }
        Some("check") => {
            // Validate a config without opening a window: `weather_shark check [file]`
            let path = std::env::args()
                .nth(2)
                .map(Into::into)
                .unwrap_or_else(config_path);
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            match config::Config::from_lua(&src) {
                Ok(_) => println!("{}: ok", path.display()),
                Err(e) => {
                    eprintln!("{}: {e}", path.display());
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        Some("test-sound") => {
            // `weather_shark test-sound [wea|eas|siren|alarm|beep|chime]` or `test-sound radio [CALLSIGN-mount]`
            let what = std::env::args().nth(2).unwrap_or_else(|| "siren".into());
            if what == "radio" {
                let mount = std::env::args()
                    .nth(3)
                    .unwrap_or_else(|| "OK-OklahomaCity-WXK85".into());
                let mut r = radio::RadioPlayer::new();
                r.play(
                    &format!("https://wxradio.org/{mount}"),
                    egui::Context::default(),
                );
                println!("playing {mount} for 15 s…");
                for _ in 0..15 {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    println!("  {}", r.status());
                }
            } else {
                let mut p = sound::Player::new();
                p.play(&what, None, 1, 1.0);
                println!("playing {what}…");
                std::thread::sleep(std::time::Duration::from_secs(4));
            }
            return Ok(());
        }
        Some("-V" | "--version") => {
            println!("weather_shark {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("-h" | "--help") => {
            println!(
                "weather_shark {}

usage:
  weather_shark                            live radar from the site in your config
  weather_shark SITE [\"YYYY-MM-DD HH:MM\"]  a radar site, optionally a past scan (UTC)
  weather_shark FILE                       open a NEXRAD Level II file
  weather_shark default-config             print the default config.lua
  weather_shark check [FILE]               validate a config without opening a window
  weather_shark test-sound [wea|eas|siren|alarm|beep|chime|radio]
  weather_shark screenshot OUT.png [SITE [TIME]] [--panes REF,VEL] [--view KM]
                [--layout columns] [--at LAT,LON] [--panel lua|alerts|radio] [--simulate]

config: {}",
                env!("CARGO_PKG_VERSION"),
                config_path().display()
            );
            return Ok(());
        }
        _ => {}
    }
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // `weather_shark screenshot out.png KTLX "2013-05-20 20:13" [--panes REF,VEL] [--view 40]`
    let mut shot = None;
    if args.first().map(String::as_str) == Some("screenshot") {
        let flag = |args: &mut Vec<String>, name: &str| -> Option<String> {
            let i = args.iter().position(|a| a == name)?;
            let v = args.get(i + 1).cloned();
            args.drain(i..(i + 2).min(args.len()));
            v
        };
        let panes = flag(&mut args, "--panes")
            .map(|p| p.split(',').filter_map(level2::Moment::from_key).collect());
        let view_km = flag(&mut args, "--view").and_then(|v| v.parse().ok());
        let at = flag(&mut args, "--at").and_then(|v| {
            let (a, b) = v.split_once(',')?;
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        });
        let panel = flag(&mut args, "--panel");
        let simulate = if let Some(i) = args.iter().position(|a| a == "--simulate") {
            args.remove(i);
            true
        } else {
            false
        };
        let layout = flag(&mut args, "--layout").map(|l| match l.as_str() {
            "columns" => config::LayoutMode::Columns,
            "grid" => config::LayoutMode::Grid,
            "dwindle" => config::LayoutMode::Dwindle,
            _ => config::LayoutMode::Spiral,
        });
        args.remove(0);
        if args.is_empty() {
            eprintln!(
                "usage: weather_shark screenshot OUT.png [SITE [\"YYYY-MM-DD HH:MM\"]] [--panes REF,VEL] [--view KM] [--layout columns] [--at LAT,LON] [--panel lua|alerts|radio] [--simulate]"
            );
            std::process::exit(2);
        }
        shot = Some(app::ShotOpts {
            path: args.remove(0).into(),
            panes,
            view_km,
            layout,
            at,
            panel,
            simulate,
        });
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("weather_shark")
            .with_app_id("weather_shark")
            .with_inner_size([1400.0, 820.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    // `weather_shark KTLX "2013-05-20 20:08"` opens a past scan
    let mut rest = args.into_iter();
    let start = rest
        .next()
        .map(|site| (site, rest.collect::<Vec<_>>().join(" ")));
    eframe::run_native(
        "weather_shark",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc, start, shot)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast as _;
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("weather_shark_canvas"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("no <canvas id=\"weather_shark_canvas\"> on the page");
        // page.html#KTLX 2013-05-20 20:08 opens a past scan
        let hash = web_sys::window()
            .and_then(|w| w.location().hash().ok())
            .unwrap_or_default();
        let hash = hash.trim_start_matches('#').replace("%20", " ");
        let start = (!hash.is_empty()).then(|| {
            let (s, t) = hash.split_once(' ').unwrap_or((&hash, ""));
            (s.to_string(), t.to_string())
        });
        eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(app::App::new(cc, start, None)))),
            )
            .await
            .expect("failed to start weather_shark");
    });
}

#[cfg(test)]
mod tests {
    use crate::{config::Config, level2, render};

    #[test]
    fn default_config_loads() {
        let c = Config::default_config();
        assert_eq!(c.site, "KTLX");
        assert_eq!(c.panes.len(), 1);
        assert_eq!(c.layout.mode, crate::config::LayoutMode::Spiral);
        assert!(c.alerts.rules.len() >= 4);
    }

    #[test]
    fn partial_config_falls_back_to_defaults() {
        let c = Config::from_lua(
            "return { site = 'kfws', panes = { 'VEL' }, theme = { accent = '#ff0000' } }",
        )
        .unwrap();
        assert_eq!(c.site, "KFWS");
        assert_eq!(c.panes, vec![level2::Moment::Vel]);
        assert_eq!(c.theme.accent, egui::Color32::from_rgb(255, 0, 0));
        assert_eq!(c.theme.bg, Config::default_config().theme.bg);
    }

    #[test]
    fn bad_config_reports_error() {
        assert!(
            Config::from_lua("return { panes = { 'NOPE' } }")
                .err()
                .unwrap()
                .contains("NOPE")
        );
        assert!(Config::from_lua("return {").is_err());
        assert!(
            Config::from_lua("while true do end")
                .err()
                .unwrap()
                .contains("too long")
        );
        assert!(Config::from_lua("return { theme = { bg = 'red' } }").is_err());
    }

    #[test]
    fn toolbar_keys_and_switches() {
        use crate::config::Action;
        let c = Config::from_lua(
            r#"return { ui = { toolbar = { "time", "lua" }, legend = false },
                        keys = { radio = false, add_pane = "F2", cities = "k" },
                        radio = { enabled = false } }"#,
        )
        .unwrap();
        assert_eq!(c.ui.toolbar, vec!["time", "lua"]);
        assert!(
            !c.ui.legend && c.ui.header,
            "unset switches keep their default"
        );
        assert!(!c.radio.enabled);
        assert!(
            !c.keys.iter().any(|(a, _)| *a == Action::Radio),
            "false unbinds a key"
        );
        assert!(c.keys.contains(&(Action::AddPane, egui::Key::F2)));
        assert!(
            c.keys.contains(&(Action::Cities, egui::Key::K)),
            "lower-case letters work"
        );
        assert!(
            c.keys.contains(&(Action::PrevScan, egui::Key::ArrowLeft)),
            "others keep defaults"
        );

        let e = Config::from_lua(r#"return { ui = { toolbar = { "site", "nope" } } }"#)
            .err()
            .unwrap();
        assert!(e.contains("nope") && e.contains("tilt"), "{e}");
        let e = Config::from_lua(r#"return { keys = { live = "NotAKey" } }"#)
            .err()
            .unwrap();
        assert!(e.contains("keys.live"), "{e}");
    }

    #[test]
    fn lua_logic_works() {
        let c = Config::from_lua(
            "local p = {} for i, m in ipairs({'REF','CC'}) do p[#p+1] = m end
             return { panes = p, refresh_seconds = 30 * 2, site = platform == 'native' and 'KINX' or 'KTLX' }",
        )
        .unwrap();
        assert_eq!(c.panes.len(), 2);
        assert_eq!(c.refresh_seconds, 60.0);
        assert_eq!(c.site, "KINX");
    }

    #[test]
    fn time_helpers() {
        use crate::net::*;
        let t = parse_time("2013-05-20 20:08").unwrap();
        assert_eq!(fmt_time(t), "2013-05-20 20:08Z");
        assert_eq!(
            key_time("2013/05/20/KTLX/KTLX20130520_200356_V06.gz"),
            Some(t - 8 * 60 + 236)
        );
    }

    /// WS_SAMPLE: streaming in 64 KB chunks must match a one-shot decode, and the
    /// lowest tilt should be ready well before the end of the file.
    #[test]
    fn streaming_matches_whole_file() {
        let Ok(p) = std::env::var("WS_SAMPLE") else {
            return;
        };
        let bytes = std::fs::read(p).unwrap();
        let whole = level2::parse(&bytes).unwrap();
        let mut d = level2::Decoder::new();
        let mut ready_at = None;
        for (i, c) in bytes.chunks(64 * 1024).enumerate() {
            d.push(c).unwrap();
            if ready_at.is_none() {
                if let Some(v) = d.snapshot() {
                    let ok = |m| v.sweep_for(m, 0.5).is_some_and(|s| s.elevation < 0.7);
                    if ok(level2::Moment::Ref) && ok(level2::Moment::Vel) {
                        ready_at = Some((i + 1) * 64 * 1024);
                    }
                }
            }
        }
        let streamed = d.finish().unwrap();
        assert_eq!(whole.sweeps.len(), streamed.sweeps.len());
        for (a, b) in whole.sweeps.iter().zip(&streamed.sweeps) {
            assert_eq!((a.number, a.radials.len()), (b.number, b.radials.len()));
        }
        let at = ready_at.unwrap();
        eprintln!(
            "0.5° REF+VEL ready after {} of {} KB ({:.0}%)",
            at / 1024,
            bytes.len() / 1024,
            100.0 * at as f64 / bytes.len() as f64
        );
        assert!(at < bytes.len() / 2);
    }

    /// Set WS_SAMPLE=path/to/level2/file to also render it to WS_OUT (a .ppm).
    #[test]
    fn render_sample() {
        let Ok(p) = std::env::var("WS_SAMPLE") else {
            return;
        };
        let v = level2::parse(&std::fs::read(p).unwrap()).unwrap();
        let c = Config::default_config();
        let mut out = Vec::new();
        for m in [
            level2::Moment::Ref,
            level2::Moment::Vel,
            level2::Moment::Rho,
        ] {
            let s = v.sweep_for(m, 0.5).unwrap();
            let t = std::time::Instant::now();
            let img = render::rasterize(s, m, c.palette(m), c.unit_factor(m), 2048, 230.0);
            eprintln!("{m:?} {:?}", t.elapsed());
            out.push(img);
        }
        if let Ok(o) = std::env::var("WS_OUT") {
            // side by side, 1/2 scale, on the theme background
            let (w, h) = (1024 * 3, 1024);
            let bg = c.theme.bg;
            let mut buf = format!("P6 {w} {h} 255\n").into_bytes();
            for y in 0..h {
                for x in 0..w {
                    let img = &out[x / 1024];
                    let p = img.pixels[(y * 2) * 2048 + (x % 1024) * 2];
                    let a = p.a() as u32;
                    let mix = |c: u8, b: u8| {
                        ((c as u32 * 255 + b as u32 * (255 - a)) / 255).min(255) as u8
                    };
                    buf.extend([mix(p.r(), bg.r()), mix(p.g(), bg.g()), mix(p.b(), bg.b())]);
                }
            }
            std::fs::write(o, buf).unwrap();
        }
    }
}

#[cfg(test)]
mod tile_tests {
    /// WS_TILE=path to a z8/60/101 OpenFreeMap tile
    #[test]
    fn decode_sample_tile() {
        let Ok(p) = std::env::var("WS_TILE") else {
            return;
        };
        let t = crate::mvt::decode(&std::fs::read(p).unwrap(), 8, 60, 101).unwrap();
        eprintln!(
            "roads {} places {} shields {}",
            t.roads.len(),
            t.places.len(),
            t.shields.len()
        );
        for pl in t.places.iter().take(8) {
            eprintln!(
                "  {:?} {} rank {} {:.3},{:.3}",
                pl.class, pl.name, pl.rank, pl.lon, pl.lat
            );
        }
        let mut sh: Vec<_> = t.shields.iter().map(|s| s.text.clone()).collect();
        sh.sort();
        sh.dedup();
        eprintln!("  shields: {:?}", &sh[..sh.len().min(15)]);
        assert!(t.roads.len() > 10);
    }
}
