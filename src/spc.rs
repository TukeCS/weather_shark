//! SPC convective outlooks (spc.noaa.gov GeoJSON): the categorical risk
//! (TSTM … HIGH) and the tornado / wind / hail probabilities for days 1–3, and the
//! day 4–8 outlooks (15% / 30% chance of severe weather only).

use egui::Color32;

use crate::net::days_from_civil;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Categorical,
    Tornado,
    Wind,
    Hail,
    /// Days 3–8 only have one probability: any severe weather.
    Severe,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Categorical => "categorical",
            Kind::Tornado => "tornado",
            Kind::Wind => "wind",
            Kind::Hail => "hail",
            Kind::Severe => "severe",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "categorical" | "cat" => Kind::Categorical,
            "tornado" | "torn" => Kind::Tornado,
            "wind" => Kind::Wind,
            "hail" => Kind::Hail,
            "severe" | "prob" => Kind::Severe,
            _ => return None,
        })
    }

    fn file(self) -> &'static str {
        match self {
            Kind::Categorical => "cat",
            Kind::Tornado => "torn",
            Kind::Wind => "wind",
            Kind::Hail => "hail",
            Kind::Severe => "prob",
        }
    }

    /// The products SPC issues for a day.
    pub fn for_day(day: u8) -> &'static [Kind] {
        if day >= 4 {
            &[Kind::Severe]
        } else if day == 3 {
            &[Kind::Categorical, Kind::Severe]
        } else {
            &[Kind::Categorical, Kind::Tornado, Kind::Wind, Kind::Hail]
        }
    }

    /// The same product on another day, or categorical when that day doesn't have it.
    pub fn on_day(self, day: u8) -> Kind {
        let k = match (self, day >= 3) {
            (Kind::Tornado | Kind::Wind | Kind::Hail, true) => Kind::Severe,
            (Kind::Severe, false) => Kind::Categorical,
            (k, _) => k,
        };
        let kinds = Kind::for_day(day);
        if kinds.contains(&k) { k } else { kinds[0] }
    }
}

/// Which outlook to show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sel {
    pub day: u8,
    pub kind: Kind,
}

impl Sel {
    pub fn url(self) -> String {
        if self.day >= 4 {
            return format!(
                "https://www.spc.noaa.gov/products/exper/day4-8/day{}prob.lyr.geojson",
                self.day
            );
        }
        format!(
            "https://www.spc.noaa.gov/products/outlook/day{}otlk_{}.lyr.geojson",
            self.day,
            self.kind.file()
        )
    }
}

pub struct Area {
    /// "MRGL", "ENH", "10%" …
    pub label: String,
    /// "Enhanced Risk", "10% Tornado Risk" …
    pub name: String,
    pub color: Color32,
    /// the hatched "significant" (EF2+ / 65 kt+ / 2 in+) area
    pub sig: bool,
    /// rings as (lon, lat): outer edges and holes, for the outline
    pub rings: Vec<Vec<(f64, f64)>>,
    /// the area as triangles, for the fill: vertices (lon, lat) and indices into them
    pub fill_pts: Vec<(f64, f64)>,
    pub fill_idx: Vec<u32>,
}

impl Area {
    /// The key used for it in the Lua `outlook.colors` table: "MRGL", "10%", "SIG" …
    pub fn key(&self) -> &str {
        &self.label
    }
}

pub struct Outlook {
    pub sel: Sel,
    pub issued: i64,
    pub valid: i64,
    pub expire: i64,
    /// lowest risk first, so higher risks draw on top
    pub areas: Vec<Area>,
}

impl Outlook {
    /// Whether it makes sense over a scan taken at `t`: not for scans from before it was
    /// issued (archived cases) or after it ran out.
    pub fn covers(&self, t: i64) -> bool {
        t >= self.issued - 6 * 3600 && t <= self.expire
    }
}

/// "202611171200" → epoch seconds
fn spc_time(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let day = days_from_civil(n(0, 4)?, n(4, 6)? as u32, n(6, 8)? as u32);
    Some(day * 86400 + n(8, 10)? * 3600 + n(10, 12)? * 60)
}

