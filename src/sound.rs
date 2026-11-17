//! Alert sounds. They're synthesized in Rust so the same sounds play on
//! native (rodio) and in the browser (Web Audio).

use std::f32::consts::TAU;

pub const RATE: u32 = 44_100;

fn tone(out: &mut Vec<f32>, secs: f32, mut f: impl FnMut(f32) -> f32) {
    let n = (secs * RATE as f32) as usize;
    out.extend((0..n).map(|i| f(i as f32 / RATE as f32)));
}

/// Built-in sound names (anything else plays nothing).
pub const SOUNDS: [&str; 6] = ["wea", "eas", "siren", "alarm", "beep", "chime"];

/// The EAS / WEA attention signal: 853 Hz and 960 Hz together.
fn attention(out: &mut Vec<f32>, secs: f32) {
    tone(out, secs, |t| {
        0.4 * ((TAU * 853.0 * t).sin() + (TAU * 960.0 * t).sin()) * env(t, secs)
    });
}

/// Short fade in/out so tones don't click.
fn env(t: f32, len: f32) -> f32 {
    (t / 0.01).min(1.0) * ((len - t) / 0.02).clamp(0.0, 1.0)
}

/// One play of a built-in sound, mono at 44.1 kHz.
pub fn synth(kind: &str) -> Vec<f32> {
    let mut v = Vec::new();
    match kind {
        // rising/falling outdoor warning siren
        "siren" => {
            let len = 3.0;
            let mut phase = 0.0f32;
            tone(&mut v, len, |t| {
                let f = 520.0 + 560.0 * (0.5 - 0.5 * (TAU * t / 1.5).cos());
                phase += TAU * f / RATE as f32;
                let s = phase.sin() + 0.35 * (2.0 * phase).sin() + 0.15 * (3.0 * phase).sin();
                0.45 * s * env(t, len)
            });
        }
        // Wireless Emergency Alert, the phone one (47 CFR 10.520): a 2 s tone, then two
        // 1 s tones, 0.5 s apart; the whole pattern twice, 0.5 s between them
        "wea" => {
            for rep in 0..2 {
                if rep > 0 {
                    tone(&mut v, 0.5, |_| 0.0);
                }
                for (i, secs) in [2.0, 1.0, 1.0].into_iter().enumerate() {
                    if i > 0 {
                        tone(&mut v, 0.5, |_| 0.0);
                    }
                    attention(&mut v, secs);
                }
            }
        }
        // the TV / radio Emergency Alert System attention signal: 8 s steady (47 CFR 11.31)
        "eas" => attention(&mut v, 8.0),
        // a short burst of the same two tones
        "alarm" => attention(&mut v, 2.0),
        "beep" => {
            for _ in 0..3 {
                tone(&mut v, 0.16, |t| {
                    0.6 * (TAU * 1050.0 * t).sin() * env(t, 0.16)
                });
                tone(&mut v, 0.09, |_| 0.0);
            }
        }
        "chime" => {
            for f0 in [880.0, 659.25] {
                tone(&mut v, 0.7, |t| {
                    let d = (-t * 5.0).exp();
                    0.5 * d
                        * ((TAU * f0 * t).sin() + 0.3 * (TAU * 2.0 * f0 * t).sin())
                        * (t / 0.005).min(1.0)
                });
            }
        }
        _ => {}
    }
    v
}

pub fn sequence(kind: &str, repeats: u32) -> Vec<f32> {
    let one = synth(kind);
    let gap = vec![0.0; (RATE as f32 * 0.35) as usize];
    let mut out = Vec::new();
    for _ in 0..repeats.max(1) {
        out.extend_from_slice(&one);
        out.extend_from_slice(&gap);
    }
    out
}

