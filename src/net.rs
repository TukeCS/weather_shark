//! Downloads. ehttp works the same on native (background thread) and on the
//! web (fetch), so results come back through a channel either way. Heavy
//! decoding happens inside the callback, which keeps it off the UI thread on
//! native.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use crate::alerts::{self, Alert};
use crate::level2::{self, Volume};
use crate::mvt::{self, TileData};
use crate::radio::{self, Station};
use crate::tiles::Key;

pub const BUCKET: &str = "https://unidata-nexrad-level2.s3.amazonaws.com";

pub enum Msg {
    Listing {
        site: String,
        keys: Result<Vec<String>, String>,
    },
    Volume {
        key: String,
        vol: Result<Arc<Volume>, String>,
    },
    Alerts(Result<Vec<Alert>, String>),
    TileJson(Result<String, String>),
    Tile {
        key: Key,
        tile: Result<TileData, String>,
    },
    Radio(Result<Vec<Station>, String>),
}

#[derive(Clone)]
pub struct Net {
    tx: Sender<Msg>,
    ctx: egui::Context,
}

impl Net {
    pub fn new(tx: Sender<Msg>, ctx: egui::Context) -> Self {
        Self { tx, ctx }
    }

    fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        self.ctx.request_repaint();
    }

    /// List the scans for one site on one UTC day (days since the Unix epoch).
    pub fn list(&self, site: &str, day: i64) {
        let (y, m, d) = civil_from_days(day);
        let url = format!("{BUCKET}/?list-type=2&prefix={y:04}/{m:02}/{d:02}/{site}/");
        let me = self.clone();
        let site = site.to_string();
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            let keys = res.and_then(|r| {
                if !r.ok {
                    return Err(format!("listing failed: HTTP {}", r.status));
                }
                let body = r.text().unwrap_or_default();
                let mut keys: Vec<String> = body
                    .split("<Key>")
                    .skip(1)
                    .filter_map(|s| s.split("</Key>").next())
                    .filter(|k| !k.ends_with("_MDM") && !k.ends_with(".tar"))
                    .map(String::from)
                    .collect();
                keys.sort();
                Ok(keys)
            });
            me.send(Msg::Listing { site, keys });
        });
    }

    pub fn volume(&self, key: &str) {
        let url = format!("{BUCKET}/{key}");
        let me = self.clone();
        let key = key.to_string();
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            let vol = res.and_then(|r| {
                if !r.ok {
                    return Err(format!("download failed: HTTP {}", r.status));
                }
                let t = web_time::Instant::now();
                let v = level2::parse(&r.bytes).map(Arc::new);
                log::debug!(
                    "decoded {} ({} KB) in {:?}",
                    r.url,
                    r.bytes.len() / 1024,
                    t.elapsed()
                );
                v
            });
            me.send(Msg::Volume { key, vol });
        });
    }

    pub fn alerts(&self, events: &[String]) {
        let mut ev: Vec<String> = events.iter().map(|e| e.replace(' ', "%20")).collect();
        ev.dedup();
        let url = format!(
            "https://api.weather.gov/alerts/active?status=actual&event={}",
            ev.join(",")
        );
        let mut req = ehttp::Request::get(url);
        req.headers.insert("Accept", "application/geo+json");
        if !cfg!(target_arch = "wasm32") {
            // Browsers set their own; the NWS API wants one from everything else.
            req.headers.insert("User-Agent", "weather_shark/0.1");
        }
        let me = self.clone();
        ehttp::fetch(req, move |res| {
            let a = res.and_then(|r| {
                if !r.ok {
                    return Err(format!("alerts: HTTP {}", r.status));
                }
                alerts::parse(&r.bytes)
            });
            me.send(Msg::Alerts(a));
        });
    }
}

impl Net {
    /// Read a TileJSON document for its tile URL template.
    pub fn tilejson(&self, url: &str) {
        let me = self.clone();
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            let t = res.and_then(|r| {
                let v: serde_json::Value =
                    serde_json::from_slice(&r.bytes).map_err(|e| format!("map tiles: {e}"))?;
                v["tiles"][0]
                    .as_str()
                    .map(String::from)
                    .ok_or_else(|| "map tiles: no tile URL".to_string())
            });
            me.send(Msg::TileJson(t));
        });
    }

    pub fn tile(&self, template: &str, key: Key) {
        let (z, x, y) = key;
        let url = template
            .replace("{z}", &z.to_string())
            .replace("{x}", &x.to_string())
            .replace("{y}", &y.to_string());
        let me = self.clone();
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            let tile = res.and_then(|r| {
                if r.status == 204 || r.status == 404 {
                    return Ok(TileData::default()); // empty ocean tile
                }
                if !r.ok {
                    return Err(format!("HTTP {}", r.status));
                }
                let mut bytes = r.bytes;
                if bytes.starts_with(&[0x1f, 0x8b]) {
                    use std::io::Read;
                    let mut out = Vec::new();
                    flate2::read::GzDecoder::new(&bytes[..])
                        .read_to_end(&mut out)
                        .map_err(|e| e.to_string())?;
                    bytes = out;
                }
                mvt::decode(&bytes, z, x, y)
            });
            me.send(Msg::Tile { key, tile });
        });
    }

    pub fn radio_list(&self) {
        let me = self.clone();
        ehttp::fetch(ehttp::Request::get(radio::STATUS_URL), move |res| {
            me.send(Msg::Radio(res.and_then(|r| radio::parse_status(&r.bytes))));
        });
    }
}

// ---- time helpers ----------------------------------------------------------------

pub fn now() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// (year, month, day) for days since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Scan time from a key like `2013/05/20/KTLX/KTLX20130520_200356_V06.gz`.
pub fn key_time(key: &str) -> Option<i64> {
    let name = key.rsplit('/').next()?;
    let ts = name.get(4..19)?; // 20130520_200356
    let n = |a: usize, b: usize| ts.get(a..b)?.parse::<i64>().ok();
    let day = days_from_civil(n(0, 4)?, n(4, 6)? as u32, n(6, 8)? as u32);
    Some(day * 86400 + n(9, 11)? * 3600 + n(11, 13)? * 60 + n(13, 15)?)
}

/// Parse "2013-05-20 20:08" (UTC) into epoch seconds.
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim().replace('T', " ");
    let (date, time) = s.split_once(' ').unwrap_or((&s, "00:00"));
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, dd) = (d.next()??, d.next()??, d.next()??);
    let mut t = time
        .trim_end_matches('Z')
        .split(':')
        .map(|x| x.parse::<i64>().ok());
    let (h, mi) = (
        t.next().flatten().unwrap_or(0),
        t.next().flatten().unwrap_or(0),
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&dd) {
        return None;
    }
    Some(days_from_civil(y, m as u32, dd as u32) * 86400 + h * 3600 + mi * 60)
}

pub fn fmt_time(t: i64) -> String {
    let (y, m, d) = civil_from_days(t.div_euclid(86400));
    let s = t.rem_euclid(86400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}Z", s / 3600, s / 60 % 60)
}

pub fn fmt_age(secs: i64) -> String {
    match secs {
        s if s < 90 => "just now".into(),
        s if s < 5400 => format!("{} min ago", s / 60),
        s if s < 172_800 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}
