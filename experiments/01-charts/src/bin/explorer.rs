//! Interactive point cloud explorer: drag to pan, scroll to zoom.
//!
//! The tile is loaded once into an in-memory DataFusion table (via
//! sedona-pointcloud), plus an overview pyramid of 1–16 m cells. Dragging and
//! zooming only move the view (GPU-side scale adjustments). Shortly after the
//! view settles, the matching overview level or the raw points for the
//! visible window are queried and swapped in.
//!
//! Usage: cargo run --release -p lidar-charts --bin explorer -- <tile.copc.laz>
//!        cargo run --release -p lidar-charts --bin explorer -- <tile.copc.laz> --snapshots <out_dir>

use lidar_common::{las_context, CLASSES, INK, MUTED, OTHER};
use std::sync::Arc;
use std::time::Instant as StdInstant;

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch};
use arrow::compute::concat;
use arrow::datatypes::{Float64Type, UInt8Type};
use avenger_app::{
    app::{AvengerApp, SceneGraphBuilder},
    error::AvengerAppError,
};
use avenger_color::ColorOrGradient;
use avenger_common::{cursor::CursorStyle, types::SymbolShape, value::ScalarOrArray};
use avenger_eventstream::{
    manager::EventStreamHandler,
    scene::{SceneGraphEvent, SceneGraphEventType},
    stream::{DebounceConfig, EventStreamConfig, EventStreamFilter, UpdateStatus},
    window::{MouseButton, MouseScrollDelta},
};
use avenger_geometry::rtree::SceneGraphRTree;
use lidar_common::make_numeric_axis_marks;
use avenger_guides::{
    axis::opts::{AxisConfig, AxisOrientation},
    legend::symbol::{make_symbol_legend, SymbolLegendConfig},
};
use avenger_scales::scales::{linear::LinearScale, ConfiguredScale};
use avenger_scenegraph::{
    marks::{
        group::{Clip, SceneGroup},
        mark::SceneMark,
        symbol::SceneSymbolMark,
        text::SceneTextMark,
    },
    scene_graph::SceneGraph,
};
use avenger_text::types::{FontWeight, FontWeightNameSpec, TextAlign, TextBaseline};
use avenger_winit_wgpu::{WinitWgpuAvengerApp, WinitWgpuAvengerAppOptions};
use datafusion::prelude::SessionContext;
use winit::{dpi::LogicalSize, window::WindowAttributes};

/// Overview cell sizes in metres, precomputed at startup.
const LEVELS: [u32; 5] = [1, 2, 4, 8, 16];
/// Switch to raw points when one metre spans at least this many pixels.
const RAW_PX_PER_M: f32 = 3.0;
/// Upper bound on raw points per reload.
const POINT_BUDGET: usize = 3_000_000;
/// Extra area loaded around the visible window, as a fraction of its size.
const MARGIN: f32 = 0.2;

const PLOT: &str = "plot";
const MARGIN_LEFT: f32 = 80.0;
const MARGIN_TOP: f32 = 56.0;
const MARGIN_RIGHT: f32 = 180.0;
const MARGIN_BOTTOM: f32 = 76.0;

/// Data loaded for one view; positions are pre-scaled with `base_x`/`base_y`.
#[derive(Clone)]
struct Layer {
    base_x: ConfiguredScale,
    base_y: ConfiguredScale,
    x: ScalarOrArray<f32>,
    y: ScalarOrArray<f32>,
    fill: ScalarOrArray<ColorOrGradient>,
    size: f32,
    /// Metres per pixel when the layer was loaded; symbols grow with zoom until the next reload.
    m_per_px: f32,
    len: usize,
    description: String,
}

#[derive(Clone)]
struct PanAnchor {
    pointer: [f32; 2],
    center: [f32; 2],
}

#[derive(Clone)]
struct State {
    ctx: SessionContext,
    data_rt: tokio::runtime::Handle,
    tile_name: String,
    total_points: usize,
    /// Plot size in pixels.
    width: f32,
    height: f32,
    /// View center in tile-relative metres and metres per pixel (equal on both axes).
    center: [f32; 2],
    m_per_px: f32,
    pan: Option<PanAnchor>,
    layer: Layer,
    legend: SceneMark,
}

