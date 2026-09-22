//! Experiment 5, live: pick a view and a coordinate system, watch the
//! transition, and drag the plot to turn the 3D view.
//!
//!     cargo run --release -p lidar-coords --bin coords_live -- <tile.copc.laz> [--cell 5]
//!     cargo run --release -p lidar-coords --bin coords_live -- <tile.copc.laz> --snapshots <dir>
//!     cargo run --release -p lidar-coords --bin coords_live -- <tile.copc.laz> --record <dir>
//!     cargo run --release -p lidar-coords --bin coords_live -- <tile.copc.laz> --tour <dir>
//!
//! Keys: 1 2 3 view · c p d f h t (planar) s e w q n (spatial) · b morph style · space spin ·
//! arrows turn the 3D view.
//!
//! Animation frames come from the host's own wake-up scheduler: while a
//! transition or spin is running, each scene build asks for a wake-up 16 ms
//! later, and the wake-up event triggers the next build.

use std::sync::Arc;

use async_trait::async_trait;
use avenger_app::app::{AvengerApp, SceneBuild, SceneGraphBuilder};
use avenger_app::error::AvengerAppError;
use avenger_color::ColorOrGradient;
use avenger_common::canvas::CanvasDimensions;
use avenger_common::time::{Duration, Instant};
use avenger_eventstream::manager::EventStreamHandler;
use avenger_eventstream::runtime::{RuntimeHostCommand, RuntimeWakeKey};
use avenger_eventstream::scene::{SceneGraphEvent as Event, SceneGraphEventType as Type};
use avenger_eventstream::stream::{EventStreamConfig, UpdateStatus};
use avenger_eventstream::window::{Key, MouseButton, NamedKey};
use avenger_geo::ProjectionKind;
use avenger_geometry::rtree::SceneGraphRTree;
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::rect::SceneRectMark;
use avenger_scenegraph::marks::symbol::SceneSymbolMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_text::TextEngine;
use avenger_wgpu::canvas::{Canvas, CanvasConfig, PngCanvas};
use avenger_widgets::prelude::*;
use lidar_common::{las_context, INK, MUTED};
use lidar_coords::coords::{
    resolve, Bend, Blend, Cartesian, Cartesian3d, CoordinateSystem, Fisheye, Hyperbolic, Paired,
    Polar, Spatial, Twirl,
};
use lidar_coords::draw;
use lidar_coords::specs::{bars, load, map, stack, Classes, MapLayers, MapSetup};

const WAKE: &str = "coords-live";
const FRAME: Duration = Duration::from_millis(16);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Bars,
    Share,
    Map,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sys {
    Cartesian,
    Polar,
    Cartesian3d,
    Fisheye,
    Hyperbolic,
    Twirl,
    Conic,
    Plate,
    Winkel,
    EqualEarth,
    NaturalEarth,
}

impl Sys {
    const ALL: [(Sys, &'static str, &'static str); 11] = [
        (Sys::Cartesian, "cartesian", "cartesian (the default)"),
        (Sys::Polar, "polar", "polar"),
        (Sys::Cartesian3d, "c3d", "cartesian3d"),
        (Sys::Fisheye, "fisheye", "fisheye (follows the mouse)"),
        (
            Sys::Hyperbolic,
            "hyperbolic",
            "hyperbolic (follows the mouse)",
        ),
        (Sys::Twirl, "twirl", "twirl"),
        (Sys::Conic, "conic", "conic conformal"),
        (Sys::Plate, "plate", "equirectangular"),
        (Sys::Winkel, "winkel", "Winkel tripel, 150° off centre"),
        (
            Sys::EqualEarth,
            "equalearth",
            "Equal Earth, 170° off centre",
        ),
        (
            Sys::NaturalEarth,
            "naturalearth",
            "Natural Earth, near the pole",
        ),
    ];
    fn id(self) -> &'static str {
        Sys::ALL.iter().find(|s| s.0 == self).unwrap().1
    }
    fn label(self) -> &'static str {
        Sys::ALL.iter().find(|s| s.0 == self).unwrap().2
    }
    fn from_id(id: &str) -> Option<Sys> {
        Sys::ALL.iter().find(|s| s.1 == id).map(|s| s.0)
    }
    /// Planar systems take unit inputs; spatial ones take lon/lat, so there
    /// is nothing to interpolate between the two families.
    fn planar(self) -> bool {
        Sys::ALL.iter().position(|s| s.0 == self).unwrap() < 6
    }
    /// Lenses whose focus follows the pointer.
    fn lens(self) -> bool {
        matches!(self, Sys::Fisheye | Sys::Hyperbolic)
    }
}

struct Data {
    classes: Classes,
    setup: MapSetup,
    cell_m: f64,
}

