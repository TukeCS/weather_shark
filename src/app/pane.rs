//! One radar pane: input (pan/zoom/click), radar image, map layers, header and legend.

use egui::{
    Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, TextureOptions, Ui, UiBuilder,
    pos2, vec2,
};

use super::App;
use crate::level2::{Moment, Sweep};
use crate::render;

impl App {
    /// Rasterize one sweep of one product into a texture (and its range in km).
    fn upload(&self, ctx: &egui::Context, name: String, sweep: &Sweep, m: Moment) -> (egui::TextureHandle, f32) {
        let range = sweep.range_km(m).clamp(50.0, 470.0);
        let img = render::rasterize(
            sweep,
            m,
            self.cfg.palette(m),
            self.cfg.unit_factor(m),
            self.cfg.image_size,
            range,
        );
        let opts = if self.cfg.smooth {
            TextureOptions::LINEAR
        } else {
            TextureOptions::NEAREST
        };
        (ctx.load_texture(name, img, opts), range)
    }

    fn texture(&mut self, ctx: &egui::Context, m: Moment) -> Option<(egui::TextureHandle, f32)> {
        let n = self.scan.as_ref()?.vol.sweep_for(m, self.tilt)?.number;
        self.sweep_texture(ctx, m, n)
    }

    /// The image of one sweep (by number) of the scan on screen.
    fn sweep_texture(&mut self, ctx: &egui::Context, m: Moment, n: u8) -> Option<(egui::TextureHandle, f32)> {
        let scan = self.scan.as_ref()?;
        let sweep = scan.vol.sweeps.iter().find(|s| s.number == n && s.has(m))?;
        let key = (scan.generation, sweep.number, m, false);
        if let Some(t) = self.textures.get(&key) {
            return Some(t.clone());
        }
        let t = self.upload(ctx, format!("{}-{}-{:?}", scan.key, sweep.number, m), sweep, m);
        self.textures.insert(key, t.clone());
        Some(t)
    }

    /// What was on screen for this tilt before sweep `number` started, shown under
    /// it while it's swept in: an earlier scan of the same tilt (SAILS rescans
    /// come several times a volume), or else the previous volume's.
    fn old_texture(
        &mut self,
        ctx: &egui::Context,
        m: Moment,
        number: u8,
    ) -> Option<(egui::TextureHandle, f32)> {
        let scan = self.scan.as_ref()?;
        let el = scan.vol.sweeps.iter().find(|s| s.number == number)?.elevation;
        if let Some(prev) = scan
            .vol
            .sweep_for_except(m, el, Some(number))
            .filter(|s| (s.elevation - el).abs() < 0.25)
            .map(|s| s.number)
        {
            return self.sweep_texture(ctx, m, prev);
        }
        let generation = scan.generation;
        let key = (generation, number, m, true);
        if let Some(t) = self.textures.get(&key) {
            return Some(t.clone());
        }
        let base = self.feed.as_ref()?.base.clone()?;
        let sweep = base.sweeps.iter().find(|s| s.number == number && s.has(m))?;
        let t = self.upload(ctx, format!("old-{generation}-{number}-{m:?}"), sweep, m);
        self.textures.insert(key, t.clone());
        Some(t)
    }

    /// Part of a radar image: the slice from `from_az` clockwise for `span` degrees.
    fn draw_wedge(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        tex: &egui::TextureHandle,
        range: f32,
        from_az: f32,
        span: f32,
    ) {
        if span <= 0.0 {
            return;
        }
        let uv = |x: f32, y: f32| pos2((x + range) / (2.0 * range), (range - y) / (2.0 * range));
        let vertex = |x: f32, y: f32| egui::epaint::Vertex {
            pos: self.to_screen(rect, pos2(x, y)),
            uv: uv(x, y),
            color: Color32::WHITE,
        };
        let mut mesh = egui::Mesh::with_texture(tex.id());
        mesh.vertices.push(vertex(0.0, 0.0));
        let steps = (span / 2.0).ceil().max(1.0) as u32;
        for i in 0..=steps {
            let a = (from_az + span * i as f32 / steps as f32).to_radians();
            mesh.vertices.push(vertex(range * a.sin(), range * a.cos()));
            if i > 0 {
                mesh.add_triangle(0, i, i + 1);
            }
        }
        painter.add(mesh);
    }

