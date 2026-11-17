//! NWS active alerts (api.weather.gov) and matching them against the Lua rules.

use crate::config::AlertRule;
use crate::map::Proj;

#[derive(Clone, Default)]
pub struct Alert {
    pub id: String,
    pub event: String,
    pub headline: String,
    pub area: String,
    pub expires: String,
    /// When it takes effect / was issued (ISO 8601, the office's local offset).
    pub onset: String,
    pub sent: String,
    /// "Extreme", "Severe" … / "Immediate", "Expected" … / "Observed", "Likely" …
    pub severity: String,
    pub urgency: String,
    pub certainty: String,
    /// e.g. "NWS Norman OK"
    pub sender: String,
    /// The full text, reflowed into paragraphs (NWS hard-wraps at ~70 columns).
    pub description: String,
    pub instruction: String,
    pub tornado: Option<String>,
    pub damage: Option<String>,
    pub hail: Option<f32>,
    /// Outer rings of the warning polygon(s), as (lon, lat).
    pub polygons: Vec<Vec<(f64, f64)>>,
    /// County FIPS codes from the SAME list (used for watches, which have no polygon).
    pub counties: Vec<u32>,
    /// True when the outline came from county borders rather than an NWS polygon.
    pub county_based: bool,
    /// Within `alerts.radius_km` of the radar: listed and sounded. Every alert is drawn.
    pub near: bool,
}

impl Alert {
    /// Tornado / severe thunderstorm / flash flood / winter storm … watches.
    pub fn is_watch(&self) -> bool {
        self.event.to_ascii_lowercase().ends_with(" watch")
    }

    pub fn confirmed(&self) -> bool {
        self.tornado
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("observed"))
    }

    pub fn emergency(&self) -> bool {
        self.damage
            .as_deref()
            .is_some_and(|d| d.eq_ignore_ascii_case("catastrophic"))
    }

    pub fn matches(&self, r: &AlertRule) -> bool {
        if !self.event.eq_ignore_ascii_case(&r.event) {
            return false;
        }
        if let Some(t) = &r.tornado
            && !self
                .tornado
                .as_deref()
                .is_some_and(|x| x.eq_ignore_ascii_case(t))
        {
            return false;
        }
        if let Some(d) = &r.damage
            && !self
                .damage
                .as_deref()
                .is_some_and(|x| x.eq_ignore_ascii_case(d))
        {
            return false;
        }
        if let Some(h) = r.hail
            && !self.hail.is_some_and(|x| x >= h)
        {
            return false;
        }
        true
    }

    /// Short tag shown next to the event name.
    pub fn tag(&self) -> Option<&'static str> {
        if self.emergency() {
            Some("EMERGENCY")
        } else if self
            .damage
            .as_deref()
            .is_some_and(|d| d.eq_ignore_ascii_case("considerable"))
        {
            Some("PDS")
        } else if self
            .damage
            .as_deref()
            .is_some_and(|d| d.eq_ignore_ascii_case("destructive"))
        {
            Some("DESTRUCTIVE")
        } else if self.confirmed() {
            Some("CONFIRMED")
        } else if self
            .tornado
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("possible"))
        {
            Some("TOR POSSIBLE")
        } else {
            None
        }
    }
}

