//! NEXRAD Level II (Archive II) decoder.
//!
//! A volume file is a 24-byte header followed by LDM records, each a 4-byte
//! big-endian length and a bzip2 block. Decompressed, the records hold radar
//! messages; we only care about message 31 (one radial of every moment).

use std::io::Read;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Moment {
    Ref,
    Vel,
    Sw,
    Zdr,
    Phi,
    Rho,
}

impl Moment {
    pub const ALL: [Moment; 6] = [
        Moment::Ref,
        Moment::Vel,
        Moment::Rho,
        Moment::Zdr,
        Moment::Sw,
        Moment::Phi,
    ];

    fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"REF" => Moment::Ref,
            b"VEL" => Moment::Vel,
            b"SW " => Moment::Sw,
            b"ZDR" => Moment::Zdr,
            b"PHI" => Moment::Phi,
            b"RHO" => Moment::Rho,
            _ => return None,
        })
    }

    /// Name used in the Lua config and the UI.
    pub fn key(self) -> &'static str {
        match self {
            Moment::Ref => "REF",
            Moment::Vel => "VEL",
            Moment::Sw => "SW",
            Moment::Zdr => "ZDR",
            Moment::Phi => "PHI",
            Moment::Rho => "CC",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.key().eq_ignore_ascii_case(s))
            .or(match s.to_ascii_uppercase().as_str() {
                "RHO" => Some(Moment::Rho),
                "BR" | "DBZ" => Some(Moment::Ref),
                "BV" => Some(Moment::Vel),
                _ => None,
            })
    }

    pub fn title(self) -> &'static str {
        match self {
            Moment::Ref => "reflectivity",
            Moment::Vel => "velocity",
            Moment::Sw => "spectrum width",
            Moment::Zdr => "differential refl.",
            Moment::Phi => "differential phase",
            Moment::Rho => "correlation coeff.",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One moment along one radial. Raw gate values: 0 = below threshold, 1 = range folded.
#[derive(Clone)]
pub struct Gates {
    pub first_m: f32,
    pub spacing_m: f32,
    pub scale: f32,
    pub offset: f32,
    pub word16: bool,
    pub data: Vec<u8>,
}

impl Gates {
    pub fn len(&self) -> usize {
        if self.word16 {
            self.data.len() / 2
        } else {
            self.data.len()
        }
    }

    pub fn raw(&self, i: usize) -> u16 {
        if self.word16 {
            u16::from_be_bytes([self.data[2 * i], self.data[2 * i + 1]])
        } else {
            self.data[i] as u16
        }
    }

    pub fn decode(&self, raw: u16) -> Option<f32> {
        (raw >= 2).then(|| (raw as f32 - self.offset) / self.scale)
    }

    /// Value at a slant range in metres.
    pub fn at_range(&self, r_m: f32) -> Option<f32> {
        let g = (r_m - self.first_m) / self.spacing_m + 0.5;
        if g < 0.0 {
            return None;
        }
        let g = g as usize;
        if g >= self.len() {
            return None;
        }
        self.decode(self.raw(g))
    }

    pub fn max_range_m(&self) -> f32 {
        self.first_m + self.spacing_m * self.len() as f32
    }
}

#[derive(Clone)]
pub struct Radial {
    pub azimuth: f32,
    /// when it was collected, seconds since the Unix epoch
    pub time: f64,
    pub spacing: f32,
    pub elevation: f32,
    pub(crate) moments: [Option<Gates>; 6],
}

impl Radial {
    pub fn get(&self, m: Moment) -> Option<&Gates> {
        self.moments[m.index()].as_ref()
    }
}

#[derive(Clone)]
pub struct Sweep {
    pub number: u8,
    pub elevation: f32,
    pub radials: Vec<Radial>,
}

impl Sweep {
    pub fn has(&self, m: Moment) -> bool {
        self.radials.iter().take(20).any(|r| r.get(m).is_some())
    }
}

#[derive(Clone)]
pub struct Volume {
    pub site: String,
    pub lat: f64,
    pub lon: f64,
    pub vcp: u16,
    /// Seconds since the Unix epoch.
    pub time: i64,
    /// Finished sweeps, lowest tilt first. Shared, so partial snapshots are cheap.
    pub sweeps: Vec<Arc<Sweep>>,
    /// Where the antenna was at the last radial decoded (live data only).
    pub antenna: Option<Antenna>,
}

/// The tilt the antenna was scanning at the newest radial, and when.
#[derive(Clone, Copy, Debug)]
pub struct Antenna {
    pub elevation: f32,
    /// seconds since the Unix epoch
    pub time: f64,
}

impl Volume {
    /// Distinct elevation angles (rounded to 0.1°), lowest first.
    pub fn tilts(&self) -> Vec<f32> {
        let mut t: Vec<f32> = self
            .sweeps
            .iter()
            .map(|s| (s.elevation * 10.0).round() / 10.0)
            .collect();
        t.sort_by(|a, b| a.partial_cmp(b).unwrap());
        t.dedup();
        t
    }

    /// The sweep to show for a moment at a tilt. At split cuts the first sweep
    /// has the longest-range reflectivity; velocity comes from the Doppler cut.
    pub fn sweep_for(&self, m: Moment, tilt: f32) -> Option<&Sweep> {
        let sweeps = || self.sweeps.iter().map(|s| &**s);
        sweeps()
            .find(|s| (s.elevation - tilt).abs() < 0.25 && s.has(m))
            .or_else(|| {
                sweeps().filter(|s| s.has(m)).min_by(|a, b| {
                    (a.elevation - tilt)
                        .abs()
                        .partial_cmp(&(b.elevation - tilt).abs())
                        .unwrap()
                })
            })
    }
}

fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}
fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn bef32(b: &[u8], o: usize) -> f32 {
    f32::from_bits(be32(b, o))
}

