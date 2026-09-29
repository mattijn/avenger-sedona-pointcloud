//! Experiment 10 in a window: the 10M flights with every selection of the
//! figures under the mouse. Brush a histogram, draw a lasso on the density of
//! departure time and arrival delay, drag a line brush or a timebox across the
//! distance bands' lines; each histogram shows the flights the other panels
//! leave, queried again through `avenger-selection` as you drag. S turns the
//! soft brush on and off. D morphs the histogram under the pointer into a
//! donut and back (`avenger-transition`, `avenger-coords`); on the donut a
//! drag along the ring brushes it, read back into the same range brush.
//!
//! Usage: cargo run --release -p lidar-flights --bin flights_live -- <flights-10m.parquet>
//!        cargo run --release -p lidar-flights --bin flights_live -- <flights-10m.parquet> --snapshots <out_dir>
//!        cargo run --release -p lidar-flights --bin flights_live -- <flights-10m.parquet> --tour <frames_dir>

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use avenger_app::{app::{AvengerApp, SceneBuild, SceneGraphBuilder}, error::AvengerAppError};
use avenger_coords::{draw, Bend, CoordinateSystem, Fitted};
use avenger_eventstream::{
    manager::EventStreamHandler,
    runtime::{RuntimeHostCommand, RuntimeWakeKey},
    scene::{SceneGraphEvent, SceneGraphEventType},
    stream::{EventStreamConfig, UpdateStatus},
    window::{Key, MouseButton, NamedKey},
};
use avenger_geometry::rtree::SceneGraphRTree;
use avenger_scenegraph::marks::{group::SceneGroup, mark::SceneMark};
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_selection::{SelectionSet, SelectionValue, SeriesTest, ValueTest};
use avenger_transition::bins::{self, Layout};
use avenger_transition::{ease, join, phases, Geo, Item, Plane, Timing, Tweened};
use avenger_winit_wgpu::{WinitWgpuAvengerApp, WinitWgpuAvengerAppOptions};
use datafusion::common::ScalarValue;
use datafusion::logical_expr::{col, lit};
use datafusion::prelude::SessionContext;
use lidar_flights::*;
use winit::{dpi::LogicalSize, window::WindowAttributes};

/// The layout: the density panel and the lines on the left, the three
/// histograms on the right.
const TOP: f32 = 100.0;
const LEFT: f32 = 80.0;
const RIGHT: f32 = LEFT + W + 120.0;
const SERIES_TOP: f32 = TOP + DENSITY_H + 80.0;
const HIST_ROW: f32 = H + 60.0;
const SIZE: [f32; 2] = [RIGHT + W + 40.0, TOP + 2.0 * HIST_ROW + H + 50.0];
/// The soft brush's reach, in pixels, as in the soft figure.
const SOFT_PX: f64 = 60.0;
const RED: [f32; 4] = [0.85, 0.2, 0.15, 1.0];
const LIGHT: [f32; 4] = [0.6, 0.75, 0.88, 1.0];
/// The bars outside a panel's own brush.
const PALE: [f32; 4] = [0.78, 0.84, 0.9, 1.0];
const MUTED: [f32; 4] = [0.42, 0.45, 0.49, 1.];
/// The morph between bars and donut: how long it takes, the donut's ring and
/// the brush's reach across it in unit y, and the brush's tint.
const MORPH_SECS: f64 = 2.0;
const RING: [f64; 2] = [0.5, 0.95];
const BRUSH_Y_STACK: [f64; 2] = [0.42, 1.0];
const BRUSH_FILL: [f32; 4] = [0.85, 0.2, 0.15, 0.08];
const WAKE: &str = "flights-morph";

type Bins = Vec<(i64, f64)>;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Target {
    Density,
    Series,
    Hist(usize),
}
fn hist_origin(i: usize) -> [f32; 2] {
    [RIGHT, TOP + i as f32 * HIST_ROW]
}
fn target_at(p: [f32; 2]) -> Option<(Target, [f32; 2])> {
    let inside = |o: [f32; 2], w: f32, h: f32| (p[0] >= o[0] && p[0] <= o[0] + w && p[1] >= o[1] && p[1] <= o[1] + h).then(|| [p[0] - o[0], p[1] - o[1]]);
    if let Some(q) = inside([LEFT, TOP], W, DENSITY_H) {
        return Some((Target::Density, q));
    }
    if let Some(q) = inside([LEFT, SERIES_TOP], W, SERIES_H) {
        return Some((Target::Series, q));
    }
    (0..3).find_map(|i| inside(hist_origin(i), W, H).map(|q| (Target::Hist(i), q)))
}

/// A series gesture, in data units: hours of departure and minutes of mean delay.
#[derive(Clone, Copy, Debug)]
enum Series {
    Line { from: [f64; 2], to: [f64; 2] },
    Box { x: (f64, f64), y: (f64, f64) },
}
impl Series {
    fn test(&self) -> SeriesTest {
        match *self {
            Series::Line { from, to } => SeriesTest::Crosses { from, to },
            Series::Box { x, y } => SeriesTest::Within { x, y },
        }
    }
}
fn series_data(q: [f32; 2]) -> [f64; 2] {
    let t = &PLOTS[1];
    [t.domain[0] + q[0] as f64 / W as f64 * (t.domain[1] - t.domain[0]),
     MEAN[0] + (SERIES_H - q[1]) as f64 / SERIES_H as f64 * (MEAN[1] - MEAN[0])]
}
fn series_px(p: [f64; 2]) -> [f32; 2] {
    let t = &PLOTS[1];
    [t.px(p[0], W), SERIES_H - ((p[1] - MEAN[0]) / (MEAN[1] - MEAN[0])) as f32 * SERIES_H]
}

#[derive(Clone, Debug)]
struct Drag {
    target: Target,
    start: [f32; 2],
    shift: bool,
}

/// What a refresh computes: each panel's histograms and the chosen bands.
struct Results {
    hard: Vec<Bins>,
    faded: Vec<Option<Bins>>,
    bands: Option<HashSet<i64>>,
    soft_failed: bool,
    ms: f64,
}

