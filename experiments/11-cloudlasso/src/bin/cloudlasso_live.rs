//! Experiment 11 in a window: draw a lasso in the tilted view of the 500 m
//! window and CloudLasso keeps the largest dense region inside it, through
//! `avenger-selection` on all 4.2M points. Shift-drag (or right-drag) turns the
//! view, so what was taken can be seen from another side; the arrow keys turn
//! it too. C switches between CloudLasso and the plain lasso, + and − change
//! how dense a voxel must be, Esc clears.
//!
//! Usage: cargo run --release -p lidar-cloudlasso --bin cloudlasso_live -- <tile>
//!        cargo run --release -p lidar-cloudlasso --bin cloudlasso_live -- <tile> --snapshots <out_dir>
//!        cargo run --release -p lidar-cloudlasso --bin cloudlasso_live -- <tile> --tour <frames_dir>

use std::sync::Arc;
use std::time::Instant;

use arrow::array::{Array, BooleanArray};
use avenger_app::{app::{AvengerApp, SceneGraphBuilder}, error::AvengerAppError};
use avenger_color::ColorOrGradient;
use avenger_eventstream::{
    manager::EventStreamHandler,
    scene::{SceneGraphEvent, SceneGraphEventType},
    stream::{EventStreamConfig, UpdateStatus},
    window::{Key, MouseButton, NamedKey},
};
use avenger_geometry::rtree::SceneGraphRTree;
use avenger_guides::axis::opts::AxisOrientation;
use avenger_scenegraph::marks::{group::SceneGroup, mark::SceneMark, rect::SceneRectMark};
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_winit_wgpu::{WinitWgpuAvengerApp, WinitWgpuAvengerAppOptions};
use datafusion::logical_expr::col;
use datafusion::prelude::SessionContext;
use lidar_cloudlasso::*;
use lidar_common::{INK, MUTED};
use winit::{dpi::LogicalSize, window::WindowAttributes};

/// Points drawn per view; the selection itself runs on all of them.
const DRAWN: usize = 300_000;
const LEFT: f32 = 80.0;
const RIGHT: f32 = LEFT + P as f32 + 110.0;
const TOP: f32 = 140.0;
const SIDE_H: f32 = 200.0;
const SIDE_TOP: f32 = TOP + P as f32 + 60.0;
const SIZE: [f32; 2] = [RIGHT + P as f32 + 40.0, SIDE_TOP + SIDE_H + 50.0];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Gesture {
    Lasso,
    Turn { yaw: f64, elevation: f64 },
}
#[derive(Clone, Debug)]
struct Drag {
    gesture: Gesture,
    start: [f32; 2],
}

/// What a selection computes: the drawn points with their layer (0 outside
/// the lasso, 1 in the lasso only, 2 in CloudLasso's region), and the counts.
struct Results {
    points: Vec<[f64; 3]>,
    layer: Vec<u8>,
    n_lasso: usize,
    n_cloud: usize,
    voxels: usize,
    regions: usize,
    ms: f64,
}

#[derive(Clone)]
struct State {
    mem: SessionContext,
    rt: tokio::runtime::Handle,
    n: usize,
    hz: (f64, f64),
    size: [f32; 2],
    yaw: f64,
    elevation: f64,
    points: Arc<Vec<[f64; 3]>>,
    layer: Arc<Vec<u8>>,
    /// The lasso in the tilted view's pixels, and the view it was drawn in.
    ring: Vec<[f64; 2]>,
    ring_view: (f64, f64),
    cloud: bool,
    structure: f64,
    drag: Option<Drag>,
    counts: Option<(usize, usize, usize, usize)>,
    last_ms: f64,
    /// Only in `--tour`: the pointer drawn into the frame (pressed or not), and a caption.
    pointer: Option<([f32; 2], bool)>,
    caption: Option<String>,
}

