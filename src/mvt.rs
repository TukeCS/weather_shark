//! Minimal Mapbox Vector Tile decoder for the OpenMapTiles schema
//! (OpenFreeMap). We only keep roads, road numbers and place names.

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum RoadClass {
    Motorway,
    Trunk,
    Primary,
    Secondary,
    Tertiary,
    Minor,
}

impl RoadClass {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "motorway" => RoadClass::Motorway,
            "trunk" => RoadClass::Trunk,
            "primary" => RoadClass::Primary,
            "secondary" => RoadClass::Secondary,
            "tertiary" => RoadClass::Tertiary,
            "minor" | "service" => RoadClass::Minor,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum PlaceClass {
    City,
    Town,
    Village,
    Hamlet,
}

pub struct Road {
    pub class: RoadClass,
    /// (lon, lat)
    pub pts: Vec<[f32; 2]>,
}

pub struct Place {
    pub name: String,
    pub class: PlaceClass,
    pub rank: i64,
    pub lon: f32,
    pub lat: f32,
}

pub struct Shield {
    pub text: String,
    pub class: RoadClass,
    pub lon: f32,
    pub lat: f32,
}

#[derive(Default)]
pub struct TileData {
    pub roads: Vec<Road>,
    pub places: Vec<Place>,
    pub shields: Vec<Shield>,
}

// ---- protobuf --------------------------------------------------------------------

struct Pb<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Pb<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }
    fn done(&self) -> bool {
        self.p >= self.b.len()
    }
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.b.get(self.p)?;
            self.p += 1;
            v |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    fn key(&mut self) -> Option<(u64, u8)> {
        let k = self.varint()?;
        Some((k >> 3, (k & 7) as u8))
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.varint()? as usize;
        let s = self.b.get(self.p..self.p + n)?;
        self.p += n;
        Some(s)
    }
    fn skip(&mut self, wire: u8) -> Option<()> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => self.p += 8,
            2 => {
                self.bytes()?;
            }
            5 => self.p += 4,
            _ => return None,
        }
        Some(())
    }
    fn packed(&mut self) -> Option<Vec<u32>> {
        let mut inner = Pb::new(self.bytes()?);
        let mut out = Vec::new();
        while !inner.done() {
            out.push(inner.varint()? as u32);
        }
        Some(out)
    }
}

#[derive(Clone, Debug)]
enum Val {
    Str(String),
    Num(f64),
    Other,
}

fn parse_value(b: &[u8]) -> Option<Val> {
    let mut r = Pb::new(b);
    let mut v = Val::Other;
    while !r.done() {
        let (f, w) = r.key()?;
        v = match (f, w) {
            (1, 2) => Val::Str(String::from_utf8_lossy(r.bytes()?).into_owned()),
            (2, 5) => {
                let x = f32::from_le_bytes(r.b.get(r.p..r.p + 4)?.try_into().ok()?);
                r.p += 4;
                Val::Num(x as f64)
            }
            (3, 1) => {
                let x = f64::from_le_bytes(r.b.get(r.p..r.p + 8)?.try_into().ok()?);
                r.p += 8;
                Val::Num(x)
            }
            (4 | 5, 0) => Val::Num(r.varint()? as i64 as f64),
            (6, 0) => {
                let z = r.varint()?;
                Val::Num(((z >> 1) as i64 ^ -((z & 1) as i64)) as f64)
            }
            _ => {
                r.skip(w)?;
                v
            }
        };
    }
    Some(v)
}

struct Feature {
    tags: Vec<u32>,
    kind: u32,
    geom: Vec<u32>,
}

/// Decode command-encoded geometry into lists of tile-space points.
fn geometry(g: &[u32]) -> Vec<Vec<(i32, i32)>> {
    let mut out: Vec<Vec<(i32, i32)>> = Vec::new();
    let (mut x, mut y) = (0i32, 0i32);
    let mut i = 0;
    let zz = |v: u32| ((v >> 1) as i32) ^ -((v & 1) as i32);
    while i < g.len() {
        let (cmd, n) = (g[i] & 7, g[i] >> 3);
        i += 1;
        match cmd {
            1 | 2 => {
                for _ in 0..n {
                    let (Some(&dx), Some(&dy)) = (g.get(i), g.get(i + 1)) else {
                        return out;
                    };
                    i += 2;
                    x += zz(dx);
                    y += zz(dy);
                    if cmd == 1 {
                        out.push(vec![(x, y)]);
                    } else if let Some(l) = out.last_mut() {
                        l.push((x, y));
                    }
                }
            }
            7 => {
                if let Some(l) = out.last_mut()
                    && let Some(&p) = l.first()
                {
                    l.push(p);
                }
            }
            _ => return out,
        }
    }
    out
}

/// US route numbers: "I-35", "US 77", or the ref as given.
fn shield_text(network: Option<&str>, r: &str) -> String {
    let r = r.trim();
    match network {
        Some("us-interstate") => {
            format!("I-{}", r.trim_start_matches("I ").trim_start_matches("I-"))
        }
        Some("us-highway") => format!("US {}", r.trim_start_matches("US ")),
        _ => r.to_string(),
    }
}

