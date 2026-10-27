//! Magnet: long straight edges on a screenshot (window frames, panels, cards, table
//! lines) that shapes snap to, and the box they form around a point.

use crate::geom::{Point, Rect};

/// An axis-aligned edge in points: at `pos` (y for a horizontal edge, x for a vertical
/// one), running `from..to` along the other axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub pos: f32,
    pub from: f32,
    pub to: f32,
}

impl Edge {
    fn length(self) -> f32 {
        self.to - self.from
    }

    /// The share of `from..to` this edge runs along, 0..=1.
    fn coverage(self, from: f32, to: f32) -> f32 {
        ((self.to.min(to) - self.from.max(from)) / (to - from)).max(0.0)
    }
}

/// Luminance step between neighbouring pixels that makes an edge (of 255).
const CONTRAST: u8 = 24;
/// Shortest edge kept, in points: longer than glyphs and icons.
const MIN_LENGTH: f32 = 20.0;
/// Pixels an edge may skip (anti-aliasing, dotted borders).
const GAP: u32 = 0;
/// Parallel edges this close are one, e.g. both sides of a 1px line (points).
const MERGE: f32 = 2.5;
/// Collinear edges with a gap this short are one, e.g. a border behind a button.
const MERGE_GAP: f32 = 8.0;
/// Smallest box `box_at` reports, in points.
const MIN_BOX: f32 = 24.0;
/// Each side of a box runs along at least this share of it.
const COVERAGE: f32 = 0.8;
/// Nearest edges per side tried by `box_at`.
const CANDIDATES: usize = 8;

/// Where the pointer lands with the magnet on, and the edges it took its x and y from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapped {
    pub point: Point,
    pub x: Option<Edge>,
    pub y: Option<Edge>,
}

