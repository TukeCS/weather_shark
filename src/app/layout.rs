//! Pane tiling. "spiral" is the fibonacci layout from dwm/trwm: each new pane
//! takes half of the previous one, turning clockwise around the screen.

use egui::{Rect, pos2, vec2};

use crate::config::{Layout, LayoutMode};

/// Split `r` along x (or y) at `ratio`, leaving `gap` between the halves.
fn split(r: Rect, vertical_cut: bool, ratio: f32, gap: f32) -> (Rect, Rect) {
    if vertical_cut {
        let x = r.min.x + (r.width() - gap) * ratio;
        (
            Rect::from_min_max(r.min, pos2(x, r.max.y)),
            Rect::from_min_max(pos2(x + gap, r.min.y), r.max),
        )
    } else {
        let y = r.min.y + (r.height() - gap) * ratio;
        (
            Rect::from_min_max(r.min, pos2(r.max.x, y)),
            Rect::from_min_max(pos2(r.min.x, y + gap), r.max),
        )
    }
}

/// A border between panes you can drag: split number `idx`, which cut `parent`.
pub struct Divider {
    pub idx: usize,
    pub parent: Rect,
    /// a vertical line (drag sideways) or a horizontal one (drag up and down)
    pub vertical: bool,
    /// the stored ratio counts from the far side of `parent`
    pub flip: bool,
    /// where to grab it
    pub strip: Rect,
}

impl Divider {
    /// The ratio this split gets with the pointer at `p`.
    pub fn ratio_at(&self, p: egui::Pos2, gap: f32) -> f32 {
        let f = if self.vertical {
            (p.x - self.parent.min.x - gap / 2.0) / (self.parent.width() - gap)
        } else {
            (p.y - self.parent.min.y - gap / 2.0) / (self.parent.height() - gap)
        };
        let f = if self.flip { 1.0 - f } else { f };
        f.clamp(0.1, 0.9)
    }
}

#[cfg(test)]
pub fn rects(area: Rect, n: usize, l: &Layout) -> Vec<Rect> {
    tile(area, n, l, &[]).0
}

