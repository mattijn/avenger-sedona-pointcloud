//! Live view of a LiDAR feed arriving over Arrow Flight.
//!
//! Each batch that arrives is aggregated once into per-cell *states*
//! (`maxState`, `countState` from avenger-datafusion-aggregate-state). The
//! states are kept for a rolling window and merged again on every frame, so
//! the picture always shows the last N seconds of scanning without ever
//! revisiting the raw points.
//!
//! In the window: space pauses, the arrow keys (or + and -) change the replay
//! speed, [ and ] change the length of the rolling window, and r restarts the
//! flight. A speed change reconnects with a new Flight ticket that carries the
//! new speed and the point in the flight to resume from.
//!
//! Usage: cargo run --release --bin stream_live -- [--addr http://127.0.0.1:50051]
//!                                                 [--speed 4] [--window 20]
//!                                                 [--size 880x700]
//!                                                 [--snapshots <dir>]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use arrow::array::{AsArray, RecordBatch};
use arrow::datatypes::{Float32Type, Float64Type, Int64Type};
use arrow_flight::{FlightClient, Ticket};
use avenger_app::app::{AvengerApp, SceneGraphBuilder};
use avenger_app::error::AvengerAppError;
use avenger_color::ColorOrGradient;
use avenger_common::canvas::CanvasDimensions;
use avenger_common::types::SymbolShape;
use avenger_datafusion_aggregate_state::register_all;
use avenger_eventstream::manager::EventStreamHandler;
use avenger_eventstream::scene::{SceneGraphEvent, SceneGraphEventType};
use avenger_eventstream::stream::{EventStreamConfig, UpdateStatus};
use avenger_eventstream::window::{Key, NamedKey};
use avenger_geometry::rtree::SceneGraphRTree;
use avenger_guides::axis::numeric::make_numeric_axis_marks;
use avenger_guides::axis::opts::{AxisConfig, AxisOrientation};
use avenger_resource::render_invalidation::{
    RenderInvalidationHub, RenderInvalidationReason, RenderInvalidationRequest,
    RenderInvalidationSink,
};
use avenger_scales::scales::linear::LinearScale;
use avenger_scales::scales::ConfiguredScale;
use avenger_scenegraph::marks::group::{Clip, SceneGroup};
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::rect::SceneRectMark;
use avenger_scenegraph::marks::symbol::SceneSymbolMark;
use avenger_scenegraph::marks::text::SceneTextMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_text::types::{FontWeight, FontWeightNameSpec, TextAlign, TextBaseline};
use avenger_wgpu::canvas::{Canvas, PngCanvas};
use avenger_winit_wgpu::{WinitWgpuAvengerApp, WinitWgpuAvengerAppOptions};
use datafusion::prelude::SessionContext;
use futures::StreamExt;
use tonic::transport::Channel;
use winit::window::WindowAttributes;

/// Cell size of the live map, in metres.
const CELL: f32 = 2.0;
const TILE_M: f32 = 1000.0;
const SPARK_H: f32 = 64.0;
/// Space around the map: axis labels and titles on the left and top, the
/// sparkline and the key hints below.
const MARGIN_L: f32 = 86.0;
const MARGIN_R: f32 = 20.0;
const MARGIN_T: f32 = 72.0;
const MARGIN_B: f32 = 46.0 + SPARK_H + 34.0;
const INK: [f32; 4] = [0.10, 0.12, 0.15, 1.0];
const MUTED: [f32; 4] = [0.38, 0.42, 0.47, 1.0];
/// Elevation ramp, evenly spaced (a linear scale takes only two domain stops).
const RAMP: [&str; 5] = ["#2c3a6b", "#2c7fb8", "#41b6c4", "#a1dab4", "#ffffcc"];

/// What the feed task shares with the renderer.
#[derive(Default)]
struct Live {
    /// Per-batch aggregate states, newest last: (stream time, state batch).
    states: VecDeque<(f64, RecordBatch)>,
    /// Points per second, per half-second bucket of stream time.
    history: VecDeque<(f64, f64)>,
    stream_t: f64,
    points: u64,
    batches: u64,
    line: i32,
    last_batch_ms: f64,
    cells: usize,
    finished: bool,
}