#[derive(Clone)]
struct State {
    ctx: SessionContext,
    rt: tokio::runtime::Handle,
    n: usize,
    all: Vec<Bins>,
    heat: Arc<(Vec<[f32; 4]>, Vec<[f32; 4]>)>,
    lines: Arc<BTreeMap<i64, Vec<[f64; 2]>>>,
    size: [f32; 2],
    // The selections, as drawn.
    brushes: [Option<[f64; 2]>; 3],
    lasso: Vec<[f64; 2]>,
    series: Option<Series>,
    soft: bool,
    /// T: the line chart's drag draws a timebox instead of a line brush (Shift-drag swaps).
    timebox: bool,
    drag: Option<Drag>,
    // The last results.
    hard: Vec<Bins>,
    faded: Vec<Option<Bins>>,
    bands: Option<HashSet<i64>>,
    soft_failed: bool,
    last_ms: f64,
    /// Only in `--tour`: the pointer drawn into the frame (pressed or not), and a caption.
    pointer: Option<([f32; 2], bool)>,
    caption: Option<String>,
    /// Each histogram's place between bars (0) and donut (1), where it is
    /// heading, and the pointer, for D to know which panel it means.
    donut: [f64; 3],
    goal: [f64; 3],
    cursor: [f32; 2],
    last_tick: Option<Instant>,
    wake_generation: u64,
}

impl State {
    /// A press starts a gesture on the panel under the pointer; a right click
    /// clears that panel's selection.
    fn press(&mut self, p: [f32; 2], button: MouseButton, shift: bool) -> bool {
        let Some((target, q)) = target_at(p) else { return false };
        println!("press on {target:?} with {button:?}{}", if shift { " and Shift" } else { "" });
        if button == MouseButton::Right {
            self.clear(target);
            return true;
        }
        if button != MouseButton::Left {
            return false;
        }
        if let Target::Hist(i) = target {
            // Half bent, a panel has no inverse to brush through.
            if self.donut[i] > 0.0 && self.donut[i] < 1.0 {
                return false;
            }
        }
        self.drag = Some(Drag { target, start: q, shift });
        match target {
            Target::Density => self.lasso = vec![[q[0] as f64, q[1] as f64]],
            Target::Hist(i) => self.brushes[i] = None,
            Target::Series => self.series = None,
        }
        false
    }
    /// The gesture follows the pointer; true when a selection changed.
    fn motion(&mut self, p: [f32; 2]) -> bool {
        let Some(d) = self.drag.clone() else { return false };
        self.motion_of(&d, p)
    }
    fn motion_of(&mut self, d: &Drag, p: [f32; 2]) -> bool {
        let o = match d.target {
            Target::Density => [LEFT, TOP],
            Target::Series => [LEFT, SERIES_TOP],
            Target::Hist(i) => hist_origin(i),
        };
        let (w, h) = match d.target { Target::Density => (W, DENSITY_H), Target::Series => (W, SERIES_H), Target::Hist(_) => (W, H) };
        let q = [(p[0] - o[0]).clamp(0.0, w), (p[1] - o[1]).clamp(0.0, h)];
        match d.target {
            Target::Hist(i) if self.donut[i] >= 1.0 => {
                // On the donut: each end read back through the bend's inverse
                // and the stacked layout, into minutes, hours or miles.
                let (a, b) = (self.donut_value(i, d.start), self.donut_value(i, q));
                self.brushes[i] = Some([a.min(b), a.max(b)]);
            }
            Target::Hist(i) => {
                let v = |x: f32| { let pl = &PLOTS[i]; pl.domain[0] + x as f64 / W as f64 * (pl.domain[1] - pl.domain[0]) };
                let (a, b) = (v(d.start[0].min(q[0])), v(d.start[0].max(q[0])));
                self.brushes[i] = Some([a, b]);
            }
            Target::Density => {
                let last = *self.lasso.last().unwrap();
                if (last[0] - q[0] as f64).hypot(last[1] - q[1] as f64) < 3.0 {
                    return false;
                }
                self.lasso.push([q[0] as f64, q[1] as f64]);
            }
            Target::Series => {
                let (a, b) = (series_data(d.start), series_data(q));
                self.series = Some(if d.shift != self.timebox {
                    Series::Box { x: (a[0].min(b[0]), a[0].max(b[0])), y: (a[1].min(b[1]), a[1].max(b[1])) }
                } else {
                    Series::Line { from: a, to: b }
                });
            }
        }
        true
    }
    /// The gesture ends. A click without a drag clears the panel, as in Jon's example.
    fn release(&mut self, p: [f32; 2]) -> bool {
        // The release point counts as the last position of the drag.
        let Some(d) = self.drag.take() else { return false };
        self.motion_of(&d, p);
        let tiny = match d.target {
            Target::Hist(i) => self.brushes[i].map_or(true, |b| self.brush_px(i, b) < 3.0),
            Target::Density => self.lasso.len() < 3,
            Target::Series => self.series.is_none(),
        };
        if tiny {
            self.clear(d.target);
        }
        true
    }
    fn clear(&mut self, t: Target) {
        match t {
            Target::Hist(i) => self.brushes[i] = None,
            Target::Density => self.lasso.clear(),
            Target::Series => self.series = None,
        }
    }
    /// A key; true when the selections changed.
    fn key(&mut self, k: &Key) -> bool {
        match k {
            Key::Named(NamedKey::Escape) => { self.clear_all(); true }
            Key::Character('s') | Key::Character('S') => { self.soft = !self.soft; true }
            Key::Character('t') | Key::Character('T') => { self.timebox = !self.timebox; false }
            Key::Character('d') | Key::Character('D') => {
                let i = match target_at(self.cursor) { Some((Target::Hist(i), _)) => i, _ => 0 };
                self.goal[i] = 1.0 - self.goal[i];
                println!("{} towards {}", PLOTS[i].name, if self.goal[i] > 0.5 { "donut" } else { "bars" });
                false
            }
            _ => false,
        }
    }
    fn clear_all(&mut self) {
        self.brushes = [None; 3];
        self.lasso.clear();
        self.series = None;
    }