/// Pane rectangles and the dividers between them. `ratios[i]` (when set) replaces
/// the ratio of split i, from dragging a divider.
pub fn tile(area: Rect, n: usize, l: &Layout, ratios: &[f32]) -> (Vec<Rect>, Vec<Divider>) {
    let n = n.max(1);
    let g = l.gap;
    let mut dividers = Vec::new();
    let mut divider = |idx, parent: Rect, vertical, flip, a: Rect| {
        // the gap between the panes, at least 8 px wide so it's easy to grab
        let w = g.max(8.0);
        let strip = if vertical {
            Rect::from_center_size(
                egui::pos2(a.max.x + g / 2.0, parent.center().y),
                vec2(w, parent.height()),
            )
        } else {
            Rect::from_center_size(
                egui::pos2(parent.center().x, a.max.y + g / 2.0),
                vec2(parent.width(), w),
            )
        };
        dividers.push(Divider {
            idx,
            parent,
            vertical,
            flip,
            strip,
        });
    };
    let rects = match l.mode {
        LayoutMode::Spiral | LayoutMode::Dwindle => {
            let mut out = Vec::with_capacity(n);
            let mut rest = area;
            for i in 0..n {
                if i == n - 1 {
                    out.push(rest);
                    break;
                }
                // Cut across the longer side (alternating on squares), so tall windows work too.
                let vertical = if (rest.width() - rest.height()).abs() < 1.0 {
                    i % 2 == 0
                } else {
                    rest.width() > rest.height()
                };
                // spiral: every 3rd and 4th split puts the older pane on the far side,
                // so new panes curl inward instead of marching to one corner
                let flip = l.mode == LayoutMode::Spiral && i % 4 >= 2;
                let ratio = ratios.get(i).copied().filter(|r| r.is_finite()).unwrap_or(l.ratio);
                let (a, b) = split(rest, vertical, if flip { 1.0 - ratio } else { ratio }, g);
                divider(i, rest, vertical, flip, a);
                let (keep, next) = if flip { (b, a) } else { (a, b) };
                out.push(keep);
                rest = next;
            }
            out
        }
        LayoutMode::Columns => {
            // each column takes its share of what's left: equal widths unless dragged
            let mut out = Vec::with_capacity(n);
            let mut rest = area;
            for i in 0..n {
                if i == n - 1 {
                    out.push(rest);
                    break;
                }
                // equal: this column's width out of what's left after the gap
                let k = (n - i) as f32;
                let equal = (rest.width() - (k - 1.0) * g) / k / (rest.width() - g).max(1.0);
                let ratio = ratios.get(i).copied().filter(|r| r.is_finite()).unwrap_or(equal);
                let (a, b) = split(rest, true, ratio, g);
                divider(i, rest, true, false, a);
                out.push(a);
                rest = b;
            }
            out
        }
        LayoutMode::Grid => {
            let cols = (n as f32).sqrt().ceil() as usize;
            let rows = n.div_ceil(cols);
            let w = (area.width() - g * (cols as f32 - 1.0)) / cols as f32;
            let h = (area.height() - g * (rows as f32 - 1.0)) / rows as f32;
            (0..n)
                .map(|i| {
                    let (c, r) = (i % cols, i / cols);
                    Rect::from_min_size(
                        area.min + vec2(c as f32 * (w + g), r as f32 * (h + g)),
                        vec2(w, h),
                    )
                })
                .collect()
        }
    };
    (rects, dividers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay(mode: LayoutMode) -> Layout {
        Layout {
            mode,
            ratio: 0.5,
            gap: 0.0,
        }
    }

    #[test]
    fn spiral_halves_each_time() {
        let a = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 800.0));
        let r = rects(a, 1, &lay(LayoutMode::Spiral));
        assert_eq!(r, vec![a]);
        let r = rects(a, 2, &lay(LayoutMode::Spiral));
        assert_eq!(r[0], Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 800.0)));
        assert_eq!(
            r[1],
            Rect::from_min_max(pos2(800.0, 0.0), pos2(1600.0, 800.0))
        );
        let r = rects(a, 4, &lay(LayoutMode::Spiral));
        // 3rd pane: bottom-right quarter's right half (turning clockwise), 4th: what's left
        assert_eq!(
            r[1],
            Rect::from_min_max(pos2(800.0, 0.0), pos2(1600.0, 400.0))
        );
        assert_eq!(
            r[2],
            Rect::from_min_max(pos2(1200.0, 400.0), pos2(1600.0, 800.0))
        );
        assert_eq!(
            r[3],
            Rect::from_min_max(pos2(800.0, 400.0), pos2(1200.0, 800.0))
        );
        let total: f32 = r.iter().map(|x| x.area()).sum();
        assert!((total - a.area()).abs() < 1.0);
    }

    #[test]
    fn dragging_a_divider_moves_the_split() {
        let a = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 800.0));
        let l = Layout {
            mode: LayoutMode::Spiral,
            ratio: 0.5,
            gap: 8.0,
        };
        let (r, d) = tile(a, 2, &l, &[]);
        assert_eq!(d.len(), 1);
        assert!(d[0].vertical && d[0].strip.contains(pos2(800.0, 400.0)));
        // drag it to x = 400: the first pane gets a quarter
        let ratio = d[0].ratio_at(pos2(400.0, 300.0), l.gap);
        let (r2, _) = tile(a, 2, &l, &[ratio]);
        assert!((r2[0].max.x - 396.0).abs() < 2.0, "{:?}", r2[0]);
        assert!(r2[1].width() > r[1].width());
        // the 3rd split in a spiral is flipped: dragging still follows the pointer
        let (_, d) = tile(a, 4, &l, &[]);
        let f = &d[2];
        assert!(f.flip);
        let p = f.parent.lerp_inside(vec2(0.3, 0.3));
        let ratio = f.ratio_at(p, l.gap);
        let (_, d2) = tile(a, 4, &l, &[0.5, 0.5, ratio]);
        let s = d2[2].strip.center();
        let target = if f.vertical { p.x } else { p.y };
        let got = if f.vertical { s.x } else { s.y };
        assert!((got - target).abs() < 2.0, "{got} vs {target}");
        // columns: equal by default, n - 1 dividers
        let (r, d) = tile(a, 3, &Layout { mode: LayoutMode::Columns, ..l }, &[]);
        assert_eq!(d.len(), 2);
        assert!((r[0].width() - r[2].width()).abs() < 1.0);
    }

    #[test]
    fn grid_and_columns_cover_area() {
        let a = Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 900.0));
        for mode in [LayoutMode::Grid, LayoutMode::Columns, LayoutMode::Dwindle] {
            for n in 1..=6 {
                let r = rects(a, n, &lay(mode));
                assert_eq!(r.len(), n);
                let total: f32 = r.iter().map(|x| x.area()).sum();
                assert!(total <= a.area() + 1.0, "{mode:?} {n}");
            }
        }
    }
}