fn in_tilted(p: [f32; 2]) -> Option<[f32; 2]> {
    let q = [p[0] - LEFT, p[1] - TOP];
    (q[0] >= 0.0 && q[1] >= 0.0 && q[0] <= P as f32 && q[1] <= P as f32).then_some(q)
}

impl State {
    /// True when the selection must be computed again.
    fn press(&mut self, p: [f32; 2], button: MouseButton, shift: bool) -> bool {
        let Some(q) = in_tilted(p) else { return false };
        let gesture = if button == MouseButton::Right || shift {
            Gesture::Turn { yaw: self.yaw, elevation: self.elevation }
        } else if button == MouseButton::Left {
            self.ring = vec![[q[0] as f64, q[1] as f64]];
            self.ring_view = (self.yaw, self.elevation);
            Gesture::Lasso
        } else {
            return false;
        };
        self.drag = Some(Drag { gesture, start: p });
        false
    }
    fn motion(&mut self, p: [f32; 2]) -> bool {
        let Some(d) = self.drag.clone() else { return false };
        match d.gesture {
            Gesture::Lasso => {
                let q = [(p[0] - LEFT).clamp(0.0, P as f32) as f64, (p[1] - TOP).clamp(0.0, P as f32) as f64];
                let last = *self.ring.last().unwrap();
                if (last[0] - q[0]).hypot(last[1] - q[1]) < 3.0 {
                    return false;
                }
                self.ring.push(q);
            }
            Gesture::Turn { yaw, elevation } => {
                self.yaw = (yaw - (p[0] - d.start[0]) as f64 * 0.4).rem_euclid(360.0);
                self.elevation = (elevation + (p[1] - d.start[1]) as f64 * 0.25).clamp(5.0, 89.0);
            }
        }
        true
    }
    fn release(&mut self, p: [f32; 2]) -> bool {
        self.motion(p);
        let Some(d) = self.drag.take() else { return false };
        match d.gesture {
            Gesture::Lasso if self.ring.len() >= 3 => true,
            Gesture::Lasso => {
                self.clear();
                false
            }
            Gesture::Turn { .. } => false,
        }
    }
    fn key(&mut self, k: &Key) -> (bool, bool) {
        // (redraw, compute again)
        let again = self.ring.len() >= 3;
        match k {
            Key::Named(NamedKey::Escape) => { self.clear(); (true, false) }
            Key::Character('c') | Key::Character('C') => { self.cloud = !self.cloud; (true, again) }
            Key::Character('+') | Key::Character('=') => { self.structure = (self.structure + 0.05).min(0.95); (true, again && self.cloud) }
            Key::Character('-') | Key::Character('_') => { self.structure = (self.structure - 0.05).max(0.05); (true, again && self.cloud) }
            Key::Named(NamedKey::ArrowLeft) => { self.yaw = (self.yaw + 15.0).rem_euclid(360.0); (true, false) }
            Key::Named(NamedKey::ArrowRight) => { self.yaw = (self.yaw - 15.0).rem_euclid(360.0); (true, false) }
            Key::Named(NamedKey::ArrowUp) => { self.elevation = (self.elevation + 5.0).min(89.0); (true, false) }
            Key::Named(NamedKey::ArrowDown) => { self.elevation = (self.elevation - 5.0).max(5.0); (true, false) }
            _ => (false, false),
        }
    }
    fn clear(&mut self) {
        self.ring.clear();
        self.counts = None;
        self.layer = Arc::new(vec![0; self.points.len()]);
    }