/// What the viewer can change while the feed is running. Every change bumps
/// `generation`, which makes the feed task reconnect with a new ticket.
#[derive(Clone, Copy)]
struct Control {
    speed: f64,
    window_s: f64,
    paused: bool,
    restart: bool,
    generation: u64,
}

impl Control {
    fn change(&mut self, f: impl FnOnce(&mut Control)) {
        f(self);
        self.generation += 1;
    }
}

#[derive(Clone)]
struct State {
    live: Arc<Mutex<Live>>,
    control: Arc<Mutex<Control>>,
    ctx: SessionContext,
    rt: tokio::runtime::Handle,
    width: f32,
    height: f32,
    color: ConfiguredScale,
}

impl State {
    /// The map is square and takes whatever room the window leaves.
    fn plot(&self) -> f32 {
        (self.width - MARGIN_L - MARGIN_R)
            .min(self.height - MARGIN_T - MARGIN_B)
            .max(120.0)
    }
}

fn context() -> SessionContext {
    let mut ctx = SessionContext::new();
    register_all(&mut ctx).expect("register aggregate-state functions");
    ctx
}

/// Aggregates one arriving batch into per-cell states, once.
async fn fold_batch(
    ctx: &SessionContext,
    batch: RecordBatch,
) -> datafusion::error::Result<RecordBatch> {
    ctx.register_batch("incoming", batch)?;
    let out = ctx
        .sql(&format!(
            "SELECT CAST(floor(x / {CELL}) * {CELL} + {half} AS DOUBLE) AS cx,
                    CAST(floor(y / {CELL}) * {CELL} + {half} AS DOUBLE) AS cy,
                    maxState(z)   AS zmax_s,
                    countState()  AS n_s
             FROM incoming
             GROUP BY 1, 2",
            half = CELL / 2.0
        ))
        .await?
        .collect()
        .await?;
    ctx.deregister_table("incoming")?;
    Ok(arrow::compute::concat_batches(&out[0].schema(), &out)?)
}

/// Merges the retained states into the current picture of the rolling window.
async fn merge_window(
    ctx: &SessionContext,
    states: Vec<RecordBatch>,
) -> datafusion::error::Result<RecordBatch> {
    let schema = states[0].schema();
    let merged = arrow::compute::concat_batches(&schema, &states)?;
    ctx.register_batch("states", merged)?;
    let out = ctx
        .sql(
            "SELECT cx, cy,
                    CAST(maxMerge(zmax_s) AS FLOAT) AS zmax,
                    CAST(countMerge(n_s) AS BIGINT) AS n
             FROM states GROUP BY cx, cy",
        )
        .await?
        .collect()
        .await?;
    ctx.deregister_table("states")?;
    Ok(arrow::compute::concat_batches(&out[0].schema(), &out)?)
}