/// NWS text is hard-wrapped with blank lines between paragraphs: join each
/// paragraph's lines so it can wrap to the panel's width.
pub fn reflow(text: &str) -> String {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .map(|para| para.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// "2013-05-20T15:30:00-05:00" → "May 20 15:30" (in the issuing office's time).
pub fn short_time(iso: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let m: usize = iso.get(5..7)?.parse().ok()?;
    let d: u32 = iso.get(8..10)?.parse().ok()?;
    Some(format!("{} {d} {}", MONTHS.get(m.checked_sub(1)?)?, iso.get(11..16)?))
}

/// Even-odd point-in-polygon on (lon, lat) rings.
pub fn contains(ring: &[(f64, f64)], lon: f64, lat: f64) -> bool {
    let mut inside = false;
    let mut j = ring.len().wrapping_sub(1);
    for i in 0..ring.len() {
        let ((xi, yi), (xj, yj)) = (ring[i], ring[j]);
        if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn first_param(p: &serde_json::Value, name: &str) -> Option<String> {
    p.get(name)?.get(0)?.as_str().map(String::from)
}

pub fn parse(bytes: &[u8]) -> Result<Vec<Alert>, String> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("alerts: {e}"))?;
    let feats = v
        .get("features")
        .and_then(|f| f.as_array())
        .ok_or("alerts: unexpected response")?;
    let s = |x: &serde_json::Value, k: &str| {
        x.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Ok(feats
        .iter()
        .filter_map(|f| {
            let p = f.get("properties")?;
            let params = p.get("parameters").cloned().unwrap_or_default();
            let mut polygons = Vec::new();
            if let Some(g) = f.get("geometry").filter(|g| !g.is_null()) {
                let coords = g.get("coordinates")?;
                let polys: Vec<&serde_json::Value> = match g.get("type")?.as_str()? {
                    "Polygon" => vec![coords],
                    "MultiPolygon" => coords.as_array()?.iter().collect(),
                    _ => vec![],
                };
                for poly in polys {
                    let ring = poly.get(0)?.as_array()?;
                    polygons.push(
                        ring.iter()
                            .filter_map(|pt| Some((pt.get(0)?.as_f64()?, pt.get(1)?.as_f64()?)))
                            .collect(),
                    );
                }
            }
            Some(Alert {
                id: s(p, "id"),
                event: s(p, "event"),
                headline: s(p, "headline"),
                area: s(p, "areaDesc"),
                expires: s(p, "expires"),
                onset: Some(s(p, "onset"))
                    .filter(|x| !x.is_empty())
                    .unwrap_or_else(|| s(p, "effective")),
                sent: s(p, "sent"),
                severity: s(p, "severity"),
                urgency: s(p, "urgency"),
                certainty: s(p, "certainty"),
                sender: s(p, "senderName"),
                description: reflow(&s(p, "description")),
                instruction: reflow(&s(p, "instruction")),
                tornado: first_param(&params, "tornadoDetection"),
                // tornado: considerable (PDS) / catastrophic (emergency); severe
                // thunderstorm: considerable / destructive; flash flood: considerable /
                // catastrophic (flash flood emergency)
                damage: first_param(&params, "tornadoDamageThreat")
                    .or_else(|| first_param(&params, "thunderstormDamageThreat"))
                    .or_else(|| first_param(&params, "flashFloodDamageThreat")),
                hail: first_param(&params, "maxHailSize").and_then(|h| h.trim().parse().ok()),
                polygons,
                counties: p["geocode"]["SAME"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| c.as_str()?.get(1..)?.parse().ok())
                            .collect()
                    })
                    .unwrap_or_default(),
                county_based: false,
                near: true,
            })
        })
        .collect())
}

/// Keep the alerts that have a rule, paired with the index of the first rule that
/// matches (rules are in priority order), and mark the ones near the radar. Watches
/// come without a polygon, so they get the outlines of their counties from `county`.
pub fn select(
    list: Vec<Alert>,
    rules: &[AlertRule],
    radius_km: f64,
    proj: &Proj,
    county: impl Fn(u32) -> Vec<Vec<(f64, f64)>>,
) -> Vec<(Alert, usize)> {
    let mut kept: Vec<(Alert, usize)> =
        list.into_iter()
            .filter_map(|mut al| {
                let rule = rules.iter().position(|r| al.matches(r))?;
                if al.polygons.is_empty() && !al.counties.is_empty() {
                    al.polygons = al.counties.iter().flat_map(|c| county(*c)).collect();
                    al.county_based = true;
                }
                al.near = radius_km <= 0.0
                    || al.polygons.iter().flatten().any(|&(lon, lat)| {
                        proj.fwd(lon, lat).to_vec2().length() as f64 <= radius_km
                    });
                Some((al, rule))
            })
            .collect();
    kept.sort_by_key(|(_, r)| *r);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::map::{Basemap, Proj};

    /// Trimmed-down api.weather.gov response: one of each kind of alert we care about.
    const SAMPLE: &str = r#"{ "features": [
      { "geometry": { "type": "Polygon", "coordinates": [[[-97.55,35.30],[-97.40,35.38],[-97.30,35.33],[-97.45,35.25],[-97.55,35.30]]] },
        "properties": { "id": "tor-moore", "event": "Tornado Warning", "headline": "Tornado Warning for Cleveland County",
          "areaDesc": "Cleveland, OK", "expires": "2013-05-20T15:30:00-05:00",
          "parameters": { "tornadoDetection": ["OBSERVED"], "tornadoDamageThreat": ["CONSIDERABLE"] },
          "geocode": { "SAME": ["040027"] } } },
      { "geometry": { "type": "MultiPolygon", "coordinates": [
            [[[-97.9,35.6],[-97.7,35.7],[-97.6,35.6],[-97.9,35.6]]],
            [[[-98.2,35.4],[-98.0,35.5],[-97.9,35.4],[-98.2,35.4]]] ] },
        "properties": { "id": "svr-1", "event": "Severe Thunderstorm Warning", "areaDesc": "Canadian, OK",
          "parameters": { "maxHailSize": ["2.50"] }, "geocode": { "SAME": ["040017"] } } },
      { "geometry": null,
        "properties": { "id": "tow-1", "event": "Tornado Watch", "areaDesc": "Oklahoma; Cleveland, OK",
          "parameters": {}, "geocode": { "SAME": ["040109", "040027"] } } },
      { "geometry": { "type": "Polygon", "coordinates": [[[-97.5,35.4],[-97.4,35.45],[-97.35,35.4],[-97.5,35.4]]] },
        "properties": { "id": "ffw-1", "event": "Flash Flood Warning", "areaDesc": "Oklahoma, OK", "parameters": {} } },
      { "geometry": { "type": "Polygon", "coordinates": [[[-81.5,28.5],[-81.3,28.6],[-81.2,28.5],[-81.5,28.5]]] },
        "properties": { "id": "tor-florida", "event": "Tornado Warning", "areaDesc": "Orange, FL",
          "parameters": { "tornadoDetection": ["RADAR INDICATED"] } } }
    ]}"#;

    fn ktlx() -> Proj {
        Proj::new(35.333, -97.278)
    }

    #[test]
    fn text_helpers() {
        let t = "...A TORNADO WARNING REMAINS IN EFFECT UNTIL 330 PM\nCDT FOR CLEVELAND COUNTY...\n\nAt 303 PM CDT, a\nconfirmed tornado.\n\n";
        assert_eq!(
            reflow(t),
            "...A TORNADO WARNING REMAINS IN EFFECT UNTIL 330 PM CDT FOR CLEVELAND COUNTY...\n\nAt 303 PM CDT, a confirmed tornado."
        );
        assert_eq!(short_time("2013-05-20T15:30:00-05:00").as_deref(), Some("May 20 15:30"));
        assert_eq!(short_time(""), None);
        let sq = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)];
        assert!(contains(&sq, 0.5, 0.5));
        assert!(!contains(&sq, 1.5, 0.5));
    }

    #[test]
    fn parses_polygons_and_parameters() {
        let a = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(a.len(), 5);
        let tor = &a[0];
        assert_eq!(tor.polygons.len(), 1);
        assert_eq!(tor.polygons[0].len(), 5, "ring keeps every vertex");
        assert_eq!(tor.polygons[0][0], (-97.55, 35.30), "(lon, lat) order");
        assert!(tor.confirmed());
        assert_eq!(tor.tag(), Some("PDS"));
        assert_eq!(tor.counties, vec![40027]);
        let svr = &a[1];
        assert_eq!(svr.polygons.len(), 2, "each part of a MultiPolygon is kept");
        assert_eq!(svr.hail, Some(2.5));
        let watch = &a[2];
        assert!(
            watch.polygons.is_empty(),
            "watches have no polygon in the API"
        );
        assert_eq!(watch.counties, vec![40109, 40027]);
    }

    #[test]
    fn rules_pick_the_right_match_and_filter_by_distance() {
        let cfg = Config::default_config();
        let map = Basemap::new(35.333, -97.278);
        let kept = select(
            parse(SAMPLE.as_bytes()).unwrap(),
            &cfg.alerts.rules,
            300.0,
            &ktlx(),
            |f| map.county(f),
        );
        let ids: Vec<&str> = kept.iter().map(|(a, _)| a.id.as_str()).collect();

        // flash floods have a rule in the default config; Florida is far from KTLX,
        // so it's kept (drawn on the map) but not near
        assert!(ids.contains(&"ffw-1"));
        let fl = kept.iter().find(|(a, _)| a.id == "tor-florida").unwrap();
        assert!(!fl.0.near);
        assert!(kept.iter().find(|(a, _)| a.id == "ffw-1").unwrap().0.near);
        // most important first: the confirmed tornado uses the "observed" rule, not the emergency one
        assert_eq!(ids[0], "tor-moore");
        let rule = &cfg.alerts.rules[kept[0].1];
        assert_eq!(rule.tornado.as_deref(), Some("observed"));
        assert_eq!(rule.sound, "alarm");
        // 2.5" hail takes the hail rule ahead of the plain severe rule
        let svr = kept.iter().find(|(a, _)| a.id == "svr-1").unwrap();
        assert_eq!(cfg.alerts.rules[svr.1].hail, Some(2.0));
    }

    #[test]
    fn watches_get_county_outlines() {
        let cfg = Config::default_config();
        let map = Basemap::new(35.333, -97.278);
        let kept = select(
            parse(SAMPLE.as_bytes()).unwrap(),
            &cfg.alerts.rules,
            300.0,
            &ktlx(),
            |f| map.county(f),
        );
        let (watch, _) = kept
            .iter()
            .find(|(a, _)| a.id == "tow-1")
            .expect("watch near KTLX is kept");
        assert!(watch.county_based);
        // Oklahoma County + Cleveland County outlines, around Oklahoma City / Norman
        assert!(watch.polygons.len() >= 2);
        let (mut lon, mut lat) = (0.0, 0.0);
        let pts: Vec<_> = watch.polygons.iter().flatten().collect();
        for (x, y) in &pts {
            lon += x;
            lat += y;
        }
        let n = pts.len() as f64;
        assert!(
            (lon / n + 97.4).abs() < 0.3 && (lat / n - 35.35).abs() < 0.3,
            "{} {}",
            lon / n,
            lat / n
        );
    }

    #[test]
    fn damage_threats_from_every_kind_of_warning() {
        let doc = r#"{ "features": [
          { "geometry": { "type": "Polygon", "coordinates": [[[-97.5,35.4],[-97.4,35.45],[-97.35,35.4],[-97.5,35.4]]] },
            "properties": { "id": "ffe", "event": "Flash Flood Warning", "parameters": { "flashFloodDamageThreat": ["CATASTROPHIC"] } } },
          { "geometry": { "type": "Polygon", "coordinates": [[[-97.5,35.4],[-97.4,35.45],[-97.35,35.4],[-97.5,35.4]]] },
            "properties": { "id": "svr", "event": "Severe Thunderstorm Warning", "parameters": { "thunderstormDamageThreat": ["DESTRUCTIVE"] } } },
          { "geometry": null,
            "properties": { "id": "bz", "event": "Blizzard Warning", "parameters": {}, "geocode": { "SAME": ["040109"] } } }
        ] }"#;
        let cfg = Config::default_config();
        let map = Basemap::new(35.333, -97.278);
        let kept = select(
            parse(doc.as_bytes()).unwrap(),
            &cfg.alerts.rules,
            300.0,
            &ktlx(),
            |f| map.county(f),
        );
        let get = |id: &str| kept.iter().find(|(a, _)| a.id == id).unwrap();
        let (ffe, r) = get("ffe");
        assert_eq!(ffe.tag(), Some("EMERGENCY"));
        assert_eq!(cfg.alerts.rules[*r].damage.as_deref(), Some("catastrophic"));
        let (svr, r) = get("svr");
        assert_eq!(svr.tag(), Some("DESTRUCTIVE"));
        assert_eq!(cfg.alerts.rules[*r].damage.as_deref(), Some("destructive"));
        let (bz, _) = get("bz");
        assert!(
            bz.county_based && !bz.is_watch(),
            "a county-based warning is still a warning"
        );
    }

    #[test]
    fn flash_flood_watches_are_shown_as_counties() {
        let doc = r#"{ "features": [ { "geometry": null,
            "properties": { "id": "ffa-1", "event": "Flash Flood Watch", "areaDesc": "Oklahoma; Cleveland",
              "parameters": {}, "geocode": { "SAME": ["040109", "040027"], "UGC": ["OKZ025", "OKZ026"] } } } ] }"#;
        let cfg = Config::default_config();
        let map = Basemap::new(35.333, -97.278);
        let kept = select(
            parse(doc.as_bytes()).unwrap(),
            &cfg.alerts.rules,
            300.0,
            &ktlx(),
            |f| map.county(f),
        );
        let (w, rule) = kept
            .first()
            .expect("the default config has a flash flood watch rule");
        assert_eq!(cfg.alerts.rules[*rule].event, "Flash Flood Watch");
        assert!(w.county_based && w.polygons.len() >= 2);
    }

    #[test]
    fn radius_zero_means_everywhere_and_new_rules_add_events() {
        let lua = r##"return { alerts = { rules = {
            { event = "Tornado Warning", color = "#ff0000" },
            { event = "Flash Flood Warning", color = "#00aaff", sound = "none", map = false },
        } } }"##;
        let cfg = Config::from_lua(lua).unwrap();
        let map = Basemap::new(35.333, -97.278);
        let kept = select(
            parse(SAMPLE.as_bytes()).unwrap(),
            &cfg.alerts.rules,
            0.0,
            &ktlx(),
            |f| map.county(f),
        );
        let ids: Vec<&str> = kept.iter().map(|(a, _)| a.id.as_str()).collect();
        assert!(
            kept.iter().any(|(a, _)| a.id == "tor-florida" && a.near),
            "radius 0 counts alerts anywhere as near"
        );
        assert!(ids.contains(&"ffw-1"), "adding a rule shows that event");
        assert!(
            !ids.contains(&"svr-1") && !ids.contains(&"tow-1"),
            "events without a rule are hidden"
        );
        let ffw = kept.iter().find(|(a, _)| a.id == "ffw-1").unwrap();
        assert!(!cfg.alerts.rules[ffw.1].map, "map = false is read");
        assert!(cfg.alerts.rules[ffw.1].list);
    }
}
