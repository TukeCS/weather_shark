//! Live radar from Unidata's real-time "chunks" bucket.
//!
//! The archive bucket only gets a scan once the whole volume is done, which is
//! 5–10 minutes after it started. The chunks bucket gets each piece of the
//! volume seconds after the radar sends it: `SITE/NUM/YYYYMMDD-HHMMSS-CCC-T`,
//! where NUM is a volume counter (1–999, then back to 1; each folder keeps a
//! few old volumes too), CCC the chunk number and T is S (start: the archive
//! header plus metadata), I (intermediate) or E (end). Glued together in
//! order, the chunks are an ordinary archive file, so the normal decoder reads
//! them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::level2::{Decoder, Volume};

pub const BUCKET: &str = "https://unidata-nexrad-level2-chunks.s3.amazonaws.com";

/// Volume folders count 1..=999 and wrap.
pub const MAX_NUM: u16 = 999;

pub fn next_num(n: u16) -> u16 {
    if n >= MAX_NUM { 1 } else { n + 1 }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ChunkKey {
    pub num: u16,
    /// volume start, "20261117-123456"
    pub stamp: String,
    pub idx: u16,
    pub end: bool,
}

/// `KTLX/718/20261115-184555-070-E` → its parts.
pub fn parse_key(key: &str) -> Option<ChunkKey> {
    let mut p = key.split('/');
    let _site = p.next()?;
    let num = p.next()?.parse().ok()?;
    let name = p.next()?;
    let (stamp, rest) = name.split_at_checked(15)?;
    let mut r = rest.strip_prefix('-')?.split('-');
    let idx = r.next()?.parse().ok()?;
    let end = r.next()? == "E";
    stamp
        .bytes()
        .enumerate()
        .all(|(i, c)| if i == 8 { c == b'-' } else { c.is_ascii_digit() })
        .then(|| ChunkKey {
            num,
            stamp: stamp.to_string(),
            idx,
            end,
        })
}

/// The `<Key>`s in an S3 listing.
pub fn listing_keys(xml: &str) -> Vec<String> {
    xml.split("<Key>")
        .skip(1)
        .filter_map(|s| s.split("</Key>").next())
        .map(String::from)
        .collect()
}

/// One volume coming in chunk by chunk.
pub struct Stream {
    pub site: String,
    pub num: u16,
    pub stamp: String,
    /// chunks listed but not downloaded yet, by number
    known: BTreeMap<u16, String>,
    requested: BTreeSet<u16>,
    /// downloaded (bytes, is the E chunk), waiting for the chunks before them
    got: BTreeMap<u16, (Vec<u8>, bool)>,
    next: u16,
    dec: Decoder,
    /// the E chunk has been decoded
    pub ended: bool,
    /// newest key listed, for the next listing's start-after
    pub last_key: String,
    /// the previous volume, shown wherever this one hasn't scanned yet
    pub base: Option<Arc<Volume>>,
    pub error: Option<String>,
    /// sweeps finished since the app last looked (their numbers)
    pub dirty: BTreeSet<u8>,
}

impl Stream {
    pub fn new(site: &str, num: u16, stamp: &str, base: Option<Arc<Volume>>) -> Self {
        Self {
            site: site.to_string(),
            num,
            stamp: stamp.to_string(),
            known: BTreeMap::new(),
            requested: BTreeSet::new(),
            got: BTreeMap::new(),
            next: 1,
            dec: Decoder::new(),
            ended: false,
            // sorts just before this volume's first chunk
            last_key: format!("{site}/{num}/{stamp}"),
            base,
            error: None,
            dirty: BTreeSet::new(),
        }
    }

    /// Display key for the app ("live:KTLX/12/20261117-123456").
    pub fn key(&self) -> String {
        format!("live:{}/{}/{}", self.site, self.num, self.stamp)
    }

    /// Where to look for the volume after this one: the next folder, newer than this stamp.
    pub fn next_after(&self) -> (u16, String) {
        let n = next_num(self.num);
        (n, format!("{}/{n}/{}-999", self.site, self.stamp))
    }

    /// Take a listing of this volume's folder; returns the chunk keys to download.
    pub fn listed(&mut self, keys: &[String]) -> Vec<String> {
        for k in keys {
            if k > &self.last_key {
                self.last_key = k.clone();
            }
            if let Some(c) = parse_key(k).filter(|c| c.stamp == self.stamp && c.num == self.num)
                && c.idx >= self.next
                && !self.got.contains_key(&c.idx)
            {
                self.known.insert(c.idx, k.clone());
            }
        }
        let want: Vec<u16> = self
            .known
            .keys()
            .filter(|i| !self.requested.contains(i))
            .copied()
            .collect();
        want.iter()
            .map(|i| {
                self.requested.insert(*i);
                self.known[i].clone()
            })
            .collect()
    }

    /// Every chunk listed so far has been downloaded and decoded.
    pub fn caught_up(&self) -> bool {
        self.known.is_empty() && self.got.is_empty() && self.requested.iter().all(|i| *i < self.next)
    }

    /// A download failed: ask for it again at the next listing.
    pub fn failed(&mut self, key: &str) {
        if let Some(c) = parse_key(key) {
            self.requested.remove(&c.idx);
        }
    }

    /// A chunk arrived. Returns true when the picture changed.
    pub fn arrived(&mut self, key: &str, bytes: Vec<u8>) -> bool {
        let Some(c) = parse_key(key).filter(|c| c.stamp == self.stamp && c.num == self.num) else {
            return false;
        };
        if c.idx < self.next {
            return false;
        }
        self.known.remove(&c.idx);
        self.got.insert(c.idx, (bytes, c.end));
        let mut changed = false;
        let before = self.dec.sweeps_done();
        while let Some((b, end)) = self.got.remove(&self.next) {
            if let Err(e) = self.dec.push(&b) {
                self.error = Some(e);
            }
            self.next += 1;
            changed = true;
            if end {
                self.ended = true;
            }
        }
        // only finished sweeps are shown, so only they change the picture
        let v = self.dec.volume();
        let fresh = &v.sweeps[before.min(v.sweeps.len())..];
        self.dirty.extend(fresh.iter().map(|s| s.number));
        changed && !fresh.is_empty()
    }

    /// What to show: this volume's finished sweeps, then the previous volume's for
    /// every tilt not finished again yet. A tilt only changes once it's fully scanned.
    pub fn composite(&self) -> Option<Volume> {
        let v = self.dec.volume();
        if v.site.is_empty() {
            return None;
        }
        let mut out = v.clone();
        if let Some(b) = self.base.as_ref().filter(|b| b.site == v.site) {
            let done: BTreeSet<u8> = v.sweeps.iter().map(|s| s.number).collect();
            out.sweeps
                .extend(b.sweeps.iter().filter(|s| !done.contains(&s.number)).cloned());
        }
        if out.sweeps.is_empty() {
            return None;
        }
        // the archive header's time can be missing in the first chunk; use the stamp
        if out.time == 0 {
            out.time = stamp_time(&self.stamp).unwrap_or(0);
        }
        Some(out)
    }

}

/// "20261117-123456" → epoch seconds
pub fn stamp_time(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let day = crate::net::days_from_civil(n(0, 4)?, n(4, 6)? as u32, n(6, 8)? as u32);
    Some(day * 86400 + n(9, 11)? * 3600 + n(11, 13)? * 60 + n(13, 15)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        let k = parse_key("KTLX/718/20261115-184555-070-E").unwrap();
        assert_eq!(k.num, 718);
        assert_eq!(k.stamp, "20261115-184555");
        assert_eq!(k.idx, 70);
        assert!(k.end);
        assert!(!parse_key("KTLX/1/20261116-193002-001-S").unwrap().end);
        assert!(parse_key("KTLX/1/garbage").is_none());
        assert_eq!(next_num(999), 1);
        assert_eq!(next_num(41), 42);
        assert_eq!(stamp_time("19700102-000010"), Some(86410));
        // a listing that starts after the stamp sorts before every chunk of that volume
        let s = Stream::new("KTLX", 12, "20261117-123456", None);
        assert!(s.last_key.as_str() < "KTLX/12/20261117-123456-001-S");
        let (n, after) = s.next_after();
        assert_eq!(n, 13);
        assert!(after.as_str() > "KTLX/13/20261117-123456-070-E");
        assert!(after.as_str() < "KTLX/13/20261117-124012-001-S");
    }
}

/// Talks to the real bucket: `cargo test live_feed -- --ignored --nocapture`
#[cfg(all(test, not(target_arch = "wasm32")))]
mod live_tests {
    use super::*;
    use crate::net::{Msg, Net};
    use std::time::Duration;

    #[test]
    #[ignore]
    fn live_feed() {
        let (tx, rx) = std::sync::mpsc::channel();
        let net = Net::new(tx, egui::Context::default());
        let site = std::env::var("SITE").unwrap_or("KTLX".into());
        let t = std::time::Instant::now();
        net.chunk_find(&site);
        let Msg::ChunkFound { found, .. } = rx.recv_timeout(Duration::from_secs(60)).unwrap() else {
            panic!("unexpected message")
        };
        let (num, stamp) = found.unwrap();
        let age = crate::net::now() - stamp_time(&stamp).unwrap();
        println!("{site}: folder {num}, volume {stamp} ({age} s old), found in {:?}", t.elapsed());
        assert!(age < 20 * 60, "the newest volume should be minutes old, not days");

        let mut s = Stream::new(&site, num, &stamp, None);
        net.chunk_list(&site, num, &s.last_key);
        let Msg::ChunkList { keys, .. } = rx.recv_timeout(Duration::from_secs(30)).unwrap() else {
            panic!("unexpected message")
        };
        let get = s.listed(&keys.unwrap());
        println!("{} chunks so far", get.len());
        for k in &get {
            net.chunk(k);
        }
        for _ in &get {
            let Msg::Chunk { key, bytes } = rx.recv_timeout(Duration::from_secs(30)).unwrap() else {
                panic!("unexpected message")
            };
            s.arrived(&key, bytes.unwrap());
        }
        assert!(s.error.is_none(), "{:?}", s.error);
        // only finished tilts are shown, so a volume that just started has nothing yet
        let Some(v) = s.composite() else {
            assert!(age < 120, "no finished tilt in a volume {age} s old");
            println!("no tilt finished yet");
            return;
        };
        assert_eq!(v.site, site);
        let a = v.antenna.unwrap();
        println!(
            "{} finished sweeps, vcp {}, scanning {:.1}°, last radial {:.0} s ago",
            v.sweeps.len(),
            v.vcp,
            a.elevation,
            crate::net::now_f64() - a.time
        );
        let (az, took) = v.sweeps[0].scan_path().unwrap();
        println!("first tilt started at {az:.0}°, took {took:.0} s");
        assert!(took > 1.0 && took < 120.0);
    }
}