/// Pulls the Flight stream and keeps the rolling window up to date.
///
/// Reconnects whenever the viewer changes something: the new ticket carries
/// the new speed and the point in the flight to resume from, so a speed
/// change does not lose the window.
async fn consume(
    addr: String,
    live: Arc<Mutex<Live>>,
    control: Arc<Mutex<Control>>,
    hub: Option<RenderInvalidationHub>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ctx = context();
    let mut resume_from = 0.0f64;

    loop {
        let (paused, start_gen, speed, restart) = {
            let mut c = control.lock().unwrap();
            let restart = c.restart;
            if !c.paused {
                c.restart = false;
            }
            (c.paused, c.generation, c.speed, restart && !c.paused)
        };
        if paused {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            continue;
        }
        if restart {
            resume_from = 0.0;
            let mut l = live.lock().unwrap();
            *l = Live::default();
        }

        let channel = Channel::from_shared(addr.clone())?.connect().await?;
        let mut client = FlightClient::new(channel);
        let ticket = Ticket::new(format!("{{\"speed\": {speed}, \"from\": {resume_from}}}"));
        let mut stream = client.do_get(ticket).await?;
        live.lock().unwrap().finished = false;

        let mut reconnect = false;
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            if batch.num_rows() == 0 {
                continue;
            }
            let t0 = Instant::now();
            let rows = batch.num_rows() as u64;
            let t_col = batch
                .column_by_name("t")
                .unwrap()
                .as_primitive::<Float64Type>();
            let (t_start, t_end) = (t_col.value(0), t_col.value(t_col.len() - 1));
            let line = batch
                .column_by_name("line")
                .unwrap()
                .as_primitive::<arrow::datatypes::Int32Type>()
                .value(0);

            let state = fold_batch(&ctx, batch).await?;
            let cells = state.num_rows();
            let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let window_s = control.lock().unwrap().window_s;

            let mut l = live.lock().unwrap();
            l.states.push_back((t_end, state));
            while let Some((t, _)) = l.states.front() {
                if t_end - t > window_s {
                    l.states.pop_front();
                } else {
                    break;
                }
            }
            let rate = if t_end > t_start {
                rows as f64 / (t_end - t_start)
            } else {
                0.0
            };
            let bucket = (t_end * 2.0).floor() / 2.0;
            match l.history.back_mut() {
                Some((b, r)) if *b == bucket => *r = (*r + rate) / 2.0,
                _ => l.history.push_back((bucket, rate)),
            }
            while let Some((b, _)) = l.history.front() {
                if t_end - b > window_s * 3.0 {
                    l.history.pop_front();
                } else {
                    break;
                }
            }
            l.stream_t = t_end;
            l.points += rows;
            l.batches += 1;
            l.line = line;
            l.last_batch_ms = elapsed_ms;
            l.cells = cells;
            drop(l);
            resume_from = t_end;

            if let Some(hub) = &hub {
                hub.request_render(RenderInvalidationRequest::now(
                    RenderInvalidationReason::EvaluationChanged {
                        kind: "lidar-stream".to_string(),
                    },
                ));
            }

            if control.lock().unwrap().generation != start_gen {
                reconnect = true;
                break;
            }
        }

        if let Some(hub) = &hub {
            hub.request_render(RenderInvalidationRequest::now(
                RenderInvalidationReason::EvaluationChanged {
                    kind: "lidar-stream".to_string(),
                },
            ));
        }
        if reconnect {
            continue;
        }

        // The flight is over: wait here so a restart can still come in.
        live.lock().unwrap().finished = true;
        println!("stream finished");
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            let restart = control.lock().unwrap().restart;
            if restart {
                break;
            }
        }
    }
}

fn text_mark(items: Vec<(String, f32, f32, f32, bool, [f32; 4])>) -> SceneTextMark {
    let len = items.len() as u32;
    SceneTextMark {
        len,
        text: items.iter().map(|i| i.0.clone()).collect::<Vec<_>>().into(),
        x: items.iter().map(|i| i.1).collect::<Vec<_>>().into(),
        y: items.iter().map(|i| i.2).collect::<Vec<_>>().into(),
        font_size: items.iter().map(|i| i.3).collect::<Vec<_>>().into(),
        font_weight: items
            .iter()
            .map(|i| {
                if i.4 {
                    FontWeight::Name(FontWeightNameSpec::Bold)
                } else {
                    FontWeight::Name(FontWeightNameSpec::Normal)
                }
            })
            .collect::<Vec<_>>()
            .into(),
        color: items
            .iter()
            .map(|i| ColorOrGradient::Color(i.5))
            .collect::<Vec<_>>()
            .into(),
        align: TextAlign::Left.into(),
        baseline: TextBaseline::Alphabetic.into(),
        ..Default::default()
    }
}

struct Builder;