    /// The queries for the selections as they stand, to run on the data runtime.
    fn job(&self) -> impl std::future::Future<Output = Result<Results, String>> + Send + 'static {
        let (ctx, brushes, lasso, series, soft) = (self.ctx.clone(), self.brushes, self.lasso.clone(), self.series, self.soft);
        async move { compute(ctx, brushes, lasso, series, soft).await.map_err(|e| e.to_string()) }
    }
    fn apply(&mut self, r: Results) {
        (self.hard, self.faded, self.bands, self.soft_failed, self.last_ms) = (r.hard, r.faded, r.bands, r.soft_failed, r.ms);
    }
    async fn refresh(&mut self) {
        match self.rt.spawn(self.job()).await {
            Ok(Ok(r)) => self.apply(r),
            Ok(Err(e)) => eprintln!("query failed: {e}"),
            Err(e) => eprintln!("query task failed: {e}"),
        }
    }

    /// Move every morph `dt` seconds on; true while one is still moving.
    fn step(&mut self, dt: f64) -> bool {
        for i in 0..3 {
            let d = self.goal[i] - self.donut[i];
            self.donut[i] += d.signum() * d.abs().min(dt / MORPH_SECS);
        }
        self.animating()
    }
    fn animating(&self) -> bool {
        (0..3).any(|i| self.donut[i] != self.goal[i])
    }
    /// Panel `i`'s bins as the panel shows them, every display bin included.
    fn bins(&self, i: usize, of: &[(i64, f64)]) -> bins::Bins {
        let p = &PLOTS[i];
        let n = ((p.domain[1] - p.domain[0]) / p.step).round() as usize;
        let mut counts = vec![0.0; n];
        for (b, v) in of {
            if (0..n as i64).contains(b) {
                counts[*b as usize] = *v;
            }
        }
        bins::Bins::new((0..=n).map(|k| p.domain[0] + k as f64 * p.step).collect(), counts)
    }
    /// A pointer on panel `i`'s donut (panel pixels) as a value of its column.
    fn donut_value(&self, i: usize, q: [f32; 2]) -> f64 {
        let bend = Bend { width: W as f64, height: H as f64, t: 1.0 };
        let cs = Fitted::new(&bend, W as f64, H as f64);
        let u = cs.invert([q[0] as f64, q[1] as f64]).unwrap_or([0.0, 0.0]);
        self.bins(i, &self.all[i]).value_at(u[0], Layout::Stack { y: RING })
    }
    /// How long a brush is on screen: along the bars, or around the ring.
    fn brush_px(&self, i: usize, b: [f64; 2]) -> f64 {
        if self.donut[i] >= 1.0 {
            let bins = self.bins(i, &self.all[i]);
            let stack = Layout::Stack { y: RING };
            let mid = 0.5 * (RING[0] + RING[1]) * 0.5 * H as f64;
            (bins.position(b[1], stack) - bins.position(b[0], stack)) * std::f64::consts::TAU * mid
        } else {
            (PLOTS[i].px(b[1], W) - PLOTS[i].px(b[0], W)) as f64
        }
    }
    /// Histogram `i` between bars and donut: the bars stack, then bend. The
    /// donut is stacked by all flights (grey), as the bars stand on them, and
    /// the flights the other panels leave fill each slice from the inside by
    /// their share, as they fill each bar from its base; so the slices' angles
    /// stay put while other panels filter. The panel's own brush tints its
    /// bars, blue for the part it takes, and is drawn as one more item, so it
    /// bends with them.
    fn morph_panel(&self, i: usize) -> SceneMark {
        let p = &PLOTS[i];
        let t = self.donut[i];
        let (shown, all) = (self.bins(i, &self.hard[i]), self.bins(i, &self.all[i]));
        let max = all.counts.iter().cloned().fold(1.0, f64::max);
        let brush = self.brushes[i];
        let frame = |layout: Layout| -> Vec<Item> {
            let mut items = all.items("all", layout, |_| GREY);
            items.extend(all.layer("bar", layout, &shown.counts, |_| if brush.is_some() { PALE } else { BLUE }));
            if let Some(b) = brush {
                items.extend(all.clipped_layer("sel", b, layout, &shown.counts, BLUE));
                let y = if matches!(layout, Layout::Bars { .. }) { [0.0, 1.0] } else { BRUSH_Y_STACK };
                items.push(Item { key: "brush".into(), parent: None, geo: Geo::Rect(all.interval(b, layout, y)), fill: BRUSH_FILL, size: 0.0, h: 0.0 });
            }
            items
        };
        let (bend, g) = phases(Plane::Cartesian, Plane::Polar, t);
        let e = ease(t);
        let items = join(&frame(Layout::Bars { max }), &frame(Layout::Stack { y: RING }), Timing { t, g, exit: 1.0 - e, enter: e });
        let bent = Bend { width: W as f64, height: H as f64, t: bend };
        let cs = Fitted::new(&bent, W as f64, H as f64);
        let layer = |prefix: &str, stroke: [f32; 4]| -> SceneMark {
            let chosen: Vec<&Tweened> = items.iter().filter(|it| it.key.starts_with(prefix)).collect();
            let r = |it: &Tweened| match it.geo { Geo::Rect(r) => r, _ => [0.0; 4] };
            let lo: Vec<[f64; 2]> = chosen.iter().map(|it| { let r = r(it); [r[0], r[2]] }).collect();
            let hi: Vec<[f64; 2]> = chosen.iter().map(|it| { let r = r(it); [r[1], r[3]] }).collect();
            let fill: Vec<[f32; 4]> = chosen.iter().map(|it| it.fill).collect();
            draw::rects(&cs, &lo, &hi, &fill, stroke).0
        };
        let mut marks = vec![layer("brush", RED), layer("all", [1.0; 4]), layer("bar", [1.0; 4]), layer("sel", [0.0; 4])];
        let what = match brush {
            Some(b) => format!("{} · brush {:.0}–{:.0} · drag along the ring to brush · D: back to bars", p.title, b[0], b[1]),
            None => format!("{} · drag along the ring to brush · D: back to bars", p.title),
        };
        marks.push(text(what, 0.0, H + 36.0, 11.0, INK));
        SceneGroup { origin: hist_origin(i), marks, ..Default::default() }.into()
    }

    fn scene(&self) -> Result<SceneGraph, Error> {
        let mut marks = vec![rects(&[[0.0, 0.0, self.size[0].max(SIZE[0]), self.size[1].max(SIZE[1])]], vec![[1.0; 4]])];
        let active = self.brushes.iter().filter(|b| b.is_some()).count() + (self.lasso.len() >= 3) as usize + self.series.is_some() as usize;
        marks.extend(heading("Cross-Filter Flights, with this repo's selections",
            &format!("{} flights · drag a histogram to brush it · draw a lasso on the density · drag across the lines for a line brush or a timebox",
                thousands(self.n))));
        marks.push(text(format!("D: a histogram as a donut · T: the lines take a {} (Shift-drag: a {}) · S: soft brush {} · click or right-click a panel to clear it · Esc clears all · {} selection{} · last update {:.0} ms",
            if self.timebox { "timebox" } else { "line brush" }, if self.timebox { "line brush" } else { "timebox" },
            if !self.soft { "off" } else if self.soft_failed { "on, but its query failed (see the terminal)" } else { "on" }, active, if active == 1 { "" } else { "s" }, self.last_ms), 24.0, 66.0, 11.0, MUTED));

        // The density panel and its lasso.
        let (t, d) = (&PLOTS[1], &PLOTS[0]);
        let mut group = axes(t.domain, d.domain, t.title, d.title, [0.0, 0.0], W, DENSITY_H)?;
        group.push(rects(&self.heat.0, self.heat.1.clone()));
        if self.lasso.len() >= 2 {
            let closed = self.drag.as_ref().map_or(true, |d| d.target != Target::Density);
            let ring: Vec<[f32; 2]> = self.lasso.iter().chain(closed.then(|| &self.lasso[0])).map(|p| [p[0] as f32, p[1] as f32]).collect();
            group.push(outline(&ring, RED, 2.0));
        }
        marks.push(SceneGroup { origin: [LEFT, TOP], marks: group, ..Default::default() }.into());

        // The distance bands' lines, the chosen ones in blue.
        let mean = Plot { name: "mean", title: "Mean Arrival Delay (min), per 200-mile band", domain: MEAN, step: 1.0 };
        let mut group = axes(t.domain, mean.domain, t.title, mean.title, [0.0, 0.0], W, SERIES_H)?;
        if let Some(Series::Box { x, y }) = self.series {
            let (a, b) = (series_px([x.0, y.1]), series_px([x.1, y.0]));
            group.push(rects(&[[a[0], a[1], b[0] - a[0], b[1] - a[1]]], vec![[0.85, 0.2, 0.15, 0.08]]));
            group.push(outline(&[a, [b[0], a[1]], b, [a[0], b[1]], a], RED, 1.5));
        }
        let chosen = |k: &i64| self.bands.as_ref().is_some_and(|b| b.contains(k));
        for selected in [false, true] {
            for (k, l) in self.lines.iter().filter(|(k, _)| chosen(k) == selected) {
                let pts: Vec<[f32; 2]> = l.iter().map(|p| series_px(*p)).collect();
                group.push(outline(&pts, if selected { BLUE } else { GREY }, if selected { 2.0 } else { 1.0 }));
                if selected {
                    let end = pts.last().unwrap();
                    group.push(text(format!("{}–{} mi", k * BAND as i64, (k + 1) * BAND as i64), end[0] + 4.0, end[1] + 3.0, 9.0, BLUE));
                }
            }
        }
        if let Some(Series::Line { from, to }) = self.series {
            group.push(outline(&[series_px(from), series_px(to)], RED, 2.5));
        }
        if let (Some(s), Some(b)) = (self.series, &self.bands) {
            let what = match s { Series::Line { .. } => "line brush", Series::Box { .. } => "timebox" };
            group.push(text(format!("{what}: {} of {} bands", b.len(), self.lines.len()), W - 150.0, 14.0, 11.0, RED));
        }
        marks.push(SceneGroup { origin: [LEFT, SERIES_TOP], marks: group, ..Default::default() }.into());

        // The histograms: grey all, light by degree when soft, blue what the others leave.
        for (i, p) in PLOTS.iter().enumerate() {
            let max = self.all[i].iter().map(|x| x.1).fold(1.0, f64::max);
            let mut shades = Vec::new();
            if let Some(b) = self.brushes[i] {
                if self.soft {
                    let reach = SOFT_PX * (p.domain[1] - p.domain[0]) / W as f64;
                    shades.push(Shade { range: [b[0] - reach, b[1] + reach], alpha: 0.03, outline: false });
                }
                shades.push(Shade { range: b, alpha: 0.06, outline: true });
            }
            let mut layers: Vec<(&[(i64, f64)], Vec<[f32; 4]>)> = Vec::new();
            if let Some(f) = &self.faded[i] {
                layers.push((f, vec![LIGHT; f.len()]));
            }
            // The panel's own brush does not filter it (a cross-filter keeps
            // the context to brush in); it tints the bars instead: blue inside,
            // pale outside, and with the soft brush fading over its reach.
            let colours = match self.brushes[i] {
                None => vec![BLUE; self.hard[i].len()],
                Some(b) => {
                    let (a, z) = (p.px(b[0], W) as f64, p.px(b[1], W) as f64);
                    self.hard[i].iter().map(|(bin, _)| {
                        let c = p.px(p.domain[0] + (*bin as f64 + 0.5) * p.step, W) as f64;
                        let out = (a - c).max(c - z).max(0.0);
                        let k = if self.soft { (1.0 - out / SOFT_PX).clamp(0.0, 1.0) } else if out > 0.0 { 0.0 } else { 1.0 };
                        [0, 1, 2, 3].map(|j| PALE[j] + k as f32 * (BLUE[j] - PALE[j]))
                    }).collect()
                }
            };
            layers.push((&self.hard[i], colours));
            if self.donut[i] > 0.0 {
                marks.push(self.morph_panel(i));
            } else {
                marks.push(panel(p, hist_origin(i), &self.all[i], &layers, &shades, max)?);
            }
        }
        if let Some(c) = &self.caption {
            marks.push(text(c.clone(), 24.0, 88.0, 13.0, RED));
        }
        if let Some((p, pressed)) = self.pointer {
            marks.push(pointer_mark(p, pressed));
        }
        let size = [self.size[0].max(SIZE[0]), self.size[1].max(SIZE[1])];
        Ok(SceneGraph { width: size[0], height: size[1], origin: [0.0; 2], marks: vec![SceneGroup { marks, ..Default::default() }.into()] })
    }
}

