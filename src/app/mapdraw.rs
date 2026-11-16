//! Map layers drawn into a pane: outlines, OSM roads, place / road labels and warnings.

use std::collections::HashSet;
use std::sync::Arc;

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

use super::App;
use crate::map::Layer;
use crate::mvt::{PlaceClass, RoadClass, TileData};
use crate::tiles::{self, Key};

/// Text with a dark outline so it stays readable over radar colours.
pub fn halo_text(
    p: &Painter,
    pos: Pos2,
    align: Align2,
    text: &str,
    font: FontId,
    color: Color32,
    halo: Color32,
) {
    for d in [
        vec2(-1.0, 0.0),
        vec2(1.0, 0.0),
        vec2(0.0, -1.0),
        vec2(0.0, 1.0),
        vec2(-1.0, -1.0),
        vec2(1.0, 1.0),
    ] {
        p.text(pos + d, align, text, font.clone(), halo);
    }
    p.text(pos, align, text, font, color);
}

fn road_width(c: RoadClass) -> f32 {
    match c {
        RoadClass::Motorway => 2.0,
        RoadClass::Trunk => 1.6,
        RoadClass::Primary => 1.3,
        RoadClass::Secondary => 1.0,
        RoadClass::Tertiary => 0.8,
        RoadClass::Minor => 0.6,
    }
}

/// Smallest zoom (px per km) at which a road class is drawn.
fn road_min_zoom(c: RoadClass) -> f32 {
    match c {
        RoadClass::Motorway => 0.0,
        RoadClass::Trunk => 0.35,
        RoadClass::Primary => 0.7,
        RoadClass::Secondary => 1.2,
        RoadClass::Tertiary => 3.0,
        RoadClass::Minor => 8.0,
    }
}

fn place_min_zoom(c: PlaceClass) -> f32 {
    match c {
        PlaceClass::City => 0.0,
        PlaceClass::Town => 0.9,
        PlaceClass::Village => 2.8,
        PlaceClass::Hamlet => 8.0,
    }
}

impl App {
    /// Simplify while projecting: drop points closer than ~1.5 px to the last one.
    fn screen_line(&self, rect: Rect, pts: impl Iterator<Item = Pos2>) -> Vec<Pos2> {
        let mut out: Vec<Pos2> = Vec::new();
        for p in pts {
            let s = self.to_screen(rect, p);
            if out
                .last()
                .is_none_or(|l| (l.x - s.x).abs() + (l.y - s.y).abs() > 1.5)
            {
                out.push(s);
            }
        }
        out
    }

    pub(super) fn draw_layer(
        &self,
        painter: &Painter,
        rect: Rect,
        layer: &Layer,
        col: Color32,
        width: f32,
    ) {
        let view =
            Rect::from_two_pos(self.to_km(rect, rect.min), self.to_km(rect, rect.max)).expand(5.0);
        for (bb, pts) in &layer.lines {
            if bb.intersects(view) {
                let out = self.screen_line(rect, pts.iter().copied());
                if out.len() >= 2 {
                    painter.add(Shape::line(out, Stroke::new(width, col)));
                }
            }
        }
    }

    /// Tiles to draw for this pane (each the best loaded version), and queue the missing ones.
    pub(super) fn visible_tiles(&mut self, rect: Rect) -> Vec<Arc<TileData>> {
        if !(self.show.roads || self.show.cities) || self.tiles.template.is_none() {
            return Vec::new();
        }
        let a = self.map.proj.inv(self.to_km(rect, rect.left_bottom()));
        let b = self.map.proj.inv(self.to_km(rect, rect.right_top()));
        let z = tiles::zoom_for(self.zoom, self.map.proj.lat0);
        let (_, keys) = tiles::cover(z, a.0, a.1, b.0, b.1);
        let mut seen: HashSet<Key> = HashSet::new();
        let mut out = Vec::new();
        for k in &keys {
            if let Some((bk, t)) = self.tiles.best(*k)
                && seen.insert(bk)
            {
                out.push(t);
            }
        }
        self.want_tiles.extend(keys);
        out
    }