#[async_trait::async_trait]
impl SceneGraphBuilder<State> for Builder {
    async fn build(&self, s: &mut State) -> Result<SceneGraph, AvengerAppError> {
        let (states, live) = {
            let l = s.live.lock().unwrap();
            let states: Vec<RecordBatch> = l.states.iter().map(|(_, b)| b.clone()).collect();
            (
                states,
                (
                    l.stream_t,
                    l.points,
                    l.batches,
                    l.line,
                    l.last_batch_ms,
                    l.history.clone(),
                    l.finished,
                ),
            )
        };
        let (stream_t, points, batches, line, batch_ms, history, finished) = live;
        let control = *s.control.lock().unwrap();
        let plot = s.plot();
        if std::env::var("STREAM_DEBUG").is_ok() {
            println!("frame: stream t {stream_t:.1} s · {batches} batches");
        }

        let px_per_m = plot / TILE_M;
        let mut marks: Vec<SceneMark> = Vec::new();

        marks.push(
            SceneRectMark {
                len: 1,
                x: 0.0.into(),
                y: 0.0.into(),
                width: Some(plot.into()),
                height: Some(plot.into()),
                fill: ColorOrGradient::Color([0.07, 0.08, 0.11, 1.0]).into(),
                ..Default::default()
            }
            .into(),
        );

        let mut merge_ms = 0.0;
        if !states.is_empty() {
            let ctx = s.ctx.clone();
            let t0 = Instant::now();
            let cells =
                s.rt.spawn(async move { merge_window(&ctx, states).await })
                    .await
                    .map_err(|e| AvengerAppError::InternalError(e.to_string()))?
                    .map_err(|e| AvengerAppError::InternalError(e.to_string()))?;
            merge_ms = t0.elapsed().as_secs_f64() * 1000.0;

            let cx = cells
                .column_by_name("cx")
                .unwrap()
                .as_primitive::<Float64Type>();
            let cy = cells
                .column_by_name("cy")
                .unwrap()
                .as_primitive::<Float64Type>();
            let zmax = cells
                .column_by_name("zmax")
                .unwrap()
                .as_primitive::<Float32Type>();
            let n = cells
                .column_by_name("n")
                .unwrap()
                .as_primitive::<Int64Type>();

            let len = cells.num_rows();
            let xs: Vec<f32> = (0..len).map(|i| cx.value(i) as f32 * px_per_m).collect();
            let ys: Vec<f32> = (0..len)
                .map(|i| plot - cy.value(i) as f32 * px_per_m)
                .collect();
            let zs: Vec<f32> = (0..len).map(|i| zmax.value(i)).collect();
            let counts: Vec<f32> = (0..len).map(|i| n.value(i) as f32).collect();

            let z_array: arrow::array::ArrayRef =
                Arc::new(arrow::array::Float32Array::from(zs.clone()));
            let fills = s
                .color
                .scale_to_color(&z_array)
                .map_err(|e| AvengerAppError::InternalError(e.to_string()))?;
            // Cells that collected more returns are drawn slightly larger.
            let sizes: Vec<f32> = counts
                .iter()
                .map(|c| (2.0 + c.min(40.0) * 0.12) * px_per_m * CELL)
                .collect();

            marks.push(
                SceneGroup {
                    origin: [0.0, 0.0],
                    clip: Clip::Rect {
                        x: 0.0,
                        y: 0.0,
                        width: plot,
                        height: plot,
                    },
                    marks: vec![SceneSymbolMark {
                        len: len as u32,
                        x: xs.into(),
                        y: ys.into(),
                        fill: fills,
                        size: sizes.into(),
                        shapes: vec![SymbolShape::Circle],
                        stroke_width: None,
                        ..Default::default()
                    }
                    .into()],
                    ..Default::default()
                }
                .into(),
            );
        }

        // Axes in metres across the tile.
        let x_scale = LinearScale::configured((0.0, TILE_M), (0.0, plot));
        let y_scale = LinearScale::configured((0.0, TILE_M), (plot, 0.0));
        let axis = |scale: &ConfiguredScale, title: &str, orientation: AxisOrientation| {
            make_numeric_axis_marks(
                scale,
                title,
                [0.0, 0.0],
                &AxisConfig {
                    dimensions: [plot, plot],
                    orientation,
                    grid: false,
                    ..Default::default()
                },
            )
        };
        marks.push(
            axis(&y_scale, "Northing − 6 867 000 (m)", AxisOrientation::Left)
                .map_err(|e| AvengerAppError::InternalError(e.to_string()))?
                .into(),
        );
        marks.push(
            axis(&x_scale, "Easting − 657 000 (m)", AxisOrientation::Bottom)
                .map_err(|e| AvengerAppError::InternalError(e.to_string()))?
                .into(),
        );

        // Sparkline: points per second over recent stream time.
        if history.len() > 1 {
            let t_lo = history.front().unwrap().0;
            let t_hi = history.back().unwrap().0.max(t_lo + 1.0);
            let r_hi = history.iter().map(|(_, r)| *r).fold(1.0f64, f64::max);
            let bar_w = (plot / history.len() as f32).max(1.0);
            let xs: Vec<f32> = history
                .iter()
                .map(|(t, _)| ((t - t_lo) / (t_hi - t_lo)) as f32 * (plot - bar_w))
                .collect();
            let hs: Vec<f32> = history
                .iter()
                .map(|(_, r)| (r / r_hi) as f32 * SPARK_H)
                .collect();
            let ys: Vec<f32> = hs.iter().map(|h| SPARK_H - h).collect();
            marks.push(
                SceneGroup {
                    origin: [0.0, plot + 46.0],
                    marks: vec![
                        SceneRectMark {
                            len: history.len() as u32,
                            x: xs.into(),
                            y: ys.into(),
                            width: Some(bar_w.max(2.0).into()),
                            height: Some(hs.into()),
                            fill: ColorOrGradient::Color([0.25, 0.71, 0.77, 0.85]).into(),
                            ..Default::default()
                        }
                        .into(),
                        text_mark(vec![(
                            format!("return rate · peak {:.0}k points/s", r_hi / 1000.0),
                            0.0,
                            -6.0,
                            10.0,
                            false,
                            MUTED,
                        )])
                        .into(),
                    ],
                    ..Default::default()
                }
                .into(),
            );
        }

        let status = if finished {
            "flight over · r restarts".to_string()
        } else if control.paused {
            format!("paused at {:.0}× · space resumes", control.speed)
        } else {
            format!("live · {:.2}× speed", control.speed)
        };
        marks.push(
            text_mark(vec![
                (
                    "LiDAR feed over Arrow Flight".into(),
                    0.0,
                    -46.0,
                    17.0,
                    true,
                    INK,
                ),
                (
                    format!(
                        "rolling {:.0} s window · flight line {line} · t = {stream_t:.1} s · {status}",
                        control.window_s
                    ),
                    0.0,
                    -26.0,
                    11.0,
                    false,
                    MUTED,
                ),
                (
                    format!(
                        "{:.1}M points received in {batches} batches · fold {batch_ms:.0} ms/batch · merge {merge_ms:.0} ms/frame",
                        points as f64 / 1e6
                    ),
                    0.0,
                    -10.0,
                    11.0,
                    false,
                    MUTED,
                ),
            ])
            .into(),
        );

        marks.push(
            text_mark(vec![(
                "space pause · ↑ ↓ speed · [ ] window · r restart".into(),
                0.0,
                plot + SPARK_H + 74.0,
                10.0,
                false,
                MUTED,
            )])
            .into(),
        );

        Ok(SceneGraph {
            marks: vec![SceneGroup {
                origin: [MARGIN_L, MARGIN_T],
                marks,
                ..Default::default()
            }
            .into()],
            width: s.width,
            height: s.height,
            origin: [0.0, 0.0],
        })
    }
}