#[cfg(not(target_arch = "wasm32"))]
pub struct Player {
    sink: Option<rodio::MixerDeviceSink>,
    failed: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl Player {
    pub fn new() -> Self {
        Self {
            sink: None,
            failed: false,
        }
    }

    pub fn unlock(&mut self) {}

    /// Stop every alert sound that's playing.
    pub fn stop(&mut self) {
        self.sink = None; // dropping the output stops its mixer; the next play reopens it
    }

    fn sink(&mut self) -> Option<&rodio::MixerDeviceSink> {
        if self.sink.is_none() && !self.failed {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut s) => {
                    s.log_on_drop(false);
                    self.sink = Some(s);
                }
                Err(e) => {
                    log::warn!("no audio output: {e}");
                    self.failed = true;
                }
            }
        }
        self.sink.as_ref()
    }

    pub fn play(&mut self, kind: &str, file: Option<&str>, repeats: u32, volume: f32) {
        use rodio::Source;
        use std::num::NonZero;
        let Some(sink) = self.sink() else { return };
        if let Some(path) = file {
            let path = crate::config_path()
                .parent()
                .map(|d| d.join(path))
                .unwrap_or_else(|| path.into());
            let decode = |b: &Vec<u8>| {
                rodio::Decoder::new(std::io::Cursor::new(b.clone())).map_err(|e| e.to_string())
            };
            match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| decode(&b).map(|d| (b, d)))
            {
                Ok((bytes, first)) => {
                    // Queue the repeats back to back (needs a known length; otherwise play once).
                    let len = first.total_duration();
                    sink.mixer().add(first.amplify(volume));
                    if let Some(len) = len {
                        let step = len + std::time::Duration::from_millis(350);
                        for i in 1..repeats.max(1) {
                            if let Ok(d) = decode(&bytes) {
                                sink.mixer().add(d.amplify(volume).delay(step * i));
                            }
                        }
                    }
                    return;
                }
                Err(e) => log::warn!("can't play {}: {e}", path.display()),
            }
        }
        let samples = sequence(kind, repeats);
        if samples.is_empty() {
            return;
        }
        let buf = rodio::buffer::SamplesBuffer::new(
            NonZero::new(1).unwrap(),
            NonZero::new(RATE).unwrap(),
            samples,
        );
        sink.mixer().add(buf.amplify(volume));
    }
}

#[cfg(target_arch = "wasm32")]
pub struct Player {
    ctx: Option<web_sys::AudioContext>,
    playing: Vec<web_sys::AudioBufferSourceNode>,
}

#[cfg(target_arch = "wasm32")]
impl Player {
    pub fn new() -> Self {
        Self {
            ctx: None,
            playing: Vec::new(),
        }
    }

    /// Stop every alert sound that's playing.
    pub fn stop(&mut self) {
        for s in self.playing.drain(..) {
            #[allow(deprecated)]
            let _ = s.stop();
        }
    }

    /// Browsers only allow audio after a user gesture, so this is called on clicks.
    pub fn unlock(&mut self) {
        if self.ctx.is_none() {
            self.ctx = web_sys::AudioContext::new().ok();
        }
        if let Some(c) = &self.ctx
            && c.state() == web_sys::AudioContextState::Suspended
        {
            let _ = c.resume();
        }
    }

    pub fn play(&mut self, kind: &str, _file: Option<&str>, repeats: u32, volume: f32) {
        self.unlock();
        let Some(c) = &self.ctx else { return };
        let samples = sequence(kind, repeats);
        if samples.is_empty() {
            return;
        }
        let run = || -> Result<web_sys::AudioBufferSourceNode, wasm_bindgen::JsValue> {
            let buf = c.create_buffer(1, samples.len() as u32, RATE as f32)?;
            buf.copy_to_channel(&samples, 0)?;
            let src = c.create_buffer_source()?;
            src.set_buffer(Some(&buf));
            let gain = c.create_gain()?;
            gain.gain().set_value(volume);
            src.connect_with_audio_node(&gain)?;
            gain.connect_with_audio_node(&c.destination())?;
            src.start()?;
            Ok(src)
        };
        match run() {
            Ok(src) => {
                if self.playing.len() > 8 {
                    self.playing.remove(0);
                }
                self.playing.push(src);
            }
            Err(e) => log::warn!("audio: {e:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wea_follows_the_fcc_cadence() {
        let v = synth("wea");
        let secs = v.len() as f32 / RATE as f32;
        // (2 + 0.5 + 1 + 0.5 + 1) × 2 + 0.5
        assert!((secs - 10.5).abs() < 0.01, "{secs}");
        let loud = |t: f32| {
            let i = (t * RATE as f32) as usize;
            v[i..i + 200].iter().any(|s| s.abs() > 0.1)
        };
        assert!(loud(1.0) && !loud(2.2) && loud(3.0) && !loud(3.7) && loud(4.5));
        assert!(!loud(5.2) && loud(6.5), "second round");
        assert!((synth("eas").len() as f32 / RATE as f32 - 8.0).abs() < 0.01);
        for s in SOUNDS {
            assert!(!synth(s).is_empty(), "{s}");
        }
    }
}