/// The histograms for a set of selections: one query per panel, run side by side.
async fn compute(ctx: SessionContext, brushes: [Option<[f64; 2]>; 3], lasso: Vec<[f64; 2]>, series: Option<Series>, soft: bool)
    -> Result<Results, Error> {
    let t0 = Instant::now();
    let mut state: SelectionSet = empty();
    for (i, b) in brushes.iter().enumerate() {
        if let Some(b) = b {
            state = state.set(&brush_producer(&PLOTS[i]), SelectionValue::tuple([(pid("value"), ValueTest::range(b[0]..b[1]))]))?;
        }
    }
    if lasso.len() >= 3 {
        let p = density_producer();
        state = state.set(&p, SelectionValue::polygon(&p, &pid("u"), &pid("v"), &lasso)?)?;
    }
    let mut bands = None;
    if let Some(s) = series {
        let test = s.test();
        let keys = test.keys(series_rows(&ctx).await?, col("band"), col("x"), col("y")).await?;
        bands = Some(keys.iter().filter_map(|k| match k { ScalarValue::Int64(Some(v)) => Some(*v), _ => None }).collect());
        state = state.set(&series_producer(), test.value(&pid("band"), keys))?;
    }
    let mut tasks = Vec::new();
    for i in 0..3 {
        let filter = cross(i).predicate(&state)?;
        let degree = if soft && brushes.iter().enumerate().any(|(j, b)| j != i && b.is_some()) { cross(i).degree(&state, SOFT_PX).ok() } else { None };
        let ctx = ctx.clone();
        tasks.push(tokio::spawn(async move {
            let hard = histogram(&ctx, &PLOTS[i], filter, None).await?;
            // Should the degree query fail, the panel goes without its light
            // layer and the header says so, rather than losing the update.
            let faded = match degree {
                Some(d) => match histogram(&ctx, &PLOTS[i], lit(true), Some(d)).await {
                    Ok(f) => Some(f),
                    Err(e) => { eprintln!("the soft layer failed: {}", e.to_string().lines().next().unwrap_or("")); None }
                },
                None => None,
            };
            Ok::<_, datafusion::error::DataFusionError>((hard, faded))
        }));
    }
    let wanted = (0..3).filter(|&i| soft && brushes.iter().enumerate().any(|(j, b)| j != i && b.is_some())).count();
    let (mut hard, mut faded) = (Vec::new(), Vec::new());
    for t in tasks {
        let (h, f) = t.await??;
        hard.push(h);
        faded.push(f);
    }
    let soft_failed = faded.iter().filter(|f| f.is_some()).count() < wanted;
    Ok(Results { hard, faded, bands, soft_failed, ms: t0.elapsed().as_secs_f64() * 1e3 })
}