    /// The rotating antenna line, with a fading wedge behind it over the fresh data.
    fn draw_sweep_line(&self, painter: &egui::Painter, rect: Rect, az: f32, range: f32) {
        let col = self.cfg.theme.accent;
        let o = self.to_screen(rect, Pos2::ZERO);
        let at = |deg: f32| {
            let a = deg.to_radians();
            self.to_screen(rect, pos2(range * a.sin(), range * a.cos()))
        };
        let (steps, trail, peak) = (32, 45.0, 0.16);
        let mut mesh = egui::Mesh::default();
        for i in 0..steps {
            let f0 = i as f32 / steps as f32;
            let f1 = (i + 1) as f32 / steps as f32;
            let (c0, c1) = (col.gamma_multiply(peak * f0), col.gamma_multiply(peak * f1));
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(o, col.gamma_multiply(peak * (f0 + f1) / 2.0));
            mesh.colored_vertex(at(az - trail * (1.0 - f0)), c0);
            mesh.colored_vertex(at(az - trail * (1.0 - f1)), c1);
            mesh.add_triangle(base, base + 1, base + 2);
        }
        painter.add(mesh);
        painter.line_segment([o, at(az)], Stroke::new(2.0, col));
    }

    pub(super) fn pane(&mut self, ui: &mut Ui, rect: Rect, idx: usize) {
        if rect.width() < 40.0 || rect.height() < 40.0 {
            return; // mid-animation sliver
        }
        let t = self.cfg.theme.clone();
        let ms = self.cfg.map.clone();
        let m = self.panes[idx].moment;
        let id = egui::Id::NULL.with(("pane", self.panes[idx].id));
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        let painter = ui.painter_at(rect);

        // input: drag to pan, scroll/pinch to zoom around the cursor
        if resp.dragged() {
            self.fitted = true;
            self.user_moved = true;
            let d = resp.drag_delta();
            self.center += vec2(-d.x, d.y) / self.zoom;
        }
        if let Some(p) = resp.hover_pos().filter(|_| self.shot.is_none()) {
            self.hovered_pane = Some(idx);
            self.hover = Some(self.to_km(rect, p));
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = (scroll * 0.0018).exp() * pinch;
            if factor != 1.0 {
                self.user_moved = true;
                let before = self.to_km(rect, p);
                self.zoom = (self.zoom * factor).clamp(0.05, 200.0);
                let after = self.to_km(rect, p);
                self.center += before - after;
            }
        }
        if resp.double_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.center = self.to_km(rect, p).to_vec2();
            self.zoom = (self.zoom * 2.0).min(200.0);
        }

        painter.rect_filled(rect, t.radius, t.panel);

        // basemap under the radar: counties and minor roads
        if self.show.counties
            && let Some(c) = ms.counties.filter(|_| self.zoom > 0.6)
        {
            self.draw_layer(&painter, rect, &self.map.counties_km, c, 0.8);
        }
        let tiles = self.visible_tiles(rect);
        if self.show.roads {
            self.draw_roads(&painter, rect, &tiles, true);
        }

        // SPC outlook fill, under the radar so echoes keep their colours
        if let Some(o) = self
            .outlook_shown()
            .filter(|_| self.cfg.outlook.fill_under_radar)
        {
            self.draw_outlook_fill(&painter, rect, o);
        }

