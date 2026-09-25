//! The chart layer's model: datasets from the tile, the chart state, and a
//! resolved frame of keyed items.
//!
//! Ideas taken from `facet-fresh-start` (Jon's chart-language branch): marks
//! are separate from the coordinate system they are drawn in, and a line mark
//! splits into series by a partition key. What is new here is that every item
//! carries a key from the data, so two frames can be joined item by item and
//! one chart object can transition into the next (see `anim.rs`).

use lidar_common::{CLASSES, OTHER};

/// The tile's data, aggregated once into small tables.
#[derive(Clone)]
pub struct Data {
    /// (class label, colour, points), in class order.
    pub classes: Vec<(String, [f32; 4], f64)>,
    /// (flight line, seconds since the line entered the tile, points per 0.5 s).
    pub flight: Vec<(i64, f64, f64)>,
    /// (class label, height band in m above 42 m, points).
    pub class_height: Vec<(String, f64, f64)>,
    /// Building cells: (cx, cy, highest point) in 5 m cells.
    pub cells: Vec<(f64, f64, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dataset {
    Classes,
    Flight,
    ClassHeight,
    Cells,
}

impl Dataset {
    pub const ALL: [(Dataset, &'static str, &'static str); 4] = [
        (Dataset::Classes, "classes", "number of points per LiDAR class"),
        (Dataset::Flight, "flight", "points per half second along each of the four flight lines"),
        (Dataset::ClassHeight, "class_height", "points per LiDAR class and 4 m height band"),
        (Dataset::Cells, "cells", "buildings in 5 m cells, with their highest point"),
    ];
    pub fn id(self) -> &'static str {
        Dataset::ALL.iter().find(|d| d.0 == self).unwrap().1
    }
    pub fn from_id(s: &str) -> Option<Dataset> {
        Dataset::ALL.iter().find(|d| d.1 == s).map(|d| d.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Bars,
    Pie,
    Line,
    Heatmap,
    /// Points at their coordinates: a map.
    Map,
}

impl Mark {
    pub fn id(self) -> &'static str {
        match self {
            Mark::Bars => "bars",
            Mark::Pie => "pie",
            Mark::Line => "line",
            Mark::Heatmap => "heatmap",
            Mark::Map => "map",
        }
    }
    /// The dataset a mark shows, and whether it fits a given dataset.
    pub fn fits(self, d: Dataset) -> bool {
        matches!(
            (self, d),
            (Mark::Bars | Mark::Pie, Dataset::Classes)
                | (Mark::Line, Dataset::Flight)
                | (Mark::Heatmap, Dataset::ClassHeight)
                | (Mark::Map, Dataset::Cells)
        )
    }
    pub fn default_for(d: Dataset) -> Mark {
        match d {
            Dataset::Classes => Mark::Bars,
            Dataset::Flight => Mark::Line,
            Dataset::ClassHeight => Mark::Heatmap,
            Dataset::Cells => Mark::Map,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quarter {
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

/// The chart state: what one chart object shows. Commands change it; the
/// frame is derived from it.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub dataset: Dataset,
    pub mark: Mark,
    pub color: Option<[f32; 4]>,
    pub zoom: Option<Quarter>,
    /// A zoom that is not a quarter (from the editor): x and y domains.
    pub range: Option<((f64, f64), (f64, f64))>,
    pub highlight: bool,
    /// An emphasis threshold other than the top 10 % (from the editor).
    pub threshold: Option<f64>,
    pub title: String,
    /// Axis titles set with `set x.axis.title`; `None` is the mark's own.
    pub x_title: Option<String>,
    pub y_title: Option<String>,
    /// `set y.scale.type log`, on bars.
    pub y_log: bool,
    /// How the plot is seen: flat, through a lens, or tilted in 3D.
    pub view: View,
    /// What is selected, and what that does to the rest.
    pub selection: Selection,
    pub effect: Effect,
    /// Smooth brushing (Doleisch & Hauser 2002): interest falls off over
    /// this width (unit square) outside a brush, instead of yes or no.
    pub soft: Option<f64>,
    /// A lens: a local pipeline over what lies under a circle.
    pub lens: Option<Lens>,
}

/// A lens as a transform plus a local pipeline (Bier et al. 1993): the items
/// under a circle in the plot's unit square go through a step of their own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lens {
    /// Every item inside kept with probability `keep`, by a hash of its key,
    /// so overplotting clears (Ellis, Bertini & Dix 2005).
    Sample { focus: [f64; 2], radius: f64, keep: f64 },
    /// A least-squares fit of what is inside, per series (Shao et al. 2017).
    Regression { focus: [f64; 2], radius: f64 },
    /// Inside, the items taller than `above` (0..1 of the height range) are
    /// taken away, to see what they hide (MoleView, Hurter et al. 2011).
    Mole { focus: [f64; 2], radius: f64, above: f64 },
}

impl Lens {
    pub fn focus(&self) -> [f64; 2] {
        match self {
            Lens::Sample { focus, .. } | Lens::Regression { focus, .. } | Lens::Mole { focus, .. } => *focus,
        }
    }
    pub fn radius(&self) -> f64 {
        match self {
            Lens::Sample { radius, .. } | Lens::Regression { radius, .. } | Lens::Mole { radius, .. } => *radius,
        }
    }
    pub fn with_focus(self, f: [f64; 2]) -> Lens {
        match self {
            Lens::Sample { radius, keep, .. } => Lens::Sample { focus: f, radius, keep },
            Lens::Regression { radius, .. } => Lens::Regression { focus: f, radius },
            Lens::Mole { radius, above, .. } => Lens::Mole { focus: f, radius, above },
        }
    }
}

/// What a lens found, drawn over it: a segment in the unit square, its
/// colour, and a line of text.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub color: [f32; 4],
    pub label: String,
}

/// A selection, as data: item keys (a click, a legend entry, a brush over
/// categories), or an interval in data units (a brush over continuous axes).
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Selection {
    #[default]
    None,
    /// Bare keys: a class label, `label|band`, a flight line, `cx,cy`.
    Keys(Vec<String>),
    Interval { x: (f64, f64), y: Option<(f64, f64)> },
    /// A line brush (Konyha et al. 2006; vega-lite#9833): the series whose
    /// line crosses the segment from `a` to `b`, in data units.
    Segment { a: [f64; 2], b: [f64; 2] },
    /// A timebox (Hochheiser & Shneiderman 2004): the series whose every
    /// point within `x` has its value within `y`.
    Timebox { x: (f64, f64), y: (f64, f64) },
    /// A lasso on the map: a polygon on the screen (the plot's unit square,
    /// y up), drawn flat or in the tilted view \`tilt\` (yaw, elevation). With
    /// \`structure\`, CloudLasso (Yu et al. 2012): of the cells whose
    /// projection falls inside, only the largest structure of voxels whose
    /// density reaches \`structure\` times the densest.
    Lasso { poly: Vec<[f64; 2]>, tilt: Option<(f64, f64)>, structure: Option<f64> },
}

/// Whether p lies inside the polygon (even-odd rule).
pub fn in_polygon(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

/// What a lasso takes, of a frame's cells: their keys, how many fell inside
/// the polygon, and (for CloudLasso) how many dense regions there were.
pub fn lasso_take(f: &Frame, poly: &[[f64; 2]], tilt: Option<(f64, f64)>, structure: Option<f64>) -> (std::collections::HashSet<String>, usize, usize) {
    use std::collections::{HashMap, HashSet};
    let dom = |a: &Axis| match a {
        Axis::Linear { lo, hi, .. } => (*lo, *hi),
        _ => (0.0, 1.0),
    };
    let ((x0, x1), (y0, y1)) = (dom(&f.x), dom(&f.y));
    let unit = |x: f64, y: f64| [(x - x0) / (x1 - x0), (y - y0) / (y1 - y0)];
    // Each cell where the screen shows it.
    let inside: Vec<(usize, [f64; 2], f64)> = f
        .items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| match &it.geo {
            Geo::Point { x, y } => {
                let u = unit(*x, *y);
                let q = match tilt {
                    None => u,
                    Some((yaw, el)) => {
                        let (p, _) = super::draw::tilt(u, it.h, yaw, el);
                        [p[0] / super::draw::P, 1.0 - p[1] / super::draw::P]
                    }
                };
                in_polygon(q, poly).then_some((i, u, it.h))
            }
            _ => None,
        })
        .collect();
    let n_in = inside.len();
    let Some(t) = structure else {
        return (inside.iter().map(|(i, _, _)| f.items[*i].key.clone()).collect(), n_in, 0);
    };
    // Voxels: 1/40 of the plot across, 1/10 of the height range up.
    let vox = |u: [f64; 2], h: f64| ((u[0] * 40.0).floor() as i64, (u[1] * 40.0).floor() as i64, (h * 10.0).floor() as i64);
    let mut count: HashMap<(i64, i64, i64), usize> = HashMap::new();
    for (_, u, h) in &inside {
        *count.entry(vox(*u, *h)).or_default() += 1;
    }
    let top = count.values().copied().max().unwrap_or(0);
    let dense: HashSet<(i64, i64, i64)> = count.iter().filter(|(_, c)| **c as f64 >= t * top as f64).map(|(v, _)| *v).collect();
    // Regions: dense voxels joined across faces, edges and corners.
    let mut seen: HashSet<(i64, i64, i64)> = HashSet::new();
    let (mut best, mut best_n, mut regions) = (HashSet::new(), 0, 0);
    for v in &dense {
        if seen.contains(v) {
            continue;
        }
        regions += 1;
        let (mut stack, mut region, mut n) = (vec![*v], HashSet::new(), 0);
        seen.insert(*v);
        while let Some(c) = stack.pop() {
            region.insert(c);
            n += count[&c];
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let q = (c.0 + dx, c.1 + dy, c.2 + dz);
                        if dense.contains(&q) && seen.insert(q) {
                            stack.push(q);
                        }
                    }
                }
            }
        }
        if n > best_n {
            (best, best_n) = (region, n);
        }
    }
    let keys = inside.iter().filter(|(_, u, h)| best.contains(&vox(*u, *h))).map(|(i, _, _)| f.items[*i].key.clone()).collect();
    (keys, n_in, regions)
}