    fn job(&self) -> impl std::future::Future<Output = Result<Results, String>> + Send + 'static {
        let (mem, hz, n, ring, view) = (self.mem.clone(), self.hz, self.n, self.ring.clone(), self.ring_view);
        let structure = self.cloud.then_some(self.structure);
        async move { compute(mem, hz, n, ring, view, structure).await.map_err(|e| e.to_string()) }
    }
    async fn refresh(&mut self) {
        match self.rt.spawn(self.job()).await {
            Ok(Ok(r)) => {
                self.points = Arc::new(r.points);
                self.layer = Arc::new(r.layer);
                self.counts = Some((r.n_lasso, r.n_cloud, r.voxels, r.regions));
                self.last_ms = r.ms;
            }
            Ok(Err(e)) => eprintln!("selection failed: {e}"),
            Err(e) => eprintln!("selection task failed: {e}"),
        }
    }

    fn scene(&self) -> Result<SceneGraph, Error> {
        let size = [self.size[0].max(SIZE[0]), self.size[1].max(SIZE[1])];
        let background: SceneMark = SceneRectMark { len: 1, x: 0.0.into(), y: 0.0.into(), width: Some(size[0].into()), height: Some(size[1].into()),
            fill: ColorOrGradient::Color([1.0; 4]).into(), interactive: false, ..Default::default() }.into();
        let mut marks = vec![background,
            text("CloudLasso through avenger-selection, live", 24.0, 30.0, 20.0, INK),
            text(format!("{} points in a 500 m window · draw a lasso in the tilted view · Shift-drag or right-drag to turn it, or the arrow keys",
                thousands(self.n)), 24.0, 50.0, 11.0, MUTED),
            text(format!("C: {} · + and −: density {:.2} of the densest voxel · Esc clears · yaw {:.0}°, elevation {:.0}°",
                if self.cloud { "CloudLasso (press for the plain lasso)" } else { "plain lasso (press for CloudLasso)" },
                self.structure, self.yaw, self.elevation), 24.0, 66.0, 11.0, MUTED)];
        let what = match self.counts {
            Some((l, c, v, r)) if self.cloud => format!("the lasso takes {} points (orange) · CloudLasso keeps the largest of {r} dense regions: {v} voxels, {} points (blue) · {:.0} ms",
                thousands(l), thousands(c), self.last_ms),
            Some((l, _, _, _)) => format!("the lasso takes {} points, from the ground up (orange) · {:.0} ms", thousands(l), self.last_ms),
            None => "no selection".into(),
        };
        marks.push(text(what, 24.0, 84.0, 12.0, INK));

        // Three views of the drawn points, in layers.
        let (a, b) = lidar_common::tilt(WIN, self.hz, self.yaw, self.elevation, P);
        let mut views: [[Vec<[f32; 2]>; 3]; 3] = Default::default();
        for (p, l) in self.points.iter().zip(self.layer.iter()) {
            let u = a[0] + a[1] * p[0] + a[2] * p[1] + a[3] * p[2];
            let v = b[0] + b[1] * p[0] + b[2] * p[1] + b[3] * p[2];
            let l = *l as usize;
            views[0][l].push([u as f32, v as f32]);
            views[1][l].push([((p[0] - WIN[0]) / (WIN[1] - WIN[0]) * P) as f32, (P - (p[1] - WIN[2]) / (WIN[3] - WIN[2]) * P) as f32]);
            views[2][l].push([u as f32, SIDE_H - ((p[2] - self.hz.0) / (self.hz.1 - self.hz.0)) as f32 * SIDE_H]);
        }
        let pf = P as f32;
        let frame = |w: f32, h: f32| line(&[[0.0, 0.0], [w, 0.0], [w, h], [0.0, h], [0.0, 0.0]], [0.9, 0.91, 0.93, 1.0], 1.0);
        let mut tilted = vec![frame(pf, pf)];
        let same_view = (self.yaw, self.elevation) == self.ring_view;
        if self.ring.len() >= 2 && same_view {
            let closed = self.drag.as_ref().map_or(true, |d| d.gesture != Gesture::Lasso);
            let ring: Vec<[f32; 2]> = self.ring.iter().chain(closed.then(|| &self.ring[0])).map(|p| [p[0] as f32, p[1] as f32]).collect();
            tilted.push(line(&ring, RED, 2.0));
        }
        let caption = if self.ring.len() >= 3 && !same_view { "The tilted view (turned since the lasso: draw again to select here)" } else { "The tilted view: draw a lasso" };
        marks.push(panel([LEFT, TOP], &views[0], tilted, caption));
        marks.push(panel([RIGHT, TOP], &views[1], vec![
            axis([WIN[0], WIN[1]], (0.0, pf), AxisOrientation::Bottom, "easting (m)", pf, pf)?,
            axis([WIN[2], WIN[3]], (pf, 0.0), AxisOrientation::Left, "northing (m)", pf, pf)?,
        ], "From above"));
        marks.push(panel([LEFT, SIDE_TOP], &views[2], vec![
            axis([self.hz.0, self.hz.1], (SIDE_H, 0.0), AxisOrientation::Left, "height (m)", pf, SIDE_H)?,
        ], "From the side, across the tilted view"));
        for (i, (colour, label)) in [(GREY, "not in the lasso"), (LASSO, "in the lasso, left out by CloudLasso"), (BLUE, "CloudLasso: the largest dense region")]
            .into_iter().enumerate() {
            let y = SIDE_TOP + 40.0 + i as f32 * 24.0;
            marks.push(SceneRectMark { len: 1, x: RIGHT.into(), y: (y - 10.0).into(), width: Some(12.0.into()), height: Some(12.0.into()),
                fill: ColorOrGradient::Color(colour).into(), interactive: false, ..Default::default() }.into());
            marks.push(text(label, RIGHT + 20.0, y, 12.0, INK));
        }
        if let Some(c) = &self.caption {
            marks.push(text(c.clone(), 24.0, 108.0, 13.0, RED));
        }
        if let Some((p, pressed)) = self.pointer {
            use avenger_scenegraph::marks::symbol::SceneSymbolMark;
            marks.push(SceneSymbolMark { len: 1, x: vec![p[0]].into(), y: vec![p[1]].into(), size: (if pressed { 260.0 } else { 110.0 }).into(),
                fill: ColorOrGradient::Color(if pressed { [0.85, 0.2, 0.15, 0.55] } else { [0.15, 0.16, 0.18, 0.8] }).into(),
                stroke: ColorOrGradient::Color([1.0; 4]).into(), stroke_width: Some(2.0), interactive: false, ..Default::default() }.into());
        }
        Ok(SceneGraph { width: size[0], height: size[1], origin: [0.0; 2], marks: vec![SceneGroup { marks, ..Default::default() }.into()] })
    }
}