impl Snapped {
    /// `p` as it is, held by no edge.
    pub fn free(p: Point) -> Snapped {
        Snapped {
            point: p,
            x: None,
            y: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SnapMap {
    size: (f32, f32),
    horizontal: Vec<Edge>,
    vertical: Vec<Edge>,
}

impl SnapMap {
    /// Finds edges in `data`: `width`x`height` pixels, 4 bytes each, RGBA or BGRA.
    /// `scale` is pixels per point; the map is in points.
    pub fn detect(data: &[u8], width: u32, height: u32, scale: f32) -> SnapMap {
        let (w, h) = (width as usize, height as usize);
        let size = (width as f32 / scale, height as f32 / scale);
        if w < 2 || h < 2 || data.len() < w * h * 4 {
            return SnapMap {
                size,
                ..SnapMap::default()
            };
        }
        // Symmetric in red and blue, so the byte order doesn't matter.
        let luma: Vec<u8> = data.as_chunks::<4>().0[..w * h]
            .iter()
            .map(|p| ((p[0] as u16 + 2 * p[1] as u16 + p[2] as u16) / 4) as u8)
            .collect();
        let min = (MIN_LENGTH * scale) as u32;
        let keep = |out: &mut Vec<Edge>, pos: usize, (start, last): (u32, u32)| {
            if last + 1 - start >= min {
                out.push(Edge {
                    pos: pos as f32 / scale,
                    from: start as f32 / scale,
                    to: (last + 1) as f32 / scale,
                });
            }
        };

        let (mut horizontal, mut vertical) = (Vec::new(), Vec::new());
        // Vertical runs grow down each column while the rows go by.
        let mut columns: Vec<Option<(u32, u32)>> = vec![None; w];
        for y in 0..h {
            let row = &luma[y * w..(y + 1) * w];
            if y > 0 {
                let above = &luma[(y - 1) * w..y * w];
                let mut run = None;
                for x in 0..w {
                    if above[x].abs_diff(row[x]) >= CONTRAST
                        && let Some(done) = step(&mut run, x as u32)
                    {
                        keep(&mut horizontal, y, done);
                    }
                }
                if let Some(done) = run {
                    keep(&mut horizontal, y, done);
                }
            }
            for x in 1..w {
                if row[x - 1].abs_diff(row[x]) >= CONTRAST
                    && let Some(done) = step(&mut columns[x], y as u32)
                {
                    keep(&mut vertical, x, done);
                }
            }
        }
        for (x, run) in columns.into_iter().enumerate() {
            if let Some(done) = run {
                keep(&mut vertical, x, done);
            }
        }

        let mut horizontal = merge(horizontal);
        let mut vertical = merge(vertical);
        // The screen's own sides frame maximized windows.
        for pos in [0.0, size.1] {
            horizontal.push(Edge {
                pos,
                from: 0.0,
                to: size.0,
            });
        }
        for pos in [0.0, size.0] {
            vertical.push(Edge {
                pos,
                from: 0.0,
                to: size.1,
            });
        }
        SnapMap {
            size,
            horizontal,
            vertical,
        }
    }

    /// Every edge found: horizontal, then vertical.
    pub fn edges(&self) -> (&[Edge], &[Edge]) {
        (&self.horizontal, &self.vertical)
    }

    /// Moves `p` onto the nearest vertical and horizontal edges within `radius`.
    pub fn snap(&self, p: Point, radius: f32) -> Snapped {
        let x = nearest(&self.vertical, p.x, p.y, radius);
        let y = nearest(&self.horizontal, p.y, p.x, radius);
        Snapped {
            point: Point::new(x.map_or(p.x, |e| e.pos), y.map_or(p.y, |e| e.pos)),
            x,
            y,
        }
    }

    /// The smallest box of edges around `p`: a window, a panel, a card. None when
    /// there is only the screen or a band along three of its sides.
    pub fn box_at(&self, p: Point) -> Option<Rect> {
        let left = around(&self.vertical, p.x, p.y, true);
        let right = around(&self.vertical, p.x, p.y, false);
        let top = around(&self.horizontal, p.y, p.x, true);
        let bottom = around(&self.horizontal, p.y, p.x, false);
        let mut best: Option<Rect> = None;
        for l in &left {
            for r in &right {
                let width = r.pos - l.pos;
                if width < MIN_BOX {
                    continue;
                }
                for t in &top {
                    for b in &bottom {
                        let height = b.pos - t.pos;
                        if height < MIN_BOX
                            || best.is_some_and(|best| width * height >= best.width * best.height)
                        {
                            continue;
                        }
                        let closed = t.coverage(l.pos, r.pos) >= COVERAGE
                            && b.coverage(l.pos, r.pos) >= COVERAGE
                            && l.coverage(t.pos, b.pos) >= COVERAGE
                            && r.coverage(t.pos, b.pos) >= COVERAGE;
                        if closed {
                            best = Some(Rect::new(l.pos, t.pos, width, height));
                        }
                    }
                }
            }
        }
        // A band along the screen's sides is just what's left of it, not a box.
        best.filter(|b| {
            let (w, h) = self.size;
            let sides = [b.x <= 0.0, b.y <= 0.0, b.right() >= w, b.bottom() >= h];
            sides.into_iter().filter(|&s| s).count() < 3
        })
    }
}

/// Extends the run with a hit at `i` or starts a new one; returns the run that ended.
fn step(run: &mut Option<(u32, u32)>, i: u32) -> Option<(u32, u32)> {
    match run {
        Some((_, last)) if i - *last <= GAP + 1 => {
            *last = i;
            None
        }
        _ => run.replace((i, i)),
    }
}

/// Joins parallel edges that lie on one line, position weighted by length.
fn merge(mut edges: Vec<Edge>) -> Vec<Edge> {
    edges.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    let mut out: Vec<(Edge, f32)> = Vec::new();
    'next: for e in edges {
        for (m, weight) in out.iter_mut().rev() {
            if e.pos - m.pos > MERGE {
                break;
            }
            if e.from <= m.to + MERGE_GAP && m.from <= e.to + MERGE_GAP {
                let w = e.length();
                m.pos = (m.pos * *weight + e.pos * w) / (*weight + w);
                m.from = m.from.min(e.from);
                m.to = m.to.max(e.to);
                *weight += w;
                continue 'next;
            }
        }
        out.push((e, e.length()));
    }
    out.into_iter().map(|(e, _)| e).collect()
}

fn nearest(edges: &[Edge], pos: f32, along: f32, radius: f32) -> Option<Edge> {
    edges
        .iter()
        .filter(|e| (e.pos - pos).abs() <= radius)
        .filter(|e| e.from - radius <= along && along <= e.to + radius)
        .min_by(|a, b| (a.pos - pos).abs().total_cmp(&(b.pos - pos).abs()))
        .copied()
}

/// The nearest edges crossing the line through `along`, before or after `pos`.
fn around(edges: &[Edge], pos: f32, along: f32, before: bool) -> Vec<Edge> {
    let mut found: Vec<Edge> = edges
        .iter()
        .filter(|e| e.from <= along && along <= e.to)
        .filter(|e| if before { e.pos <= pos } else { e.pos >= pos })
        .copied()
        .collect();
    found.sort_by(|a, b| (a.pos - pos).abs().total_cmp(&(b.pos - pos).abs()));
    found.truncate(CANDIDATES);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A light 400x300 screen with a framed window, a grey panel and some "text".
    fn screen(scale: u32) -> (Vec<u8>, u32, u32) {
        let (w, h) = (400 * scale, 300 * scale);
        let mut data = vec![235u8; (w * h * 4) as usize];
        let mut fill = |x0: u32, y0: u32, x1: u32, y1: u32, v: u8| {
            for y in y0 * scale..y1 * scale {
                for x in x0 * scale..x1 * scale {
                    let i = ((y * w + x) * 4) as usize;
                    data[i..i + 3].fill(v);
                }
            }
        };
        // A window with a 1px dark frame.
        fill(50, 40, 250, 41, 40);
        fill(50, 199, 250, 200, 40);
        fill(50, 40, 51, 200, 40);
        fill(249, 40, 250, 200, 40);
        // Words: short dark bars inside it.
        for i in 0..8 {
            fill(60 + i * 20, 100, 72 + i * 20, 108, 30);
        }
        // A grey panel without a frame.
        fill(300, 50, 380, 250, 150);
        (data, w, h)
    }

    #[test]
    fn finds_the_box_around_a_point() {
        let (data, w, h) = screen(1);
        let map = SnapMap::detect(&data, w, h, 1.0);
        let b = map.box_at(Point::new(120.0, 150.0)).unwrap();
        assert!(
            (b.x - 50.5).abs() < 1.0 && (b.y - 40.5).abs() < 1.0,
            "{b:?}"
        );
        assert!((b.width - 199.0).abs() < 1.5 && (b.height - 159.0).abs() < 1.5);
        let panel = map.box_at(Point::new(340.0, 100.0)).unwrap();
        assert_eq!(
            (panel.x, panel.y, panel.width, panel.height),
            (300.0, 50.0, 80.0, 200.0)
        );
        // Outside every box there's only the screen.
        assert_eq!(map.box_at(Point::new(20.0, 280.0)), None);
        // Below the window: a band from it to the screen's bottom isn't a box.
        assert_eq!(map.box_at(Point::new(150.0, 260.0)), None);
    }

    #[test]
    fn text_is_not_an_edge() {
        let (data, w, h) = screen(1);
        let map = SnapMap::detect(&data, w, h, 1.0);
        let s = map.snap(Point::new(150.0, 103.0), 6.0);
        assert_eq!(s.y, None);
    }

    #[test]
    fn snaps_to_nearby_edges_only() {
        let (data, w, h) = screen(1);
        let map = SnapMap::detect(&data, w, h, 1.0);
        let s = map.snap(Point::new(296.0, 120.0), 6.0);
        assert_eq!(s.point, Point::new(300.0, 120.0));
        assert!(s.x.is_some() && s.y.is_none());
        let far = map.snap(Point::new(280.0, 120.0), 6.0);
        assert_eq!(far.point, Point::new(280.0, 120.0));
        // Beyond the panel's end the edge doesn't pull.
        assert_eq!(map.snap(Point::new(298.0, 280.0), 6.0).x, None);
    }

    #[test]
    fn works_in_points_on_hidpi() {
        let (data, w, h) = screen(2);
        let map = SnapMap::detect(&data, w, h, 2.0);
        let panel = map.box_at(Point::new(340.0, 100.0)).unwrap();
        assert_eq!(
            (panel.x, panel.y, panel.width, panel.height),
            (300.0, 50.0, 80.0, 200.0)
        );
    }

    #[test]
    fn empty_image_has_no_edges() {
        let map = SnapMap::detect(&[], 0, 0, 1.0);
        assert_eq!(map.box_at(Point::new(1.0, 1.0)), None);
        assert_eq!(map.snap(Point::new(5.0, 5.0), 6.0).x, None);
    }
}