fn redraw() -> UpdateStatus {
    UpdateStatus {
        rerender: true,
        rebuild_geometry: true,
        ..Default::default()
    }
}

/// Keeps the layout inside the window.
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
            return UpdateStatus::default();
        };
        s.width = e.size[0].max(320.0);
        s.height = e.size[1].max(320.0);
        redraw()
    }
}

/// Speed, pause, window length and restart, from the keyboard.
struct Keys;
#[async_trait::async_trait]
impl EventStreamHandler<State> for Keys {
    async fn handle(
        &self,
        event: &SceneGraphEvent,
        s: &mut State,
        _: &SceneGraphRTree,
    ) -> UpdateStatus {
        let SceneGraphEvent::KeyPress(e) = event else {
            return UpdateStatus::default();
        };
        let text = e.text.as_deref().unwrap_or("");
        let mut c = s.control.lock().unwrap();
        match (&e.key, text) {
            (Key::Named(NamedKey::Space), _) => c.change(|c| c.paused = !c.paused),
            (Key::Named(NamedKey::ArrowUp), _) | (_, "+") | (_, "=") => {
                c.change(|c| c.speed = (c.speed * 2.0).min(32.0))
            }
            (Key::Named(NamedKey::ArrowDown), _) | (_, "-") | (_, "_") => {
                c.change(|c| c.speed = (c.speed / 2.0).max(0.25))
            }
            (_, "]") => c.change(|c| c.window_s = (c.window_s * 1.5).min(120.0)),
            (_, "[") => c.change(|c| c.window_s = (c.window_s / 1.5).max(1.0)),
            (_, "r") | (_, "R") => c.change(|c| {
                c.restart = true;
                c.paused = false;
            }),
            _ => return UpdateStatus::default(),
        }
        println!(
            "control: {:.2}× · {:.0} s window{}",
            c.speed,
            c.window_s,
            if c.paused { " · paused" } else { "" }
        );
        drop(c);
        redraw()
    }
}

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let addr = arg("--addr").unwrap_or_else(|| "http://127.0.0.1:50051".to_string());
    let speed: f64 = arg("--speed").and_then(|s| s.parse().ok()).unwrap_or(4.0);
    let window_s: f64 = arg("--window").and_then(|s| s.parse().ok()).unwrap_or(20.0);
    let snapshots = arg("--snapshots");
    // Sized so the square map uses the width and the height about equally,
    // and the whole window fits a 1280 x 800 screen. The layout follows the
    // window from there.
    let (width, height) = arg("--size")
        .and_then(|s| {
            let (w, h) = s.split_once(['x', 'X'])?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        })
        .unwrap_or((660.0f32, 760.0f32));

    let color = LinearScale::configured((40.0, 90.0), (0.0, 1.0))
        .with_range(
            Arc::new(arrow::array::StringArray::from(RAMP.to_vec())) as arrow::array::ArrayRef
        );

    let data_rt = tokio::runtime::Runtime::new().unwrap();
    let live = Arc::new(Mutex::new(Live::default()));
    let control = Arc::new(Mutex::new(Control {
        speed,
        window_s,
        paused: false,
        restart: false,
        generation: 0,
    }));
    let hub = RenderInvalidationHub::default();

    let state = State {
        live: live.clone(),
        control: control.clone(),
        ctx: context(),
        rt: data_rt.handle().clone(),
        width,
        height,
        color,
    };

    if let Some(dir) = snapshots {
        data_rt.block_on(async move {
            std::fs::create_dir_all(&dir).unwrap();
            let feed = tokio::spawn(consume(addr, live.clone(), control, None));
            let mut state = state;
            // One frame each time the scanner's clock passes a mark, so the
            // images are the same whatever replay speed is used.
            for (i, mark_s) in [25.0f64, 55.0, 104.0].iter().enumerate() {
                loop {
                    let l = state.live.lock().unwrap();
                    let (t, done) = (l.stream_t, l.finished);
                    drop(l);
                    if t >= *mark_s || done {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                let scene = Builder.build(&mut state).await.expect("build scene");
                let mut canvas = PngCanvas::new(
                    CanvasDimensions {
                        size: [width, height],
                        scale: 2.0,
                    },
                    Default::default(),
                )
                .await
                .unwrap();
                canvas.set_scene(&scene).unwrap();
                let path = format!("{dir}/stream_{i}.png");
                canvas.render().await.unwrap().save(&path).unwrap();
                let l = state.live.lock().unwrap();
                println!(
                    "  -> {path} · t {:.1} s · {:.1}M points · {} state batches in the window",
                    l.stream_t,
                    l.points as f64 / 1e6,
                    l.states.len()
                );
            }
            feed.abort();
        });
        return;
    }

    let feed_hub = hub.clone();
    data_rt.spawn(async move {
        if let Err(e) = consume(addr, live, control, Some(feed_hub)).await {
            eprintln!("feed failed: {e}");
        }
    });

    let handlers: Vec<(EventStreamConfig, Arc<dyn EventStreamHandler<State>>)> = vec![
        (
            EventStreamConfig {
                types: vec![SceneGraphEventType::WindowResize],
                ..Default::default()
            },
            Arc::new(Resize),
        ),
        (
            EventStreamConfig {
                types: vec![SceneGraphEventType::KeyPress],
                ..Default::default()
            },
            Arc::new(Keys),
        ),
    ];

    let window_rt = tokio::runtime::Runtime::new().unwrap();
    let app = window_rt
        .block_on(AvengerApp::try_new(state, Arc::new(Builder), handlers))
        .expect("build app");

    let options = WinitWgpuAvengerAppOptions::new(2.0)
        .render_invalidation_hub(hub)
        .window_attributes(
            WindowAttributes::default()
                .with_title("LiDAR feed · Arrow Flight → DataFusion → Avenger")
                .with_resizable(true)
                .with_inner_size(winit::dpi::LogicalSize::new(width, height))
                .with_min_inner_size(winit::dpi::LogicalSize::new(420.0, 380.0)),
        );
    let (mut app, event_loop) =
        WinitWgpuAvengerApp::new_and_event_loop_with_options(app, options, window_rt);
    event_loop.run_app(&mut app).expect("event loop");
    drop(data_rt);
}