pub fn decode(bytes: &[u8], z: u8, tx: u32, ty: u32) -> Result<TileData, String> {
    let n = (1u64 << z) as f64;
    let mut out = TileData::default();
    let mut tile = Pb::new(bytes);
    while !tile.done() {
        let (f, w) = tile.key().ok_or("bad tile")?;
        if f != 3 || w != 2 {
            tile.skip(w).ok_or("bad tile")?;
            continue;
        }
        let mut layer = Pb::new(tile.bytes().ok_or("bad layer")?);
        let mut name = String::new();
        let mut keys = Vec::new();
        let mut vals = Vec::new();
        let mut feats = Vec::new();
        let mut extent = 4096u32;
        while !layer.done() {
            let (f, w) = layer.key().ok_or("bad layer")?;
            match (f, w) {
                (1, 2) => {
                    name = String::from_utf8_lossy(layer.bytes().ok_or("bad layer")?).into_owned()
                }
                (2, 2) => feats.push(layer.bytes().ok_or("bad layer")?),
                (3, 2) => keys
                    .push(String::from_utf8_lossy(layer.bytes().ok_or("bad layer")?).into_owned()),
                (4, 2) => {
                    vals.push(parse_value(layer.bytes().ok_or("bad layer")?).unwrap_or(Val::Other))
                }
                (5, 0) => extent = layer.varint().ok_or("bad layer")? as u32,
                _ => layer.skip(w).ok_or("bad layer")?,
            }
        }
        if !matches!(
            name.as_str(),
            "transportation" | "transportation_name" | "place"
        ) {
            continue;
        }
        let ext = extent as f64;
        let to_ll = |(px, py): (i32, i32)| -> [f32; 2] {
            let lon = (tx as f64 + px as f64 / ext) / n * 360.0 - 180.0;
            let my = std::f64::consts::PI * (1.0 - 2.0 * (ty as f64 + py as f64 / ext) / n);
            [lon as f32, my.sinh().atan().to_degrees() as f32]
        };
        for fb in feats {
            let mut r = Pb::new(fb);
            let mut feat = Feature {
                tags: Vec::new(),
                kind: 0,
                geom: Vec::new(),
            };
            while !r.done() {
                let Some((f, w)) = r.key() else { break };
                match (f, w) {
                    (2, 2) => feat.tags = r.packed().unwrap_or_default(),
                    (3, 0) => feat.kind = r.varint().unwrap_or(0) as u32,
                    (4, 2) => feat.geom = r.packed().unwrap_or_default(),
                    _ => {
                        if r.skip(w).is_none() {
                            break;
                        }
                    }
                }
            }
            let tag = |k: &str| -> Option<&Val> {
                feat.tags
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .find(|t| keys.get(t[0] as usize).is_some_and(|x| x == k))
                    .and_then(|t| vals.get(t[1] as usize))
            };
            let s = |k: &str| match tag(k) {
                Some(Val::Str(s)) => Some(s.as_str()),
                _ => None,
            };
            match name.as_str() {
                "transportation" if feat.kind == 2 => {
                    let Some(class) = s("class").and_then(RoadClass::parse) else {
                        continue;
                    };
                    for line in geometry(&feat.geom) {
                        if line.len() >= 2 {
                            out.roads.push(Road {
                                class,
                                pts: line.into_iter().map(to_ll).collect(),
                            });
                        }
                    }
                }
                "transportation_name" if feat.kind == 2 => {
                    let Some(class) = s("class").and_then(RoadClass::parse) else {
                        continue;
                    };
                    if class > RoadClass::Primary {
                        continue;
                    }
                    let Some(r) = s("ref").filter(|r| !r.is_empty()) else {
                        continue;
                    };
                    let text = shield_text(s("network"), r);
                    let lines = geometry(&feat.geom);
                    if let Some(l) = lines.iter().max_by_key(|l| l.len()) {
                        let [lon, lat] = to_ll(l[l.len() / 2]);
                        out.shields.push(Shield {
                            text,
                            class,
                            lon,
                            lat,
                        });
                    }
                }
                "place" if feat.kind == 1 => {
                    let class = match s("class") {
                        Some("city") => PlaceClass::City,
                        Some("town") => PlaceClass::Town,
                        Some("village") => PlaceClass::Village,
                        Some("hamlet") => PlaceClass::Hamlet,
                        _ => continue,
                    };
                    let Some(nm) = s("name:en").or(s("name")).or(s("name:latin")) else {
                        continue;
                    };
                    let rank = match tag("rank") {
                        Some(Val::Num(v)) => *v as i64,
                        _ => 99,
                    };
                    if let Some(p) = geometry(&feat.geom).first().and_then(|l| l.first()) {
                        let [lon, lat] = to_ll(*p);
                        out.places.push(Place {
                            name: nm.to_string(),
                            class,
                            rank,
                            lon,
                            lat,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    Ok(out)
}