// ---------------------------------------------------------------------------
// The window

struct Builder;
#[async_trait::async_trait]
impl SceneGraphBuilder<State> for Builder {
    async fn build(&self, s: &mut State) -> Result<SceneGraph, AvengerAppError> {
        self.build_with_effects(s).await.map(|b| b.scene_graph)
    }
    /// While a panel morphs, each build moves it on by the time since the
    /// last and asks the host to wake again in 16 ms.
    async fn build_with_effects(&self, s: &mut State) -> Result<SceneBuild, AvengerAppError> {
        let now = Instant::now();
        let dt = s.last_tick.map_or(0.0, |l| now.duration_since(l).as_secs_f64()).min(0.1);
        let animating = s.step(dt);
        s.last_tick = animating.then_some(now);
        let scene_graph = s.scene().map_err(|e| AvengerAppError::InternalError(e.to_string()))?;
        let mut commands = vec![];
        if animating {
            s.wake_generation += 1;
            commands.push(RuntimeHostCommand::RequestWakeup {
                key: RuntimeWakeKey::new(WAKE, 0, "frame"),
                deadline: avenger_common::time::Instant::now() + avenger_common::time::Duration::from_millis(16),
                generation: s.wake_generation,
            });
        }
        Ok(SceneBuild { scene_graph, commands, rebuild_geometry: false })
    }
}

fn redraw() -> UpdateStatus {
    UpdateStatus { rerender: true, rebuild_geometry: false, ..Default::default() }
}
fn nothing() -> UpdateStatus {
    UpdateStatus { rerender: false, rebuild_geometry: false, ..Default::default() }
}

/// Every pointer and key event, in order: the gesture, then the queries it needs.
struct Input;
#[async_trait::async_trait]
impl EventStreamHandler<State> for Input {
    async fn handle(&self, event: &SceneGraphEvent, s: &mut State, _: &SceneGraphRTree) -> UpdateStatus {
        let query = match event {
            SceneGraphEvent::MouseDown(e) => s.press(e.position, e.button, e.modifiers.shift),
            SceneGraphEvent::MouseUp(e) if e.button == MouseButton::Left => s.release(e.position),
            SceneGraphEvent::KeyPress(e) => s.key(&e.key),
            SceneGraphEvent::RuntimeWake(w) if w.key.namespace == WAKE => return redraw(),
            SceneGraphEvent::WindowResize(e) => { s.size = e.size; return redraw() }
            _ => return nothing(),
        };
        if query {
            s.refresh().await;
        }
        redraw()
    }
}
/// The pointer while dragging: the shape follows at once.
struct Move;
#[async_trait::async_trait]
impl EventStreamHandler<State> for Move {
    async fn handle(&self, event: &SceneGraphEvent, s: &mut State, _: &SceneGraphRTree) -> UpdateStatus {
        if let Some(p) = event.position() {
            s.cursor = p;
        }
        match event.position() {
            Some(p) if s.motion(p) => redraw(),
            _ => nothing(),
        }
    }
}
/// And the histograms follow while dragging, as fast as the queries allow.
struct LiveQuery;
#[async_trait::async_trait]
impl EventStreamHandler<State> for LiveQuery {
    async fn handle(&self, _: &SceneGraphEvent, s: &mut State, _: &SceneGraphRTree) -> UpdateStatus {
        if s.drag.is_none() {
            return nothing();
        }
        s.refresh().await;
        redraw()
    }
}

