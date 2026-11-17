//! Top bar, side panels (alerts, radio, Lua settings) and the error toast.

use egui::{Color32, FontId, Key, Rect, RichText, Sense, Stroke, StrokeKind, Ui, vec2};

use super::{App, Side, heading};
use crate::config::{Action, DEFAULT_LUA, MAX_PANES};
use crate::net;

#[derive(Clone, Copy)]
enum CardAction {
    Focus,
    Remove,
    Hide,
    Show,
    ShowAll,
}

const ERR: Color32 = Color32::from_rgb(255, 130, 130);
const LIVE_RED: Color32 = Color32::from_rgb(255, 69, 95);

impl App {
    fn side_toggle(&mut self, ui: &mut Ui, side: Side, text: RichText, hint: &str) {
        if ui
            .selectable_label(self.side == side, text)
            .on_hover_text(hint)
            .clicked()
        {
            self.side = if self.side == side { Side::None } else { side };
        }
    }

    /// " (N)" for a hover hint, or "" if the action has no key.
    fn key_hint(&self, a: Action) -> String {
        self.cfg
            .keys
            .iter()
            .find(|(x, _)| *x == a)
            .map(|(_, k)| format!(" ({})", k.symbol_or_name()))
            .unwrap_or_default()
    }

    /// Alerts to count on the button (listed and not hidden), and the colour of
    /// the most important one.
    fn alert_badge(&self) -> (usize, Color32) {
        let listed: Vec<usize> = self
            .listed_alerts()
            .filter(|(_, a)| !self.hidden.contains(&a.id))
            .map(|(i, _)| i)
            .collect();
        let col = listed
            .first()
            .map(|&i| self.cfg.alerts.rules[self.alerts[i].1].color.to_opaque())
            .unwrap_or(self.cfg.theme.fg);
        (listed.len(), col)
    }

    /// The top bar shows exactly the items listed in `ui.toolbar`, in that order.
    /// `level` squeezes it into narrow windows: 1 folds the panel buttons (pane,
    /// map, events, alerts, radio, lua) into a "more" menu, 2 also drops the app name,
    /// the scan's age and the loop-length picker (that one moves into "more" too).
    pub(super) fn top_bar(&mut self, ui: &mut Ui, now: f64, level: u8) {
        let t = self.cfg.theme.clone();
        ui.horizontal_centered(|ui| {
            // logo dot: pulses red while a new alert is fresh
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            let c = if now < self.alarm_until {
                let k = (0.5 + 0.5 * (now * 6.0).sin()) as f32;
                Color32::from_rgb(255, (55.0 * k) as u8, (95.0 * k) as u8)
            } else {
                t.accent
            };
            ui.painter().circle_filled(r.center(), 4.0, c);
            if level < 2 {
                ui.label(RichText::new("weather_shark").strong());
            }

            let items = self.cfg.ui.toolbar.clone();
            let folded = |item: &str| {
                level >= 1 && matches!(item, "panes" | "map" | "events" | "alerts" | "radio" | "lua")
            };
            let mut last_group = "";
            for item in items.iter().filter(|i| !folded(i)) {
                // a thin divider between the radar controls and the panels
                let group = if matches!(item.as_str(), "site" | "time" | "tilt" | "panes") {
                    "radar"
                } else {
                    "panels"
                };
                if group != last_group {
                    ui.separator();
                    last_group = group;
                }
                match item.as_str() {
                    "site" => self.site_picker(ui),
                    "time" => self.time_controls(ui, level >= 2),
                    "tilt" => {
                        let tilts = self.tilts();
                        egui::ComboBox::from_id_salt("tilt")
                            .selected_text(format!("{:.1}°", self.tilt))
                            .width(64.0)
                            .show_ui(ui, |ui| {
                                for tl in &tilts {
                                    ui.selectable_value(&mut self.tilt, *tl, format!("{tl:.1}°"));
                                }
                            })
                            .response
                            .on_hover_text(format!("tilt{}", self.key_hint(Action::TiltUp)));
                    }
                    "panes" => {
                        let can_add = self.panes.len() < MAX_PANES;
                        let hint = format!("split another pane{}", self.key_hint(Action::AddPane));
                        if ui
                            .add_enabled(can_add, egui::Button::new("+ pane"))
                            .on_hover_text(hint)
                            .clicked()
                        {
                            self.add_pane();
                        }
                    }
                    "map" => self.map_menu(ui),
                    "events" => self.events_menu(ui),
                    "alerts" if self.cfg.alerts.enabled => {
                        let (n, col) = self.alert_badge();
                        let txt = RichText::new(format!("alerts {n}")).color(col);
                        self.side_toggle(
                            ui,
                            Side::Alerts,
                            txt,
                            "active warnings and watches",
                        );
                        self.tornado_label(ui);
                    }
                    "radio" if self.cfg.radio.enabled => {
                        let txt = match &self.playing {
                            Some(call) => RichText::new(format!("● {call}")).color(LIVE_RED),
                            None => RichText::new("radio"),
                        };
                        let hint = format!("NOAA Weather Radio{}", self.key_hint(Action::Radio));
                        self.side_toggle(ui, Side::Radio, txt, &hint);
                    }
                    "lua" => {
                        let hint = format!("edit config.lua{}", self.key_hint(Action::Settings));
                        self.side_toggle(ui, Side::Settings, RichText::new("lua"), &hint);
                    }
                    _ => {}
                }
            }
            if level >= 1 {
                ui.separator();
                self.more_menu(ui, &items, level);
                if items.iter().any(|i| i == "alerts") && self.cfg.alerts.enabled {
                    self.tornado_label(ui);
                }
            }
        });
    }