        // radar image. The tilt being scanned is swept in over its previous scan,
        // chunk by chunk, behind a rotating line.
        let now = ui.input(|i| i.time);
        let shown = self
            .scan
            .as_ref()
            .and_then(|s| s.vol.sweep_for(m, self.tilt))
            .map(|s| s.number);
        let reveal = shown.and_then(|n| Some((n, self.reveal(n, now)?)));
        let full = |s: &Self, tex: &egui::TextureHandle, range: f32| {
            let a = s.to_screen(rect, pos2(-range, range));
            let b = s.to_screen(rect, pos2(range, -range));
            painter.image(
                tex.id(),
                Rect::from_two_pos(a, b),
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        };
        match reveal {
            Some((n, (from_az, swept))) => {
                // the previous scan of this tilt, then the new data behind the line
                if let Some((old, range)) = self.old_texture(ui.ctx(), m, n) {
                    full(self, &old, range);
                }
                if let Some((tex, range)) = self.texture(ui.ctx(), m) {
                    self.draw_wedge(&painter, rect, &tex, range, from_az, swept);
                    self.draw_sweep_line(&painter, rect, from_az + swept, range);
                }
            }
            None => {
                if let Some((tex, range)) = self.texture(ui.ctx(), m) {
                    full(self, &tex, range);
                }
            }
        }

        // over the radar: main roads, state lines, rings, warnings
        if self.show.roads {
            self.draw_roads(&painter, rect, &tiles, false);
        }
        if let Some(c) = ms.states {
            self.draw_layer(&painter, rect, &self.map.states_km, c, 1.2);
        }
        if self.show.rings
            && let Some(c) = ms.range_rings
        {
            let o = self.to_screen(rect, Pos2::ZERO);
            for r in (50..=450).step_by(50) {
                painter.circle_stroke(o, r as f32 * self.zoom, Stroke::new(1.0, c));
            }
        }
        if let Some(o) = self.outlook_shown() {
            if !self.cfg.outlook.fill_under_radar {
                self.draw_outlook_fill(&painter, rect, o);
            }
            self.draw_outlook_lines(&painter, rect, o);
        }
        if (self.show.warnings || self.show.watches) && self.cfg.alerts.enabled {
            self.draw_warnings(&painter, rect);
        }
        self.draw_labels(&painter, rect, &tiles);

        // radar sites (click to switch)
        let pointer = resp.hover_pos();
        let mut hovered_site = None;
        if self.show.sites {
            for s in &self.sites {
                let p = self.ll_to_screen(rect, s.lon, s.lat);
                if !rect.expand(20.0).contains(p) {
                    continue;
                }
                let cur = s.id == self.site;
                let near = pointer.is_some_and(|q| q.distance(p) < 10.0);
                if near {
                    hovered_site = Some(s.id.clone());
                }
                let r = if near { 5.0 } else { 3.5 };
                painter.circle_filled(p, r + 1.5, t.bg.gamma_multiply(0.8));
                painter.circle_filled(p, r, if cur { t.fg } else { ms.sites });
                if cur {
                    painter.circle_stroke(p, 7.0, Stroke::new(1.5, ms.sites));
                }
                if ms.site_labels && (self.zoom > 0.35 || near || cur) {
                    super::mapdraw::halo_text(
                        &painter,
                        p + vec2(7.0, -7.0),
                        Align2::LEFT_BOTTOM,
                        &s.id,
                        FontId::monospace(10.5),
                        if near { t.fg } else { ms.sites },
                        t.bg,
                    );
                }
            }
        }
        if let Some(s) = &hovered_site {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            if resp.clicked() && *s != self.site {
                let s = s.clone();
                self.live = true;
                self.select_site(&s, None);
            }
            resp.clone().on_hover_text(
                self.sites
                    .iter()
                    .find(|x| x.id == *s)
                    .map(|x| format!("{}  {}", x.id, x.name))
                    .unwrap_or_default(),
            );
        }

        // crosshair (shared by all panes)
        let mut readout = None;
        if let Some(h) = self.hover {
            let p = self.to_screen(rect, h);
            if rect.contains(p) && self.cfg.ui.crosshair {
                let c = ms.crosshair;
                for (a, b) in [
                    (vec2(-9.0, 0.0), vec2(-3.0, 0.0)),
                    (vec2(3.0, 0.0), vec2(9.0, 0.0)),
                    (vec2(0.0, -9.0), vec2(0.0, -3.0)),
                    (vec2(0.0, 3.0), vec2(0.0, 9.0)),
                ] {
                    painter.line_segment([p + a, p + b], Stroke::new(1.5, c));
                }
            }
            if let Some(scan) = &self.scan
                && self.cfg.ui.readout
                && let Some(sw) = scan.vol.sweep_for(m, self.tilt)
            {
                readout = Some(
                    match render::sample(
                        sw,
                        m,
                        self.cfg.unit_factor(m),
                        self.cfg.palette(m).mask_ref,
                        h.x,
                        h.y,
                    ) {
                        Some(v) if m == Moment::Rho => format!("{v:.3}"),
                        Some(v) => format!("{v:.1} {}", self.cfg.unit_label(m)),
                        None => String::new(),
                    },
                );
            }
        }

        if self.scan.is_none() {
            let at = rect.center() + vec2(0.0, 44.0);
            match (&self.error, self.load_fraction()) {
                (Some(e), _) => {
                    painter.text(
                        at,
                        Align2::CENTER_CENTER,
                        e,
                        FontId::monospace(13.0),
                        t.muted,
                    );
                }
                (None, frac) => {
                    let what = match self.progress {
                        Some((got, _)) if got > 0 => {
                            format!("downloading {} · {}", self.site, self.progress_text())
                        }
                        _ => format!("finding the latest {} scan…", self.site),
                    };
                    painter.text(
                        at,
                        Align2::CENTER_CENTER,
                        what,
                        FontId::monospace(12.0),
                        t.muted,
                    );
                    let bar = Rect::from_center_size(at + vec2(0.0, 18.0), vec2(220.0, 4.0));
                    painter.rect_filled(bar, 2, t.line);
                    let f = frac.unwrap_or(0.0);
                    if f > 0.0 {
                        let fill =
                            Rect::from_min_size(bar.min, vec2(bar.width() * f, bar.height()));
                        painter.rect_filled(fill, 2, t.accent);
                    } else {
                        // unknown size yet: a sliding pulse
                        let x =
                            ((ui.input(|i| i.time) * 0.8).fract() as f32) * (bar.width() - 50.0);
                        painter.rect_filled(
                            Rect::from_min_size(bar.min + vec2(x, 0.0), vec2(50.0, bar.height())),
                            2,
                            t.accent,
                        );
                        ui.ctx().request_repaint();
                    }
                }
            }
        }

        let mut close = false;
        if self.cfg.ui.header {
            // header: a soft band so map lines don't run through the labels
            let band = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + 44.0));
            painter.rect_filled(
                band,
                egui::CornerRadius {
                    nw: t.radius as u8,
                    ne: t.radius as u8,
                    sw: 0,
                    se: 0,
                },
                t.panel.gamma_multiply(0.82),
            );

