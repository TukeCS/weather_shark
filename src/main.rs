mod alerts;
mod app;
mod config;
mod level2;
mod map;
mod net;
mod render;
mod sound;

#[cfg(not(target_arch = "wasm32"))]
pub fn config_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| ".".into());
    base.join("weather_shark").join("config.lua")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn config_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(config_path()).and_then(|m| m.modified()).ok()
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
            let path = std::env::args().nth(2).map(Into::into).unwrap_or_else(config_path);
            let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            match config::Config::from_lua(&src) {
                Ok(_) => println!("{}: ok", path.display()),
                Err(e) => {
                    eprintln!("{}: {e}", path.display());
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        Some("-h" | "--help") => {
            println!(
                "weather_shark [SITE [\"YYYY-MM-DD HH:MM\"]] | default-config | check [file]\nconfig: {}",
                config_path().display()
            );
            return Ok(());
        }
        _ => {}
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
    let mut args = std::env::args().skip(1);
    let start = args.next().map(|site| (site, args.collect::<Vec<_>>().join(" ")));
    eframe::run_native("weather_shark", options, Box::new(|cc| Ok(Box::new(app::App::new(cc, start)))))
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
        let hash = web_sys::window().and_then(|w| w.location().hash().ok()).unwrap_or_default();
        let hash = hash.trim_start_matches('#').replace("%20", " ");
        let start = (!hash.is_empty()).then(|| {
            let (s, t) = hash.split_once(' ').unwrap_or((&hash, ""));
            (s.to_string(), t.to_string())
        });
        eframe::WebRunner::new()
            .start(canvas, eframe::WebOptions::default(), Box::new(|cc| Ok(Box::new(app::App::new(cc, start)))))
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
        assert_eq!(c.panes.len(), 3);
        assert!(c.alerts.rules.len() >= 4);
    }

    #[test]
    fn partial_config_falls_back_to_defaults() {
        let c = Config::from_lua("return { site = 'kfws', panes = { 'VEL' }, theme = { accent = '#ff0000' } }").unwrap();
        assert_eq!(c.site, "KFWS");
        assert_eq!(c.panes, vec![level2::Moment::Vel]);
        assert_eq!(c.theme.accent, egui::Color32::from_rgb(255, 0, 0));
        assert_eq!(c.theme.bg, Config::default_config().theme.bg);
    }

    #[test]
    fn bad_config_reports_error() {
        assert!(Config::from_lua("return { panes = { 'NOPE' } }").err().unwrap().contains("NOPE"));
        assert!(Config::from_lua("return {").is_err());
        assert!(Config::from_lua("while true do end").err().unwrap().contains("too long"));
        assert!(Config::from_lua("return { theme = { bg = 'red' } }").is_err());
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
        assert_eq!(key_time("2013/05/20/KTLX/KTLX20130520_200356_V06.gz"), Some(t - 8 * 60 + 236));
    }

    /// Set WS_SAMPLE=path/to/level2/file to also render it to WS_OUT (a .ppm).
    #[test]
    fn render_sample() {
        let Ok(p) = std::env::var("WS_SAMPLE") else { return };
        let v = level2::parse(&std::fs::read(p).unwrap()).unwrap();
        let c = Config::default_config();
        let mut out = Vec::new();
        for m in [level2::Moment::Ref, level2::Moment::Vel, level2::Moment::Rho] {
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
                    let mix = |c: u8, b: u8| ((c as u32 * 255 + b as u32 * (255 - a)) / 255).min(255) as u8;
                    buf.extend([mix(p.r(), bg.r()), mix(p.g(), bg.g()), mix(p.b(), bg.b())]);
                }
            }
            std::fs::write(o, buf).unwrap();
        }
    }
}
