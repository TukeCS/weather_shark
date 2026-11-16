//! Basemap (US states + counties, 2010 Census 1:20m), radar site list and the
//! flat projection used for drawing everything around the current radar.

use egui::{Pos2, Rect, pos2};

static STATES: &[u8] = include_bytes!("../assets/states.bin");
static COUNTIES: &[u8] = include_bytes!("../assets/counties.bin");
static SITES: &str = include_str!("../assets/sites.txt");

pub struct Site {
    pub id: String,
    pub lat: f64,
    pub lon: f64,
    pub name: String,
}

pub fn sites() -> Vec<Site> {
    SITES
        .lines()
        .filter_map(|l| {
            let mut p = l.split('|');
            Some(Site {
                id: p.next()?.to_string(),
                lat: p.next()?.parse().ok()?,
                lon: p.next()?.parse().ok()?,
                name: p.next()?.to_string(),
            })
        })
        .collect()
}

/// Polylines in degrees (lon, lat). See the asset format in README.
fn decode(b: &[u8]) -> Vec<Vec<(f64, f64)>> {
    let mut out = Vec::new();
    let mut o = 0;
    let i16_at = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]) as i32;
    let i32_at = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    while o + 10 <= b.len() {
        let n = u16::from_le_bytes([b[o], b[o + 1]]) as usize;
        let (mut x, mut y) = (i32_at(o + 2), i32_at(o + 6));
        o += 10;
        let mut line = Vec::with_capacity(n);
        line.push((x as f64 * 1e-4, y as f64 * 1e-4));
        for _ in 1..n {
            x += i16_at(o);
            y += i16_at(o + 2);
            o += 4;
            line.push((x as f64 * 1e-4, y as f64 * 1e-4));
        }
        out.push(line);
    }
    out
}

/// Equirectangular projection centred on a radar, in km (x east, y north).
#[derive(Clone, Copy)]
pub struct Proj {
    pub lat0: f64,
    pub lon0: f64,
    kx: f64,
}

const KY: f64 = 110.574;

impl Proj {
    pub fn new(lat0: f64, lon0: f64) -> Self {
        Self { lat0, lon0, kx: 111.320 * lat0.to_radians().cos() }
    }
    pub fn fwd(&self, lon: f64, lat: f64) -> Pos2 {
        pos2(((lon - self.lon0) * self.kx) as f32, ((lat - self.lat0) * KY) as f32)
    }
}

pub struct Layer {
    /// (bounding box, points) in km.
    pub lines: Vec<(Rect, Vec<Pos2>)>,
}

impl Layer {
    fn project(src: &[Vec<(f64, f64)>], p: &Proj) -> Self {
        Self {
            lines: src
                .iter()
                .map(|l| {
                    let pts: Vec<Pos2> = l.iter().map(|&(x, y)| p.fwd(x, y)).collect();
                    (Rect::from_points(&pts), pts)
                })
                .collect(),
        }
    }
}

pub struct Basemap {
    states: Vec<Vec<(f64, f64)>>,
    counties: Vec<Vec<(f64, f64)>>,
    pub proj: Proj,
    pub states_km: Layer,
    pub counties_km: Layer,
}

impl Basemap {
    pub fn new(lat0: f64, lon0: f64) -> Self {
        let states = decode(STATES);
        let counties = decode(COUNTIES);
        let proj = Proj::new(lat0, lon0);
        Self {
            states_km: Layer::project(&states, &proj),
            counties_km: Layer::project(&counties, &proj),
            states,
            counties,
            proj,
        }
    }

    pub fn recenter(&mut self, lat0: f64, lon0: f64) {
        self.proj = Proj::new(lat0, lon0);
        self.states_km = Layer::project(&self.states, &self.proj);
        self.counties_km = Layer::project(&self.counties, &self.proj);
    }
}