    pub(super) fn request_tiles(&mut self) {
        let want = std::mem::take(&mut self.want_tiles);
        let Some(tpl) = self.tiles.template.clone() else {
            return;
        };
        for k in self.tiles.wanted(&want) {
            self.net.tile(&tpl, k);
        }
    }

    /// `minor` = the small roads drawn under the radar; otherwise the main roads drawn over it.
    pub(super) fn draw_roads(
        &self,
        painter: &Painter,
        rect: Rect,
        tiles: &[Arc<TileData>],
        minor: bool,
    ) {
        let colors = self.cfg.map.road_colors;
        let classes: &[RoadClass] = if minor {
            &[RoadClass::Minor, RoadClass::Tertiary, RoadClass::Secondary]
        } else {
            &[RoadClass::Primary, RoadClass::Trunk, RoadClass::Motorway]
        };
        let proj = self.map.proj;
        for &class in classes {
            if self.zoom < road_min_zoom(class) {
                continue;
            }
            let col = colors[class as usize];
            if col.a() == 0 {
                continue;
            }
            // Roads get a bit wider as you zoom in.
            let w = road_width(class) * (self.zoom / 3.0).clamp(0.55, 1.8);
            for t in tiles {
                for r in t.roads.iter().filter(|r| r.class == class) {
                    let out = self.screen_line(
                        rect,
                        r.pts.iter().map(|p| proj.fwd(p[0] as f64, p[1] as f64)),
                    );
                    if out.len() >= 2 && out.iter().any(|p| rect.expand(50.0).contains(*p)) {
                        painter.add(Shape::line(out, Stroke::new(w, col)));
                    }
                }
            }
        }
    }