/// Whether segments p1-p2 and q1-q2 cross (or touch).
pub fn crosses(p1: [f64; 2], p2: [f64; 2], q1: [f64; 2], q2: [f64; 2]) -> bool {
    let o = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let (d1, d2, d3, d4) = (o(q1, q2, p1), o(q1, q2, p2), o(p1, p2, q1), o(p1, p2, q2));
    (d1 * d2 <= 0.0) && (d3 * d4 <= 0.0)
}

/// Whether a series (its vertices in data units) meets a series predicate.
pub fn series_selected(sel: &Selection, pts: &[[f64; 2]]) -> bool {
    match sel {
        Selection::Segment { a, b } => pts.windows(2).any(|w| crosses(w[0], w[1], *a, *b)),
        Selection::Timebox { x, y } => {
            let within: Vec<&[f64; 2]> = pts.iter().filter(|p| p[0] >= x.0 && p[0] <= x.1).collect();
            !within.is_empty() && within.iter().all(|p| p[1] >= y.0 && p[1] <= y.1)
        }
        _ => true,
    }
}

/// What a selection does to the unselected items, chosen apart from the
/// selection itself (vega/altair#3394). Filtering keeps the scale domains, so
/// axes and legends keep their meaning.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Effect {
    #[default]
    Fade,
    Filter,
}