fn panel(origin: [f32; 2], layers: &[Vec<[f32; 2]>; 3], mut extra: Vec<SceneMark>, caption: &str) -> SceneMark {
    let mut marks = vec![text(caption, 0.0, -10.0, 12.0, INK)];
    for (pts, colour) in layers.iter().zip([GREY, LASSO, BLUE]) {
        marks.push(dots(pts, colour));
    }
    marks.append(&mut extra);
    SceneGroup { origin, marks, ..Default::default() }.into()
}

/// The lasso (and CloudLasso's region) over every point, and the points to draw.
async fn compute(mem: SessionContext, hz: (f64, f64), n: usize, ring: Vec<[f64; 2]>, view: (f64, f64), structure: Option<f64>)
    -> Result<Results, Error> {
    let t0 = Instant::now();
    let s = select(&mem, hz, view.0, view.1, &ring, structure).await?;
    let rows = mem.table("pts").await?
        .select(vec![col("x"), col("y"), col("z"), members().predicate(&s.drawn)?.alias("in_lasso"), members().predicate(&s.cloud)?.alias("in_cloud")])?
        .collect().await?;
    let step = (n / DRAWN).max(1);
    let (mut points, mut layer, mut n_lasso, mut n_cloud, mut i) = (Vec::new(), Vec::new(), 0, 0, 0usize);
    for bt in &rows {
        let (x, y, z) = (f64s(bt, "x"), f64s(bt, "y"), f64s(bt, "z"));
        let flag = |n: &str| bt.column_by_name(n).unwrap().as_any().downcast_ref::<BooleanArray>().unwrap().clone();
        let (l, c) = (flag("in_lasso"), flag("in_cloud"));
        for r in 0..bt.num_rows() {
            let inl = l.is_valid(r) && l.value(r);
            let inc = structure.is_some() && c.is_valid(r) && c.value(r);
            n_lasso += inl as usize;
            n_cloud += inc as usize;
            if i % step == 0 {
                points.push([x.value(r), y.value(r), z.value(r)]);
                layer.push(if inc { 2 } else if inl { 1 } else { 0 });
            }
            i += 1;
        }
    }
    Ok(Results { points, layer, n_lasso, n_cloud, voxels: s.voxels, regions: s.regions, ms: ms(t0) })
}