fn main() -> Result<(), Error> {
    let data = std::env::args().nth(1).unwrap_or_else(|| "data/flights-10m.parquet".into());
    let data_rt = tokio::runtime::Runtime::new()?;
    let ctx = SessionContext::new();
    let t0 = Instant::now();
    let (n, all, heat, lines) = data_rt.block_on(async {
        let n = load(&ctx, &data).await?;
        let mut all = Vec::new();
        for p in &PLOTS {
            all.push(histogram(&ctx, p, lit(true), None).await?);
        }
        Ok::<_, Error>((n, all, density(&ctx).await?, series_lines(&ctx).await?))
    })?;
    println!("{n} flights ready in {:.0} ms", t0.elapsed().as_secs_f64() * 1e3);
    let state = State {
        ctx, rt: data_rt.handle().clone(), n, hard: all.clone(), faded: vec![None; 3], all, heat: Arc::new(heat), lines: Arc::new(lines),
        size: SIZE, brushes: [None; 3], lasso: Vec::new(), series: None, soft: false, timebox: false, drag: None, bands: None, soft_failed: false, last_ms: 0.0, pointer: None, caption: None,
        donut: [0.0; 3], goal: [0.0; 3], cursor: [0.0; 2], last_tick: None, wake_generation: 0,
    };

    if let Some(i) = std::env::args().position(|a| a == "--snapshots") {
        let out = std::env::args().nth(i + 1).expect("--snapshots <out_dir>");
        return data_rt.block_on(snapshots(state, &out));
    }
    if let Some(i) = std::env::args().position(|a| a == "--tour") {
        let out = std::env::args().nth(i + 1).expect("--tour <frames_dir>");
        return data_rt.block_on(tour(state, &out));
    }

    let handlers: Vec<(EventStreamConfig, Arc<dyn EventStreamHandler<State>>)> = vec![
        (EventStreamConfig { types: vec![SceneGraphEventType::CursorMoved], ..Default::default() }, Arc::new(Move)),
        (EventStreamConfig { types: vec![SceneGraphEventType::CursorMoved], throttle: Some(60), ..Default::default() }, Arc::new(LiveQuery)),
        (EventStreamConfig {
            types: vec![SceneGraphEventType::MouseDown, SceneGraphEventType::MouseUp, SceneGraphEventType::KeyPress, SceneGraphEventType::WindowResize,
                SceneGraphEventType::RuntimeWake],
            ..Default::default()
        }, Arc::new(Input)),
    ];
    let window_rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let app = window_rt.block_on(AvengerApp::try_new(state, Arc::new(Builder), handlers))?;
    let options = WinitWgpuAvengerAppOptions::new(2.0).window_attributes(
        WindowAttributes::default()
            .with_title("Experiment 10 · Mosaic flights with avenger-selection")
            .with_resizable(true)
            .with_inner_size(LogicalSize::new(SIZE[0], SIZE[1])),
    );
    let (mut app, event_loop) = WinitWgpuAvengerApp::new_and_event_loop_with_options(app, options, window_rt);
    event_loop.run_app(&mut app)?;
    drop(data_rt);
    Ok(())
}

/// Headless check: the same gestures as the mouse makes them, through the
/// same press, motion and release, each frame saved as a PNG.
async fn snapshots(mut s: State, out: &str) -> Result<(), Error> {
    use avenger_common::canvas::CanvasDimensions;
    use avenger_wgpu::canvas::{Canvas, PngCanvas};
    std::fs::create_dir_all(out)?;
    let h = |i: usize, x: f32| [hist_origin(i)[0] + x, hist_origin(i)[1] + H / 2.0];
    let drag = |s: &mut State, path: &[[f32; 2]], shift: bool| {
        s.press(path[0], MouseButton::Left, shift);
        for p in &path[1..] {
            s.motion(*p);
        }
        s.release(*path.last().unwrap());
    };
    let dens = |q: [f64; 2]| [LEFT + q[0] as f32, TOP + q[1] as f32];
    let ser = |p: [f64; 2]| { let q = series_px(p); [LEFT + q[0], SERIES_TOP + q[1]] };
    let steps: Vec<(&str, Box<dyn Fn(&mut State)>)> = vec![
        ("0-start", Box::new(|_| {})),
        ("1-brush-delay", Box::new(move |s| drag(s, &[h(0, PLOTS[0].px(60.0, W)), h(0, 300.0), h(0, PLOTS[0].px(180.0, W))], false))),
        ("2-lasso", Box::new(move |s| {
            s.clear_all();
            let ring = the_lasso();
            let path: Vec<[f32; 2]> = ring.iter().map(|q| dens(*q)).collect();
            drag(s, &path, false);
        })),
        ("3-soft-distance", Box::new(move |s| {
            s.clear_all();
            s.soft = true;
            drag(s, &[h(2, PLOTS[2].px(1500.0, W)), h(2, PLOTS[2].px(2500.0, W))], false);
        })),
        ("4-line-brush", Box::new(move |s| {
            s.clear_all();
            s.soft = false;
            drag(s, &[ser([21.6, 45.0]), ser([22.6, 55.0])], false);
        })),
        ("5-timebox-and-brush", Box::new(move |s| {
            s.clear_all();
            s.timebox = true;
            drag(s, &[ser([6.0, 5.0]), ser([12.0, -5.0])], false);
            s.timebox = false;
            drag(s, &[h(1, PLOTS[1].px(6.0, W)), h(1, PLOTS[1].px(12.0, W))], false);
        })),
        ("6-click-clears", Box::new(move |s| {
            let p = h(1, 100.0);
            s.press(p, MouseButton::Left, false);
            s.release(p);
        })),
        // Every kind at once, soft: the degree beside keys planned only once
        // `degree()` combined with least/greatest, and the lasso's degree is a
        // table lookup (FINDINGS.md 36).
        ("7-soft-with-everything", Box::new(move |s| {
            s.clear_all();
            s.soft = true;
            let ring: Vec<[f32; 2]> = the_lasso().iter().map(|q| dens(*q)).collect();
            drag(s, &ring, false);
            drag(s, &[h(2, PLOTS[2].px(300.0, W)), h(2, PLOTS[2].px(1500.0, W))], false);
            drag(s, &[ser([15.0, 0.0]), ser([18.0, 30.0])], false);
        })),
        // The morph: a brush on distance, so the delay panel shows fewer
        // flights than all, a brush on the delay bars, D, and the donut with
        // the brush bent and the flights left filling each slice.
        ("8-donut", Box::new(move |s| {
            s.clear_all();
            s.soft = false;
            drag(s, &[h(2, PLOTS[2].px(1000.0, W)), h(2, PLOTS[2].px(2500.0, W))], false);
            drag(s, &[h(0, PLOTS[0].px(45.0, W)), h(0, PLOTS[0].px(125.0, W))], false);
            s.cursor = h(0, 300.0);
            s.key(&Key::Character('d'));
            while s.step(1.0 / 30.0) {}
        })),
        // A drag along the ring brushes the donut: read back into minutes.
        ("9-donut-drag", Box::new(move |s| {
            let path = ring_path(s, 0, -20.0, 15.0, 24);
            drag(s, &path, false);
        })),
        // Half way back, the panel ignores a press: it has no inverse there.
        ("10-half-way", Box::new(move |s| {
            s.key(&Key::Character('d'));
            s.step(MORPH_SECS * 0.25);
            let before = s.brushes[0];
            drag(s, &[h(0, 100.0), h(0, 400.0)], false);
            assert_eq!(s.brushes[0], before, "a press half way changed the brush");
        })),
        ("11-back-to-bars", Box::new(move |s| {
            while s.step(1.0 / 30.0) {}
        })),
    ];
    for (name, step) in steps {
        step(&mut s);
        s.refresh().await;
        let scene = s.scene()?;
        let mut canvas = PngCanvas::new(CanvasDimensions { size: [scene.width, scene.height], scale: 2.0 }, Default::default()).await?;
        canvas.set_scene(&scene)?;
        let path = format!("{out}/flights_live-{name}.png");
        canvas.render().await?.save(&path)?;
        let flights: f64 = s.hard[0].iter().map(|x| x.1).sum();
        println!("{name}: {:.0} ms, {} flights in the delay panel, brushes {:?}, lasso {} points, bands {:?}, donut {:?}{} -> {path}",
            s.last_ms, thousands(flights as usize), s.brushes, s.lasso.len(), s.bands.as_ref().map(|b| b.len()), s.donut,
            if s.soft_failed { ", THE SOFT LAYER FAILED" } else { "" });
    }
    Ok(())
}