#[derive(Clone)]
struct State {
    engine: TextEngine,
    widgets: WidgetRuntime,
    data: Arc<Data>,
    size: [f32; 2],
    view: View,
    sys: Sys,
    /// The running transition: the system it started from, and when.
    from: Sys,
    started: Option<Instant>,
    /// Pin the transition to a fixed t (headless snapshots).
    t_fixed: Option<f64>,
    duration_s: f64,
    bend: bool,
    spin: bool,
    yaw: f64,
    elevation: f64,
    drag: Option<([f32; 2], f64, f64)>,
    /// Focus of the fisheye and hyperbolic lenses, in unit space.
    focus: [f64; 2],
    message: String,
    wake_generation: u64,
    last_frame: Option<Instant>,
    frame_ms: f64,
    build_ms: f64,
}

impl State {
    fn plot(&self) -> Rect {
        let side = (self.size[0] - 440.0).min(self.size[1] - 190.0).max(300.0);
        Rect::new(70.0, 130.0, side, side)
    }

    fn animating(&self) -> bool {
        self.t_fixed.is_none()
            && (self.started.is_some() || (self.spin && self.sys == Sys::Cartesian3d))
    }

    /// The channels this view's specification sets under a system.
    fn channels(&self, sys: Sys) -> &'static [&'static str] {
        match (self.view, sys) {
            (View::Map, s) if !s.planar() => &["lon", "lat"],
            (View::Map, Sys::Cartesian3d) => &["x", "y", "z"],
            _ => &["x", "y"],
        }
    }

    fn system(&self, sys: Sys, w: f64, h: f64) -> Box<dyn CoordinateSystem> {
        let extent = self.data.setup.extent;
        match sys {
            Sys::Cartesian => Box::new(Cartesian {
                width: w,
                height: h,
            }),
            Sys::Polar => Box::new(Polar {
                width: w,
                height: h,
                inner: 0.0,
            }),
            Sys::Cartesian3d => Box::new(Cartesian3d {
                width: w,
                height: h,
                yaw: self.yaw,
                elevation: self.elevation,
                z_scale: if self.view == View::Map { 0.22 } else { 0.35 },
            }),
            // Lambert-93's parameters on a sphere.
            Sys::Conic => Box::new(Spatial::fit(
                "conic conformal",
                ProjectionKind::ConicConformal {
                    parallels: (44.0, 49.0),
                },
                [-3.0, 0.0, 0.0],
                extent,
                w,
                h,
            )),
            Sys::Plate => Box::new(Spatial::fit(
                "equirectangular",
                ProjectionKind::Equirectangular,
                [0.0; 3],
                extent,
                w,
                h,
            )),
            // Rotated so the tile (2.42°E, 48.9°N) lands far from the
            // projection centre, where these projections shear.
            Sys::Winkel => Box::new(Spatial::fit(
                "Winkel tripel",
                ProjectionKind::WinkelTripel,
                [147.6, 0.0, 0.0],
                extent,
                w,
                h,
            )),
            Sys::EqualEarth => Box::new(Spatial::fit(
                "Equal Earth",
                ProjectionKind::EqualEarth,
                [167.6, 0.0, 0.0],
                extent,
                w,
                h,
            )),
            Sys::NaturalEarth => Box::new(Spatial::fit(
                "Natural Earth",
                ProjectionKind::NaturalEarth1,
                [140.0, -33.0, 0.0],
                extent,
                w,
                h,
            )),
            Sys::Fisheye => Box::new(Fisheye {
                width: w,
                height: h,
                focus: self.focus,
                radius: 0.35,
                distortion: 4.0,
            }),
            Sys::Hyperbolic => Box::new(Hyperbolic {
                width: w,
                height: h,
                focus: self.focus,
                k: 2.2,
            }),
            Sys::Twirl => Box::new(Twirl {
                width: w,
                height: h,
                angle: 160.0,
            }),
        }
    }

    /// Switch system, checking the view's channels against it first.
    fn select(&mut self, sys: Sys, now: Instant) {
        if sys == self.sys {
            return;
        }
        let cs = self.system(sys, 100.0, 100.0);
        if let Err(e) = resolve(cs.as_ref(), self.channels(sys)) {
            self.message = format!("rejected: {e}");
            return;
        }
        // Same inputs (planar ↔ planar, spatial ↔ spatial) blend directly;
        // the map carries both encodings, so it can also cross between the
        // two families.
        let same_inputs = self.sys.planar() == sys.planar();
        let animate = (same_inputs || self.view == View::Map) && self.duration_s > 0.0;
        self.from = self.sys;
        self.sys = sys;
        self.started = animate.then_some(now);
        self.message = match resolve(cs.as_ref(), self.channels(sys)) {
            Ok(r) if r.iter().any(|c| c.is_err()) => {
                format!("{}: z left out, so z = 0", sys.label())
            }
            _ => format!("{} {:?} ok", sys.label(), self.channels(sys)),
        };
    }

    fn set_view(&mut self, view: View, now: Instant) {
        self.view = view;
        self.started = None;
        let cs = self.system(self.sys, 100.0, 100.0);
        if let Err(e) = resolve(cs.as_ref(), self.channels(self.sys)) {
            let from = self.sys;
            self.sys = Sys::Cartesian;
            self.from = Sys::Cartesian;
            self.message = format!("{e} · switched {} → cartesian", from.label());
            let _ = now;
        }
    }

    fn controls(&self) -> Vec<WidgetSpec> {
        let items = |v: &[(&str, &str)]| {
            v.iter()
                .map(|(id, l)| ChoiceItem::new(*id, *l))
                .collect::<Vec<_>>()
        };
        vec![
            RadioGroup::new(
                "view",
                items(&[("bars", "Bars"), ("share", "Share"), ("map", "Map")]),
                Some(
                    match self.view {
                        View::Bars => "bars",
                        View::Share => "share",
                        View::Map => "map",
                    }
                    .into(),
                ),
            )
            .label("View")
            .orientation(ChoiceOrientation::Horizontal)
            .into(),
            RadioGroup::new(
                "planar",
                items(&Sys::ALL[..6].iter().map(|s| (s.1, s.2)).collect::<Vec<_>>()),
                self.sys.planar().then(|| self.sys.id().into()),
            )
            .label("Planar systems · x, y")
            .into(),
            RadioGroup::new(
                "spatial",
                items(&Sys::ALL[6..].iter().map(|s| (s.1, s.2)).collect::<Vec<_>>()),
                (!self.sys.planar()).then(|| self.sys.id().into()),
            )
            .label("Spatial systems · lon, lat")
            .into(),
            RadioGroup::new(
                "morph",
                items(&[("bend", "bend"), ("blend", "blend")]),
                Some(if self.bend { "bend" } else { "blend" }.into()),
            )
            .label("Cartesian ↔ polar transition")
            .orientation(ChoiceOrientation::Horizontal)
            .into(),
            Slider::new(
                "duration",
                SliderDomain::continuous(0.0, 4.0).unwrap(),
                self.duration_s,
            )
            .value_label(format!("{:.1} s", self.duration_s))
            .semantic_name("Transition duration")
            .into(),
            Checkbox::new("spin", "Spin the 3D view", self.spin).into(),
        ]
    }

    fn apply(&mut self, events: Vec<WidgetEvent>, now: Instant) {
        for event in events {
            match (event.id.as_str(), event.action) {
                ("view", WidgetAction::SelectionChanged { item }) => {
                    let view = match item.as_str() {
                        "bars" => View::Bars,
                        "share" => View::Share,
                        _ => View::Map,
                    };
                    self.set_view(view, now);
                }
                ("planar" | "spatial", WidgetAction::SelectionChanged { item }) => {
                    if let Some(sys) = Sys::from_id(item.as_str()) {
                        self.select(sys, now);
                    }
                }
                ("morph", WidgetAction::SelectionChanged { item }) => {
                    self.bend = item.as_str() == "bend";
                }
                ("duration", WidgetAction::SliderChanged { value }) => self.duration_s = value,
                ("spin", WidgetAction::CheckedChanged { value }) => self.spin = value,
                _ => {}
            }
        }
    }

    /// The transition's progress, eased, or None when there is none.
    fn progress(&mut self, now: Instant) -> Option<f64> {
        let raw = match (self.t_fixed, self.started) {
            (Some(t), _) => t,
            (None, Some(start)) => {
                (now.duration_since(start).as_secs_f64() / self.duration_s.max(1e-3)).min(1.0)
            }
            _ => return None,
        };
        if raw >= 1.0 && self.t_fixed.is_none() {
            self.started = None;
            return None;
        }
        Some(raw * raw * (3.0 - 2.0 * raw))
    }
}