/// An item's bare key, as a selection names it: `class:Ground` is
/// `Ground`, `class:Ground|h:8` is `Ground|8`, `line:31` is `31`,
/// `cell:657200,6867300` is `657200,6867300`.
pub fn bare_key(key: &str) -> String {
    let k = key.split_once(':').map_or(key, |(_, v)| v);
    k.replace("|h:", "|")
}

/// A view over the plot's unit square, after the coordinate system (the
/// family of experiment 5): the same items, projected differently.
#[derive(Clone, Copy, Debug)]
pub enum View {
    Flat,
    /// Sarkar–Brown fisheye around `focus` (unit space): the context stays,
    /// distances inside the lens do not.
    Fisheye { focus: [f64; 2], radius: f64, distortion: f64 },
    /// A round magnifier, `zoom` times and undistorted. In place, it sits
    /// over `focus` and covers what lies around it. Offset (DragMag), a
    /// source circle of `radius` sits over `focus` and a callout of `radius
    /// * zoom` beside it, on `side` (one of `CALLOUT_SIDES`), chosen where it
    /// covers the least data. `side` is derived, not part of the view's
    /// identity.
    Magnifier { focus: [f64; 2], radius: f64, zoom: f64, offset: bool, side: usize, anchor: [f64; 2] },
    /// The map in 3D: height as z, seen from `yaw` degrees around and
    /// `elevation` degrees above the horizon.
    Tilt { yaw: f64, elevation: f64 },
}

impl PartialEq for View {
    fn eq(&self, other: &View) -> bool {
        match (self, other) {
            (View::Flat, View::Flat) => true,
            (View::Fisheye { focus: a, radius: b, distortion: c }, View::Fisheye { focus: d, radius: e, distortion: f }) => a == d && b == e && c == f,
            (View::Magnifier { focus: a, radius: b, zoom: c, offset: o, .. }, View::Magnifier { focus: d, radius: e, zoom: f, offset: p, .. }) => a == d && b == e && c == f && o == p,
            (View::Tilt { yaw: a, elevation: b }, View::Tilt { yaw: c, elevation: d }) => a == c && b == d,
            _ => false,
        }
    }
}

/// Where an offset magnifier's callout may go, as directions from its
/// source circle (unit vectors in the plot's unit square, y up): the four
/// diagonals first, then the sides.
pub const CALLOUT_SIDES: [[f64; 2]; 8] = [
    [0.7071, 0.7071],
    [-0.7071, 0.7071],
    [0.7071, -0.7071],
    [-0.7071, -0.7071],
    [1.0, 0.0],
    [-1.0, 0.0],
    [0.0, 1.0],
    [0.0, -1.0],
];

/// The callout of an offset magnifier, placed from `anchor` (the focus when
/// it was last placed): centre and radius in unit space. The source circle
/// shows the same area as the callout, so its radius is the callout's over
/// the magnification; the gap between them is half a callout diameter, as in
/// Shift (Vogel & Baudisch 2007).
pub fn callout(anchor: [f64; 2], radius: f64, zoom: f64, side: usize) -> ([f64; 2], f64) {
    let rc = (radius * zoom).min(0.32);
    let d = CALLOUT_SIDES[side % CALLOUT_SIDES.len()];
    let dist = radius + rc + rc;
    ([anchor[0] + d[0] * dist, anchor[1] + d[1] * dist], rc)
}

/// Place an offset magnifier's callout like a label: on the side where it
/// covers the fewest items and stays in the plot (or in the empty margin
/// right of a map), keeping the current side unless another is clearly
/// better, so it does not jump while the pointer moves.
pub fn place_callout(view: View, items: &[[f64; 2]], margin: bool) -> View {
    let View::Magnifier { focus, radius, zoom, offset: true, side, anchor } = view else { return view };
    // The callout stays where it is until the source has moved about one
    // callout radius (Shift's and the Ring lens's tracking-menu hysteresis).
    let rc = (radius * zoom).min(0.32);
    if anchor[0].is_finite() && (focus[0] - anchor[0]).hypot(focus[1] - anchor[1]) < rc {
        return view;
    }
    // How far a callout on side k reaches out of the plot (or, beside a map
    // with no legend, out of the margin to its right).
    let out = |k: usize| {
        let (c, r) = callout(focus, radius, zoom, k);
        let x_max = if margin { 1.45 } else { 1.0 };
        (0.0f64 - (c[0] - r)).max(0.0) + ((c[0] + r) - x_max).max(0.0) + (0.0f64 - (c[1] - r)).max(0.0) + ((c[1] + r) - 1.0).max(0.0)
    };
    // Staying inside is a constraint, not a cost: only when no side fits
    // does the one that reaches out least win.
    let fits: Vec<usize> = (0..CALLOUT_SIDES.len()).filter(|k| out(*k) < 1e-9).collect();
    let score = |k: usize| {
        let (c, r) = callout(focus, radius, zoom, k);
        let covered = items.iter().filter(|p| (p[0] - c[0]).hypot(p[1] - c[1]) < r).count() as f64;
        // Up and to the right first, for predictability; the order of
        // `CALLOUT_SIDES` breaks ties.
        covered + k as f64 * 0.5 + if fits.is_empty() { out(k) * 1e6 } else { 0.0 }
    };
    let allowed: Vec<usize> = if fits.is_empty() { (0..CALLOUT_SIDES.len()).collect() } else { fits.clone() };
    let best = allowed.iter().copied().min_by(|a, b| score(*a).total_cmp(&score(*b))).unwrap_or(0);
    let keep = anchor[0].is_finite() && allowed.contains(&side) && score(side) <= score(best) * 1.25 + 3.0;
    View::Magnifier { focus, radius, zoom, offset: true, side: if keep { side } else { best }, anchor: focus }
}