// ---------------------------------------------------------------------------
// The window

struct Builder;
#[async_trait::async_trait]
impl SceneGraphBuilder<State> for Builder {
    async fn build(&self, s: &mut State) -> Result<SceneGraph, AvengerAppError> {
        s.scene().map_err(|e| AvengerAppError::InternalError(e.to_string()))
    }
}
fn redraw() -> UpdateStatus {
    UpdateStatus { rerender: true, rebuild_geometry: false, ..Default::default() }
}
fn nothing() -> UpdateStatus {
    UpdateStatus { rerender: false, rebuild_geometry: false, ..Default::default() }
}

struct Input;
#[async_trait::async_trait]
impl EventStreamHandler<State> for Input {
    async fn handle(&self, event: &SceneGraphEvent, s: &mut State, _: &SceneGraphRTree) -> UpdateStatus {
        let (draw, again) = match event {
            SceneGraphEvent::MouseDown(e) => (true, s.press(e.position, e.button, e.modifiers.shift)),
            SceneGraphEvent::MouseUp(e) => (true, s.release(e.position)),
            SceneGraphEvent::KeyPress(e) => s.key(&e.key),
            SceneGraphEvent::WindowResize(e) => { s.size = e.size; (true, false) }
            _ => (false, false),
        };
        if again {
            s.refresh().await;
        }
        if draw { redraw() } else { nothing() }
    }
}
struct Move;
#[async_trait::async_trait]
impl EventStreamHandler<State> for Move {
    async fn handle(&self, event: &SceneGraphEvent, s: &mut State, _: &SceneGraphRTree) -> UpdateStatus {
        match event.position() {
            Some(p) if s.motion(p) => redraw(),
            _ => nothing(),
        }
    }
}

fn main() -> Result<(), Error> {
    let tile = std::env::args().nth(1).unwrap_or_else(|| "data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz".into());
    let data_rt = tokio::runtime::Runtime::new()?;
    let t0 = Instant::now();
    let (mem, batches, n, hz) = data_rt.block_on(load(&tile))?;
    let step = (n / DRAWN).max(1);
    let points: Vec<[f64; 3]> = batches.iter().flat_map(|b| {
        let (x, y, z) = (f64s(b, "x"), f64s(b, "y"), f64s(b, "z"));
        (0..b.num_rows()).map(move |r| [x.value(r), y.value(r), z.value(r)])
    }).step_by(step).collect();
    drop(batches);
    println!("{n} points in the window, {} drawn per view, ready in {:.0} ms", points.len(), ms(t0));
    let state = State {
        mem, rt: data_rt.handle().clone(), n, hz, size: SIZE, yaw: YAW, elevation: ELEVATION, layer: Arc::new(vec![0; points.len()]),
        points: Arc::new(points), ring: Vec::new(), ring_view: (YAW, ELEVATION), cloud: true, structure: STRUCTURE, drag: None, counts: None, last_ms: 0.0, pointer: None, caption: None,
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
        (EventStreamConfig { types: vec![SceneGraphEventType::CursorMoved], throttle: Some(16), ..Default::default() }, Arc::new(Move)),
        (EventStreamConfig {
            types: vec![SceneGraphEventType::MouseDown, SceneGraphEventType::MouseUp, SceneGraphEventType::KeyPress, SceneGraphEventType::WindowResize],
            ..Default::default()
        }, Arc::new(Input)),
    ];
    let window_rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let app = window_rt.block_on(AvengerApp::try_new(state, Arc::new(Builder), handlers))?;
    let options = WinitWgpuAvengerAppOptions::new(2.0).window_attributes(
        WindowAttributes::default()
            .with_title("Experiment 11 · CloudLasso with avenger-selection")
            .with_resizable(true)
            .with_inner_size(LogicalSize::new(SIZE[0], SIZE[1])),
    );
    let (mut app, event_loop) = WinitWgpuAvengerApp::new_and_event_loop_with_options(app, options, window_rt);
    event_loop.run_app(&mut app)?;
    drop(data_rt);
    Ok(())
}

