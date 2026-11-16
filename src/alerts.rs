//! NWS active alerts (api.weather.gov) and matching them against the Lua rules.

use crate::config::AlertRule;

#[derive(Clone)]
pub struct Alert {
    pub id: String,
    pub event: String,
    pub headline: String,
    pub area: String,
    pub expires: String,
    pub tornado: Option<String>,
    pub damage: Option<String>,
    pub hail: Option<f32>,
    /// Outer rings of the warning polygon(s), as (lon, lat).
    pub polygons: Vec<Vec<(f64, f64)>>,
    /// County FIPS codes from the SAME list (used for watches, which have no polygon).
    pub counties: Vec<u32>,
    /// True when the outline came from county borders rather than an NWS polygon.
    pub county_based: bool,
}

impl Alert {
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
        } else if self.confirmed() {
            Some("CONFIRMED")
        } else {
            None
        }
    }
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
                tornado: first_param(&params, "tornadoDetection"),
                damage: first_param(&params, "tornadoDamageThreat"),
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
            })
        })
        .collect())
}
