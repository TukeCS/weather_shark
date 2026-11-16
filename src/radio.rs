//! NOAA Weather Radio streams from wxradio.org.
//!
//! wxradio.org's terms allow direct listening by end users but not relaying,
//! and they block clients that open several streams at once. So we only ever
//! play one stream, straight from their server, and never record or relay it.

static TRANSMITTERS: &str = include_str!("../assets/nwr.txt");

pub const STATUS_URL: &str = "https://wxradio.org/status-json.xsl";

#[derive(Clone)]
pub struct Station {
    pub url: String,
    pub call: String,
    pub site: String,
    pub state: String,
    pub freq: String,
    pub lat: f64,
    pub lon: f64,
}

/// Match the live stream list against the NWS transmitter table by callsign.
pub fn parse_status(bytes: &[u8]) -> Result<Vec<Station>, String> {
    let v: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("radio list: {e}"))?;
    let src = &v["icestats"]["source"];
    let list: Vec<&serde_json::Value> = match src {
        serde_json::Value::Array(a) => a.iter().collect(),
        serde_json::Value::Object(_) => vec![src],
        _ => vec![],
    };
    let tx: Vec<Vec<&str>> = TRANSMITTERS
        .lines()
        .map(|l| l.split('|').collect())
        .filter(|p: &Vec<&str>| p.len() >= 6)
        .collect();
    let mut out: Vec<Station> = Vec::new();
    for s in list {
        let Some(listen) = s["listenurl"].as_str() else {
            continue;
        };
        let mount = listen.rsplit('/').next().unwrap_or_default().to_string();
        // e.g. OK-OklahomaCity-WXK85, TX-Dallas-KEC56-alt1
        let Some(t) = mount
            .split('-')
            .find_map(|seg| tx.iter().find(|t| t[0] == seg))
        else {
            continue;
        };
        if out.iter().any(|o| o.call == t[0]) {
            continue; // keep the main mount, skip -alt mirrors
        }
        out.push(Station {
            url: format!("https://wxradio.org/{mount}"),
            call: t[0].to_string(),
            lat: t[1].parse().unwrap_or(0.0),
            lon: t[2].parse().unwrap_or(0.0),
            site: t[3].to_string(),
            state: t[4].to_string(),
            freq: t[5].to_string(),
        });
    }
    Ok(out)
}