fn label(s: &str, x: f32, y: f32, size: f32, color: [f32; 4], bold: bool) -> SceneMark {
    let mut m = draw::title(&[(s, size, bold, color)], [x, y - size * 1.35]);
    if let SceneMark::Group(g) = &mut m {
        g.interactive = false;
    }
    m
}

fn build_scene(s: &mut State) -> Result<SceneBuild, String> {
    let now = Instant::now();
    if let Some(last) = s.last_frame.replace(now) {
        let dt = now.duration_since(last).as_secs_f64();
        s.frame_ms = 0.8 * s.frame_ms + 0.2 * dt * 1e3;
        if s.spin && s.sys == Sys::Cartesian3d && s.t_fixed.is_none() {
            s.yaw = (s.yaw + 25.0 * dt.min(0.1)) % 360.0;
        }
    }
    let [width, height] = s.size;
    let plot = s.plot();
    let (w, h) = (plot.width as f64, plot.height as f64);

    // ---- the chart, through the current (or transitioning) system ---------
    let t0 = std::time::Instant::now();
    let to = s.system(s.sys, w, h);
    let from = s.system(s.from, w, h);
    let t = s.progress(now);
    let pair = [s.from, s.sys];
    let cart_polar = pair.contains(&Sys::Cartesian) && pair.contains(&Sys::Polar);
    let bend;
    let blend;
    let paired;
    let crossing = s.from.planar() != s.sys.planar();
    let (cs, how): (&dyn CoordinateSystem, String) = match t {
        Some(t) if cart_polar && s.bend => {
            let t = if s.sys == Sys::Polar { t } else { 1.0 - t };
            bend = Bend {
                width: w,
                height: h,
                t,
            };
            (&bend, format!("bend t = {t:.2}"))
        }
        Some(t) if crossing => {
            let split = if s.from.planar() { 3 } else { 2 };
            paired = Paired {
                a: from.as_ref(),
                b: to.as_ref(),
                split,
                t,
            };
            (
                &paired,
                format!("paired {} → {}, t = {t:.2}", s.from.label(), s.sys.label()),
            )
        }
        Some(t) => {
            blend = Blend {
                a: from.as_ref(),
                b: to.as_ref(),
                t,
            };
            (
                &blend,
                format!("blend {} → {}, t = {t:.2}", s.from.label(), s.sys.label()),
            )
        }
        None => (to.as_ref(), s.sys.label().to_string()),
    };
    let data = s.data.clone();
    let point_size = ((data.cell_m * w / 1120.0).powi(2) * 1.1) as f32;
    let (chart, cost) = match s.view {
        View::Bars => bars(cs, &data.classes),
        View::Share => stack(cs, &data.classes),
        View::Map if t.is_some() && crossing => {
            // Each cell in both encodings; no grid, since the two systems'
            // grids are in different units.
            let st = &data.setup;
            let inputs = |sys: Sys| {
                if sys.planar() {
                    &st.in_unit
                } else {
                    &st.in_lonlat
                }
            };
            let layers = MapLayers::paired(inputs(s.from), inputs(s.sys));
            map(cs, &layers, [&[], &[]], point_size)
        }
        View::Map => {
            let st = &data.setup;
            if s.sys.planar() {
                map(cs, &st.in_unit, [&st.metres, &st.metres], point_size)
            } else {
                map(
                    cs,
                    &st.in_lonlat,
                    [&st.lon_ticks, &st.lat_ticks],
                    point_size,
                )
            }
        }
    };
    s.build_ms = 0.8 * s.build_ms + 0.2 * t0.elapsed().as_secs_f64() * 1e3;

    let mut marks: Vec<SceneMark> = vec![
        SceneRectMark {
            interactive: false,
            len: 1,
            x: 0.0.into(),
            y: 0.0.into(),
            width: Some(width.into()),
            height: Some(height.into()),
            fill: ColorOrGradient::Color([1.0; 4]).into(),
            ..Default::default()
        }
        .into(),
        label("One coordinate system for charts and maps", 32.0, 44.0, 22.0, INK, true),
        label(
            "Keys: 1 2 3 view · c p d f h t planar · s e w q n spatial · b bend/blend · space spin · drag turns 3D",
            32.0,
            68.0,
            13.0,
            MUTED,
            false,
        ),
        label(&how, plot.x, 110.0, 15.0, INK, true),
        draw::group([plot.x, plot.y], chart),
    ];
    if s.sys.lens() {
        let f = s.focus;
        marks.push(
            SceneSymbolMark {
                interactive: false,
                len: 1,
                x: (plot.x + f[0] as f32 * plot.width).into(),
                y: (plot.y + (1.0 - f[1] as f32) * plot.height).into(),
                size: 90.0.into(),
                fill: ColorOrGradient::Color([0.0; 4]).into(),
                stroke: ColorOrGradient::Color(INK).into(),
                stroke_width: Some(1.5),
                ..Default::default()
            }
            .into(),
        );
    }

    // ---- side panel ----------------------------------------------------------
    let x = width - 350.0;
    let specs = s.controls();
    let theme = WidgetTheme::light();
    let mut prepared = s
        .widgets
        .prepare(&specs, &theme, &s.engine)
        .map_err(|e| e.to_string())?;
    let mut y = 100.0;
    for spec in &specs {
        let hgt = prepared
            .metrics(spec.id().clone())
            .unwrap()
            .preferred
            .height;
        if spec.id().as_str() == "duration" {
            marks.push(label("Transition duration", x, y + 12.0, 13.0, INK, false));
            y += 22.0;
        }
        prepared
            .place(spec.id().clone(), Rect::new(x, y, 320.0, hgt), None)
            .map_err(|e| e.to_string())?;
        y += hgt + 16.0;
    }
    let lines = [
        s.message.clone(),
        String::new(),
        format!(
            "{} instances · {} vertices · drawn as {}",
            cost.instances,
            cost.vertices,
            match s.view {
                View::Map => "symbols",
                _ if cost.as_rects => "rect instances",
                _ => "paths",
            }
        ),
        format!(
            "chart build {:.1} ms · frame {:.0} ms",
            s.build_ms, s.frame_ms
        ),
    ];
    for (i, l) in lines.iter().enumerate() {
        let color = if i == 0 && l.starts_with("rejected") {
            [0.70, 0.10, 0.08, 1.0]
        } else if i == 0 {
            INK
        } else {
            MUTED
        };
        marks.push(label(l, x, y + 14.0 + i as f32 * 19.0, 12.5, color, false));
    }
    let frame = prepared.finish().map_err(|e| e.to_string())?;
    marks.push(frame.scene.clone().into());
    let update = s.widgets.install(frame).map_err(|e| e.to_string())?;

    let mut commands = update.status.commands;
    if s.animating() {
        s.wake_generation += 1;
        commands.push(RuntimeHostCommand::RequestWakeup {
            key: RuntimeWakeKey::new(WAKE, 0, "frame"),
            deadline: now + FRAME,
            generation: s.wake_generation,
        });
    }
    Ok(SceneBuild {
        scene_graph: SceneGraph {
            marks,
            width,
            height,
            origin: [0.0; 2],
        },
        commands,
        rebuild_geometry: update.status.rebuild_geometry,
    })
}

