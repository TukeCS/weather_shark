//! Turn one sweep of one moment into an image centred on the radar.

use egui::{Color32, ColorImage};

use crate::config::Palette;
use crate::level2::{Moment, Sweep};

/// Azimuth (0.1° bins) -> radial index, so each pixel finds its radial in O(1).
fn az_table(s: &Sweep) -> Vec<u16> {
    let mut t = vec![u16::MAX; 3600];
    for (i, r) in s.radials.iter().enumerate() {
        let half = r.spacing * 0.5 + 0.05;
        let a = ((r.azimuth - half) * 10.0).floor() as i32;
        let b = ((r.azimuth + half) * 10.0).ceil() as i32;
        for k in a..b {
            t[k.rem_euclid(3600) as usize] = i as u16;
        }
    }
    t
}

/// Max range (km) of a moment in a sweep.
pub fn range_km(s: &Sweep, m: Moment) -> f32 {
    s.radials.iter().filter_map(|r| r.get(m)).map(|g| g.max_range_m()).fold(0.0, f32::max) / 1000.0
}

/// Rasterize to a `size`×`size` image spanning ±`range` km.
/// `factor` converts native units to the palette's units (m/s -> kt etc).
pub fn rasterize(s: &Sweep, m: Moment, pal: &Palette, factor: f32, size: usize, range: f32) -> ColorImage {
    let table = az_table(s);
    let cos_el = s.elevation.to_radians().cos().max(0.2);
    let rf = if m == Moment::Vel { pal.range_folded } else { Color32::TRANSPARENT };

    // Colour lookup per raw byte for each distinct (scale, offset) – almost always one.
    let mut luts: Vec<((u32, u32), Vec<Color32>)> = Vec::new();
    let lut_idx: Vec<Option<usize>> = s
        .radials
        .iter()
        .map(|r| {
            let g = r.get(m)?;
            if g.word16 {
                return None;
            }
            let key = (g.scale.to_bits(), g.offset.to_bits());
            Some(luts.iter().position(|(k, _)| *k == key).unwrap_or_else(|| {
                let lut = (0..256u16)
                    .map(|raw| match raw {
                        0 => Color32::TRANSPARENT,
                        1 => rf,
                        _ => pal.color(g.decode(raw).unwrap() * factor),
                    })
                    .collect();
                luts.push((key, lut));
                luts.len() - 1
            }))
        })
        .collect();

    let mut px = vec![Color32::TRANSPARENT; size * size];
    let km_per_px = 2.0 * range / size as f32;
    for j in 0..size {
        let y = range - (j as f32 + 0.5) * km_per_px;
        let row = &mut px[j * size..(j + 1) * size];
        for (i, out) in row.iter_mut().enumerate() {
            let x = (i as f32 + 0.5) * km_per_px - range;
            let r2 = x * x + y * y;
            if r2 > range * range {
                continue;
            }
            let az = x.atan2(y).to_degrees();
            let bin = ((az * 10.0) as i32).rem_euclid(3600) as usize;
            let ri = table[bin];
            if ri == u16::MAX {
                continue;
            }
            let rad = &s.radials[ri as usize];
            let Some(g) = rad.get(m) else { continue };
            let r_m = r2.sqrt() * 1000.0 / cos_el;
            if let Some(min) = pal.mask_ref {
                if rad.get(Moment::Ref).and_then(|z| z.at_range(r_m)).is_none_or(|z| z < min) {
                    continue;
                }
            }
            let gi = ((r_m - g.first_m) / g.spacing_m + 0.5) as isize;
            if gi < 0 || gi as usize >= g.len() {
                continue;
            }
            *out = match lut_idx[ri as usize] {
                Some(l) => luts[l].1[g.data[gi as usize] as usize],
                None => match g.raw(gi as usize) {
                    0 => Color32::TRANSPARENT,
                    1 => rf,
                    raw => pal.color(g.decode(raw).unwrap() * factor),
                },
            };
        }
    }
    ColorImage::new([size, size], px)
}

/// Value (in palette units) at a point `x`,`y` km from the radar.
pub fn sample(s: &Sweep, m: Moment, factor: f32, mask: Option<f32>, x: f32, y: f32) -> Option<f32> {
    let az = x.atan2(y).to_degrees().rem_euclid(360.0);
    let i = s.radials.partition_point(|r| r.azimuth < az);
    let n = s.radials.len();
    let cands = [s.radials.get(i % n)?, &s.radials[(i + n - 1) % n]];
    let d = |a: f32| {
        let d = (a - az).abs();
        d.min(360.0 - d)
    };
    let r = if d(cands[0].azimuth) < d(cands[1].azimuth) { cands[0] } else { cands[1] };
    let cos_el = s.elevation.to_radians().cos().max(0.2);
    let r_m = (x * x + y * y).sqrt() * 1000.0 / cos_el;
    if let Some(min) = mask {
        if r.get(Moment::Ref).and_then(|z| z.at_range(r_m)).is_none_or(|z| z < min) {
            return None;
        }
    }
    r.get(m)?.at_range(r_m).map(|v| v * factor)
}