impl View {
    pub fn id(&self) -> &'static str {
        match self {
            View::Flat => "flat",
            View::Fisheye { .. } => "fisheye",
            View::Magnifier { .. } => "magnifier",
            View::Tilt { .. } => "tilt",
        }
    }
    pub fn focus(&self) -> Option<[f64; 2]> {
        match self {
            View::Fisheye { focus, .. } | View::Magnifier { focus, .. } => Some(*focus),
            _ => None,
        }
    }
    pub fn with_focus(self, f: [f64; 2]) -> View {
        match self {
            View::Fisheye { radius, distortion, .. } => View::Fisheye { focus: f, radius, distortion },
            View::Magnifier { radius, zoom, offset, side, anchor, .. } => View::Magnifier { focus: f, radius, zoom, offset, side, anchor },
            v => v,
        }
    }
}

impl State {
    pub fn new(dataset: Dataset) -> Self {
        let mut s = State { dataset, mark: Mark::default_for(dataset), color: None, zoom: None, range: None, highlight: false, threshold: None, title: String::new(), x_title: None, y_title: None, y_log: false, view: View::Flat, selection: Selection::None, effect: Effect::Fade, soft: None, lens: None };
        s.title = s.default_title();
        s
    }
    /// A title that follows the mark and data, so a mark change never leaves
    /// a stale title behind.
    pub fn default_title(&self) -> String {
        match (self.mark, self.dataset) {
            (Mark::Bars, _) => "Points per LiDAR class".into(),
            (Mark::Pie, _) => "Share of points per LiDAR class".into(),
            (Mark::Line, _) => "Points per half second along each flight line".into(),
            (Mark::Heatmap, _) => "Points per class and height band".into(),
            (Mark::Map, _) => "Buildings, 5 m cells".into(),
        }
    }
}

// ---------------------------------------------------------------------------

/// Item geometry. Rects are in unit space of their frame; points and line
/// vertices are in data coordinates, so a zoom is a pure domain change.
#[derive(Clone, Debug)]
pub enum Geo {
    Rect { x0: f64, x1: f64, y0: f64, y1: f64 },
    Point { x: f64, y: f64 },
    Line { pts: Vec<[f64; 2]> },
}

#[derive(Clone, Debug)]
pub struct Item {
    pub key: String,
    /// A coarser item this one belongs to: a heatmap cell's class bar.
    pub parent: Option<String>,
    pub geo: Geo,
    pub fill: [f32; 4],
    /// Symbol area for points, stroke width for lines.
    pub size: f64,
    /// Height in unit space, for a tilted view (a map cell's highest point).
    pub h: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Axis {
    None,
    Band { field: String, labels: Vec<String> },
    Linear { field: String, lo: f64, hi: f64 },
    /// Ticks at powers of ten; `lo` and `hi` are the values at the ends.
    Log { field: String, lo: f64, hi: f64 },
}

impl Axis {
    fn retitle(&mut self, t: &str) {
        match self {
            Axis::Band { field, .. } | Axis::Linear { field, .. } | Axis::Log { field, .. } => *field = t.to_string(),
            Axis::None => {}
        }
    }
}

/// Polar frames are drawn through `Bend(1)`, Cartesian ones through `Bend(0)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Coords {
    Cartesian,
    Polar,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub state: State,
    pub coords: Coords,
    pub items: Vec<Item>,
    pub x: Axis,
    pub y: Axis,
    pub legend: Vec<(String, [f32; 4])>,
    /// The item key each legend entry stands for, so an entry can select.
    pub legend_keys: Vec<String>,
    /// For a heatmap: the value range of the colour scale.
    pub colorbar: Option<(f64, f64)>,
    /// Equal aspect for maps.
    pub square: bool,
    pub view: View,
    /// How many items a selection kept, when there is one.
    pub selected: Option<usize>,
    /// The lens, and what it found.
    pub lens: Option<(Lens, Vec<Fit>)>,
    /// A lasso's polygon (screen unit square), the view it was drawn in,
    /// and what it took.
    pub lasso: Option<(Vec<[f64; 2]>, Option<(f64, f64)>, String)>,
}

pub fn class_color(label: &str) -> [f32; 4] {
    CLASSES.iter().find(|c| c.1 == label).map_or(OTHER, |c| c.2)
}

const LINE_COLOURS: [[f32; 4]; 4] = [
    [0.30, 0.47, 0.66, 1.0],
    [0.95, 0.56, 0.17, 1.0],
    [0.35, 0.63, 0.31, 1.0],
    [0.69, 0.48, 0.63, 1.0],
];
pub const HIGHLIGHT: [f32; 4] = [0.95, 0.56, 0.17, 1.0];

/// Viridis, sampled at 6 stops.
pub fn viridis(t: f64) -> [f32; 4] {
    const S: [[f32; 3]; 6] = [
        [0.267, 0.005, 0.329],
        [0.254, 0.265, 0.530],
        [0.164, 0.471, 0.558],
        [0.135, 0.659, 0.518],
        [0.478, 0.821, 0.318],
        [0.993, 0.906, 0.144],
    ];
    let t = t.clamp(0.0, 1.0) * 5.0;
    let i = (t.floor() as usize).min(4);
    let f = (t - i as f64) as f32;
    let (a, b) = (S[i], S[i + 1]);
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f, 1.0]
}