/// Headless check: the figure's lasso drawn with the mouse's own press,
/// motion and release, then a turn, the plain lasso and a denser threshold.
async fn snapshots(mut s: State, out: &str) -> Result<(), Error> {
    use avenger_common::canvas::CanvasDimensions;
    use avenger_wgpu::canvas::{Canvas, PngCanvas};
    std::fs::create_dir_all(out)?;
    let at = |q: [f64; 2]| [LEFT + (q[0] * P) as f32, TOP + ((1.0 - q[1]) * P) as f32];
    let drag = |s: &mut State, path: &[[f32; 2]], shift: bool| -> bool {
        s.press(path[0], MouseButton::Left, shift);
        for p in &path[1..] {
            s.motion(*p);
        }
        s.release(*path.last().unwrap())
    };
    let steps: Vec<(&str, Box<dyn Fn(&mut State) -> bool>)> = vec![
        ("0-start", Box::new(|_| false)),
        ("1-lasso", Box::new(move |s| drag(s, &RING.iter().map(|q| at(*q)).collect::<Vec<_>>(), false))),
        ("2-turned", Box::new(move |s| { drag(s, &[at([0.5, 0.5]), at([0.3, 0.45])], true); false })),
        ("3-plain-lasso", Box::new(|s| s.key(&Key::Character('c')).1)),
        ("4-denser", Box::new(|s| { s.key(&Key::Character('c')); s.key(&Key::Character('+')); s.key(&Key::Character('+')).1 })),
        ("5-cleared", Box::new(|s| s.key(&Key::Named(NamedKey::Escape)).1)),
    ];
    for (name, step) in steps {
        if step(&mut s) {
            s.refresh().await;
        }
        let scene = s.scene()?;
        let mut canvas = PngCanvas::new(CanvasDimensions { size: [scene.width, scene.height], scale: 2.0 }, Default::default()).await?;
        canvas.set_scene(&scene)?;
        let path = format!("{out}/cloudlasso_live-{name}.png");
        canvas.render().await?.save(&path)?;
        println!("{name}: yaw {:.0}, elevation {:.0}, cloud {}, structure {:.2}, counts {:?}, {:.0} ms -> {path}",
            s.yaw, s.elevation, s.cloud, s.structure, s.counts, s.last_ms);
    }
    Ok(())
}

/// The recorder of `--tour`: a pointer moved through the window's own press,
/// motion and release at 30 frames a second, each frame saved as a PNG.
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
    fn say(&mut self, s: &mut State, caption: &str) {
        s.caption = Some(caption.into());
    }
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
            if drag {
                s.motion(self.at);
            }
            self.shot(s).await?;
        }
        Ok(())
    }
    async fn drag(&mut self, s: &mut State, path: &[[f32; 2]], secs: f32, shift: bool) -> Result<(), Error> {
        let from = self.at;
        self.trace(s, &[from, path[0]], 0.5, false).await?;
        s.press(path[0], MouseButton::Left, shift);
        s.pointer = Some((path[0], true));
        self.hold(s, 0.15).await?;
        self.trace(s, path, secs, true).await?;
        if s.release(*path.last().unwrap()) {
            s.refresh().await;
        }
        s.pointer = Some((self.at, false));
        Ok(())
    }
    async fn key(&mut self, s: &mut State, k: Key) {
        if s.key(&k).1 {
            s.refresh().await;
        }
    }
}