struct Builder;
#[async_trait]
impl SceneGraphBuilder<State> for Builder {
    async fn build(&self, s: &mut State) -> Result<SceneGraph, AvengerAppError> {
        self.build_with_effects(s).await.map(|b| b.scene_graph)
    }
    async fn build_with_effects(&self, s: &mut State) -> Result<SceneBuild, AvengerAppError> {
        build_scene(s).map_err(AvengerAppError::InternalError)
    }
}

struct Input;
#[async_trait]
impl EventStreamHandler<State> for Input {
    async fn handle(&self, event: &Event, s: &mut State, rtree: &SceneGraphRTree) -> UpdateStatus {
        let now = Instant::now();
        if let Event::RuntimeWake(w) = event {
            if w.key.namespace == WAKE {
                return UpdateStatus {
                    rerender: true,
                    ..Default::default()
                };
            }
        }
        let mut status = match s.widgets.handle(event, rtree, now) {
            Ok(update) => {
                let mut status = update.status;
                if !update.events.is_empty() {
                    status.rerender = true;
                }
                s.apply(update.events, now);
                status
            }
            Err(e) => {
                s.message = e.to_string();
                UpdateStatus {
                    rerender: true,
                    ..Default::default()
                }
            }
        };
        if status.consume {
            return status;
        }
        let plot = s.plot();
        let inside = |p: [f32; 2]| {
            p[0] >= plot.x
                && p[0] <= plot.x + plot.width
                && p[1] >= plot.y
                && p[1] <= plot.y + plot.height
        };
        match event {
            Event::KeyPress(e) => {
                let text = e.text.as_deref().unwrap_or("");
                let before = (s.view, s.sys, s.bend, s.spin, s.yaw, s.elevation);
                match (&e.key, text) {
                    (_, "1") => s.set_view(View::Bars, now),
                    (_, "2") => s.set_view(View::Share, now),
                    (_, "3") => s.set_view(View::Map, now),
                    (_, "c") => s.select(Sys::Cartesian, now),
                    (_, "p") => s.select(Sys::Polar, now),
                    (_, "d") => s.select(Sys::Cartesian3d, now),
                    (_, "s") => s.select(Sys::Conic, now),
                    (_, "e") => s.select(Sys::Plate, now),
                    (_, "f") => s.select(Sys::Fisheye, now),
                    (_, "h") => s.select(Sys::Hyperbolic, now),
                    (_, "t") => s.select(Sys::Twirl, now),
                    (_, "w") => s.select(Sys::Winkel, now),
                    (_, "q") => s.select(Sys::EqualEarth, now),
                    (_, "n") => s.select(Sys::NaturalEarth, now),
                    (_, "b") => s.bend = !s.bend,
                    (Key::Named(NamedKey::Space), _) => s.spin = !s.spin,
                    (Key::Named(NamedKey::ArrowLeft), _) => s.yaw -= 5.0,
                    (Key::Named(NamedKey::ArrowRight), _) => s.yaw += 5.0,
                    (Key::Named(NamedKey::ArrowUp), _) => {
                        s.elevation = (s.elevation + 5.0).min(90.0)
                    }
                    (Key::Named(NamedKey::ArrowDown), _) => {
                        s.elevation = (s.elevation - 5.0).max(5.0)
                    }
                    _ => {}
                }
                status.rerender = true;
                let _ = before;
            }
            Event::MouseDown(e) if e.button == MouseButton::Left && inside(e.position) => {
                s.drag = Some((e.position, s.yaw, s.elevation));
                status.consume = true;
                status.suppress_click = true;
                status
                    .commands
                    .push(RuntimeHostCommand::SetPointerCapture { captured: true });
            }
            Event::CursorMoved(e) if s.drag.is_none() && s.sys.lens() && inside(e.position) => {
                s.focus = [
                    ((e.position[0] - plot.x) / plot.width) as f64,
                    1.0 - ((e.position[1] - plot.y) / plot.height) as f64,
                ];
                status.rerender = true;
            }
            Event::CursorMoved(e) => {
                if let Some((start, yaw, elevation)) = s.drag {
                    s.yaw = yaw + (e.position[0] - start[0]) as f64 * 0.4;
                    s.elevation =
                        (elevation + (e.position[1] - start[1]) as f64 * 0.3).clamp(5.0, 90.0);
                    status.rerender = true;
                    status.consume = true;
                }
            }
            Event::MouseUp(e) if e.button == MouseButton::Left => {
                if s.drag.take().is_some() {
                    status.consume = true;
                    status
                        .commands
                        .push(RuntimeHostCommand::SetPointerCapture { captured: false });
                }
            }
            Event::WindowFocused(false) | Event::PointerCaptureLost => s.drag = None,
            Event::WindowResize(e) => {
                s.size = e.size;
                status.rerender = true;
                status.rebuild_geometry = true;
            }
            Event::CanvasResize(e) => {
                s.size = e.size;
                status.rerender = true;
                status.rebuild_geometry = true;
            }
            _ => {}
        }
        status
    }
}