fn hex(s: &str) -> Option<Color32> {
    let s = s.strip_prefix('#')?;
    let v = u32::from_str_radix(s, 16).ok().filter(|_| s.len() == 6)?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Append one polygon (outer ring + holes) as triangles.
fn triangulate(rings: &[Vec<(f64, f64)>], pts: &mut Vec<(f64, f64)>, idx: &mut Vec<u32>) {
    let mut flat = Vec::new();
    let mut holes = Vec::new();
    for (i, r) in rings.iter().enumerate() {
        if i > 0 {
            holes.push(flat.len() / 2);
        }
        // GeoJSON repeats the first point at the end; earcut doesn't want it
        let r = match (r.first(), r.last()) {
            (Some(a), Some(b)) if a == b && r.len() > 3 => &r[..r.len() - 1],
            _ => &r[..],
        };
        flat.extend(r.iter().flat_map(|&(x, y)| [x, y]));
    }
    if let Ok(tris) = earcutr::earcut(&flat, &holes, 2) {
        let base = pts.len() as u32;
        pts.extend(flat.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])));
        idx.extend(tris.into_iter().map(|i| base + i as u32));
    }
}

pub fn parse(bytes: &[u8], sel: Sel) -> Result<Outlook, String> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("SPC: {e}"))?;
    let feats = v["features"].as_array().ok_or("SPC: unexpected response")?;
    let mut out = Outlook {
        sel,
        issued: 0,
        valid: 0,
        expire: 0,
        areas: Vec::new(),
    };
    for f in feats {
        let p = &f["properties"];
        let s = |k: &str| p[k].as_str().unwrap_or_default();
        out.issued = out.issued.max(spc_time(s("ISSUE")).unwrap_or(0));
        out.valid = out.valid.max(spc_time(s("VALID")).unwrap_or(0));
        out.expire = out.expire.max(spc_time(s("EXPIRE")).unwrap_or(0));

        let g = &f["geometry"];
        let polys: Vec<&serde_json::Value> = match g["type"].as_str() {
            Some("Polygon") => vec![&g["coordinates"]],
            Some("MultiPolygon") => g["coordinates"]
                .as_array()
                .map(|a| a.iter().collect())
                .unwrap_or_default(),
            _ => continue, // "Less Than 2% All Areas" comes as an empty GeometryCollection
        };
        let mut rings: Vec<Vec<(f64, f64)>> = Vec::new();
        let (mut fill_pts, mut fill_idx) = (Vec::new(), Vec::new());
        for poly in polys.iter().filter_map(|p| p.as_array()) {
            // first ring is the outside, the rest are holes
            let poly_rings: Vec<Vec<(f64, f64)>> = poly
                .iter()
                .filter_map(|ring| {
                    let pts: Vec<(f64, f64)> = ring
                        .as_array()?
                        .iter()
                        .filter_map(|pt| Some((pt[0].as_f64()?, pt[1].as_f64()?)))
                        .collect();
                    (pts.len() >= 3).then_some(pts)
                })
                .collect();
            if poly_rings.is_empty() {
                continue;
            }
            triangulate(&poly_rings, &mut fill_pts, &mut fill_idx);
            rings.extend(poly_rings);
        }
        if rings.is_empty() {
            continue;
        }
        let label = s("LABEL");
        let sig = label.eq_ignore_ascii_case("SIGN");
        let label = match label.parse::<f32>() {
            Ok(x) if x < 1.0 => format!("{:.0}%", x * 100.0),
            _ if sig => "SIG".to_string(),
            _ => label.to_string(),
        };
        // SPC's fill colours are the light ones; they read better on a dark map than the strokes
        let color = hex(s("fill"))
            .or_else(|| hex(s("stroke")))
            .unwrap_or(Color32::GRAY);
        out.areas.push(Area {
            label,
            name: s("LABEL2").to_string(),
            color: if sig { Color32::from_gray(220) } else { color },
            sig,
            rings,
            fill_pts,
            fill_idx,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TORN: &str = r##"{"type": "FeatureCollection", "features": [
      {"type": "Feature", "geometry": {"type": "MultiPolygon", "coordinates": [[[[-98.0, 34.0], [-96.0, 34.0], [-96.0, 36.0], [-98.0, 36.0], [-98.0, 34.0]]]]},
       "properties": {"DN": 5, "VALID": "202405061630", "EXPIRE": "202405071200", "ISSUE": "202405061609",
         "LABEL": "0.05", "LABEL2": "5% Tornado Risk", "stroke": "#70380f", "fill": "#9d4e15"}},
      {"type": "Feature", "geometry": {"type": "MultiPolygon", "coordinates": [[[[-97.8, 34.5], [-96.5, 34.5], [-96.5, 35.5], [-97.8, 34.5]]]]},
       "properties": {"DN": 10, "VALID": "202405061630", "EXPIRE": "202405071200", "ISSUE": "202405061609",
         "LABEL": "SIGN", "LABEL2": "10% Significant Tornado Risk", "stroke": "#000000", "fill": "#888888"}}
    ]}"##;

    const NONE: &str = r#"{"type": "FeatureCollection", "features": [
      {"type": "Feature", "geometry": {"type": "GeometryCollection", "geometries": []},
       "properties": {"DN": 0, "VALID": "202611171200", "EXPIRE": "202611181200", "ISSUE": "202611170554",
         "LABEL": "Less Than 2% All Areas", "LABEL2": "", "stroke": "", "fill": ""}}
    ]}"#;

    #[test]
    fn parses_probabilities_and_sig_area() {
        let sel = Sel {
            day: 1,
            kind: Kind::Tornado,
        };
        let o = parse(TORN.as_bytes(), sel).unwrap();
        assert_eq!(o.areas.len(), 2);
        assert_eq!(o.areas[0].label, "5%");
        assert_eq!(o.areas[0].color, Color32::from_rgb(0x9d, 0x4e, 0x15));
        assert_eq!(o.areas[0].rings[0][0], (-98.0, 34.0));
        assert!(o.areas[1].sig);
        assert_eq!(o.areas[1].label, "SIG");
        assert_eq!(net_fmt(o.valid), "2024-05-06 16:30Z");
        assert_eq!(net_fmt(o.expire), "2024-05-07 12:00Z");
        assert!(o.covers(o.valid + 3600));
        assert!(!o.covers(o.expire + 60), "expired");
        assert!(
            !o.covers(o.issued - 86400),
            "an archived scan from the day before"
        );
    }

    #[test]
    fn areas_are_triangulated_holes_included() {
        let doc = r##"{"features": [{"geometry": {"type": "Polygon", "coordinates": [
            [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]],
            [[4.0, 4.0], [6.0, 4.0], [6.0, 6.0], [4.0, 6.0], [4.0, 4.0]] ]},
          "properties": {"LABEL": "TSTM", "fill": "#C1E9C1"}}]}"##;
        let sel = Sel {
            day: 1,
            kind: Kind::Categorical,
        };
        let o = parse(doc.as_bytes(), sel).unwrap();
        let a = &o.areas[0];
        assert_eq!(a.rings.len(), 2, "the hole's edge is outlined too");
        assert_eq!(a.fill_pts.len(), 8);
        // area of the triangles = square minus the hole
        let t = |i: usize| a.fill_pts[a.fill_idx[i] as usize];
        let area: f64 = (0..a.fill_idx.len() / 3)
            .map(|k| {
                let (p, q, r) = (t(3 * k), t(3 * k + 1), t(3 * k + 2));
                ((q.0 - p.0) * (r.1 - p.1) - (r.0 - p.0) * (q.1 - p.1)).abs() / 2.0
            })
            .sum();
        assert!((area - 96.0).abs() < 1e-9, "{area}");
    }

    #[test]
    fn empty_outlook_has_no_areas() {
        let sel = Sel {
            day: 1,
            kind: Kind::Tornado,
        };
        let o = parse(NONE.as_bytes(), sel).unwrap();
        assert!(o.areas.is_empty());
        assert!(o.expire > o.valid);
    }

    #[test]
    fn day_three_only_has_categorical_and_severe() {
        assert_eq!(Kind::Tornado.on_day(3), Kind::Severe);
        assert_eq!(Kind::Severe.on_day(1), Kind::Categorical);
        assert_eq!(Kind::Hail.on_day(2), Kind::Hail);
        let s = Sel {
            day: 3,
            kind: Kind::Severe,
        };
        assert!(s.url().ends_with("/day3otlk_prob.lyr.geojson"));
    }

    #[test]
    fn days_four_to_eight_are_severe_only() {
        for d in 4..=8 {
            assert_eq!(Kind::for_day(d), &[Kind::Severe]);
            assert_eq!(Kind::Categorical.on_day(d), Kind::Severe);
            assert_eq!(Kind::Tornado.on_day(d), Kind::Severe);
        }
        assert_eq!(Kind::Severe.on_day(3), Kind::Severe);
        let s = Sel {
            day: 6,
            kind: Kind::Severe,
        };
        assert_eq!(
            s.url(),
            "https://www.spc.noaa.gov/products/exper/day4-8/day6prob.lyr.geojson"
        );
        let doc = r##"{"features": [{"geometry": {"type": "MultiPolygon", "coordinates": [[[[-98.0, 34.0], [-96.0, 34.0], [-96.0, 36.0], [-98.0, 34.0]]]]},
            "properties": {"VALID": "202611201200", "EXPIRE": "202611211200", "ISSUE": "202611170958",
              "LABEL": "0.15", "LABEL2": "15% Any Severe Risk", "fill": "#FFE066"}}]}"##;
        let o = parse(doc.as_bytes(), s).unwrap();
        assert_eq!(o.areas[0].label, "15%");
    }

    fn net_fmt(t: i64) -> String {
        crate::net::fmt_time(t)
    }
}

