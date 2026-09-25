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
}

/// A view over the plot's unit square, after the coordinate system (the
/// family of experiment 5): the same items, projected differently.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum View {
    Flat,
    /// Sarkar–Brown fisheye around `focus` (unit space): the context stays,
    /// distances inside the lens do not.
    Fisheye { focus: [f64; 2], radius: f64, distortion: f64 },
    /// A round inset over `focus`, magnified `zoom` times and undistorted;
    /// it covers what lies around it.
    Magnifier { focus: [f64; 2], radius: f64, zoom: f64 },
    /// The map in 3D: height as z, seen from `yaw` degrees around and
    /// `elevation` degrees above the horizon.
    Tilt { yaw: f64, elevation: f64 },
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
            View::Magnifier { radius, zoom, .. } => View::Magnifier { focus: f, radius, zoom },
            v => v,
        }
    }
}

impl State {
    pub fn new(dataset: Dataset) -> Self {
        let mut s = State { dataset, mark: Mark::default_for(dataset), color: None, zoom: None, range: None, highlight: false, threshold: None, title: String::new(), x_title: None, y_title: None, y_log: false, view: View::Flat };
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
    /// For a heatmap: the value range of the colour scale.
    pub colorbar: Option<(f64, f64)>,
    /// Equal aspect for maps.
    pub square: bool,
    pub view: View,
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
    if let Some(t) = &s.x_title {
        f.x.retitle(t);
    }
    if let Some(t) = &s.y_title {
        f.y.retitle(t);
    }
    f
}
