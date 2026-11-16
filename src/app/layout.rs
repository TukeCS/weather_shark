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

pub fn rects(area: Rect, n: usize, l: &Layout) -> Vec<Rect> {
    let n = n.max(1);
    let g = l.gap;
    match l.mode {
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
                let (a, b) = split(
                    rest,
                    vertical,
                    if flip { 1.0 - l.ratio } else { l.ratio },
                    g,
                );
                let (keep, next) = if flip { (b, a) } else { (a, b) };
                out.push(keep);
                rest = next;
            }
            out
        }
        LayoutMode::Columns => {
            let w = (area.width() - g * (n as f32 - 1.0)) / n as f32;
            (0..n)
                .map(|i| {
                    Rect::from_min_size(
                        area.min + vec2(i as f32 * (w + g), 0.0),
                        vec2(w, area.height()),
                    )
                })
                .collect()
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
    }
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