/// A scripted walk: a lasso, CloudLasso against the plain lasso, a denser
/// threshold, and the view turned to see what was taken. Frames for ffmpeg.
async fn tour(mut s: State, dir: &str) -> Result<(), Error> {
    use avenger_common::canvas::CanvasDimensions;
    use avenger_wgpu::canvas::PngCanvas;
    std::fs::create_dir_all(dir)?;
    let canvas = PngCanvas::new(CanvasDimensions { size: SIZE, scale: 2.0 }, Default::default()).await?;
    let mut r = Recorder { dir: dir.into(), frame: 0, canvas, at: [LEFT + P as f32 * 0.5, TOP + P as f32 * 0.9] };
    let at = |q: [f64; 2]| [LEFT + (q[0] * P) as f32, TOP + ((1.0 - q[1]) * P) as f32];
    let t0 = Instant::now();
    s.pointer = Some((r.at, false));

    r.say(&mut s, "4.2M points, tilted: draw a lasso around the block");
    r.hold(&mut s, 1.5).await?;
    let mut ring: Vec<[f32; 2]> = RING.iter().map(|q| at(*q)).collect();
    ring.push(ring[0]);
    r.drag(&mut s, &ring, 3.0, false).await?;
    r.say(&mut s, "Blue: CloudLasso keeps the largest dense region");
    r.hold(&mut s, 2.5).await?;
    r.say(&mut s, "C: the plain lasso takes everything behind it too");
    r.key(&mut s, Key::Character('c')).await;
    r.hold(&mut s, 2.0).await?;
    r.say(&mut s, "C again: back to CloudLasso");
    r.key(&mut s, Key::Character('c')).await;
    r.hold(&mut s, 1.5).await?;
    r.say(&mut s, "+ asks for denser voxels: a smaller region");
    for _ in 0..3 {
        r.key(&mut s, Key::Character('+')).await;
        r.hold(&mut s, 0.8).await?;
    }
    r.say(&mut s, "- and back");
    for _ in 0..3 {
        r.key(&mut s, Key::Character('-')).await;
    }
    r.hold(&mut s, 1.2).await?;
    r.say(&mut s, "Shift-drag turns the view: what was taken, from other sides");
    let c = at([0.5, 0.5]);
    r.drag(&mut s, &[c, [c[0] - 220.0, c[1]], [c[0] - 220.0, c[1] + 120.0]], 4.0, true).await?;
    r.hold(&mut s, 1.5).await?;
    let c2 = r.at;
    r.drag(&mut s, &[c2, [c2[0] + 440.0, c2[1] - 60.0]], 4.0, true).await?;
    r.hold(&mut s, 1.5).await?;
    r.say(&mut s, "A lasso drawn in this view selects in this view");
    let ring2: Vec<[f32; 2]> = [[0.35, 0.62], [0.62, 0.66], [0.66, 0.45], [0.4, 0.4], [0.35, 0.62]].iter().map(|q| at(*q)).collect();
    r.drag(&mut s, &ring2, 3.0, false).await?;
    r.hold(&mut s, 2.5).await?;
    r.say(&mut s, "Esc clears");
    r.key(&mut s, Key::Named(NamedKey::Escape)).await;
    r.hold(&mut s, 1.5).await?;
    println!("wrote {} frames ({:.1} s of video) to {dir} in {:.0} s", r.frame, r.frame as f32 / FPS, t0.elapsed().as_secs_f64());
    Ok(())
}