/// View, from, to, pinned t, bend, file name.
type Snapshot = (View, Sys, Sys, Option<f64>, bool, &'static str);

/// Render a few states headlessly, including mid-transition frames, to check
/// what the window shows without screenshotting it.
async fn snapshots(mut s: State, dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let steps: [Snapshot; 14] = [
        (
            View::Share,
            Sys::Cartesian,
            Sys::Cartesian,
            None,
            true,
            "live_0_share",
        ),
        (
            View::Share,
            Sys::Cartesian,
            Sys::Polar,
            Some(0.5),
            true,
            "live_1_share_bend",
        ),
        (
            View::Share,
            Sys::Cartesian,
            Sys::Polar,
            Some(0.5),
            false,
            "live_2_share_blend",
        ),
        (
            View::Bars,
            Sys::Cartesian,
            Sys::Cartesian3d,
            Some(0.5),
            true,
            "live_3_bars_to_3d",
        ),
        (
            View::Map,
            Sys::Cartesian3d,
            Sys::Cartesian3d,
            None,
            true,
            "live_4_map_3d",
        ),
        (
            View::Map,
            Sys::Conic,
            Sys::Conic,
            None,
            true,
            "live_5_map_conic",
        ),
        (
            View::Map,
            Sys::Cartesian,
            Sys::Conic,
            Some(0.5),
            true,
            "live_6_map_cart_to_conic",
        ),
        (
            View::Map,
            Sys::Conic,
            Sys::Plate,
            Some(0.5),
            true,
            "live_7_map_conic_to_plate",
        ),
        (
            View::Map,
            Sys::Winkel,
            Sys::Winkel,
            None,
            true,
            "live_8_map_winkel",
        ),
        (
            View::Map,
            Sys::EqualEarth,
            Sys::EqualEarth,
            None,
            true,
            "live_9_map_equalearth",
        ),
        (
            View::Map,
            Sys::NaturalEarth,
            Sys::NaturalEarth,
            None,
            true,
            "live_10_map_naturalearth",
        ),
        (
            View::Map,
            Sys::Fisheye,
            Sys::Fisheye,
            None,
            true,
            "live_11_map_fisheye",
        ),
        (
            View::Share,
            Sys::Hyperbolic,
            Sys::Hyperbolic,
            None,
            true,
            "live_12_share_hyperbolic",
        ),
        (
            View::Bars,
            Sys::Twirl,
            Sys::Twirl,
            None,
            true,
            "live_13_bars_twirl",
        ),
    ];
    let mut canvas = PngCanvas::new(
        CanvasDimensions {
            size: s.size,
            scale: 2.0,
        },
        CanvasConfig {
            text_engine: Some(s.engine.clone()),
            ..Default::default()
        },
    )
    .await?;
    for (view, from, sys, t, bend, name) in steps {
        s.view = view;
        s.from = from;
        s.sys = sys;
        s.bend = bend;
        s.t_fixed = t;
        s.message = format!("snapshot {name}");
        let scene = build_scene(&mut s)?.scene_graph;
        canvas.set_scene(&scene)?;
        let path = format!("{dir}/{name}.png");
        canvas.render().await?.save(&path)?;
        println!("  {path}");
    }
    Ok(())
}

/// Where the fisheye goes in `--record`: through the map, into each corner
/// and along every edge, then back to the centre (unit plot coordinates).
const TOUR: [[f64; 2]; 16] = [
    [0.50, 0.50],
    [0.32, 0.64],
    [0.18, 0.82],
    [0.04, 0.96],
    [0.50, 0.99],
    [0.96, 0.96],
    [0.99, 0.50],
    [0.96, 0.04],
    [0.50, 0.01],
    [0.04, 0.04],
    [0.01, 0.50],
    [0.30, 0.30],
    [0.62, 0.40],
    [0.75, 0.70],
    [0.55, 0.62],
    [0.50, 0.50],
];

/// Render the fisheye tour headlessly, one PNG per frame, at a constant
/// pointer speed. Encode with ffmpeg (see the README).
async fn record(
    mut s: State,
    dir: &str,
    fps: f64,
    seconds: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    s.view = View::Map;
    s.sys = Sys::Fisheye;
    s.from = Sys::Fisheye;
    let lengths: Vec<f64> = TOUR
        .windows(2)
        .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
        .collect();
    let total: f64 = lengths.iter().sum();
    let mut canvas = PngCanvas::new(
        CanvasDimensions {
            size: s.size,
            scale: 1.0,
        },
        CanvasConfig {
            text_engine: Some(s.engine.clone()),
            ..Default::default()
        },
    )
    .await?;
    let frames = (fps * seconds) as usize;
    let started = std::time::Instant::now();
    for frame in 0..frames {
        // Ease in and out over the whole tour, constant speed per metre.
        let u = frame as f64 / (frames - 1) as f64;
        let mut d = total * u * u * (3.0 - 2.0 * u);
        let mut i = 0;
        while i < lengths.len() - 1 && d > lengths[i] {
            d -= lengths[i];
            i += 1;
        }
        let k = (d / lengths[i]).clamp(0.0, 1.0);
        let (a, b) = (TOUR[i], TOUR[i + 1]);
        s.focus = [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k];
        s.message = "fisheye · focus follows the pointer".into();
        let scene = build_scene(&mut s)?.scene_graph;
        canvas.set_scene(&scene)?;
        canvas
            .render()
            .await?
            .save(format!("{dir}/frame_{frame:05}.png"))?;
    }
    println!(
        "wrote {frames} frames to {dir} in {:.1?}",
        started.elapsed()
    );
    Ok(())
}

/// One shot of `--tour`: cut to a view, or animate to a system; then hold.
enum Shot {
    Cut(View, Sys, &'static str),
    Go(Sys, &'static str),
}

const SHOTS: [Shot; 17] = [
    Shot::Cut(
        View::Share,
        Sys::Cartesian,
        "one stacked bar in cartesian, the default",
    ),
    Shot::Go(Sys::Polar, "polar: the bar bends into a donut"),
    Shot::Go(Sys::Hyperbolic, "hyperbolic lens"),
    Shot::Go(Sys::Twirl, "twirl"),
    Shot::Go(Sys::Cartesian, "back to cartesian"),
    Shot::Cut(View::Bars, Sys::Cartesian, "points per class"),
    Shot::Go(Sys::Polar, "polar: a rose chart"),
    Shot::Go(
        Sys::Cartesian3d,
        "cartesian3d: no z, so the bars lie at z = 0",
    ),
    Shot::Cut(View::Map, Sys::Cartesian3d, "the tile, z ← surface height"),
    Shot::Go(
        Sys::Cartesian,
        "cartesian: Lambert-93 metres, no projection",
    ),
    Shot::Go(Sys::Fisheye, "fisheye"),
    Shot::Go(
        Sys::Conic,
        "conic conformal: lon/lat, the grid is the graticule",
    ),
    Shot::Go(Sys::Plate, "equirectangular: stretched east–west at 49°N"),
    Shot::Go(Sys::Winkel, "Winkel tripel, tile 150° off centre"),
    Shot::Go(Sys::EqualEarth, "Equal Earth, tile 170° off centre"),
    Shot::Go(Sys::NaturalEarth, "Natural Earth, tile near the pole"),
    Shot::Go(Sys::Cartesian, "back to cartesian"),
];

/// Render the scripted tour headlessly: each `Go` is an animated transition
/// of `move_s`, each shot then holds for `hold_s`. 3D shots spin while
/// holding, and the fisheye circles through the map.
async fn tour(mut s: State, dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (fps, move_s, hold_s) = (30.0, 1.1, 1.3);
    std::fs::create_dir_all(dir)?;
    let mut canvas = PngCanvas::new(
        CanvasDimensions {
            size: s.size,
            scale: 1.0,
        },
        CanvasConfig {
            text_engine: Some(s.engine.clone()),
            ..Default::default()
        },
    )
    .await?;
    let mut frame = 0usize;
    let started = std::time::Instant::now();
    s.yaw = -30.0;
    for shot in &SHOTS {
        let (sys, caption, animate) = match *shot {
            Shot::Cut(view, sys, caption) => {
                s.view = view;
                (sys, caption, false)
            }
            Shot::Go(sys, caption) => (sys, caption, true),
        };
        s.message = caption.to_string();
        let moves = if animate { (fps * move_s) as usize } else { 0 };
        let holds = (fps * hold_s) as usize;
        s.from = if animate { s.sys } else { sys };
        s.sys = sys;
        for i in 0..moves + holds {
            if i < moves {
                s.t_fixed = Some(i as f64 / moves as f64);
            } else {
                s.t_fixed = None;
                s.started = None;
                s.from = s.sys;
                let h = (i - moves) as f64 / fps;
                if s.sys == Sys::Cartesian3d {
                    s.yaw += 40.0 / fps;
                }
                if s.sys == Sys::Fisheye {
                    let a = h / hold_s * std::f64::consts::TAU;
                    s.focus = [0.5 + 0.28 * a.sin(), 0.5 - 0.28 * (1.0 - a.cos())];
                }
            }
            if s.sys != Sys::Fisheye {
                s.focus = [0.5, 0.5];
            }
            let scene = build_scene(&mut s)?.scene_graph;
            canvas.set_scene(&scene)?;
            canvas
                .render()
                .await?
                .save(format!("{dir}/frame_{frame:05}.png"))?;
            frame += 1;
        }
    }
    println!("wrote {frame} frames to {dir} in {:.1?}", started.elapsed());
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let tile = args
        .get(1)
        .expect("usage: coords_live <tile.copc.laz> [--cell m] [--snapshots dir]");
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let cell_m: f64 = flag("--cell").map_or(5.0, |v| v.parse().expect("--cell metres"));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let t = std::time::Instant::now();
    let ctx = las_context();
    let (classes, cells, _) = runtime.block_on(load(&ctx, tile, cell_m))?;
    let setup = MapSetup::new(&cells);
    println!(
        "loaded {} cells of {cell_m} m in {:.1?}",
        cells.len(),
        t.elapsed()
    );

    let engine = avenger_text::default_text_engine();
    let state = State {
        engine: engine.clone(),
        widgets: WidgetRuntime::new().with_focus_boundary(FocusBoundary::Cycle),
        data: Arc::new(Data {
            classes,
            setup,
            cell_m,
        }),
        size: [1240.0, 900.0],
        view: View::Share,
        sys: Sys::Cartesian,
        from: Sys::Cartesian,
        started: None,
        t_fixed: None,
        duration_s: 1.6,
        bend: true,
        spin: false,
        yaw: -30.0,
        elevation: 35.0,
        drag: None,
        focus: [0.5, 0.5],
        message: "Pick a coordinate system, or press c p d s e.".into(),
        wake_generation: 0,
        last_frame: None,
        frame_ms: 16.0,
        build_ms: 0.0,
    };

    if let Some(dir) = flag("--snapshots") {
        return runtime.block_on(snapshots(state, dir));
    }
    if let Some(dir) = flag("--tour") {
        return runtime.block_on(tour(state, dir));
    }
    if let Some(dir) = flag("--record") {
        return runtime.block_on(record(state, dir, 30.0, 14.0));
    }

    let app = runtime.block_on(AvengerApp::try_new_with_text_engine(
        state,
        Arc::new(Builder),
        vec![(
            EventStreamConfig {
                types: vec![
                    Type::MouseDown,
                    Type::MouseUp,
                    Type::CursorMoved,
                    Type::MarkMouseLeave,
                    Type::KeyPress,
                    Type::KeyRelease,
                    Type::RuntimeWake,
                    Type::FocusEntered,
                    Type::PointerCaptureLost,
                    Type::WindowFocused,
                    Type::WindowCloseRequested,
                    Type::WindowResize,
                    Type::CanvasResize,
                ],
                ..Default::default()
            },
            Arc::new(Input) as Arc<dyn EventStreamHandler<State>>,
        )],
        engine.clone(),
    ))?;
    let options = avenger_winit_wgpu::WinitWgpuAvengerAppOptions::new(2.0)
        .window_attributes(
            winit::window::WindowAttributes::default()
                .with_title("Experiment 5 · one coordinate system")
                .with_resizable(true)
                .with_inner_size(winit::dpi::LogicalSize::new(1240.0, 900.0))
                .with_min_inner_size(winit::dpi::LogicalSize::new(900.0, 700.0)),
        )
        .canvas_config(CanvasConfig {
            text_engine: Some(engine),
            ..Default::default()
        });
    let (mut host, event_loop) =
        avenger_winit_wgpu::WinitWgpuAvengerApp::try_new_and_event_loop_with_options(
            app, options, runtime,
        )?;
    event_loop.run_app(&mut host)?;
    if let Some(error) = host.take_fatal_error() {
        return Err(error.into());
    }
    Ok(())
}