            // header: product picker, close button, readout
            let head =
                Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(rect.width() - 20.0, 24.0));
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(head)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    let mut sel = m;
                    egui::ComboBox::from_id_salt(("product", self.panes[idx].id))
                        .selected_text(egui::RichText::new(m.key()).color(t.accent).strong())
                        .width(64.0)
                        .show_ui(ui, |ui| {
                            for mm in Moment::ALL {
                                ui.selectable_value(
                                    &mut sel,
                                    mm,
                                    format!("{:<4} {}", mm.key(), mm.title()),
                                );
                            }
                        });
                    self.panes[idx].moment = sel;
                    if rect.width() > 360.0 {
                        let elev = self
                            .scan
                            .as_ref()
                            .and_then(|s| s.vol.sweep_for(m, self.tilt))
                            .map(|s| s.elevation);
                        let sub = match elev {
                            Some(e) => format!("{} · {e:.1}°", m.title()),
                            None => m.title().to_string(),
                        };
                        ui.label(egui::RichText::new(sub).color(t.muted).small());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.panes.len() > 1
                            && ui
                                .small_button("×")
                                .on_hover_text("close pane (x)")
                                .clicked()
                        {
                            close = true;
                        }
                        if let Some(r) = readout {
                            ui.label(egui::RichText::new(r).color(t.fg));
                        }
                    });
                },
            );
        }

        // legend
        let small = FontId::monospace(10.0);
        if self.cfg.ui.legend {
            let pal = self.cfg.palette(m);
            let lw = (rect.width() - 24.0).min(220.0);
            let lr = Rect::from_min_size(pos2(rect.min.x + 12.0, rect.max.y - 22.0), vec2(lw, 6.0));
            let (lo, hi) = (pal.min(), pal.max());
            let plate = Rect::from_min_max(lr.min - vec2(8.0, 18.0), lr.max + vec2(8.0, 7.0));
            painter.rect_filled(plate, 6, t.panel.gamma_multiply(0.85));
            let steps = 64;
            for i in 0..steps {
                let v = lo + (hi - lo) * (i as f32 + 0.5) / steps as f32;
                let x0 = lr.min.x + lr.width() * i as f32 / steps as f32;
                let x1 = lr.min.x + lr.width() * (i + 1) as f32 / steps as f32 + 0.5;
                painter.rect_filled(
                    Rect::from_x_y_ranges(x0..=x1, lr.y_range()),
                    0,
                    pal.color(v),
                );
            }
            let fmt = |v: f32| {
                if m == Moment::Rho {
                    format!("{v:.2}")
                } else {
                    format!("{v:.0}")
                }
            };
            painter.text(
                lr.left_top() - vec2(0.0, 3.0),
                Align2::LEFT_BOTTOM,
                fmt(lo),
                small.clone(),
                t.muted,
            );
            painter.text(
                lr.right_top() - vec2(0.0, 3.0),
                Align2::RIGHT_BOTTOM,
                format!("{} {}", fmt(hi), self.cfg.unit_label(m)),
                small.clone(),
                t.muted,
            );
        }

        if let Some(o) = self.outlook_shown()
            && rect.width() > 300.0
        {
            let (_, _, d) = crate::net::civil_from_days(o.expire.div_euclid(86400));
            let text = format!(
                "SPC day {} {} · until {:02}Z on the {d}{}",
                o.sel.day,
                o.sel.kind.name(),
                o.expire.rem_euclid(86400) / 3600,
                if o.areas.is_empty() {
                    " · no risk areas"
                } else {
                    ""
                },
            );
            painter.text(
                rect.right_bottom() - vec2(10.0, 22.0),
                Align2::RIGHT_BOTTOM,
                text,
                small.clone(),
                t.muted,
            );
        }
        if (self.show.roads || self.show.cities)
            && self.tiles.template.is_some()
            && rect.width() > 420.0
        {
            painter.text(
                rect.right_bottom() - vec2(10.0, 8.0),
                Align2::RIGHT_BOTTOM,
                "© OpenStreetMap · OpenFreeMap",
                small,
                t.muted.gamma_multiply(0.7),
            );
        }

        painter.rect_stroke(
            rect,
            t.radius,
            Stroke::new(
                1.0,
                if self.hovered_pane == Some(idx) && self.panes.len() > 1 {
                    t.line.gamma_multiply(1.8)
                } else {
                    t.line
                },
            ),
            StrokeKind::Inside,
        );

        if close {
            self.close_pane(idx);
        }
    }
}