/// Decode a whole file at once.
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse(bytes: &[u8]) -> Result<Volume, String> {
    let mut d = Decoder::new();
    d.push(bytes)?;
    d.finish()
}

/// Incremental decoder: feed it the file as it downloads. Sweeps come out
/// lowest tilt first, so the 0.5° scan is usable long before the file ends.
pub struct Decoder {
    gz: Option<flate2::write::MultiGzDecoder<Vec<u8>>>,
    sniffed: Vec<u8>,
    raw: Vec<u8>,
    pos: usize,
    header: bool,
    /// None until we've seen what follows the header: Some(true) = bzip2 LDM
    /// records (2008+), Some(false) = a plain stream of messages (older files).
    ldm: Option<bool>,
    vol: Volume,
    cur: Option<Sweep>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            gz: None,
            sniffed: Vec::new(),
            raw: Vec::new(),
            pos: 0,
            header: false,
            ldm: None,
            vol: Volume {
                site: String::new(),
                lat: 0.0,
                lon: 0.0,
                vcp: 0,
                time: 0,
                sweeps: Vec::new(),
                antenna: None,
            },
            cur: None,
        }
    }

    /// Completed sweeps so far.
    pub fn sweeps_done(&self) -> usize {
        self.vol.sweeps.len()
    }

    /// Feed more bytes. Old archive files are gzipped as a whole, so unzip on the fly.
    pub fn push(&mut self, data: &[u8]) -> Result<(), String> {
        if !self.header && self.gz.is_none() && self.raw.is_empty() && self.sniffed.len() < 2 {
            self.sniffed.extend_from_slice(data);
            if self.sniffed.len() < 2 {
                return Ok(());
            }
            let first = std::mem::take(&mut self.sniffed);
            if first.starts_with(&[0x1f, 0x8b]) {
                self.gz = Some(flate2::write::MultiGzDecoder::new(Vec::new()));
            }
            return self.push_inner(&first);
        }
        self.push_inner(data)
    }

    fn push_inner(&mut self, data: &[u8]) -> Result<(), String> {
        match &mut self.gz {
            Some(gz) => {
                use std::io::Write;
                gz.write_all(data).map_err(|e| format!("gunzip: {e}"))?;
                let out = std::mem::take(gz.get_mut());
                self.raw.extend_from_slice(&out);
            }
            None => self.raw.extend_from_slice(data),
        }
        self.records()
    }

    /// Process every complete LDM record in the buffer.
    fn records(&mut self) -> Result<(), String> {
        if !self.header {
            if self.raw.len() < 24 {
                return Ok(());
            }
            if &self.raw[..4] != b"AR2V" {
                return Err("not a Level II file".into());
            }
            let julian = be32(&self.raw, 12) as i64;
            let ms = be32(&self.raw, 16) as i64;
            self.vol.site = String::from_utf8_lossy(&self.raw[20..24])
                .trim()
                .to_string();
            self.vol.time = (julian - 1) * 86400 + ms / 1000;
            self.header = true;
            self.pos = 24;
        }
        if self.ldm.is_none() {
            if self.raw.len() < self.pos + 8 {
                return Ok(());
            }
            let p = self.pos;
            let compressed = &self.raw[p + 4..p + 7] == b"BZh";
            self.ldm = Some(compressed || be32(&self.raw, p) != 0);
        }
        if self.ldm == Some(false) {
            // one message at a time: msg 31 has its own length, the rest are 2432 bytes
            while self.pos + 28 <= self.raw.len() {
                let h = self.pos + 12;
                let size_hw = be16(&self.raw, h) as usize;
                let len = if self.raw[h + 3] == 31 && size_hw > 0 {
                    12 + size_hw * 2
                } else {
                    2432
                };
                if self.pos + len > self.raw.len() {
                    break;
                }
                let msg = self.raw[self.pos..self.pos + len].to_vec();
                self.pos += len;
                self.messages(&msg);
            }
        }
        while self.ldm == Some(true) {
            if self.pos + 4 > self.raw.len() {
                break;
            }
            let size = (be32(&self.raw, self.pos) as i32).unsigned_abs() as usize;
            if size == 0 {
                self.pos += 4;
                continue;
            }
            if self.pos + 4 + size > self.raw.len() {
                break; // wait for the rest of this record
            }
            let chunk = self.raw[self.pos + 4..self.pos + 4 + size].to_vec();
            self.pos += 4 + size;
            if chunk.starts_with(b"BZh") {
                let mut buf = Vec::with_capacity(size * 6);
                bzip2::read::MultiBzDecoder::new(&chunk[..])
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("bzip2: {e}"))?;
                self.messages(&buf);
            } else {
                self.messages(&chunk);
            }
        }
        // Drop consumed input now and then so the buffer doesn't hold the whole file.
        if self.pos > 1 << 20 {
            self.raw.drain(..self.pos);
            self.pos = 0;
        }
        Ok(())
    }

    fn messages(&mut self, buf: &[u8]) {
        let mut off = 0;
        while off + 28 <= buf.len() {
            let h = off + 12;
            let size_hw = be16(buf, h) as usize;
            let mtype = buf[h + 3];
            if mtype == 31 && size_hw > 0 {
                let len = size_hw * 2;
                let end = (h + len).min(buf.len());
                if let Some((radial, number, status, site)) =
                    parse_m31(&buf[h + 16..end], &mut self.vol)
                {
                    if self.vol.site.is_empty() {
                        self.vol.site = site;
                    }
                    if self.cur.as_ref().is_some_and(|c| c.number != number) {
                        self.close_sweep();
                    }
                    self.vol.antenna = Some(Antenna {
                        elevation: radial.elevation,
                        time: radial.time,
                    });
                    self.cur
                        .get_or_insert_with(|| Sweep {
                            number,
                            elevation: radial.elevation,
                            radials: Vec::new(),
                        })
                        .radials
                        .push(radial);
                    // 2 = end of elevation, 4 = end of volume
                    if status == 2 || status == 4 {
                        self.close_sweep();
                    }
                }
                off = h + len;
            } else {
                off += 2432;
            }
        }
    }

    fn close_sweep(&mut self) {
        let Some(mut s) = self.cur.take() else { return };
        if s.radials.len() <= 100 {
            return;
        }
        let n = s.radials.len() as f32;
        s.elevation = s.radials.iter().map(|r| r.elevation).sum::<f32>() / n;
        s.radials
            .sort_by(|a, b| a.azimuth.partial_cmp(&b.azimuth).unwrap());
        self.vol.sweeps.push(Arc::new(s));
    }

    /// The sweep being decoded right now, radials in the order they were collected.
    pub fn in_progress(&self) -> Option<&Sweep> {
        self.cur.as_ref()
    }

    /// The volume so far (site, time, finished sweeps), even with no sweep finished yet.
    pub fn volume(&self) -> &Volume {
        &self.vol
    }

    /// The completed sweeps so far, as a volume (cheap: sweeps are shared).
    pub fn snapshot(&self) -> Option<Volume> {
        (!self.vol.sweeps.is_empty()).then(|| self.vol.clone())
    }

    pub fn finish(mut self) -> Result<Volume, String> {
        if let Some(gz) = self.gz.take() {
            let rest = gz.finish().map_err(|e| format!("gunzip: {e}"))?;
            self.raw.extend_from_slice(&rest);
            self.records()?;
        }
        self.close_sweep();
        if self.vol.sweeps.is_empty() {
            return Err("no radar data in file".into());
        }
        Ok(self.vol)
    }
}