/// The pointer as the tour draws it: a dot, ringed while the button is down.
fn pointer_mark(p: [f32; 2], pressed: bool) -> SceneMark {
    use avenger_scenegraph::marks::symbol::SceneSymbolMark;
    SceneSymbolMark { len: 1, x: vec![p[0]].into(), y: vec![p[1]].into(), size: (if pressed { 260.0 } else { 110.0 }).into(),
        fill: avenger_color::ColorOrGradient::Color(if pressed { [0.85, 0.2, 0.15, 0.55] } else { [0.15, 0.16, 0.18, 0.8] }).into(),
        stroke: avenger_color::ColorOrGradient::Color([1.0; 4]).into(), stroke_width: Some(2.0), interactive: false, ..Default::default() }.into()
}

/// A pointer path along the middle of panel `i`'s donut, from the place of
/// value `from` to that of `to`, in window pixels.
fn ring_path(s: &State, i: usize, from: f64, to: f64, n: usize) -> Vec<[f32; 2]> {
    let bins = s.bins(i, &s.all[i]);
    let stack = Layout::Stack { y: RING };
    let bend = Bend { width: W as f64, height: H as f64, t: 1.0 };
    let cs = Fitted::new(&bend, W as f64, H as f64);
    let (a, b) = (bins.position(from, stack), bins.position(to, stack));
    let o = hist_origin(i);
    (0..=n).map(|k| {
        let q = cs.project(&[a + (b - a) * k as f64 / n as f64, 0.5 * (RING[0] + RING[1])]).unwrap();
        [o[0] + q[0] as f32, o[1] + q[1] as f32]
    }).collect()
}

/// The recorder of `--tour`: a pointer moved through the window's own press,
/// motion and release at 30 frames a second, each frame saved as a PNG.
/// While dragging, the histograms are queried every third frame (10 Hz, about
/// as often as the window's throttled queries return) and on release.
struct Recorder {
    dir: String,
    frame: u32,
    canvas: avenger_wgpu::canvas::PngCanvas,
    at: [f32; 2],
}
const FPS: f32 = 30.0;
impl Recorder {
    async fn shot(&mut self, s: &mut State) -> Result<(), Error> {
        use avenger_wgpu::canvas::Canvas;
        s.step(1.0 / FPS as f64);
        let scene = s.scene()?;
        self.canvas.set_scene(&scene)?;
        self.canvas.render().await?.save(format!("{}/frame_{:05}.png", self.dir, self.frame))?;
        self.frame += 1;
        Ok(())
    }
    async fn hold(&mut self, s: &mut State, secs: f32) -> Result<(), Error> {
        for _ in 0..(secs * FPS) as u32 {
            self.shot(s).await?;
        }
        Ok(())
    }
    async fn say(&mut self, s: &mut State, caption: &str) {
        s.caption = Some(caption.into());
    }
    /// Along `path` over `secs`, eased; with the button down when `drag` is set.
    async fn trace(&mut self, s: &mut State, path: &[[f32; 2]], secs: f32, drag: bool) -> Result<(), Error> {
        let lens: Vec<f32> = path.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).collect();
        let total: f32 = lens.iter().sum::<f32>().max(1e-3);
        let n = (secs * FPS).max(1.0) as u32;
        for f in 1..=n {
            let t = f as f32 / n as f32;
            let e = t * t * (3.0 - 2.0 * t);
            let (mut d, mut i) = (e * total, 0);
            while i < lens.len() - 1 && d > lens[i] {
                d -= lens[i];
                i += 1;
            }
            let k = if lens[i] > 0.0 { (d / lens[i]).min(1.0) } else { 1.0 };
            self.at = [path[i][0] + k * (path[i + 1][0] - path[i][0]), path[i][1] + k * (path[i + 1][1] - path[i][1])];
            s.pointer = Some((self.at, drag));
            if drag && s.motion(self.at) && f % 3 == 0 {
                s.refresh().await;
            }
            self.shot(s).await?;
        }
        Ok(())
    }
    async fn glide(&mut self, s: &mut State, to: [f32; 2], secs: f32) -> Result<(), Error> {
        let from = self.at;
        self.trace(s, &[from, to], secs, false).await
    }
    async fn drag(&mut self, s: &mut State, path: &[[f32; 2]], secs: f32, shift: bool) -> Result<(), Error> {
        self.glide(s, path[0], 0.5).await?;
        s.press(path[0], MouseButton::Left, shift);
        s.pointer = Some((path[0], true));
        self.hold(s, 0.15).await?;
        self.trace(s, path, secs, true).await?;
        s.release(*path.last().unwrap());
        s.pointer = Some((self.at, false));
        s.refresh().await;
        Ok(())
    }
    async fn right_click(&mut self, s: &mut State, p: [f32; 2]) -> Result<(), Error> {
        self.glide(s, p, 0.5).await?;
        s.pointer = Some((p, true));
        if s.press(p, MouseButton::Right, false) {
            s.refresh().await;
        }
        self.hold(s, 0.2).await?;
        s.pointer = Some((p, false));
        Ok(())
    }
    async fn key(&mut self, s: &mut State, k: Key) {
        s.cursor = self.at;
        if s.key(&k) {
            s.refresh().await;
        }
    }
}

