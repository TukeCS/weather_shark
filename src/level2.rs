//! NEXRAD Level II (Archive II) decoder.
//!
//! A volume file is a 24-byte header followed by LDM records, each a 4-byte
//! big-endian length and a bzip2 block. Decompressed, the records hold radar
//! messages; we only care about message 31 (one radial of every moment).

use std::io::Read;

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

pub struct Radial {
    pub azimuth: f32,
    pub spacing: f32,
    pub elevation: f32,
    moments: [Option<Gates>; 6],
}

impl Radial {
    pub fn get(&self, m: Moment) -> Option<&Gates> {
        self.moments[m.index()].as_ref()
    }
}

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

pub struct Volume {
    pub site: String,
    pub lat: f64,
    pub lon: f64,
    pub vcp: u16,
    /// Seconds since the Unix epoch.
    pub time: i64,
    pub sweeps: Vec<Sweep>,
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
        let near = |s: &&Sweep| (s.elevation - tilt).abs() < 0.25 && s.has(m);
        self.sweeps.iter().find(near).or_else(|| {
            self.sweeps.iter().filter(|s| s.has(m)).min_by(|a, b| {
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

pub fn parse(bytes: &[u8]) -> Result<Volume, String> {
    // Old archive files in the bucket are gzipped as a whole.
    let unzipped;
    let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::MultiGzDecoder::new(bytes)
            .read_to_end(&mut out)
            .map_err(|e| format!("gunzip: {e}"))?;
        unzipped = out;
        &unzipped[..]
    } else {
        bytes
    };

    if bytes.len() < 24 || &bytes[..4] != b"AR2V" {
        return Err("not a Level II file".into());
    }
    let julian = be32(bytes, 12) as i64;
    let ms = be32(bytes, 16) as i64;
    let site = String::from_utf8_lossy(&bytes[20..24]).trim().to_string();

    let mut buf = Vec::with_capacity(bytes.len() * 6);
    let mut pos = 24;
    while pos + 4 <= bytes.len() {
        let size = (be32(bytes, pos) as i32).unsigned_abs() as usize;
        pos += 4;
        if size == 0 || pos + size > bytes.len() {
            break;
        }
        let chunk = &bytes[pos..pos + size];
        if chunk.starts_with(b"BZh") {
            bzip2::read::MultiBzDecoder::new(chunk)
                .read_to_end(&mut buf)
                .map_err(|e| format!("bzip2: {e}"))?;
        } else {
            buf.extend_from_slice(chunk);
        }
        pos += size;
    }
    if buf.is_empty() {
        // Pre-2008 style uncompressed files
        buf.extend_from_slice(&bytes[24..]);
    }

    let mut vol = Volume {
        site,
        lat: 0.0,
        lon: 0.0,
        vcp: 0,
        time: (julian - 1) * 86400 + ms / 1000,
        sweeps: Vec::new(),
    };

    let mut off = 0;
    while off + 28 <= buf.len() {
        let h = off + 12;
        let size_hw = be16(&buf, h) as usize;
        let mtype = buf[h + 3];
        if mtype == 31 && size_hw > 0 {
            let len = size_hw * 2;
            let end = (h + len).min(buf.len());
            if let Some((radial, number, site)) = parse_m31(&buf[h + 16..end], &mut vol) {
                if vol.site.is_empty() {
                    vol.site = site;
                }
                match vol.sweeps.last_mut() {
                    Some(s) if s.number == number => s.radials.push(radial),
                    _ => vol.sweeps.push(Sweep {
                        number,
                        elevation: radial.elevation,
                        radials: vec![radial],
                    }),
                }
            }
            off = h + len;
        } else {
            off += 2432;
        }
    }

    for s in &mut vol.sweeps {
        let n = s.radials.len() as f32;
        s.elevation = s.radials.iter().map(|r| r.elevation).sum::<f32>() / n;
        s.radials
            .sort_by(|a, b| a.azimuth.partial_cmp(&b.azimuth).unwrap());
    }
    vol.sweeps.retain(|s| s.radials.len() > 100);
    if vol.sweeps.is_empty() {
        return Err("no radar data in file".into());
    }
    Ok(vol)
}

fn parse_m31(b: &[u8], vol: &mut Volume) -> Option<(Radial, u8, String)> {
    if b.len() < 32 {
        return None;
    }
    let site = String::from_utf8_lossy(&b[0..4]).to_string();
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
    Some((radial, number, site))
}