    /// "TOR n" in red while any tornado warning is in effect in the country.
    fn tornado_label(&self, ui: &mut Ui) {
        let tor = self.tornado_total();
        if tor > 0 {
            ui.label(RichText::new(format!("TOR {tor}")).color(LIVE_RED).strong())
                .on_hover_text(format!(
                    "{tor} tornado warning{} in effect across the US",
                    if tor == 1 { "" } else { "s" }
                ));
        }
    }

    /// The "more" menu for narrow windows: the folded toolbar buttons, as menu items.
    fn more_menu(&mut self, ui: &mut Ui, items: &[String], level: u8) {
        let t = self.cfg.theme.clone();
        let (n, col) = self.alert_badge();
        let has = |x: &str| items.iter().any(|i| i == x);
        let alerts = has("alerts") && self.cfg.alerts.enabled;
        let mut label = egui::text::LayoutJob::default();
        label.append("more ▾", 0.0, egui::TextFormat::simple(FontId::monospace(14.0), t.fg));
        if alerts && n > 0 {
            label.append(&format!(" {n}"), 0.0, egui::TextFormat::simple(FontId::monospace(13.0), col));
        }
        let btn = ui.button(label).on_hover_text("more");
        egui::Popup::menu(&btn)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(180.0);
                if has("panes")
                    && ui
                        .add_enabled(self.panes.len() < MAX_PANES, egui::Button::new("+ pane"))
                        .clicked()
                {
                    self.add_pane();
                    ui.close();
                }
                if has("map") {
                    ui.menu_button("map", |ui| self.map_menu_body(ui));
                }
                if has("events") {
                    ui.menu_button("events", |ui| self.events_menu_body(ui));
                }
                let mut toggle = |ui: &mut Ui, side: Side, text: RichText| {
                    if ui.selectable_label(self.side == side, text).clicked() {
                        self.side = if self.side == side { Side::None } else { side };
                        ui.close();
                    }
                };
                if alerts {
                    toggle(ui, Side::Alerts, RichText::new(format!("alerts {n}")).color(col));
                }
                if has("radio") && self.cfg.radio.enabled {
                    let txt = match &self.playing {
                        Some(call) => RichText::new(format!("radio ● {call}")).color(LIVE_RED),
                        None => RichText::new("radio"),
                    };
                    toggle(ui, Side::Radio, txt);
                }
                if has("lua") {
                    toggle(ui, Side::Settings, RichText::new("lua"));
                }
                if level >= 2 && has("time") {
                    ui.separator();
                    ui.menu_button(format!("loop: {} min", self.lopt.minutes), |ui| {
                        self.loop_options(ui);
                    });
                }
            });
    }

    /// The loop's settings. They apply while it plays; a new length restarts it.
    fn loop_options(&mut self, ui: &mut Ui) {
        let muted = self.cfg.theme.muted;
        ui.set_min_width(240.0);
        heading(ui, "LENGTH", muted);
        let mut mins = self.lopt.minutes;
        ui.horizontal_wrapped(|ui| {
            for m in [10, 15, 30, 60, 120] {
                let txt = if m < 60 { format!("{m} min") } else { format!("{} h", m / 60) };
                ui.selectable_value(&mut mins, m, txt);
            }
        });
        self.set_loop_minutes(ui, mins);

        heading(ui, "SPEED", muted);
        ui.horizontal(|ui| {
            for (name, fps) in [("slow", 1.5), ("normal", 2.5), ("fast", 6.0)] {
                if ui
                    .selectable_label((self.lopt.fps - fps).abs() < 0.05, name)
                    .clicked()
                {
                    self.lopt.fps = fps;
                }
            }
        });
        ui.add(
            egui::Slider::new(&mut self.lopt.fps, 0.5..=12.0)
                .step_by(0.5)
                .suffix(" frames/s"),
        );

        heading(ui, "HOLD THE NEWEST FRAME", muted);
        ui.add(
            egui::Slider::new(&mut self.lopt.hold, 0.0..=5.0)
                .step_by(0.1)
                .suffix(" s"),
        );

        ui.add_space(6.0);
        ui.checkbox(&mut self.lopt.rock, "back and forth")
            .on_hover_text("play forward, then backward, instead of jumping back to the start");
        ui.checkbox(&mut self.lopt.rescans, "every SAILS rescan")
            .on_hover_text("each rescan of the tilt is its own frame; off: one frame per scan");
    }

    fn set_loop_minutes(&mut self, ui: &Ui, mins: u32) {
        if mins != self.lopt.minutes {
            self.lopt.minutes = mins;
            if self.anim.is_some() {
                let now = ui.input(|i| i.time);
                self.anim = None;
                self.toggle_loop(now);
            }
        }
    }

    fn site_picker(&mut self, ui: &mut Ui) {
        let t = self.cfg.theme.clone();
        let site_btn = ui.button(RichText::new(format!("{} ▾", self.site)).color(t.accent));
        egui::Popup::menu(&site_btn).show(|ui| {
            ui.set_min_width(260.0);
            ui.add(egui::TextEdit::singleline(&mut self.site_search).hint_text("search sites…"));
            let q = self.site_search.to_lowercase();
            let mut pick = None;
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for s in &self.sites {
                        if (q.is_empty()
                            || s.id.to_lowercase().contains(&q)
                            || s.name.to_lowercase().contains(&q))
                            && ui
                                .selectable_label(
                                    s.id == self.site,
                                    format!("{}  {}", s.id, s.name),
                                )
                                .clicked()
                        {
                            pick = Some(s.id.clone());
                        }
                    }
                });
            if let Some(p) = pick {
                self.live = true;
                self.select_site(&p, None);
                ui.close();
            }
        });
    }

    /// Scan time, loading progress, ◀ ▶ and the live switch.
    fn time_controls(&mut self, ui: &mut Ui, compact: bool) {
        let t = self.cfg.theme.clone();
        match &self.scan {
            Some(s) => {
                let age = net::now() - s.vol.time;
                // stepping through rescans: the time of the frame on screen
                let shown = self.frame.map_or(s.vol.time, |t| t as i64);
                ui.label(net::fmt_time(shown));
                let tag = if s.key.starts_with("file:") {
                    "file".into()
                } else if s.key.starts_with("live:") {
                    match s.vol.antenna {
                        Some(a) if net::now_f64() - a.time < 60.0 => {
                            format!("scanning {:.1}°", a.elevation)
                        }
                        _ => "real time".into(),
                    }
                } else if age < 172_800 {
                    net::fmt_age(age)
                } else {
                    "archive".into()
                };
                let hover = s.key.strip_prefix("file:").unwrap_or(&s.key).to_string();
                if !compact {
                    ui.label(RichText::new(tag).color(t.muted))
                        .on_hover_text(hover);
                }
                if !self.live
                    && let Some((frames, i)) = self.frame_pos()
                    && frames.len() > 1
                {
                    ui.label(RichText::new(format!("SAILS {}/{}", i + 1, frames.len())).color(t.accent))
                        .on_hover_text("this tilt was scanned several times in this scan (SAILS); ◀ ▶ step through them");
                }
            }
            None => {
                ui.label(RichText::new("no scan").color(t.muted));
            }
        }
        if self.loading.is_some() || (self.scan.is_none() && self.error.is_none()) {
            ui.spinner();
            if let Some(f) = self.load_fraction() {
                ui.label(RichText::new(format!("{:.0}%", f * 100.0)).color(t.accent))
                    .on_hover_text(self.progress_text());
            }
        }
        if ui
            .button("◀")
            .on_hover_text(format!("previous scan{}", self.key_hint(Action::PrevScan)))
            .clicked()
        {
            self.step(-1);
        }
        if ui
            .button("▶")
            .on_hover_text(format!("next scan{}", self.key_hint(Action::NextScan)))
            .clicked()
        {
            self.step(1);
        }
        let playing = self.anim.is_some();
        let label = match self.loop_ready() {
            Some((have, n)) if have < n => format!("loop {have}/{n}"),
            _ if playing => "■ loop".to_string(),
            _ => "▶ loop".to_string(),
        };
        if ui
            .selectable_label(playing, label)
            .on_hover_text(format!(
                "play the last {} minutes{}",
                self.lopt.minutes,
                self.key_hint(Action::Play)
            ))
            .clicked()
        {
            let now = ui.input(|i| i.time);
            self.toggle_loop(now);
        }
        // the loop's settings (in the "more" menu when the bar is squeezed)
        if !compact {
            let btn = ui
                .button(format!("{} min ▾", self.lopt.minutes))
                .on_hover_text("loop length, speed, hold, back and forth");
            egui::Popup::menu(&btn)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| self.loop_options(ui));
        }
        let live = RichText::new("● live").color(if self.live { LIVE_RED } else { t.muted });
        if ui
            .selectable_label(self.live, live)
            .on_hover_text(format!(
                "follow the newest scan{}",
                self.key_hint(Action::Live)
            ))
            .clicked()
        {
            if self.live {
                self.live = false;
            } else {
                self.go_live();
            }
        }
    }

    fn map_menu(&mut self, ui: &mut Ui) {
        let btn = ui.button("map");
        egui::Popup::menu(&btn)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| self.map_menu_body(ui));
    }

    /// Layers, alert types and the SPC outlook (in the "map" button or the "more" menu).
    fn map_menu_body(&mut self, ui: &mut Ui) {
        ui.set_min_width(200.0);
        let muted = self.cfg.theme.muted;
        heading(ui, "LAYERS", muted);
        ui.checkbox(&mut self.show.roads, "roads  (m)");
        ui.add_enabled(
            self.show.roads,
            egui::Checkbox::new(&mut self.show.road_labels, "highway numbers"),
        );
        ui.checkbox(&mut self.show.cities, "cities & towns  (c)");
        ui.checkbox(&mut self.show.counties, "counties");
        ui.checkbox(&mut self.show.rings, "range rings");
        ui.checkbox(&mut self.show.sites, "radar sites");
        let (wk, ak) = (
            self.key_hint(Action::Warnings),
            self.key_hint(Action::Watches),
        );
        ui.checkbox(&mut self.show.warnings, format!("warnings{wk}"));
        ui.checkbox(&mut self.show.watches, format!("watches{ak}"))
            .on_hover_text(
                "tornado / severe thunderstorm watches, as dashed county outlines",
            );
        heading(ui, "READOUT", muted);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("speeds").color(muted).small());
            for u in crate::config::VelUnits::ALL {
                ui.selectable_value(&mut self.speed, u, u.label());
            }
        })
        .response
        .on_hover_text("units for velocity and spectrum width on screen; the colours don't change");
        ui.checkbox(&mut self.cursor_value, "value next to the cursor");
        self.alert_types_menu(ui);
        self.outlook_menu(ui);
        if let Some(e) = &self.tiles.error {
            ui.colored_label(ERR, e);
        } else if self.tiles.failed_count() > 0 {
            ui.label(
                RichText::new(format!("{} map tiles failed", self.tiles.failed_count()))
                    .color(muted)
                    .small(),
            );
        }
    }

    /// One checkbox per alert event that has a rule: switch a kind of alert off
    /// (map, list and sound) without editing the Lua.
    fn alert_types_menu(&mut self, ui: &mut Ui) {
        self.alert_types(ui, "alert_types");
    }

    /// The alert-type checkboxes (in the map menu and the alerts panel; they share
    /// one set of switches).
    fn alert_types(&mut self, ui: &mut Ui, salt: &str) {
        if !self.cfg.alerts.enabled || self.cfg.alerts.rules.is_empty() {
            return;
        }
        // unique events in rule order, coloured like their first rule
        let mut events: Vec<(String, Color32)> = Vec::new();
        for r in &self.cfg.alerts.rules {
            if !events.iter().any(|(e, _)| e == &r.event) {
                events.push((r.event.clone(), r.color.to_opaque()));
            }
        }
        let off = events.iter().filter(|(e, _)| self.off_events.contains(e)).count();
        let title = if off > 0 {
            format!("alert types ({} of {} on)", events.len() - off, events.len())
        } else {
            "alert types".to_string()
        };
        egui::CollapsingHeader::new(title)
            .id_salt(salt)
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.small_button("all").clicked() {
                        self.off_events.clear();
                    }
                    if ui.small_button("none").clicked() {
                        self.off_events = events.iter().map(|(e, _)| e.clone()).collect();
                    }
                });
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for (ev, col) in &events {
                        let mut on = !self.off_events.contains(ev);
                        if ui.checkbox(&mut on, RichText::new(ev).color(*col)).changed() {
                            if on {
                                self.off_events.remove(ev);
                            } else {
                                self.off_events.insert(ev.clone());
                            }
                        }
                    }
                });
            });
    }

    fn outlook_menu(&mut self, ui: &mut Ui) {
        use crate::spc::{Kind, Sel};
        let muted = self.cfg.theme.muted;
        heading(
            ui,
            &format!("SPC OUTLOOK{}", self.key_hint(Action::Outlook)),
            muted,
        );
        let cur = self.show.outlook;
        let day = cur.map_or(0, |s| s.day);
        let kind = cur.map_or(self.outlook_last.kind, |s| s.kind);
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(cur.is_none(), "off").clicked() {
                self.show.outlook = None;
            }
            ui.label(RichText::new("day").color(muted).small());
            for d in 1..=8u8 {
                let hint = if d >= 4 {
                    "15% / 30% chance of severe weather (SPC's day 4–8 outlook)"
                } else {
                    "categorical risk and probabilities"
                };
                if ui.selectable_label(day == d, d.to_string()).on_hover_text(hint).clicked() {
                    self.show.outlook = Some(Sel {
                        day: d,
                        kind: kind.on_day(d),
                    });
                }
            }
        });
        if let Some(sel) = cur {
            ui.horizontal_wrapped(|ui| {
                for &k in Kind::for_day(sel.day) {
                    if ui.selectable_label(sel.kind == k, k.name()).clicked() {
                        self.show.outlook = Some(Sel { kind: k, ..sel });
                    }
                }
            });
            let small = |s: String| RichText::new(s).color(muted).small();
            if let Some(e) = &self.outlook_error {
                ui.colored_label(ERR, e);
            } else if let Some(o) = self.outlook.as_ref().filter(|o| o.sel == sel) {
                if self.outlook_shown().is_none() {
                    ui.label(small(
                        "hidden: this scan is from outside the outlook's time".into(),
                    ));
                } else if o.areas.is_empty() {
                    ui.label(small("no risk areas in this outlook".into()));
                }
                ui.label(small(format!(
                    "valid {} – {}",
                    net::fmt_time(o.valid),
                    net::fmt_time(o.expire)
                )));
                for a in o.areas.iter().rev() {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 2, a.color);
                        ui.label(RichText::new(&a.name).small());
                    });
                }
            } else {
                ui.label(small("loading…".into()));
            }
        }
    }

    fn events_menu(&mut self, ui: &mut Ui) {
        let bm = ui.button("events");
        egui::Popup::menu(&bm).show(|ui| self.events_menu_body(ui));
    }

    /// Open a file, bookmarks and go-to-time (in the "events" button or the "more" menu).
    fn events_menu_body(&mut self, ui: &mut Ui) {
        let t = self.cfg.theme.clone();
        ui.set_min_width(280.0);
        if ui
            .button(format!("open file…{}", self.key_hint(Action::Open)))
            .on_hover_text("a NEXRAD Level II file (or drop one on the window)")
            .clicked()
        {
            if let Err(e) = self.net.pick_file() {
                self.error = Some(e);
            }
            ui.close();
        }
        if self.cfg.bookmarks.is_empty() {
            ui.separator();
        } else {
            heading(ui, "BOOKMARKS", t.muted);
        }
        let mut go = None;
        for b in &self.cfg.bookmarks {
            if ui
                .button(format!("{:<14} {} {}", b.name, b.site, b.time))
                .clicked()
            {
                go = Some((b.site.clone(), b.time.clone()));
            }
        }
        ui.separator();
        ui.label(
            RichText::new("go to  SITE YYYY-MM-DD HH:MM (UTC)")
                .color(t.muted)
                .small(),
        );
        let r = ui
            .add(egui::TextEdit::singleline(&mut self.jump).hint_text("KTLX 2013-05-20 20:08"));
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            let s = self.jump.trim().to_string();
            let (site, time) = s.split_once(' ').unwrap_or((&s, ""));
            go = Some((site.to_uppercase(), time.to_string()));
        }
        if let Some((site, time)) = go {
            match net::parse_time(&time) {
                Some(t) => self.select_site(&site, Some(t + 60)),
                None => self.error = Some(format!("can't read time {time:?}")),
            }
            ui.close();
        }
    }

    pub(super) fn settings_panel(&mut self, ui: &mut Ui) {
        let t = self.cfg.theme.clone();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("config.lua").strong());
            #[cfg(not(target_arch = "wasm32"))]
            {
                // long paths are shortened with … (full path on hover) so they can't widen the panel
                let path = crate::config_path().display().to_string();
                ui.add(egui::Label::new(RichText::new(&path).color(t.muted).small()).truncate())
                    .on_hover_text(path);
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("apply").on_hover_text("ctrl+s").clicked() {
                self.save_editor(ui.ctx());
            }
            if ui.button("reset to default").clicked() {
                self.editor = DEFAULT_LUA.to_string();
                self.editor_dirty = true;
            }
            ui.label(RichText::new("test:").color(t.muted));
            for s in crate::sound::SOUNDS {
                if ui.small_button(s).clicked() {
                    self.player.play(s, None, 1, self.cfg.alerts.volume);
                }
            }
        });
        match &self.cfg_error {
            Some(e) => ui.colored_label(ERR, e),
            None => ui.label(
                RichText::new(if self.editor_dirty {
                    "unsaved changes"
                } else {
                    "ok"
                })
                .color(t.muted)
                .small(),
            ),
        };
        ui.add_space(4.0);
        let syntax = self.cfg.syntax.clone();
        let font = FontId::monospace(t.font_size);
        let fg = t.fg;
        let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
            let mut job = super::luahl::highlight(buf.as_str(), &syntax, font.clone(), fg);
            job.wrap.max_width = wrap;
            ui.painter().layout_job(job)
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            // exactly the panel's width: INFINITY would stretch the whole window's layout
            let w = ui.available_width();
            let r = ui.add(
                egui::TextEdit::multiline(&mut self.editor)
                    .code_editor()
                    .desired_width(w)
                    .desired_rows(40)
                    .lock_focus(true)
                    .layouter(&mut layouter),
            );
            if r.changed() {
                self.editor_dirty = true;
            }
        });
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::S)) {
            self.save_editor(ui.ctx());
        }
    }

    pub(super) fn alerts_panel(&mut self, ui: &mut Ui) {
        let t = self.cfg.theme.clone();
        ui.add_space(8.0);
        ui.label(RichText::new("ACTIVE WARNINGS & WATCHES").color(t.muted).small());
        if let Some(e) = &self.alerts_error {
            ui.colored_label(ERR, e);
            if !self.alerts.is_empty() {
                ui.label(RichText::new("showing the last alerts received").color(t.muted).small());
            }
        }
        let tor = self.tornado_total();
        let tor_near = self
            .alerts
            .iter()
            .filter(|(a, _)| a.near && super::is_real_tornado(a))
            .count();
        ui.label(
            RichText::new(format!(
                "tornado warnings: {tor} in the US, {tor_near} near {}",
                self.site
            ))
            .color(if tor > 0 { LIVE_RED } else { t.muted }),
        );
        // filters: a search box and the alert-type switches
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.alert_query)
                .hint_text("search: state, county, event…")
                .desired_width(f32::INFINITY),
        );
        self.alert_types(ui, "alert_types_panel");
        let q = self.alert_query.trim().to_lowercase();
        let hit = |a: &crate::alerts::Alert| {
            q.is_empty()
                || [&a.event, &a.area, &a.headline]
                    .iter()
                    .any(|s| s.to_lowercase().contains(&q))
        };
        let mut focus = None;
        let mut action: Option<(CardAction, String)> = None;
        let listed: Vec<(usize, bool)> = self
            .listed_alerts()
            .filter(|(_, a)| hit(a))
            .map(|(i, a)| (i, self.hidden.contains(&a.id)))
            .collect();
        ui.label(
            RichText::new(match (listed.len(), q.is_empty()) {
                (0, true) => "no active alerts".to_string(),
                (0, false) => "no alerts match".to_string(),
                (n, _) => format!("{n} alert{}", if n == 1 { "" } else { "s" }),
            })
            .color(t.muted)
            .small(),
        );
        egui::ScrollArea::vertical().show(ui, |ui| {
            for &(i, _) in listed.iter().filter(|(_, hidden)| !hidden) {
                if let Some(a) = self.alert_card(ui, i, false) {
                    action = Some(a);
                }
            }
            let hidden: Vec<usize> = listed.iter().filter(|(_, h)| *h).map(|(i, _)| *i).collect();
            if !hidden.is_empty() {
                ui.add_space(6.0);
                egui::Sides::new().show(
                    ui,
                    |ui| {
                        ui.label(
                            RichText::new(format!("HIDDEN ({})", hidden.len()))
                                .color(t.muted)
                                .small(),
                        )
                    },
                    |ui| {
                        if ui.small_button("show all").clicked() {
                            action = Some((CardAction::ShowAll, String::new()));
                        }
                    },
                );
                for i in hidden {
                    if let Some(a) = self.alert_card(ui, i, true) {
                        action = Some(a);
                    }
                }
            }
        });
        match action {
            Some((CardAction::Focus, id)) => {
                focus = self
                    .alerts
                    .iter()
                    .find(|(a, _)| a.id == id)
                    .map(|(a, _)| a.polygons.clone());
            }
            Some((CardAction::Remove, id)) => self.clear_simulated(Some(&id)),
            Some((CardAction::Hide, id)) => {
                self.hidden.insert(id);
            }
            Some((CardAction::Show, id)) => {
                self.hidden.remove(&id);
            }
            Some((CardAction::ShowAll, _)) => self.hidden.clear(),
            None => {}
        }
        if let Some(polys) = focus {
            let pts: Vec<egui::Pos2> = polys
                .iter()
                .flatten()
                .map(|&(x, y)| self.map.proj.fwd(x, y))
                .collect();
            if !pts.is_empty() {
                let r = Rect::from_points(&pts);
                self.center = r.center().to_vec2();
                self.zoom = self.zoom.max(3.0);
            }
        }
    }

    /// One alert card. Clicking the card zooms to it; real warnings can be hidden
    /// (never deleted), simulated ones removed.
    fn alert_card(&self, ui: &mut Ui, i: usize, hidden: bool) -> Option<(CardAction, String)> {
        let t = &self.cfg.theme;
        let (al, r) = &self.alerts[i];
        let rule_col = self.cfg.alerts.rules[*r].color.to_opaque();
        let col = if hidden {
            rule_col.gamma_multiply(0.5)
        } else {
            rule_col
        };
        let simulated = al.id.starts_with("simulated-");
        let mut out = None;
        // card fills the panel minus its own margin + border, so it can't push the panel wider
        let card_w = ui.available_width() - 22.0;
        // The card's click is registered first, so the buttons inside it still get their clicks.
        let card = ui.scope_builder(egui::UiBuilder::new().id_salt(("alert", &al.id)).sense(Sense::click()), |ui| {
            egui::Frame::new()
                .fill(if hidden { t.bg } else { t.panel })
                .stroke(Stroke::new(1.0, t.line))
                .corner_radius(8)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(card_w);
                    // title row (wraps on narrow panels)
                    ui.horizontal_wrapped(|ui| {
                        let (b, _) = ui.allocate_exact_size(vec2(4.0, 16.0), Sense::hover());
                        ui.painter().rect_filled(b, 2, col);
                        ui.label(RichText::new(&al.event).color(col).strong());
                        if let Some(tag) = al.tag() {
                            ui.label(RichText::new(tag).small().background_color(col.gamma_multiply(0.25)).color(col));
                        }
                        if simulated {
                            ui.label(RichText::new("TEST").small().background_color(t.line).color(t.fg));
                        }
                        if al.near && self.cfg.alerts.radius_km > 0.0 {
                            ui.label(RichText::new(format!("near {}", self.site)).small().color(t.muted))
                                .on_hover_text(format!("within {:.0} km of the radar: this one plays a sound", self.cfg.alerts.radius_km));
                        }
                    });
                    if !hidden {
                        ui.label(RichText::new(&al.area).small());
                        let mut extra = String::new();
                        if let Some(h) = al.hail {
                            extra += &format!("hail {h:.2}\"  ");
                        }
                        if let Some(e) = al.expires.get(11..16) {
                            extra += &format!("until {e} local");
                        }
                        if !extra.is_empty() {
                            ui.label(RichText::new(extra).color(t.muted).small());
                        }
                    }
                    // actions row: hide / show, and remove for test warnings
                    ui.horizontal(|ui| {
                        let id = al.id.clone();
                        if hidden {
                            if ui.small_button("show").on_hover_text("put its polygon back on the map").clicked() {
                                out = Some((CardAction::Show, id.clone()));
                            }
                        } else if ui
                            .small_button("hide")
                            .on_hover_text("hide its polygon from the map (it stays in the list under HIDDEN, and you can show it again)")
                            .clicked()
                        {
                            out = Some((CardAction::Hide, id.clone()));
                        }
                        if simulated && ui.small_button("remove").on_hover_text("delete this test warning").clicked() {
                            out = Some((CardAction::Remove, id));
                        }
                    });
                });
        });
        if out.is_none() && card.response.on_hover_text(&al.headline).clicked() {
            out = Some((CardAction::Focus, al.id.clone()));
        }
        ui.add_space(4.0);
        out
    }

    pub(super) fn radio_panel(&mut self, ui: &mut Ui, now: f64) {
        let t = self.cfg.theme.clone();
        // Refresh the stream list when the panel is open (it changes as feeds come and go).
        if now - self.radio_requested > 600.0 {
            self.radio_requested = now;
            self.net.radio_list();
        }
        heading(ui, "NOAA WEATHER RADIO", t.muted);

        if let Some(call) = self.playing.clone() {
            let st = self.stations.iter().find(|s| s.call == call).cloned();
            let card_w = ui.available_width() - 22.0;
            egui::Frame::new()
                .fill(t.panel)
                .stroke(Stroke::new(1.0, t.line))
                .corner_radius(8)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(card_w);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("● {call}")).color(LIVE_RED).strong());
                        if let Some(s) = &st {
                            ui.label(format!("{}, {}", s.site, s.state));
                        }
                    });
                    if let Some(s) = &st {
                        ui.label(
                            RichText::new(format!("{} MHz · {}", s.freq, self.radio.status()))
                                .color(t.muted)
                                .small(),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button("■ stop").clicked() {
                            self.stop_radio();
                        }
                        let mut v = self.cfg.radio.volume;
                        if ui
                            .add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false))
                            .changed()
                        {
                            self.cfg.radio.volume = v;
                            self.radio.set_volume(v);
                        }
                    });
                });
            ui.add_space(6.0);
        }

        if let Some(e) = &self.radio_error {
            ui.colored_label(ERR, e);
        }
        if self.stations.is_empty() && self.radio_error.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("loading stations…").color(t.muted));
            });
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.radio_search).hint_text("search all stations…"),
        );
        let q = self.radio_search.to_lowercase();
        let mut play = None;
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 70.0)
            .show(ui, |ui| {
                let list = self.stations_by_distance();
                let shown = list.iter().filter(|(d, s)| {
                    if q.is_empty() {
                        *d < 600.0
                    } else {
                        s.call.to_lowercase().contains(&q)
                            || s.site.to_lowercase().contains(&q)
                            || s.state.to_lowercase() == q
                    }
                });
                let mut any = false;
                for (d, s) in shown.take(40) {
                    any = true;
                    let on = self.playing.as_deref() == Some(s.call.as_str());
                    let lbl = format!("{:<7} {}, {}", s.call, s.site, s.state);
                    let (resp, _) = egui::Sides::new().show(
                        ui,
                        |ui| ui.selectable_label(on, lbl),
                        |ui| ui.label(RichText::new(format!("{d:.0} km")).color(t.muted).small()),
                    );
                    if resp.on_hover_text(format!("{} MHz", s.freq)).clicked() {
                        play = Some((*s).clone());
                    }
                }
                if !any && !self.stations.is_empty() {
                    ui.label(
                        RichText::new("no streams near this radar. search by state, e.g. ok")
                            .color(t.muted),
                    );
                }
            });
        if let Some(s) = play {
            if self.playing.as_deref() == Some(s.call.as_str()) {
                self.stop_radio();
            } else {
                self.play_radio(ui.ctx(), &s);
            }
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new("streams by wxradio.org volunteers. They can lag 30 s or more: use a real NWR receiver for warnings.")
                .color(t.muted)
                .small(),
        );
    }

    /// Errors float over the bottom-right of the map.
    pub(super) fn error_toast(&self, ui: &Ui, area: Rect) {
        if !self.cfg.ui.toasts && self.cfg_error.is_none() {
            return; // config errors always show: otherwise a typo would fail silently
        }
        let Some(e) = self.cfg_error.as_ref().or(self.error.as_ref()) else {
            return;
        };
        let t = &self.cfg.theme;
        let font = FontId::monospace(t.font_size * 0.9);
        let galley = ui
            .painter()
            .layout(e.clone(), font, ERR, (area.width() * 0.6).max(200.0));
        let r = Rect::from_min_size(
            area.right_bottom() - galley.size() - vec2(28.0, 44.0),
            galley.size(),
        )
        .expand(8.0);
        let p = ui.painter();
        p.rect_filled(r, 8, t.panel);
        p.rect_stroke(
            r,
            8,
            Stroke::new(1.0, Color32::from_rgb(160, 60, 70)),
            StrokeKind::Inside,
        );
        p.galley(r.min + vec2(8.0, 8.0), galley, t.fg);
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, PointerButton, Pos2, RawInput, Rect, Sense, UiBuilder, vec2};

    #[derive(Clone, Copy, Debug)]
    enum Card {
        /// what shipped: Frame + Sides, then the whole frame made clickable afterwards
        Old,
        /// the fix: click sense registered first via UiBuilder, then Frame + Sides
        New,
    }

    /// Render a card like the alerts panel's, click its button and report (button clicked, card clicked).
    fn click_x(kind: Card) -> (bool, bool) {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(360.0, 300.0));
        let mut x_rect = Rect::NOTHING;
        let mut result = (false, false);
        for frame in 0..6 {
            let at = x_rect.center();
            let mut input = RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            match frame {
                3 => input.events.push(Event::PointerMoved(at)),
                4 => input.events.push(Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }),
                5 => input.events.push(Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }),
                _ => {}
            }
            let _ = ctx.run_ui(input, |ui| {
                let mut x = false;
                let body = |ui: &mut egui::Ui, x: &mut bool, x_rect: &mut Rect| {
                    egui::Frame::new().inner_margin(10).show(ui, |ui| {
                        ui.set_width(300.0);
                        ui.horizontal_wrapped(|ui| ui.label("Tornado Warning"));
                        ui.label("Simulated County (test)");
                        ui.horizontal(|ui| {
                            let r = ui.small_button("remove");
                            *x_rect = r.rect;
                            *x |= r.clicked();
                        });
                    })
                };
                let card = match kind {
                    Card::Old => body(ui, &mut x, &mut x_rect)
                        .response
                        .interact(Sense::click()),
                    Card::New => {
                        ui.scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
                            body(ui, &mut x, &mut x_rect)
                        })
                        .response
                    }
                };
                result.0 |= x;
                result.1 |= card.clicked();
            });
        }
        result
    }

    #[test]
    fn the_x_on_an_alert_card_gets_the_click() {
        let old = click_x(Card::Old);
        let new = click_x(Card::New);
        eprintln!("old {old:?}  new {new:?}");
        assert_eq!(
            new,
            (true, false),
            "the button must receive the click, not the card"
        );
    }
}