impl State {
    fn x_domain(&self) -> (f32, f32) {
        let half = self.width / 2.0 * self.m_per_px;
        (self.center[0] - half, self.center[0] + half)
    }
    fn y_domain(&self) -> (f32, f32) {
        let half = self.height / 2.0 * self.m_per_px;
        (self.center[1] - half, self.center[1] + half)
    }
    fn x_scale(&self) -> ConfiguredScale {
        LinearScale::configured(self.x_domain(), (0.0, self.width))
    }
    fn y_scale(&self) -> ConfiguredScale {
        LinearScale::configured(self.y_domain(), (self.height, 0.0))
    }
    /// Pointer position relative to the plot, or None outside it.
    fn plot_pointer(&self, position: [f32; 2], rtree: &SceneGraphRTree) -> Option<[f32; 2]> {
        let origin = rtree.named_group_origin(PLOT)?;
        let p = [position[0] - origin[0], position[1] - origin[1]];
        (p[0] >= 0.0 && p[0] <= self.width && p[1] >= 0.0 && p[1] <= self.height).then_some(p)
    }
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

fn column(batches: &[RecordBatch], i: usize) -> ArrayRef {
    let parts: Vec<&dyn Array> = batches.iter().map(|b| b.column(i).as_ref()).collect();
    concat(&parts).unwrap()
}

async fn run_sql(ctx: &SessionContext, sql: &str) -> datafusion::error::Result<Vec<RecordBatch>> {
    ctx.sql(sql).await?.collect().await
}

/// Load the tile into memory and build the overview pyramid.
async fn prepare(ctx: &SessionContext, tile: &str) -> datafusion::error::Result<(usize, [f64; 2])> {
    run_sql(ctx, "SET las.geometry_encoding = 'plain'").await?;
    let t = StdInstant::now();
    let b = run_sql(
        ctx,
        &format!("SELECT min(x), min(y), count(*) FROM '{tile}'"),
    )
    .await?;
    let origin = [
        (b[0].column(0).as_primitive::<Float64Type>().value(0) / 1000.0).floor() * 1000.0,
        (b[0].column(1).as_primitive::<Float64Type>().value(0) / 1000.0).floor() * 1000.0,
    ];
    let total = b[0]
        .column(2)
        .as_primitive::<arrow::datatypes::Int64Type>()
        .value(0) as usize;
    run_sql(
        ctx,
        &format!(
            "CREATE TABLE pts AS
             SELECT CAST(x - {ox} AS FLOAT) AS x, CAST(y - {oy} AS FLOAT) AS y,
                    CAST(z AS FLOAT) AS z, classification
             FROM '{tile}'",
            ox = origin[0],
            oy = origin[1]
        ),
    )
    .await?;
    println!("loaded {total} points into memory in {:.2?}", t.elapsed());

    let t = StdInstant::now();
    run_sql(
        ctx,
        "CREATE TABLE cells_1 AS
         SELECT CAST(gx + 0.5 AS FLOAT) AS cx, CAST(gy + 0.5 AS FLOAT) AS cy,
                max(z) AS z, first_value(classification ORDER BY z DESC) AS class
         FROM (SELECT floor(x) AS gx, floor(y) AS gy, z, classification FROM pts)
         GROUP BY gx, gy",
    )
    .await?;
    for g in &LEVELS[1..] {
        run_sql(
            ctx,
            &format!(
                "CREATE TABLE cells_{g} AS
                 SELECT CAST(gx * {g} + {h} AS FLOAT) AS cx, CAST(gy * {g} + {h} AS FLOAT) AS cy,
                        max(z) AS z, first_value(class ORDER BY z DESC) AS class
                 FROM (SELECT floor(cx / {g}) AS gx, floor(cy / {g}) AS gy, z, class FROM cells_1)
                 GROUP BY gx, gy",
                h = *g as f32 / 2.0
            ),
        )
        .await?;
    }
    println!("built overview levels {LEVELS:?} m in {:.2?}", t.elapsed());
    Ok((total, origin))
}

fn class_colors(class: &ArrayRef) -> Vec<ColorOrGradient> {
    class
        .as_primitive::<UInt8Type>()
        .values()
        .iter()
        .map(|c| {
            let rgba = CLASSES
                .iter()
                .find(|(k, _, _)| k == c)
                .map(|(_, _, rgba)| *rgba);
            ColorOrGradient::Color(rgba.unwrap_or(OTHER))
        })
        .collect()
}

/// Query the data for the current view and pre-scale it.
async fn load_layer(
    ctx: SessionContext,
    x_domain: (f32, f32),
    y_domain: (f32, f32),
    width: f32,
    height: f32,
) -> datafusion::error::Result<Layer> {
    let t = StdInstant::now();
    let px_per_m = width / (x_domain.1 - x_domain.0);
    let (mx, my) = (
        (x_domain.1 - x_domain.0) * MARGIN,
        (y_domain.1 - y_domain.0) * MARGIN,
    );
    let (x0, x1, y0, y1) = (
        x_domain.0 - mx,
        x_domain.1 + mx,
        y_domain.0 - my,
        y_domain.1 + my,
    );

    let (sql, size, what) = if px_per_m >= RAW_PX_PER_M {
        (
            format!(
                "SELECT x, y, classification FROM pts
                 WHERE x >= {x0} AND x < {x1} AND y >= {y0} AND y < {y1}
                 ORDER BY z LIMIT {POINT_BUDGET}"
            ),
            (0.3 * px_per_m).powi(2).clamp(1.5, 36.0),
            "raw points".to_string(),
        )
    } else {
        let g = LEVELS
            .iter()
            .copied()
            .find(|g| *g as f32 * px_per_m >= 1.2)
            .unwrap_or(*LEVELS.last().unwrap());
        (
            format!(
                "SELECT cx, cy, class FROM cells_{g}
                 WHERE cx >= {x0} AND cx < {x1} AND cy >= {y0} AND cy < {y1}
                 ORDER BY z"
            ),
            (g as f32 * px_per_m * 1.15).powi(2).max(1.0),
            format!("{g} m overview cells"),
        )
    };
    let batches = run_sql(&ctx, &sql).await?;
    let (xs, ys, class) = (
        column(&batches, 0),
        column(&batches, 1),
        column(&batches, 2),
    );

    let base_x = LinearScale::configured(x_domain, (0.0, width));
    let base_y = LinearScale::configured(y_domain, (height, 0.0));
    let layer = Layer {
        x: base_x.scale_to_numeric(&xs).unwrap(),
        y: base_y.scale_to_numeric(&ys).unwrap(),
        fill: class_colors(&class).into(),
        base_x,
        base_y,
        size,
        m_per_px: (x_domain.1 - x_domain.0) / width,
        len: xs.len(),
        description: format!("{what} · {} points · query {:.0?}", xs.len(), t.elapsed()),
    };
    Ok(layer)
}

// ---------------------------------------------------------------------------
// Scene
// ---------------------------------------------------------------------------

fn text(x: f32, y: f32, s: String, size: f32, bold: bool, color: [f32; 4]) -> SceneMark {
    SceneTextMark {
        len: 1,
        text: s.into(),
        x: x.into(),
        y: y.into(),
        font_size: size.into(),
        font_weight: FontWeight::Name(if bold {
            FontWeightNameSpec::Bold
        } else {
            FontWeightNameSpec::Normal
        })
        .into(),
        color: ColorOrGradient::Color(color).into(),
        align: TextAlign::Left.into(),
        baseline: TextBaseline::Alphabetic.into(),
        ..Default::default()
    }
    .into()
}

struct Builder;

#[async_trait::async_trait]
impl SceneGraphBuilder<State> for Builder {
    async fn build(&self, s: &mut State) -> Result<SceneGraph, AvengerAppError> {
        let (x_scale, y_scale) = (s.x_scale(), s.y_scale());
        let layer = &s.layer;
        let points = SceneSymbolMark {
            name: "points".into(),
            interactive: false,
            len: layer.len as u32,
            x: layer.x.clone(),
            y: layer.y.clone(),
            x_adjustment: Some(layer.base_x.adjust(&x_scale).unwrap()),
            y_adjustment: Some(layer.base_y.adjust(&y_scale).unwrap()),
            fill: layer.fill.clone(),
            size: (layer.size * (layer.m_per_px / s.m_per_px).powi(2))
                .clamp(0.5, 2500.0)
                .into(),
            stroke_width: 0.0f32.into(),
            shapes: vec![SymbolShape::Circle],
            ..Default::default()
        };
        let plot = SceneGroup {
            name: PLOT.into(),
            marks: vec![points.into()],
            clip: Clip::Rect {
                x: 0.0,
                y: 0.0,
                width: s.width,
                height: s.height,
            },
            ..Default::default()
        };
        let axis = |scale: &ConfiguredScale, title: &str, o| {
            make_numeric_axis_marks(
                scale,
                title,
                [0.0, 0.0],
                &AxisConfig {
                    dimensions: [s.width, s.height],
                    orientation: o,
                    grid: false,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let mut legend = s.legend.clone();
        if let SceneMark::Group(g) = &mut legend {
            g.origin[0] += s.width - 600.0; // legend was built for a 600 px plot
        }
        let (x0, x1) = s.x_domain();
        let chart = SceneGroup {
            origin: [MARGIN_LEFT, MARGIN_TOP],
            marks: vec![
                plot.into(),
                axis(
                    &y_scale,
                    "Northing (m, tile-relative)",
                    AxisOrientation::Left,
                )
                .into(),
                axis(
                    &x_scale,
                    "Easting (m, tile-relative)",
                    AxisOrientation::Bottom,
                )
                .into(),
                legend,
                text(s.width + 16.0, -6.0, "LiDAR class".into(), 12.0, true, INK),
                text(
                    0.0,
                    -30.0,
                    format!("{} · {} points", s.tile_name, s.total_points),
                    16.0,
                    true,
                    INK,
                ),
                text(
                    0.0,
                    -12.0,
                    "Drag to pan · scroll to zoom · data reloads when the view settles".into(),
                    11.0,
                    false,
                    MUTED,
                ),
                text(
                    0.0,
                    s.height + 62.0,
                    format!("View {:.0} m wide · {}", x1 - x0, layer.description),
                    12.0,
                    false,
                    MUTED,
                ),
            ],
            ..Default::default()
        };
        Ok(SceneGraph {
            marks: vec![chart.into()],
            width: s.width + MARGIN_LEFT + MARGIN_RIGHT,
            height: s.height + MARGIN_TOP + MARGIN_BOTTOM,
            origin: [0.0; 2],
        })
    }
}

// ---------------------------------------------------------------------------
// Interaction
// ---------------------------------------------------------------------------

fn no_update() -> UpdateStatus {
    UpdateStatus {
        rerender: false,
        rebuild_geometry: false,
        ..Default::default()
    }
}

struct PanStart;
#[async_trait::async_trait]
impl EventStreamHandler<State> for PanStart {
    async fn handle(
        &self,
        event: &SceneGraphEvent,
        s: &mut State,
        rtree: &SceneGraphRTree,
    ) -> UpdateStatus {
        let Some(p) = event.position().and_then(|p| s.plot_pointer(p, rtree)) else {
            return no_update();
        };
        s.pan = Some(PanAnchor {
            pointer: p,
            center: s.center,
        });
        UpdateStatus {
            cursor: Some(CursorStyle::Grabbing),
            ..no_update()
        }
    }
}

struct PanMove;
#[async_trait::async_trait]
impl EventStreamHandler<State> for PanMove {
    async fn handle(
        &self,
        event: &SceneGraphEvent,
        s: &mut State,
        rtree: &SceneGraphRTree,
    ) -> UpdateStatus {
        let (Some(anchor), Some(pos), Some(origin)) = (
            s.pan.clone(),
            event.position(),
            rtree.named_group_origin(PLOT),
        ) else {
            return no_update();
        };
        let dx = pos[0] - origin[0] - anchor.pointer[0];
        let dy = pos[1] - origin[1] - anchor.pointer[1];
        s.center = [
            anchor.center[0] - dx * s.m_per_px,
            anchor.center[1] + dy * s.m_per_px,
        ];
        UpdateStatus {
            rerender: true,
            ..no_update()
        }
    }
}

struct PanEnd;
#[async_trait::async_trait]
impl EventStreamHandler<State> for PanEnd {
    async fn handle(
        &self,
        _: &SceneGraphEvent,
        s: &mut State,
        _: &SceneGraphRTree,
    ) -> UpdateStatus {
        s.pan = None;
        UpdateStatus {
            cursor: Some(CursorStyle::Default),
            ..no_update()
        }
    }
}

struct WheelZoom;
#[async_trait::async_trait]
impl EventStreamHandler<State> for WheelZoom {
    async fn handle(
        &self,
        event: &SceneGraphEvent,
        s: &mut State,
        rtree: &SceneGraphRTree,
    ) -> UpdateStatus {
        let SceneGraphEvent::MouseWheel(wheel) = event else {
            return no_update();
        };
        let Some(p) = s.plot_pointer(wheel.position, rtree) else {
            return no_update();
        };
        let factor = match wheel.delta {
            MouseScrollDelta::LineDelta(dx, dy) => (1.0 - (dx + dy) * 0.1).clamp(0.5, 2.0),
            MouseScrollDelta::PixelDelta(dx, dy) => {
                (1.0 - (dx + dy) as f32 * 0.004).clamp(0.5, 2.0)
            }
        };
        // Keep the data point under the cursor fixed while zooming.
        let data = [
            s.center[0] + (p[0] - s.width / 2.0) * s.m_per_px,
            s.center[1] - (p[1] - s.height / 2.0) * s.m_per_px,
        ];
        s.m_per_px = (s.m_per_px * factor).clamp(0.01, 5.0);
        s.center = [
            data[0] - (p[0] - s.width / 2.0) * s.m_per_px,
            data[1] + (p[1] - s.height / 2.0) * s.m_per_px,
        ];
        UpdateStatus {
            rerender: true,
            ..no_update()
        }
    }
}

struct Resize;
#[async_trait::async_trait]
impl EventStreamHandler<State> for Resize {
    async fn handle(
        &self,
        event: &SceneGraphEvent,
        s: &mut State,
        _: &SceneGraphRTree,
    ) -> UpdateStatus {
        let SceneGraphEvent::WindowResize(e) = event else {
            return no_update();
        };
        s.width = (e.size[0] - MARGIN_LEFT - MARGIN_RIGHT).max(100.0);
        s.height = (e.size[1] - MARGIN_TOP - MARGIN_BOTTOM).max(100.0);
        UpdateStatus {
            rerender: true,
            rebuild_geometry: true,
            ..no_update()
        }
    }
}

/// Swap in data for the settled view.
struct Reload;
#[async_trait::async_trait]
impl EventStreamHandler<State> for Reload {
    async fn handle(
        &self,
        _: &SceneGraphEvent,
        s: &mut State,
        _: &SceneGraphRTree,
    ) -> UpdateStatus {
        if s.pan.is_some() {
            return no_update();
        }
        let job = load_layer(s.ctx.clone(), s.x_domain(), s.y_domain(), s.width, s.height);
        match s.data_rt.spawn(job).await {
            Ok(Ok(layer)) => {
                println!("{}", layer.description);
                s.layer = layer;
                UpdateStatus {
                    rerender: true,
                    ..no_update()
                }
            }
            Ok(Err(e)) => {
                eprintln!("query failed: {e}");
                no_update()
            }
            Err(e) => {
                eprintln!("query task failed: {e}");
                no_update()
            }
        }
    }
}

fn button_config(kind: SceneGraphEventType, down: bool) -> EventStreamConfig {
    EventStreamConfig {
        types: vec![kind],
        filter: Some(vec![EventStreamFilter::event(move |event| match event {
            SceneGraphEvent::MouseDown(e) if down => e.button == MouseButton::Left,
            SceneGraphEvent::MouseUp(e) if !down => e.button == MouseButton::Left,
            _ => false,
        })]),
        ..Default::default()
    }
}

fn main() {
    let tile = std::env::args()
        .nth(1)
        .expect("usage: explorer <tile.copc.laz>");
    let tile_name = std::path::Path::new(&tile)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .trim_end_matches(".copc.laz")
        .to_string();

    // Multi-threaded runtime for DataFusion; the window runs on its own runtime.
    let data_rt = tokio::runtime::Runtime::new().unwrap();
    let ctx = las_context();
    let (total_points, _origin) = data_rt.block_on(prepare(&ctx, &tile)).expect("load tile");

    let (width, height) = (820.0f32, 820.0f32);
    let (center, m_per_px) = ([500.0f32, 500.0f32], 1000.0 / width);
    let layer = data_rt
        .block_on(load_layer(
            ctx.clone(),
            (center[0] - 500.0, center[0] + 500.0),
            (center[1] - 500.0, center[1] + 500.0),
            width,
            height,
        ))
        .expect("initial layer");
    println!("{}", layer.description);

    let mut labels: Vec<String> = CLASSES.iter().map(|(_, l, _)| l.to_string()).collect();
    labels.push("Other".into());
    let mut swatches: Vec<ColorOrGradient> = CLASSES
        .iter()
        .map(|(_, _, c)| ColorOrGradient::Color(*c))
        .collect();
    swatches.push(ColorOrGradient::Color(OTHER));
    let legend = make_symbol_legend(&SymbolLegendConfig {
        title: None,
        text: labels.into(),
        shape: SymbolShape::Circle.into(),
        size: 60.0.into(),
        fill: swatches.into(),
        stroke: ColorOrGradient::Color([0.0; 4]).into(),
        inner_width: 600.0,
        inner_height: 600.0,
        ..Default::default()
    })
    .unwrap();

    let state = State {
        ctx,
        data_rt: data_rt.handle().clone(),
        tile_name,
        total_points,
        width,
        height,
        center,
        m_per_px,
        pan: None,
        layer,
        legend: legend.into(),
    };

    let mouse_down = button_config(SceneGraphEventType::MouseDown, true);
    let mouse_up = button_config(SceneGraphEventType::MouseUp, false);
    let handlers: Vec<(EventStreamConfig, Arc<dyn EventStreamHandler<State>>)> = vec![
        (mouse_down.clone(), Arc::new(PanStart)),
        (
            EventStreamConfig {
                types: vec![SceneGraphEventType::CursorMoved],
                between: Some((Box::new(mouse_down), Box::new(mouse_up.clone()))),
                throttle: Some(8),
                ..Default::default()
            },
            Arc::new(PanMove),
        ),
        (mouse_up, Arc::new(PanEnd)),
        (
            EventStreamConfig {
                types: vec![SceneGraphEventType::MouseWheel],
                throttle: Some(8),
                ..Default::default()
            },
            Arc::new(WheelZoom),
        ),
        (
            EventStreamConfig {
                types: vec![SceneGraphEventType::WindowResize],
                ..Default::default()
            },
            Arc::new(Resize),
        ),
        (
            EventStreamConfig {
                types: vec![
                    SceneGraphEventType::MouseUp,
                    SceneGraphEventType::MouseWheel,
                    SceneGraphEventType::WindowResize,
                ],
                debounce: Some(DebounceConfig::new(150)),
                ..Default::default()
            },
            Arc::new(Reload),
        ),
    ];

    if let Some(pos) = std::env::args().position(|a| a == "--snapshots") {
        let out = std::env::args()
            .nth(pos + 1)
            .expect("--snapshots <out_dir>");
        data_rt.block_on(snapshots(state, &out));
        return;
    }

    let window_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let app = window_rt
        .block_on(AvengerApp::try_new(state, Arc::new(Builder), handlers))
        .expect("create app");
    let options = WinitWgpuAvengerAppOptions::new(2.0).window_attributes(
        WindowAttributes::default()
            .with_title("LiDAR explorer · sedona-pointcloud + DataFusion + Avenger")
            .with_resizable(true)
            .with_inner_size(LogicalSize::new(
                width + MARGIN_LEFT + MARGIN_RIGHT,
                height + MARGIN_TOP + MARGIN_BOTTOM,
            ))
            .with_min_inner_size(LogicalSize::new(500.0, 400.0)),
    );
    let (mut app, event_loop) =
        WinitWgpuAvengerApp::new_and_event_loop_with_options(app, options, window_rt);
    event_loop.run_app(&mut app).expect("event loop");
    drop(data_rt);
}

/// Headless check: render what the window shows at a few zoom steps, both
/// right after zooming (GPU-adjusted old data) and after the reload.
async fn snapshots(mut s: State, out: &str) {
    use avenger_common::canvas::CanvasDimensions;
    use avenger_wgpu::canvas::{Canvas, PngCanvas};

    async fn save(s: &mut State, path: String) {
        let scene = Builder.build(s).await.unwrap();
        let mut canvas = PngCanvas::new(
            CanvasDimensions {
                size: [scene.width, scene.height],
                scale: 1.0,
            },
            Default::default(),
        )
        .await
        .unwrap();
        canvas.set_scene(&scene).unwrap();
        canvas.render().await.unwrap().save(&path).unwrap();
        println!("  -> {path}");
    }
    async fn reload(s: &mut State) {
        s.layer = load_layer(s.ctx.clone(), s.x_domain(), s.y_domain(), s.width, s.height)
            .await
            .unwrap();
        println!("{}", s.layer.description);
    }

    save(&mut s, format!("{out}/explore_0_overview.png")).await;
    // Zoom to ~350 m around the rail/road junction, then to ~100 m.
    for (i, (center, width_m)) in [([700.0f32, 420.0f32], 350.0f32), ([760.0, 400.0], 100.0)]
        .into_iter()
        .enumerate()
    {
        s.center = center;
        s.m_per_px = width_m / s.width;
        save(
            &mut s,
            format!("{out}/explore_{}a_zoomed_before_reload.png", i + 1),
        )
        .await;
        reload(&mut s).await;
        save(&mut s, format!("{out}/explore_{}b_after_reload.png", i + 1)).await;
    }
}
