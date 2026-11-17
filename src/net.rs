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
    /// `done` = false for an early snapshot of the sweeps decoded so far.
    Volume {
        key: String,
        vol: Result<Arc<Volume>, String>,
        done: bool,
    },
    Progress {
        key: String,
        got: usize,
        total: Option<usize>,
    },
    Alerts(Result<Vec<Alert>, String>),
    TileJson(Result<String, String>),
    Tile {
        key: Key,
        tile: Result<TileData, String>,
    },
    Radio(Result<Vec<Station>, String>),
    Outlook(Result<crate::spc::Outlook, String>),
    /// The live volume folder for a site in the chunks bucket: (number, volume stamp).
    ChunkFound {
        site: String,
        found: Result<(u16, String), String>,
    },
    /// Chunk keys in one volume folder.
    ChunkList {
        site: String,
        num: u16,
        keys: Result<Vec<String>, String>,
    },
    Chunk {
        key: String,
        bytes: Result<Vec<u8>, String>,
    },
    /// A radar file the user opened (dropped, picked, or passed on the command line).
    Local {
        name: String,
        vol: Result<Arc<Volume>, String>,
    },
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

    pub fn ctx(&self) -> egui::Context {
        self.ctx.clone()
    }

    pub(crate) fn send(&self, m: Msg) {
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

    /// List one volume folder of the chunks bucket (keys after `after`, sorted).
    fn chunk_keys(
        site: &str,
        num: u16,
        after: &str,
        done: impl 'static + Send + FnOnce(Result<Vec<String>, String>),
    ) {
        let mut url = format!("{}/?list-type=2&prefix={site}/{num}/", crate::chunks::BUCKET);
        if !after.is_empty() {
            url += &format!("&start-after={after}");
        }
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            done(res.and_then(|r| {
                if !r.ok {
                    return Err(format!("live data: HTTP {}", r.status));
                }
                Ok(crate::chunks::listing_keys(r.text().unwrap_or_default()))
            }))
        });
    }

    /// List a volume folder of the chunks bucket.
    pub fn chunk_list(&self, site: &str, num: u16, after: &str) {
        let me = self.clone();
        let s = site.to_string();
        Self::chunk_keys(site, num, after, move |keys| {
            me.send(Msg::ChunkList { site: s, num, keys })
        });
    }

    /// Download one chunk.
    pub fn chunk(&self, key: &str) {
        let me = self.clone();
        let key = key.to_string();
        let url = format!("{}/{key}", crate::chunks::BUCKET);
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            let bytes = res.and_then(|r| {
                if r.ok {
                    Ok(r.bytes)
                } else {
                    Err(format!("live data: HTTP {}", r.status))
                }
            });
            me.send(Msg::Chunk { key, bytes });
        });
    }

    /// Find the folder the radar is writing to now. Folders are numbered 1..=999 and
    /// reused in turn, so the newest volume in each, read around the circle, goes up
    /// and then drops once. Requests are slow (~1–2 s) but cheap, so it's two rounds
    /// in parallel: 40 folders spread around the circle, then every folder in the gap
    /// after the newest of those (the current folder is in there).
    pub fn chunk_find(&self, site: &str) {
        use crate::chunks::{MAX_NUM, parse_key};
        type Done = Box<dyn FnOnce(Result<Vec<(u16, String)>, String>) + Send>;
        // newest volume stamp in each folder ("" when empty)
        fn newest_of(site: &str, nums: Vec<u16>, done: Done) {
            let n = nums.len();
            let st = Arc::new(std::sync::Mutex::new((Vec::new(), 0usize, None, Some(done))));
            for num in nums {
                let st = st.clone();
                Net::chunk_keys(site, num, "", move |r| {
                    let mut g = st.lock().unwrap();
                    match r {
                        Ok(keys) => {
                            let t = keys.iter().filter_map(|k| parse_key(k)).map(|c| c.stamp).max();
                            g.0.push((num, t.unwrap_or_default()));
                        }
                        Err(e) => {
                            g.2 = Some(e);
                        }
                    }
                    g.1 += 1;
                    if g.1 == n
                        && let Some(done) = g.3.take()
                    {
                        let res = std::mem::take(&mut g.0);
                        let err = g.2.take();
                        drop(g);
                        // a few failed folders are fine; all of them failing isn't
                        match err {
                            Some(e) if res.is_empty() => done(Err(e)),
                            _ => done(Ok(res)),
                        }
                    }
                });
            }
        }
        let newest = |r: &[(u16, String)]| r.iter().max_by(|a, b| a.1.cmp(&b.1)).cloned();
        const SAMPLES: u16 = 40;
        let gap = MAX_NUM.div_ceil(SAMPLES);
        let me = self.clone();
        let site = site.to_string();
        let s2 = site.clone();
        let first: Vec<u16> = (0..SAMPLES).map(|i| 1 + i * gap).filter(|n| *n <= MAX_NUM).collect();
        newest_of(
            &site,
            first,
            Box::new(move |r| {
                let fail = |me: &Net, site: String, e: String| {
                    me.send(Msg::ChunkFound { site, found: Err(e) })
                };
                let best = match r.map(|r| newest(&r)) {
                    Ok(Some(b)) if !b.1.is_empty() => b,
                    Ok(_) => return fail(&me, s2, "no live data for this radar".into()),
                    Err(e) => return fail(&me, s2, e),
                };
                let rest: Vec<u16> = (1..gap)
                    .map(|i| best.0 + i)
                    .filter(|n| *n <= MAX_NUM)
                    .collect();
                let site = s2.clone();
                newest_of(
                    &s2,
                    rest,
                    Box::new(move |r| {
                        let mut all = r.unwrap_or_default();
                        all.push(best);
                        let (num, stamp) = newest(&all).unwrap();
                        me.send(Msg::ChunkFound {
                            site,
                            found: Ok((num, stamp)),
                        });
                    }),
                );
            }),
        );
    }

    /// Stream a volume: report progress, and hand over the sweeps decoded so far
    /// as they finish (lowest tilt first) so the radar shows up early.
    pub fn volume(&self, key: &str) {
        struct St {
            dec: Option<level2::Decoder>,
            got: usize,
            total: Option<usize>,
            reported: usize,
            sent_sweeps: usize,
        }
        let url = format!("{BUCKET}/{key}");
        let me = self.clone();
        let key = key.to_string();
        let st = std::sync::Mutex::new(St {
            dec: Some(level2::Decoder::new()),
            got: 0,
            total: None,
            reported: 0,
            sent_sweeps: 0,
        });
        ehttp::streaming::fetch(ehttp::Request::get(url), move |part| {
            use ehttp::streaming::Part;
            use std::ops::ControlFlow;
            let mut st = st.lock().unwrap();
            let fail = |e: String| {
                me.send(Msg::Volume {
                    key: key.clone(),
                    vol: Err(e),
                    done: true,
                });
                ControlFlow::Break(())
            };
            match part {
                Err(e) => fail(e),
                Ok(Part::Response(r)) => {
                    if !r.ok {
                        return fail(format!("download failed: HTTP {}", r.status));
                    }
                    st.total = r.headers.get("content-length").and_then(|v| v.parse().ok());
                    ControlFlow::Continue(())
                }
                Ok(Part::Chunk(c)) if c.is_empty() => {
                    let vol = st
                        .dec
                        .take()
                        .map_or_else(|| Err("download ended twice".into()), |d| d.finish())
                        .map(Arc::new);
                    me.send(Msg::Volume {
                        key: key.clone(),
                        vol,
                        done: true,
                    });
                    ControlFlow::Break(())
                }
                Ok(Part::Chunk(c)) => {
                    st.got += c.len();
                    let sent = st.sent_sweeps;
                    let Some(dec) = st.dec.as_mut() else {
                        return ControlFlow::Break(());
                    };
                    if let Err(e) = dec.push(&c) {
                        return fail(e);
                    }
                    // Hand over each newly finished sweep among the first few (the low tilts).
                    let done = dec.sweeps_done();
                    let snap = (done > sent && done <= 6).then(|| dec.snapshot()).flatten();
                    if let Some(v) = snap {
                        st.sent_sweeps = done;
                        me.send(Msg::Volume {
                            key: key.clone(),
                            vol: Ok(Arc::new(v)),
                            done: false,
                        });
                    }
                    if st.got - st.reported > 96 * 1024 {
                        st.reported = st.got;
                        me.send(Msg::Progress {
                            key: key.clone(),
                            got: st.got,
                            total: st.total,
                        });
                    }
                    ControlFlow::Continue(())
                }
            }
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

    /// Decode a local Level II file (off the UI thread on native).
    pub fn open_bytes(&self, name: String, bytes: Vec<u8>) {
        let me = self.clone();
        let work = move || {
            let vol = level2::parse(&bytes).map(Arc::new);
            me.send(Msg::Local { name, vol });
        };
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(work);
        #[cfg(target_arch = "wasm32")]
        work();
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_path(&self, path: std::path::PathBuf) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read(&path) {
            Ok(b) => self.open_bytes(name, b),
            Err(e) => self.send(Msg::Local {
                name,
                vol: Err(format!("can't read {}: {e}", path.display())),
            }),
        }
    }

    /// Show the system file dialog. On the web this is a file input; the bytes come back as a message.
    pub fn pick_file(&self) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let t = web_time::Instant::now();
            match rfd::FileDialog::new()
                .set_title("Open a NEXRAD Level II file")
                .pick_file()
            {
                Some(p) => self.open_path(p),
                // Returned instantly without a dialog: no file chooser on this system.
                None if t.elapsed().as_millis() < 250 => {
                    return Err("no file dialog on this system: drop the file on the window, or run weather_shark FILE".into());
                }
                None => {}
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let me = self.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(f) = rfd::AsyncFileDialog::new().pick_file().await {
                    let name = f.file_name();
                    me.open_bytes(name, f.read().await);
                }
            });
        }
        Ok(())
    }

    /// An SPC convective outlook.
    pub fn outlook(&self, sel: crate::spc::Sel) {
        let me = self.clone();
        ehttp::fetch(ehttp::Request::get(sel.url()), move |res| {
            let o = res.and_then(|r| {
                if !r.ok {
                    return Err(format!("SPC outlook: HTTP {}", r.status));
                }
                crate::spc::parse(&r.bytes, sel)
            });
            me.send(Msg::Outlook(o));
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

/// Seconds since the Unix epoch, with the fraction.
pub fn now_f64() -> f64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

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

/// "03:33:59Z"
pub fn fmt_clock(t: f64) -> String {
    let s = (t as i64).rem_euclid(86400);
    format!("{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

pub fn fmt_age(secs: i64) -> String {
    match secs {
        s if s < 90 => "just now".into(),
        s if s < 5400 => format!("{} min ago", s / 60),
        s if s < 172_800 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}