fn percentile(mut v: Vec<f64>, p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// A domain rounded out to a "nice" step.
pub fn nice(lo: f64, hi: f64) -> (f64, f64) {
    let span = (hi - lo).max(1e-9);
    let step = 10f64.powf((span / 5.0).log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * step).find(|s| span / s <= 6.0).unwrap_or(step * 10.0);
    ((lo / step).floor() * step, (hi / step).ceil() * step)
}

fn zoomed(lo: f64, hi: f64, lower: bool) -> (f64, f64) {
    let mid = (lo + hi) / 2.0;
    if lower { (lo, mid) } else { (mid, hi) }
}

pub fn quarter_domains(q: Option<Quarter>, x: (f64, f64), y: (f64, f64)) -> ((f64, f64), (f64, f64)) {
    match q {
        None => (x, y),
        Some(q) => {
            let east = matches!(q, Quarter::NorthEast | Quarter::SouthEast);
            let north = matches!(q, Quarter::NorthEast | Quarter::NorthWest);
            (zoomed(x.0, x.1, !east), zoomed(y.0, y.1, !north))
        }
    }
}

/// The unzoomed x and y domains of a mark with continuous axes.
pub fn base_domains(m: Mark, d: &Data) -> Option<((f64, f64), (f64, f64))> {
    match m {
        Mark::Line => {
            let xmax = d.flight.iter().map(|r| r.1).fold(0.0, f64::max);
            let ymax = nice(0.0, d.flight.iter().map(|r| r.2).fold(0.0, f64::max)).1;
            Some(((0.0, xmax.ceil()), (0.0, ymax)))
        }
        Mark::Map => Some(((657_000.0, 658_000.0), (6_867_000.0, 6_868_000.0))),
        _ => None,
    }
}

/// The emphasis a mark supports: the field and the threshold of its top
/// 10 %.
pub fn emphasis(m: Mark, d: &Data) -> Option<(&'static str, f64)> {
    match m {
        Mark::Bars | Mark::Pie => Some(("n", percentile(d.classes.iter().map(|c| c.2).collect(), 0.9))),
        Mark::Map => Some(("h", percentile(d.cells.iter().map(|c| c.2).collect(), 0.9))),
        _ => None,
    }
}

fn fmt_m(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{v:.0}") } else { format!("{v}") }
}

/// Every item's position in the plot's unit square: rect centres, points,
/// and line vertices, under the frame's domains.
pub fn unit_points(f: &Frame) -> Vec<[f64; 2]> {
    let dom = |a: &Axis| match a {
        Axis::Linear { lo, hi, .. } => Some((*lo, *hi)),
        _ => None,
    };
    let (dx, dy) = (dom(&f.x), dom(&f.y));
    let u = |v: f64, d: Option<(f64, f64)>| d.map_or(v, |(lo, hi)| (v - lo) / (hi - lo));
    let mut out = vec![];
    for it in &f.items {
        match &it.geo {
            Geo::Rect { x0, x1, y0, y1 } => out.push([(x0 + x1) / 2.0, (y0 + y1) / 2.0]),
            Geo::Point { x, y } => out.push([u(*x, dx), u(*y, dy)]),
            Geo::Line { pts } => out.extend(pts.iter().map(|p| [u(p[0], dx), u(p[1], dy)])),
        }
    }
    out
}

/// A key to a number in [0, 1), the same on every run.
fn hash01(key: &str) -> f64 {
    let mut h = key.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    // FNV alone leaves keys that differ in their last bytes close together
    // (cells in a column, as stripes): finish with splitmix64.
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
    h ^= h >> 31;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Least squares of y on x: slope, intercept and r².
fn ols(p: &[[f64; 2]]) -> Option<(f64, f64, f64)> {
    if p.len() < 3 {
        return None;
    }
    let n = p.len() as f64;
    let (mx, my) = (p.iter().map(|q| q[0]).sum::<f64>() / n, p.iter().map(|q| q[1]).sum::<f64>() / n);
    let (sxx, syy, sxy) = p.iter().fold((0.0, 0.0, 0.0), |a, q| (a.0 + (q[0] - mx).powi(2), a.1 + (q[1] - my).powi(2), a.2 + (q[0] - mx) * (q[1] - my)));
    if sxx < 1e-12 {
        return None;
    }
    let slope = sxy / sxx;
    Some((slope, my - slope * mx, if syy < 1e-12 { 1.0 } else { sxy * sxy / (sxx * syy) }))
}

/// A slope with its sign and a k or M: -12345 → −12.3k.
fn signed(v: f64) -> String {
    let (sign, a) = (if v < 0.0 { "−" } else { "+" }, v.abs());
    if a >= 1e6 {
        format!("{sign}{:.1}M", a / 1e6)
    } else if a >= 1e3 {
        format!("{sign}{:.1}k", a / 1e3)
    } else {
        format!("{sign}{a:.2}")
    }
}

/// The unit of an x axis, for "per s".
fn unit_word(a: &Axis) -> &'static str {
    match a {
        Axis::Linear { field, .. } if field.contains("second") => "s",
        Axis::Linear { field, .. } if field.contains("(m)") || field.contains("Lambert") => "m",
        _ => "unit of x",
    }
}

/// Resolve a state into a frame of keyed items.
pub fn resolve(s: &State, d: &Data) -> Frame {
    let mut f = Frame {
        state: s.clone(),
        coords: Coords::Cartesian,
        items: vec![],
        x: Axis::None,
        y: Axis::None,
        legend: vec![],
        colorbar: None,
        square: false,
        view: s.view,
        legend_keys: vec![],
        selected: None,
        lens: None,
        lasso: None,
    };
    match s.mark {
        Mark::Bars | Mark::Pie => {
            let n = d.classes.len() as f64;
            let max = nice(0.0, d.classes.iter().map(|c| c.2).fold(0.0, f64::max)).1;
            let total: f64 = d.classes.iter().map(|c| c.2).sum();
            let p90 = s.threshold.unwrap_or(emphasis(s.mark, d).unwrap().1);
            let mut acc = 0.0;
            // A log scale runs from the power of ten below the smallest bar
            // to the one above the largest.
            let lmin = d.classes.iter().map(|c| c.2).filter(|v| *v > 0.0).fold(f64::INFINITY, f64::min);
            let (llo, lhi) = (10f64.powf(lmin.max(1.0).log10().floor()), 10f64.powf(max.max(1.0).log10().ceil()));
            let unit = |v: f64| if s.y_log { ((v.max(llo).log10() - llo.log10()) / (lhi.log10() - llo.log10())).clamp(0.0, 1.0) } else { v / max };
            for (i, (label, colour, v)) in d.classes.iter().enumerate() {
                let geo = if s.mark == Mark::Bars {
                    Geo::Rect { x0: (i as f64 + 0.12) / n, x1: (i as f64 + 0.88) / n, y0: 0.0, y1: unit(*v) }
                } else {
                    // A stacked bar in the upper part of y: under `Bend(1)`
                    // it is a donut ring.
                    let g = Geo::Rect { x0: acc / total, x1: (acc + v) / total, y0: 0.42, y1: 1.0 };
                    acc += v;
                    g
                };
                let mut fill = s.color.unwrap_or(*colour);
                if s.highlight && *v >= p90 {
                    fill = HIGHLIGHT;
                }
                f.items.push(Item { key: format!("class:{label}"), parent: None, geo, fill, size: 0.0, h: 0.0 });
            }
            if s.mark == Mark::Bars {
                f.x = Axis::Band { field: "class".into(), labels: d.classes.iter().map(|c| c.0.clone()).collect() };
                f.y = if s.y_log { Axis::Log { field: "points (log scale)".into(), lo: llo, hi: lhi } } else { Axis::Linear { field: "points".into(), lo: 0.0, hi: max } };
            } else {
                f.coords = Coords::Polar;
                f.legend = d.classes.iter().map(|c| (format!("{} {:.0} %", c.0, 100.0 * c.2 / total), s.color.unwrap_or(c.1))).collect();
                f.legend_keys = d.classes.iter().map(|c| format!("class:{}", c.0)).collect();
            }
        }
        Mark::Line => {
            let mut lines: Vec<i64> = d.flight.iter().map(|r| r.0).collect();
            lines.sort();
            lines.dedup();
            let (bx, by) = base_domains(Mark::Line, d).unwrap();
            let ((x0, x1), (y0, y1)) = s.range.unwrap_or_else(|| quarter_domains(s.zoom, bx, by));
            for (k, l) in lines.iter().enumerate() {
                let pts: Vec<[f64; 2]> = d.flight.iter().filter(|r| r.0 == *l).map(|r| [r.1, r.2]).collect();
                let fill = s.color.filter(|_| lines.len() == 1).unwrap_or(LINE_COLOURS[k % 4]);
                f.items.push(Item { key: format!("line:{l}"), parent: None, geo: Geo::Line { pts }, fill, size: 2.0, h: 0.0 });
                f.legend.push((format!("flight line {l}"), fill));
                f.legend_keys.push(format!("line:{l}"));
            }
            f.x = Axis::Linear { field: "seconds since the line entered the tile".into(), lo: x0, hi: x1 };
            f.y = Axis::Linear { field: "points per 0.5 s".into(), lo: y0, hi: y1 };
        }
        Mark::Heatmap => {
            let classes: Vec<String> = d.classes.iter().map(|c| c.0.clone()).collect();
            let mut bands: Vec<f64> = d.class_height.iter().map(|r| r.1).collect();
            bands.sort_by(f64::total_cmp);
            bands.dedup();
            let vmax = d.class_height.iter().map(|r| r.2).fold(0.0, f64::max);
            let (nx, ny) = (bands.len() as f64, classes.len() as f64);
            for (label, band, v) in &d.class_height {
                let (Some(i), Some(j)) = (bands.iter().position(|b| b == band), classes.iter().position(|c| c == label)) else { continue };
                // Rows top to bottom in class order.
                let row = ny - 1.0 - j as f64;
                let t = (v.ln_1p() / vmax.ln_1p()).clamp(0.0, 1.0);
                f.items.push(Item {
                    key: format!("class:{label}|h:{band}"),
                    parent: Some(format!("class:{label}")),
                    geo: Geo::Rect { x0: i as f64 / nx, x1: (i as f64 + 1.0) / nx, y0: row / ny, y1: (row + 1.0) / ny },
                    fill: viridis(t),
                    size: 0.0,
                    h: 0.0,
                });
            }
            f.x = Axis::Band { field: "height above 42 m".into(), labels: bands.iter().map(|b| format!("{b:.0} m")).collect() };
            let mut rows = classes.clone();
            rows.reverse();
            f.y = Axis::Band { field: "class".into(), labels: rows };
            f.colorbar = Some((0.0, vmax));
        }
        Mark::Map => {
            let p90 = s.threshold.unwrap_or(emphasis(Mark::Map, d).unwrap().1);
            let (bx, by) = base_domains(Mark::Map, d).unwrap();
            let ((x0, x1), (y0, y1)) = s.range.unwrap_or_else(|| quarter_domains(s.zoom, bx, by));
            let base = s.color.unwrap_or([0.30, 0.47, 0.66, 1.0]);
            // Symbol area follows the zoom and the cell size (the smallest
            // step between cell origins), so the map has no gaps.
            let mut xs: Vec<f64> = d.cells.iter().map(|c| c.0).collect();
            xs.sort_by(f64::total_cmp);
            let cell = xs.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 1e-9).fold(f64::INFINITY, f64::min);
            let cell = if cell.is_finite() { cell } else { 5.0 };
            let k = (1000.0 / (x1 - x0)).powi(2) * (cell / 5.0).powi(2);
            // The default title names the cell size the data has.
            if s.title == s.default_title() {
                f.state.title = format!("Buildings, {} m cells", fmt_m(cell));
            }
            let (hlo, hhi) = d.cells.iter().fold((f64::MAX, f64::MIN), |a, c| (a.0.min(c.2), a.1.max(c.2)));
            for (cx, cy, h) in &d.cells {
                let hot = s.highlight && *h >= p90;
                f.items.push(Item {
                    key: format!("cell:{cx},{cy}"),
                    parent: None,
                    geo: Geo::Point { x: *cx, y: *cy },
                    fill: if hot { HIGHLIGHT } else { base },
                    size: k * if hot { 14.0 } else { 5.0 },
                    h: ((h - hlo) / (hhi - hlo).max(1e-9)).clamp(0.0, 1.0),
                });
            }
            f.x = Axis::Linear { field: "easting (Lambert-93, m)".into(), lo: x0, hi: x1 };
            f.y = Axis::Linear { field: "northing (m)".into(), lo: y0, hi: y1 };
            f.square = true;
        }
    }
    // A selection fades or removes what it leaves out: by a degree of
    // interest in [0, 1], which is yes or no unless the selection is soft.
    if s.selection != Selection::None {
        let dom = |a: &Axis| match a {
            Axis::Linear { lo, hi, .. } => Some((*lo, *hi)),
            _ => None,
        };
        let (dx, dy) = (dom(&f.x), dom(&f.y));
        let unit = |p: [f64; 2]| [dx.map_or(p[0], |(lo, hi)| (p[0] - lo) / (hi - lo)), dy.map_or(p[1], |(lo, hi)| (p[1] - lo) / (hi - lo))];
        let fall = |d: f64| s.soft.map_or(if d <= 0.0 { 1.0 } else { 0.0 }, |w| (1.0 - d / w.max(1e-9)).clamp(0.0, 1.0));
        let degree = |it: &Item| -> f64 {
            match &s.selection {
                Selection::None => 1.0,
                Selection::Keys(keys) => if keys.contains(&bare_key(&it.key)) { 1.0 } else { 0.0 },
                Selection::Lasso { .. } => 1.0,
                Selection::Interval { x, y } => {
                    let (a, b) = (unit([x.0, y.map_or(f64::MIN, |y| y.0)]), unit([x.1, y.map_or(f64::MAX, |y| y.1)]));
                    let dist = |p: [f64; 2]| {
                        let q = unit(p);
                        let ddx = (a[0] - q[0]).max(q[0] - b[0]).max(0.0);
                        let ddy = if y.is_some() { (a[1] - q[1]).max(q[1] - b[1]).max(0.0) } else { 0.0 };
                        ddx.hypot(ddy)
                    };
                    match &it.geo {
                        Geo::Point { x, y } => fall(dist([*x, *y])),
                        Geo::Line { pts } => fall(pts.iter().map(|p| dist(*p)).fold(f64::MAX, f64::min)),
                        Geo::Rect { .. } => 1.0,
                    }
                }
                Selection::Segment { a, b } => match &it.geo {
                    Geo::Line { pts } if series_selected(&s.selection, pts) => 1.0,
                    Geo::Line { pts } if s.soft.is_some() => {
                        let (ua, ub) = (unit(*a), unit(*b));
                        let seg = |p: [f64; 2]| {
                            let (vx, vy) = (ub[0] - ua[0], ub[1] - ua[1]);
                            let t = (((p[0] - ua[0]) * vx + (p[1] - ua[1]) * vy) / (vx * vx + vy * vy).max(1e-12)).clamp(0.0, 1.0);
                            (p[0] - (ua[0] + t * vx)).hypot(p[1] - (ua[1] + t * vy))
                        };
                        fall(pts.iter().map(|p| seg(unit(*p))).fold(f64::MAX, f64::min))
                    }
                    Geo::Line { .. } => 0.0,
                    _ => 1.0,
                },
                Selection::Timebox { x, y } => match &it.geo {
                    Geo::Line { pts } if s.soft.is_some() => {
                        let within: Vec<&[f64; 2]> = pts.iter().filter(|p| p[0] >= x.0 && p[0] <= x.1).collect();
                        if within.is_empty() { 0.0 } else { within.iter().filter(|p| p[1] >= y.0 && p[1] <= y.1).count() as f64 / within.len() as f64 }
                    }
                    Geo::Line { pts } => if series_selected(&s.selection, pts) { 1.0 } else { 0.0 },
                    _ => 1.0,
                },
            }
        };
        let degrees: Vec<f64> = match &s.selection {
            Selection::Lasso { poly, tilt, structure } => {
                let (keys, n_in, regions) = lasso_take(&f, poly, *tilt, *structure);
                let label = match structure {
                    None => format!("lasso: {n_in} cells"),
                    Some(_) => format!("CloudLasso: {} of the {n_in} cells in the lasso, the largest of {regions} dense regions", keys.len()),
                };
                f.lasso = Some((poly.clone(), *tilt, label));
                f.items.iter().map(|it| if keys.contains(&it.key) { 1.0 } else { 0.0 }).collect()
            }
            _ => f.items.iter().map(degree).collect(),
        };
        f.selected = Some(degrees.iter().filter(|d| **d >= 0.5).count());
        match s.effect {
            Effect::Fade => {
                for (it, d) in f.items.iter_mut().zip(&degrees) {
                    it.fill[3] *= (0.15 + 0.85 * d) as f32;
                }
            }
            Effect::Filter => {
                let mut k = degrees.iter();
                f.items.retain(|_| *k.next().unwrap() >= 0.5);
            }
        }
    }
    // A lens runs its own step over what lies under it.
    if let Some(lens) = s.lens {
        let dom = |a: &Axis| match a {
            Axis::Linear { lo, hi, .. } => Some((*lo, *hi)),
            _ => None,
        };
        let (dx, dy) = (dom(&f.x), dom(&f.y));
        let unit = |p: [f64; 2]| [dx.map_or(p[0], |(lo, hi)| (p[0] - lo) / (hi - lo)), dy.map_or(p[1], |(lo, hi)| (p[1] - lo) / (hi - lo))];
        let (c, r) = (lens.focus(), lens.radius());
        let inside = |q: [f64; 2]| (q[0] - c[0]).hypot(q[1] - c[1]) <= r;
        let mut fits = vec![];
        match lens {
            Lens::Sample { keep, .. } | Lens::Mole { above: keep, .. } => {
                let sample = matches!(lens, Lens::Sample { .. });
                let (mut under, mut kept) = (0, 0);
                f.items.retain(|it| match &it.geo {
                    Geo::Point { x, y } if inside(unit([*x, *y])) => {
                        let k = if sample { hash01(&it.key) < keep } else { it.h <= keep };
                        under += 1;
                        kept += k as usize;
                        k
                    }
                    _ => true,
                });
                let label = if sample {
                    format!("sample: {kept} of {under} cells shown ({:.0} %)", keep * 100.0)
                } else {
                    format!("mole: {} of {under} cells above {:.0} % of the height range taken away", under - kept, keep * 100.0)
                };
                fits.push(Fit { a: c, b: c, color: [0.13, 0.13, 0.13, 1.0], label });
            }
            Lens::Regression { .. } => {
                // Per series, least squares of y on x over the points inside.
                let per = unit_word(&f.x);
                for it in &f.items {
                    let Geo::Line { pts } = &it.geo else { continue };
                    let pts: Vec<[f64; 2]> = pts.iter().copied().filter(|p| inside(unit(*p))).collect();
                    let Some((slope, icpt, r2)) = ols(&pts) else { continue };
                    let (x0, x1) = pts.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p[0]), a.1.max(p[0])));
                    fits.push(Fit {
                        a: unit([x0, icpt + slope * x0]),
                        b: unit([x1, icpt + slope * x1]),
                        color: it.fill,
                        label: format!("line {}: {} per {per}, r² {:.2}, {} points", bare_key(&it.key), signed(slope), r2, pts.len()),
                    });
                }
            }
        }
        f.lens = Some((lens, fits));
    }
    // An offset magnifier's callout goes where it covers the least data.
    if matches!(s.view, View::Magnifier { offset: true, .. }) {
        let pts = unit_points(&f);
        f.view = place_callout(s.view, &pts, f.legend.is_empty() && f.colorbar.is_none());
        f.state.view = f.view;
    }
    if let Some(t) = &s.x_title {
        f.x.retitle(t);
    }
    if let Some(t) = &s.y_title {
        f.y.retitle(t);
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn magnifier(focus: [f64; 2], side: usize) -> View {
        View::Magnifier { focus, radius: 0.06, zoom: 4.0, offset: true, side, anchor: [f64::NAN; 2] }
    }
    fn side(v: View) -> usize {
        match v {
            View::Magnifier { side, .. } => side,
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_callout_goes_where_there_is_no_data() {
        // Data fills the upper half; the callout of a focus in the middle
        // goes down.
        let items: Vec<[f64; 2]> = (0..400).map(|i| [(i % 20) as f64 / 20.0, 0.55 + (i / 20) as f64 / 45.0]).collect();
        let v = place_callout(magnifier([0.5, 0.5], 0), &items, false);
        let d = CALLOUT_SIDES[side(v)];
        assert!(d[1] < 0.0, "callout went {d:?}, not down");
    }

    #[test]
    fn a_small_move_keeps_the_side() {
        let items: Vec<[f64; 2]> = (0..400).map(|i| [(i % 20) as f64 / 20.0, 0.55 + (i / 20) as f64 / 45.0]).collect();
        let first = place_callout(magnifier([0.5, 0.5], 0), &items, false);
        let moved = place_callout(first.with_focus([0.52, 0.49]), &items, false);
        assert_eq!(side(first), side(moved));
    }

    #[test]
    fn the_callout_stays_put_until_the_source_moves_a_radius() {
        let first = place_callout(magnifier([0.3, 0.3], 0), &[], false);
        let a = |v: View| match v {
            View::Magnifier { anchor, .. } => anchor,
            _ => unreachable!(),
        };
        // 0.1 is less than the callout radius (0.24): the callout stays.
        let near = place_callout(first.with_focus([0.4, 0.3]), &[], false);
        assert_eq!(a(first), a(near));
        // 0.3 is more: it is placed again, from the new focus.
        let far = place_callout(near.with_focus([0.6, 0.3]), &[], false);
        assert_eq!(a(far), [0.6, 0.3]);
    }

    #[test]
    fn the_callout_stays_in_the_plot() {
        let v = place_callout(magnifier([0.9, 0.9], 0), &[], false);
        let View::Magnifier { radius, zoom, side, anchor, .. } = v else { unreachable!() };
        let (c, r) = callout(anchor, radius, zoom, side);
        assert!(c[0] - r >= -1e-9 && c[0] + r <= 1.0 + 1e-9 && c[1] - r >= -1e-9 && c[1] + r <= 1.0 + 1e-9, "{c:?} {r}");
    }
}