/// A scripted walk through every selection, as frames for ffmpeg (see the README).
async fn tour(mut s: State, dir: &str) -> Result<(), Error> {
    use avenger_common::canvas::CanvasDimensions;
    use avenger_wgpu::canvas::PngCanvas;
    std::fs::create_dir_all(dir)?;
    let canvas = PngCanvas::new(CanvasDimensions { size: SIZE, scale: 2.0 }, Default::default()).await?;
    let mut r = Recorder { dir: dir.into(), frame: 0, canvas, at: [SIZE[0] / 2.0, SIZE[1] / 2.0] };
    let h = |i: usize, v: f64| [hist_origin(i)[0] + PLOTS[i].px(v, W), hist_origin(i)[1] + H * 0.55];
    let dens = |q: [f64; 2]| [LEFT + q[0] as f32, TOP + q[1] as f32];
    let ser = |p: [f64; 2]| { let q = series_px(p); [LEFT + q[0], SERIES_TOP + q[1]] };
    let t0 = Instant::now();
    s.pointer = Some((r.at, false));

    r.say(&mut s, "10M flights: each histogram shows the flights the other panels' selections leave").await;
    r.hold(&mut s, 2.0).await?;
    r.say(&mut s, "Drag a histogram to brush it: arrival delay from 60 to 180 minutes").await;
    r.drag(&mut s, &[h(0, 60.0), h(0, 180.0)], 1.6, false).await?;
    r.hold(&mut s, 1.5).await?;
    r.say(&mut s, "A second brush, on departure time: both apply to the third panel").await;
    r.drag(&mut s, &[h(1, 17.0), h(1, 21.0)], 1.2, false).await?;
    r.hold(&mut s, 1.5).await?;
    r.say(&mut s, "A right click clears one panel's brush").await;
    r.right_click(&mut s, h(0, 100.0)).await?;
    r.hold(&mut s, 1.0).await?;
    r.right_click(&mut s, h(1, 10.0)).await?;
    r.hold(&mut s, 0.8).await?;

    r.say(&mut s, "S turns the soft brush on: light blue counts each flight by how near the brush it is").await;
    r.key(&mut s, Key::Character('s')).await;
    r.hold(&mut s, 0.8).await?;
    r.drag(&mut s, &[h(2, 1500.0), h(2, 2500.0)], 1.4, false).await?;
    r.hold(&mut s, 2.0).await?;
    r.say(&mut s, "Esc clears everything").await;
    r.key(&mut s, Key::Character('s')).await;
    r.key(&mut s, Key::Named(NamedKey::Escape)).await;
    r.hold(&mut s, 1.0).await?;

    r.say(&mut s, "Draw a lasso on the density: evening departures that arrive late").await;
    let mut ring: Vec<[f32; 2]> = the_lasso().iter().map(|q| dens(*q)).collect();
    ring.push(ring[0]);
    r.drag(&mut s, &ring, 3.0, false).await?;
    r.hold(&mut s, 2.0).await?;
    r.key(&mut s, Key::Named(NamedKey::Escape)).await;

    r.say(&mut s, "Drag across the lines for a line brush: the bands whose line crosses it").await;
    r.drag(&mut s, &[ser([20.5, 25.0]), ser([22.8, 70.0])], 1.5, false).await?;
    r.hold(&mut s, 2.0).await?;
    r.say(&mut s, "T, or Shift-drag, for a timebox: the bands whose line stays inside it").await;
    r.key(&mut s, Key::Character('t')).await;
    r.drag(&mut s, &[ser([6.0, 5.0]), ser([12.0, -5.0])], 1.5, false).await?;
    r.key(&mut s, Key::Character('t')).await;
    r.hold(&mut s, 1.5).await?;
    r.say(&mut s, "The timebox combines with a brush on departure time").await;
    r.drag(&mut s, &[h(1, 6.0), h(1, 12.0)], 1.2, false).await?;
    r.hold(&mut s, 2.0).await?;
    r.say(&mut s, "Esc clears everything").await;
    r.key(&mut s, Key::Named(NamedKey::Escape)).await;
    r.hold(&mut s, 1.5).await?;

    r.say(&mut s, "A brush on distance, then one on arrival delay").await;
    r.drag(&mut s, &[h(2, 1000.0), h(2, 2500.0)], 1.2, false).await?;
    r.drag(&mut s, &[h(0, 45.0), h(0, 125.0)], 1.2, false).await?;
    r.say(&mut s, "D over the panel: the bars stack by all flights and bend into a donut; the flights left fill each slice, and the brush bends along").await;
    r.hold(&mut s, 0.8).await?;
    r.key(&mut s, Key::Character('d')).await;
    r.hold(&mut s, MORPH_SECS as f32 + 1.0).await?;
    r.say(&mut s, "On the donut, drag along the ring to brush: read back into minutes, the other panels follow").await;
    let path = ring_path(&s, 0, -20.0, 15.0, 40);
    r.drag(&mut s, &path, 2.0, false).await?;
    r.hold(&mut s, 1.5).await?;
    r.say(&mut s, "D again: back into bars, with the brush drawn on the donut").await;
    r.key(&mut s, Key::Character('d')).await;
    r.hold(&mut s, MORPH_SECS as f32 + 1.2).await?;
    r.say(&mut s, "Esc clears everything").await;
    r.key(&mut s, Key::Named(NamedKey::Escape)).await;
    r.hold(&mut s, 1.0).await?;
    println!("wrote {} frames ({:.1} s of video) to {dir} in {:.0} s", r.frame, r.frame as f32 / FPS, t0.elapsed().as_secs_f64());
    Ok(())
}
