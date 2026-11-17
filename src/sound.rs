//! Alert sounds. They're synthesized in Rust so the same sounds play on
//! native (rodio) and in the browser (Web Audio).

use std::f32::consts::TAU;

pub const RATE: u32 = 44_100;

fn tone(out: &mut Vec<f32>, secs: f32, mut f: impl FnMut(f32) -> f32) {
    let n = (secs * RATE as f32) as usize;
    out.extend((0..n).map(|i| f(i as f32 / RATE as f32)));
}

/// Built-in sound names (anything else plays nothing). Your own sounds come from
/// `alerts.custom_sounds` and the `sounds` folder next to config.lua.
pub const SOUNDS: [&str; 3] = ["alarm", "beep", "chime"];

/// A built-in sound by name, case-insensitive. The sounds that were removed
/// ("eas", "wea", "siren") play the alarm, so older configs keep working.
pub fn builtin(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    match n.as_str() {
        "eas" | "wea" | "siren" => Some("alarm"),
        _ => SOUNDS.iter().copied().find(|s| *s == n),
    }
}

/// Audio files the native player can decode.
#[cfg(not(target_arch = "wasm32"))]
pub const FILE_TYPES: [&str; 2] = ["wav", "mp3"];

/// Your own sounds dropped into `dir` (the `sounds` folder next to config.lua),
/// as (name, path): `klaxon.wav` is the sound "klaxon". Sorted by name.
#[cfg(not(target_arch = "wasm32"))]
pub fn sounds_in(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let ext = p.extension()?.to_str()?.to_ascii_lowercase();
            FILE_TYPES.contains(&ext.as_str()).then_some(())?;
            Some((p.file_stem()?.to_str()?.to_lowercase(), p.display().to_string()))
        })
        .collect();
    out.sort();
    out
}

/// The alarm: the two-tone 853 + 960 Hz attention signal.
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
        // a 2 s burst of the two-tone attention signal: the main warning sound
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
    fn builtins_and_old_names() {
        for s in SOUNDS {
            assert!(!synth(s).is_empty(), "{s}");
        }
        assert!((synth("alarm").len() as f32 / RATE as f32 - 2.0).abs() < 0.01);
        assert_eq!(builtin("EAS"), Some("alarm"), "removed sounds fall back to the alarm");
        assert_eq!(builtin("chime"), Some("chime"));
        assert_eq!(builtin("klaxon"), None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn finds_sounds_in_a_folder() {
        let dir = std::env::temp_dir().join(format!("ws_sounds_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["Klaxon.wav", "horn.mp3", "notes.txt"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let names: Vec<String> = sounds_in(&dir).into_iter().map(|(n, _)| n).collect();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(names, ["horn", "klaxon"]);
    }
}