// ---- playback --------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A network stream can't seek; symphonia is told so via `with_seekable(false)`.
    pub struct NoSeek<R>(pub R);
    impl<R: Read> Read for NoSeek<R> {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(b)
        }
    }
    impl<R> Seek for NoSeek<R> {
        fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
            Err(std::io::Error::other("stream is not seekable"))
        }
    }

    #[cfg(test)]
    pub use NoSeek as NoSeekPub;

    pub struct RadioPlayer {
        sink: Option<rodio::MixerDeviceSink>,
        player: Option<Arc<rodio::Player>>,
        stop: Arc<AtomicBool>,
        volume: f32,
        pub status: Arc<std::sync::Mutex<String>>,
    }

    impl RadioPlayer {
        pub fn new() -> Self {
            Self {
                sink: None,
                player: None,
                stop: Arc::new(AtomicBool::new(false)),
                volume: 0.8,
                status: Default::default(),
            }
        }

        pub fn play(&mut self, url: &str, ctx: egui::Context) {
            self.stop();
            if self.sink.is_none() {
                match rodio::DeviceSinkBuilder::open_default_sink() {
                    Ok(mut s) => {
                        s.log_on_drop(false);
                        self.sink = Some(s);
                    }
                    Err(e) => {
                        *self.status.lock().unwrap() = format!("no audio output: {e}");
                        return;
                    }
                }
            }
            let player = Arc::new(rodio::Player::connect_new(
                self.sink.as_ref().unwrap().mixer(),
            ));
            player.set_volume(self.volume);
            self.player = Some(player.clone());
            let stop = Arc::new(AtomicBool::new(false));
            self.stop = stop.clone();
            let status = self.status.clone();
            *status.lock().unwrap() = "connecting…".into();
            let url = url.to_string();
            std::thread::spawn(move || {
                let set = |s: String| {
                    *status.lock().unwrap() = s;
                    ctx.request_repaint();
                };
                let res = (|| -> Result<(), String> {
                    let resp = ureq::get(&url).call().map_err(|e| e.to_string())?;
                    let reader = resp.into_body().into_reader();
                    let dec = rodio::Decoder::builder()
                        .with_data(NoSeek(reader))
                        .with_seekable(false)
                        .with_hint("mp3")
                        .with_mime_type("audio/mpeg")
                        .build()
                        .map_err(|e| e.to_string())?;
                    use rodio::Source;
                    let ch = dec.channels();
                    let sr = dec.sample_rate();
                    set("live".into());
                    // Decode here (the network blocks), hand finished chunks to the player.
                    let mut buf = Vec::with_capacity(8192);
                    for s in dec {
                        if stop.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        buf.push(s);
                        if buf.len() >= 8192 {
                            player.append(rodio::buffer::SamplesBuffer::new(
                                ch,
                                sr,
                                std::mem::take(&mut buf),
                            ));
                        }
                    }
                    Ok(())
                })();
                if !stop.load(Ordering::Relaxed) {
                    set(match res {
                        Ok(()) => "stream ended".into(),
                        Err(e) => format!("can't play: {e}"),
                    });
                }
            });
        }

        pub fn stop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(p) = self.player.take() {
                p.stop();
            }
            *self.status.lock().unwrap() = String::new();
        }

        pub fn set_volume(&mut self, v: f32) {
            self.volume = v;
            if let Some(p) = &self.player {
                p.set_volume(v);
            }
        }

        pub fn status(&self) -> String {
            self.status.lock().unwrap().clone()
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    pub struct RadioPlayer {
        audio: Option<web_sys::HtmlAudioElement>,
        volume: f32,
    }

    impl RadioPlayer {
        pub fn new() -> Self {
            Self {
                audio: None,
                volume: 0.8,
            }
        }

        pub fn play(&mut self, url: &str, _ctx: egui::Context) {
            self.stop();
            if let Ok(a) = web_sys::HtmlAudioElement::new_with_src(url) {
                a.set_volume(self.volume as f64);
                let _ = a.play();
                self.audio = Some(a);
            }
        }

        pub fn stop(&mut self) {
            if let Some(a) = self.audio.take() {
                let _ = a.pause();
                a.set_src(""); // closes the connection, so only one stream is ever open
            }
        }

        pub fn set_volume(&mut self, v: f32) {
            self.volume = v;
            if let Some(a) = &self.audio {
                a.set_volume(v as f64);
            }
        }

        pub fn status(&self) -> String {
            match &self.audio {
                Some(a) if a.paused() => "connecting…".into(),
                Some(a) if a.ready_state() < 3 => "buffering…".into(),
                Some(_) => "live".into(),
                None => String::new(),
            }
        }
    }
}

pub use imp::RadioPlayer;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    /// WS_RADIO_JSON=saved status-json.xsl
    #[test]
    fn matches_streams_to_transmitters() {
        let Ok(p) = std::env::var("WS_RADIO_JSON") else {
            return;
        };
        let st = super::parse_status(&std::fs::read(p).unwrap()).unwrap();
        assert!(st.len() > 80, "{}", st.len());
        let okc = st.iter().find(|s| s.call == "WXK85").unwrap();
        assert_eq!(okc.url, "https://wxradio.org/OK-OklahomaCity-WXK85");
        assert!((okc.lat - 35.57).abs() < 0.1);
        // -alt mirrors are folded into the main stream
        assert_eq!(
            st.iter().filter(|s| s.call == "WXK36").count(),
            st.iter().any(|s| s.call == "WXK36") as usize
        );
    }

    /// WS_RADIO_LIVE=1 decodes ~2 s of a real stream (no audio output needed).
    #[test]
    fn decodes_live_stream() {
        if std::env::var("WS_RADIO_LIVE").is_err() {
            return;
        }
        use rodio::Source;
        let r = ureq::get("https://wxradio.org/OK-OklahomaCity-WXK85")
            .call()
            .unwrap()
            .into_body()
            .into_reader();
        let dec = rodio::Decoder::builder()
            .with_data(super::imp::NoSeekPub(r))
            .with_seekable(false)
            .with_hint("mp3")
            .build()
            .unwrap();
        let (ch, sr) = (dec.channels().get(), dec.sample_rate().get());
        let n = dec.take((sr * ch as u32 * 2) as usize).count();
        eprintln!("decoded {n} samples, {ch} ch @ {sr} Hz");
        assert!(n > 10_000);
    }
}