#[cfg(test)]
mod style_tests {
    use crate::config::{Config, LevelStyle};
    use egui::Color32;

    #[test]
    fn lua_colours_fill_and_hiding() {
        let spc = Color32::from_rgb(0xff, 0xa3, 0x66);
        let cfg = Config::default_config();
        let def = cfg.outlook.style("ENH", spc).unwrap();
        assert_eq!(def.line, Some(spc), "SPC colour by default");
        assert_eq!(def.fill, None, "no fill by default");

        let lua = r##"return { outlook = { show = "tornado", day = 2, fill = 0.5, colors = {
            TSTM = false,
            ["10%"] = "#112233",
            HIGH = { line = false, fill = "#ff00ff40" },
            sig = "#00000000",
        } } }"##;
        let cfg = Config::from_lua(lua).unwrap();
        let o = &cfg.outlook;
        assert_eq!(o.show.unwrap().day, 2);
        assert_eq!(o.style("TSTM", spc), None, "false hides a level");
        let ten = o.style("10%", spc).unwrap();
        assert_eq!(ten.line, Some(Color32::from_rgb(0x11, 0x22, 0x33)));
        assert_eq!(
            ten.fill,
            Some(Color32::from_rgb(0x11, 0x22, 0x33).gamma_multiply(0.5))
        );
        assert_eq!(
            o.style("HIGH", spc),
            Some(LevelStyle {
                line: None,
                fill: Some(Color32::from_rgba_unmultiplied(0xff, 0, 0xff, 0x40)),
            })
        );
        assert_eq!(
            o.style("SIG", spc).unwrap().line,
            Some(Color32::TRANSPARENT)
        );
        assert_eq!(
            o.style("MRGL", spc).unwrap().fill,
            Some(spc.gamma_multiply(0.5))
        );

        assert!(Config::from_lua(r#"return { outlook = { show = "tornado", day = 3 } }"#).is_err());

        // tornado rules without a sound get the alarm; everything else a beep
        let cfg = Config::from_lua(
            r#"return { alerts = { rules = {
                { event = "Tornado Watch" },
                { event = "Severe Thunderstorm Warning", tornado = "possible" },
                { event = "Winter Storm Warning" },
            } } }"#,
        )
        .unwrap();
        let s: Vec<&str> = cfg.alerts.rules.iter().map(|r| r.sound.as_str()).collect();
        assert_eq!(s, ["alarm", "alarm", "beep"]);
        // and every tornado rule in the default config uses it
        for r in &Config::default_config().alerts.rules {
            if r.event.contains("Tornado") || r.tornado.is_some() {
                assert_eq!(r.sound, "alarm", "{}", r.event);
            }
        }
        assert!(Config::from_lua(r#"return { outlook = { colors = { MDT = 5 } } }"#).is_err());
        // your own sounds by name; the removed built-ins still load, as the alarm
        let cfg = Config::from_lua(
            r#"return { alerts = { custom_sounds = { Klaxon = "k.wav" }, rules = {
                { event = "Tornado Warning", sound = "klaxon" },
                { event = "Tornado Watch", sound = "eas" },
            } } }"#,
        )
        .unwrap();
        let r = &cfg.alerts.rules;
        assert_eq!((r[0].sound.as_str(), r[0].file.as_deref()), ("alarm", Some("k.wav")));
        assert_eq!((r[1].sound.as_str(), r[1].file.as_deref()), ("alarm", None));
        assert!(Config::from_lua(r#"return { alerts = { rules = { { event = "X", sound = "nope" } } } }"#).is_err());
    }
}