fn parse_m31(b: &[u8], vol: &mut Volume) -> Option<(Radial, u8, u8, String)> {
    if b.len() < 32 {
        return None;
    }
    let site = String::from_utf8_lossy(&b[0..4]).to_string();
    // collection time: ms after midnight, then the day (1 = 1970-01-01)
    let time = (be16(b, 8) as f64 - 1.0) * 86400.0 + be32(b, 4) as f64 / 1000.0;
    let azimuth = bef32(b, 12);
    let spacing = match b[20] {
        1 => 0.5,
        _ => 1.0,
    };
    let number = b[22];
    let elevation = bef32(b, 24);
    let count = (be16(b, 30) as usize).min(10);
    let mut radial = Radial {
        azimuth,
        time,
        spacing,
        elevation,
        moments: Default::default(),
    };
    for i in 0..count {
        let p = be32(b, 32 + 4 * i) as usize;
        if p + 28 > b.len() {
            continue;
        }
        let blk = &b[p..];
        match (blk[0], &blk[1..4]) {
            (b'R', b"VOL") if blk.len() >= 42 => {
                if vol.lat == 0.0 {
                    vol.lat = bef32(blk, 8) as f64;
                    vol.lon = bef32(blk, 12) as f64;
                    vol.vcp = be16(blk, 40);
                }
            }
            (b'D', name) => {
                let Some(m) = Moment::from_name(name) else {
                    continue;
                };
                let ngates = be16(blk, 8) as usize;
                let word16 = blk[19] == 16;
                let nbytes = ngates * if word16 { 2 } else { 1 };
                if 28 + nbytes > blk.len() {
                    continue;
                }
                radial.moments[m.index()] = Some(Gates {
                    first_m: be16(blk, 10) as f32,
                    spacing_m: be16(blk, 12) as f32,
                    scale: bef32(blk, 20),
                    offset: bef32(blk, 24),
                    word16,
                    data: blk[28..28 + nbytes].to_vec(),
                });
            }
            _ => {}
        }
    }
    Some((radial, number, b[21], site))
}
