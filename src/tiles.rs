//! Vector tile cache: decides which tiles a view needs, fetches them a few
//! at a time and falls back to already-loaded parent tiles while they load.

use std::collections::HashMap;
use std::sync::Arc;

use crate::mvt::TileData;

pub type Key = (u8, u32, u32);

pub const MAX_Z: u8 = 14;
const MAX_INFLIGHT: usize = 8;
const KEEP: usize = 500;

enum Slot {
    Loading,
    Ready(Arc<TileData>),
    Failed,
}

#[derive(Default)]
pub struct Tiles {
    /// URL template with {z}/{x}/{y}, from the TileJSON.
    pub template: Option<String>,
    pub json_requested: bool,
    pub error: Option<String>,
    slots: HashMap<Key, (Slot, u64)>,
    inflight: usize,
    frame: u64,
}

impl Tiles {
    pub fn next_frame(&mut self) {
        self.frame += 1;
    }

    pub fn ready(&mut self, k: Key) -> Option<Arc<TileData>> {
        match self.slots.get_mut(&k) {
            Some((Slot::Ready(t), used)) => {
                *used = self.frame;
                Some(t.clone())
            }
            _ => None,
        }
    }

    /// The tile itself if loaded, otherwise the closest loaded ancestor.
    pub fn best(&mut self, (z, x, y): Key) -> Option<(Key, Arc<TileData>)> {
        for up in 0..=6u8.min(z) {
            let k = (z - up, x >> up, y >> up);
            if let Some(t) = self.ready(k) {
                return Some((k, t));
            }
        }
        None
    }

    /// Keys still to fetch, nearest the middle of the list first. The caller fetches them.
    pub fn wanted(&mut self, keys: &[Key]) -> Vec<Key> {
        let mut out = Vec::new();
        for &k in keys {
            if self.inflight + out.len() >= MAX_INFLIGHT {
                break;
            }
            match self.slots.get_mut(&k) {
                None => out.push(k),
                Some((_, used)) => *used = self.frame,
            }
        }
        for &k in &out {
            self.slots.insert(k, (Slot::Loading, self.frame));
        }
        self.inflight += out.len();
        out
    }

    pub fn done(&mut self, k: Key, t: Result<TileData, String>) {
        self.inflight = self.inflight.saturating_sub(1);
        let slot = match t {
            Ok(t) => Slot::Ready(Arc::new(t)),
            Err(e) => {
                log::warn!("tile {k:?}: {e}");
                Slot::Failed
            }
        };
        self.slots.insert(k, (slot, self.frame));
        if self.slots.len() > KEEP + 100 {
            let mut v: Vec<(Key, u64)> = self
                .slots
                .iter()
                .filter(|(_, (s, _))| !matches!(s, Slot::Loading))
                .map(|(k, (_, u))| (*k, *u))
                .collect();
            v.sort_by_key(|(_, u)| *u);
            for (k, _) in v.iter().take(self.slots.len() - KEEP) {
                self.slots.remove(k);
            }
        }
    }

    pub fn failed_count(&self) -> usize {
        self.slots
            .values()
            .filter(|(s, _)| matches!(s, Slot::Failed))
            .count()
    }
}

/// Tile zoom for a view scale (px per km) at a latitude, aiming for ~512 px tiles.
pub fn zoom_for(px_per_km: f32, lat: f64) -> u8 {
    let world_km = 40_075.016 * lat.to_radians().cos();
    let z = (world_km * px_per_km as f64 / 512.0).log2().floor();
    z.clamp(2.0, MAX_Z as f64) as u8
}

pub fn tile_x(lon: f64, z: u8) -> u32 {
    let n = (1u32 << z) as f64;
    (((lon + 180.0) / 360.0 * n).floor()).clamp(0.0, n - 1.0) as u32
}

pub fn tile_y(lat: f64, z: u8) -> u32 {
    let n = (1u32 << z) as f64;
    let lat = lat.clamp(-85.0, 85.0).to_radians();
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    y.floor().clamp(0.0, n - 1.0) as u32
}

/// Tiles covering a lon/lat box, centre first. Drops detail if there would be too many.
pub fn cover(mut z: u8, west: f64, south: f64, east: f64, north: f64) -> (u8, Vec<Key>) {
    loop {
        let (x0, x1) = (tile_x(west, z), tile_x(east, z));
        let (y0, y1) = (tile_y(north, z), tile_y(south, z));
        let count = (x1 - x0 + 1) as usize * (y1 - y0 + 1) as usize;
        if count <= 48 || z <= 2 {
            let (cx, cy) = ((x0 + x1) as f32 / 2.0, (y0 + y1) as f32 / 2.0);
            let mut keys: Vec<Key> = (y0..=y1)
                .flat_map(|y| (x0..=x1).map(move |x| (z, x, y)))
                .collect();
            keys.sort_by(|a, b| {
                let d = |k: &Key| (k.1 as f32 - cx).powi(2) + (k.2 as f32 - cy).powi(2);
                d(a).partial_cmp(&d(b)).unwrap()
            });
            return (z, keys);
        }
        z -= 1;
    }
}