    pub(super) fn draw_labels(&self, painter: &Painter, rect: Rect, tiles: &[Arc<TileData>]) {
        let t = &self.cfg.theme;
        let ms = &self.cfg.map;
        // (priority, position, text, kind) – lower priority places first
        enum Kind {
            Place(PlaceClass),
            Shield(RoadClass),
        }
        let mut cands: Vec<(i64, Pos2, &str, Kind)> = Vec::new();
        for tile in tiles {
            if self.show.cities {
                for p in &tile.places {
                    if self.zoom < place_min_zoom(p.class) {
                        continue;
                    }
                    let s = self.ll_to_screen(rect, p.lon as f64, p.lat as f64);
                    if rect.shrink(4.0).contains(s) {
                        cands.push((
                            (p.class as i64) * 100 + p.rank.min(99),
                            s,
                            &p.name,
                            Kind::Place(p.class),
                        ));
                    }
                }
            }
            if self.show.road_labels && self.show.roads {
                for sh in &tile.shields {
                    let min = match sh.class {
                        RoadClass::Motorway => 0.6,
                        RoadClass::Trunk => 1.2,
                        _ => 2.5,
                    };
                    if self.zoom < min {
                        continue;
                    }
                    let s = self.ll_to_screen(rect, sh.lon as f64, sh.lat as f64);
                    if rect.shrink(10.0).contains(s) {
                        cands.push((
                            150 + sh.class as i64 * 20,
                            s,
                            &sh.text,
                            Kind::Shield(sh.class),
                        ));
                    }
                }
            }
        }
        cands.sort_by_key(|c| c.0);

        // Keep labels off the pane header, the legend and the radar sites drawn on top.
        let mut placed: Vec<Rect> = vec![
            Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + 44.0)),
            Rect::from_min_max(
                pos2(rect.min.x, rect.max.y - 46.0),
                pos2(rect.min.x + 250.0, rect.max.y),
            ),
        ];
        if self.show.sites {
            for s in &self.sites {
                let p = self.ll_to_screen(rect, s.lon, s.lat);
                if rect.expand(20.0).contains(p) {
                    placed.push(Rect::from_center_size(p, vec2(16.0, 16.0)));
                    if ms.site_labels && (self.zoom > 0.35 || s.id == self.site) {
                        placed.push(Rect::from_min_size(p + vec2(6.0, -20.0), vec2(36.0, 14.0)));
                    }
                }
            }
        }
        let mut shown_shields: Vec<(&str, Pos2)> = Vec::new();
        // tiles overlap a little, so the same town can come from two of them
        let mut shown_places: Vec<(&str, Pos2)> = Vec::new();
        for (_, pos, text, kind) in cands {
            if placed.len() > 160 {
                break;
            }
            match kind {
                Kind::Place(class) => {
                    if shown_places
                        .iter()
                        .any(|(n, p)| *n == text && p.distance(pos) < 40.0)
                    {
                        continue;
                    }
                    let (size, col) = match class {
                        PlaceClass::City => (13.0, ms.city_color),
                        PlaceClass::Town => (12.0, ms.city_color.gamma_multiply(0.9)),
                        PlaceClass::Village => (11.0, t.muted),
                        PlaceClass::Hamlet => (10.0, t.muted.gamma_multiply(0.85)),
                    };
                    let font = FontId::monospace(size);
                    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), col);
                    let size = galley.size();
                    let right =
                        Rect::from_min_size(pos + vec2(5.0, -size.y / 2.0), size).expand(2.0);
                    let left = Rect::from_min_size(pos + vec2(-5.0 - size.x, -size.y / 2.0), size)
                        .expand(2.0);
                    let free = |r: &Rect| !placed.iter().any(|p| p.intersects(*r));
                    let (r, anchor, at) = if free(&right) {
                        (right, Align2::LEFT_CENTER, pos + vec2(6.0, 0.0))
                    } else if free(&left) {
                        (left, Align2::RIGHT_CENTER, pos - vec2(6.0, 0.0))
                    } else {
                        continue;
                    };
                    placed.push(r);
                    shown_places.push((text, pos));
                    painter.circle_filled(
                        pos,
                        if class == PlaceClass::City { 2.5 } else { 1.8 },
                        col,
                    );
                    halo_text(painter, at, anchor, text, font, col, t.bg);
                }
                Kind::Shield(class) => {
                    // one shield per route per ~180 px
                    if shown_shields
                        .iter()
                        .any(|(s, p)| *s == text && p.distance(pos) < 180.0)
                    {
                        continue;
                    }
                    let font = FontId::monospace(10.0);
                    let col = self.cfg.map.road_colors[class as usize].to_opaque();
                    let galley = painter.layout_no_wrap(text.to_string(), font, t.fg);
                    let r = Rect::from_center_size(pos, galley.size() + vec2(8.0, 4.0));
                    if placed.iter().any(|p| p.intersects(r)) {
                        continue;
                    }
                    placed.push(r);
                    shown_shields.push((text, pos));
                    painter.rect_filled(r, 4, t.bg);
                    painter.rect_stroke(r, 4, Stroke::new(1.0, col), egui::StrokeKind::Inside);
                    painter.galley(r.min + vec2(4.0, 2.0), galley, t.fg);
                }
            }
        }
    }

    pub(super) fn draw_warnings(&self, painter: &Painter, rect: Rect) {
        for (al, r) in &self.alerts {
            if !self.cfg.alerts.rules[*r].map || self.hidden.contains(&al.id) {
                continue;
            }
            let col = self.cfg.alerts.rules[*r].color;
            for poly in &al.polygons {
                let pts: Vec<Pos2> = poly
                    .iter()
                    .map(|&(x, y)| self.ll_to_screen(rect, x, y))
                    .collect();
                if pts.len() < 2 {
                    continue;
                }
                if al.county_based {
                    // watches: dashed county outlines
                    painter.extend(Shape::dashed_line(&pts, Stroke::new(1.5, col), 6.0, 4.0));
                } else {
                    // warnings: solid polygon with a dark edge so it reads over radar
                    painter.add(Shape::closed_line(
                        pts.clone(),
                        Stroke::new(4.0, self.cfg.theme.bg.gamma_multiply(0.7)),
                    ));
                    painter.add(Shape::closed_line(pts, Stroke::new(2.0, col)));
                }
            }
        }
    }
}
