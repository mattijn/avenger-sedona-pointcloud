//! CloudLasso (Yu et al. 2012) through `avenger-selection`, on a 500 m window
//! of the tile, drawn three ways: in the tilted view the lasso is drawn in,
//! from above, and from the side. The lasso takes every point whose screen
//! cell falls inside it, from the ground up; CloudLasso keeps only the
//! largest region of dense voxels among them. The chart finds the region
//! (experiment 7's algorithm, in lidar-common) from the crate's voxel
//! counts, and the crate selects its voxels as a value (FINDINGS.md 28).
//!
//! Usage: cargo run --release -p lidar-cloudlasso --bin cloudlasso -- <tile> [out-dir]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch};
use arrow::datatypes::DataType;
use avenger_color::ColorOrGradient;
use avenger_common::canvas::CanvasDimensions;
use avenger_guides::axis::opts::{AxisConfig, AxisOrientation};
use avenger_scales::scales::linear::LinearScale;
use avenger_scales_datafusion::BuiltinScale;
use avenger_scenegraph::marks::{group::SceneGroup, line::SceneLineMark, mark::SceneMark, rect::SceneRectMark, symbol::SceneSymbolMark,
    text::SceneTextMark};
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_selection::{
    ConsumerFilter, EmptySelection, PixelGrid, ProducerDefinition, ProducerId, Projection, ProjectionId, Resolution, SelectionFilter,
    SelectionId, SelectionSet, SelectionValue, ViewId,
};
use avenger_wgpu::canvas::{Canvas, PngCanvas};
use datafusion::datasource::MemTable;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::logical_expr::{col, lit, Expr};
use datafusion::prelude::SessionContext;
use lidar_common::{las_context, INK, MUTED};

type Error = Box<dyn std::error::Error>;

/// Experiment 7's plot, its window, view and lasso (as the probe measures them).
const P: f64 = 400.0;
const WIN: [f64; 4] = [657500.0, 658000.0, 6867250.0, 6867750.0];
const YAW: f64 = 30.0;
const ELEVATION: f64 = 35.0;
const RING: [[f64; 2]; 4] = [[0.42, 0.52], [0.75, 0.55], [0.78, 0.32], [0.45, 0.28]];
const STRUCTURE: f64 = 0.3;
/// Voxels: 1/40 of the window across, 1/10 of its height range up.
const VOXELS: [f64; 3] = [40.0, 40.0, 10.0];

const GREY: [f32; 4] = [0.8, 0.82, 0.85, 1.0];
const LASSO: [f32; 4] = [0.96, 0.66, 0.35, 1.0];
const BLUE: [f32; 4] = [70. / 255., 130. / 255., 180. / 255., 1.];
const RED: [f32; 4] = [0.85, 0.2, 0.15, 1.0];

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}
fn thousands(n: usize) -> String {
    let s = n.to_string();
    s.as_bytes().rchunks(3).rev().map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join(",")
}
fn f64s(b: &RecordBatch, n: &str) -> Float64Array {
    let c = arrow::compute::cast(b.column_by_name(n).unwrap(), &DataType::Float64).unwrap();
    c.as_any().downcast_ref::<Float64Array>().unwrap().clone()
}
fn i64s(b: &RecordBatch, n: &str) -> Int64Array {
    let c = arrow::compute::cast(b.column_by_name(n).unwrap(), &DataType::Int64).unwrap();
    c.as_any().downcast_ref::<Int64Array>().unwrap().clone()
}
fn pid(s: &str) -> ProjectionId {
    ProjectionId::new(s).unwrap()
}
fn linear(domain: [f64; 2], range: [f64; 2]) -> PixelGrid {
    let a = |v: [f64; 2]| -> ArrayRef { Arc::new(Float64Array::from(v.to_vec())) };
    PixelGrid::new(BuiltinScale::Linear, a(domain), a(range), Default::default(), 0.0, 1.0).unwrap()
}
fn producer(name: &str, projections: Vec<(&str, Expr, PixelGrid)>) -> ProducerDefinition {
    let (projections, grids): (Vec<_>, Vec<_>) =
        projections.into_iter().map(|(n, e, g)| (Projection::new(pid(n), e).unwrap(), (pid(n), g))).unzip();
    ProducerDefinition::new(SelectionId::new("cloud").unwrap(), ProducerId::new(name).unwrap(), ViewId::new("map").unwrap(), projections)
        .unwrap()
        .with_pixel_grids(grids)
        .unwrap()
}
/// What another view shows of the selection: its members, nothing when empty.
fn members() -> ConsumerFilter {
    ConsumerFilter::new(ViewId::new("points").unwrap(), SelectionFilter::membership(&SelectionId::new("cloud").unwrap(), EmptySelection::MatchNone))
}
fn linear_expr(c: &[f64; 4]) -> Expr {
    lit(c[0]) + lit(c[1]) * col("x") + lit(c[2]) * col("y") + lit(c[3]) * col("z")
}
fn in_ring(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let mut inside = false;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + ring.len() - 1) % ring.len()]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

// ---------------------------------------------------------------------------
// Drawing

fn text(s: impl Into<String>, x: f32, y: f32, size: f32, color: [f32; 4]) -> SceneMark {
    SceneTextMark { text: s.into().into(), x: x.into(), y: y.into(), font_size: size.into(), color: ColorOrGradient::Color(color).into(),
        interactive: false, ..Default::default() }.into()
}
fn line(points: &[[f32; 2]], color: [f32; 4], width: f32) -> SceneMark {
    SceneLineMark { len: points.len() as u32, x: points.iter().map(|p| p[0]).collect::<Vec<_>>().into(),
        y: points.iter().map(|p| p[1]).collect::<Vec<_>>().into(), stroke: ColorOrGradient::Color(color), stroke_width: width,
        interactive: false, ..Default::default() }.into()
}
fn dots(points: &[[f32; 2]], color: [f32; 4]) -> SceneMark {
    SceneSymbolMark { len: points.len() as u32, x: points.iter().map(|p| p[0]).collect::<Vec<_>>().into(),
        y: points.iter().map(|p| p[1]).collect::<Vec<_>>().into(), fill: ColorOrGradient::Color(color).into(), size: 1.5.into(),
        stroke_width: 0.0f32.into(), interactive: false, ..Default::default() }.into()
}
fn axis(domain: [f64; 2], range: (f32, f32), orientation: AxisOrientation, title: &str, w: f32, h: f32) -> Result<SceneMark, Error> {
    let scale = LinearScale::configured((domain[0] as f32, domain[1] as f32), range);
    let config = AxisConfig { dimensions: [w, h], orientation, tick_count: Some(4.), format_number: Some(",.0f".into()), ..Default::default() };
    Ok(lidar_common::make_numeric_axis_marks(&scale, title, [0.0, 0.0], &config)?.into())
}
/// A panel: its points in three layers (all, the lasso's, CloudLasso's),
/// each drawn every `step`-th, then `extra` on top.
fn panel(origin: [f32; 2], layers: [&[[f32; 2]]; 3], step: usize, mut extra: Vec<SceneMark>, caption: &str) -> SceneMark {
    let mut marks = vec![text(caption, 0.0, -10.0, 12.0, INK)];
    for (pts, colour) in layers.into_iter().zip([GREY, LASSO, BLUE]) {
        let thin: Vec<[f32; 2]> = pts.iter().step_by(step).copied().collect();
        marks.push(dots(&thin, colour));
    }
    marks.append(&mut extra);
    SceneGroup { origin, marks, ..Default::default() }.into()
}
async fn render(marks: Vec<SceneMark>, size: [f32; 2], path: &str) -> Result<(), Error> {
    let background: SceneMark = SceneRectMark { len: 1, x: 0.0.into(), y: 0.0.into(), width: Some(size[0].into()), height: Some(size[1].into()),
        fill: ColorOrGradient::Color([1.0; 4]).into(), interactive: false, ..Default::default() }.into();
    let mut all = vec![background];
    all.extend(marks);
    let scene = SceneGraph { width: size[0], height: size[1], origin: [0.0; 2], marks: vec![SceneGroup { marks: all, ..Default::default() }.into()] };
    let mut canvas = PngCanvas::new(CanvasDimensions { size, scale: 2.0 }, Default::default()).await?;
    canvas.set_scene(&scene)?;
    canvas.render().await?.save(path)?;
    println!("  -> {path}");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let tile = args.next().unwrap_or_else(|| "data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz".into());
    let out = args.next().unwrap_or_else(|| "experiments/11-cloudlasso/images".into());
    std::fs::create_dir_all(&out)?;
    let ctx = las_context();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?.collect().await?;
    let t0 = Instant::now();
    let points = ctx.sql(&format!("SELECT x, y, CAST(z AS DOUBLE) AS z FROM '{tile}' WHERE x BETWEEN {} AND {} AND y BETWEEN {} AND {}",
        WIN[0], WIN[1], WIN[2], WIN[3])).await?.collect().await?;
    let n: usize = points.iter().map(|b| b.num_rows()).sum();
    let hz = points.iter().flat_map(|b| f64s(b, "z").values().to_vec()).fold((f64::MAX, f64::MIN), |(a, b), z| (a.min(z), b.max(z)));
    let mem = SessionContext::new();
    mem.register_table("pts", Arc::new(MemTable::try_new(points[0].schema(), vec![points.clone()])?))?;
    println!("{n} points in the window, heights {:.1}–{:.1} m, read in {:.0} ms", hz.0, hz.1, ms(t0));

    // The chart's two producers of one selection: the lasso on the screen's
    // 1 px cells, and the voxels.
    let (a, b) = lidar_common::tilt(WIN, hz, YAW, ELEVATION, P);
    let screen = linear([0.0, P], [0.0, P]);
    let lasso = producer("lasso", vec![("u", linear_expr(&a), screen.clone()), ("v", linear_expr(&b), screen)]);
    let grids = [linear([WIN[0], WIN[1]], [0.0, VOXELS[0]]), linear([WIN[2], WIN[3]], [0.0, VOXELS[1]]), linear([hz.0, hz.1], [0.0, VOXELS[2]])];
    let [gx, gy, gz] = &grids;
    let voxels = producer("voxels", vec![("vx", col("x"), gx.clone()), ("vy", col("y"), gy.clone()), ("vz", col("z"), gz.clone())]);
    let ring: Vec<[f64; 2]> = RING.iter().map(|q| [q[0] * P, (1.0 - q[1]) * P]).collect();

    let t0 = Instant::now();
    let empty = SelectionSet::new([(SelectionId::new("cloud").unwrap(), Resolution::Intersect)])?;
    let drawn = empty.set(&lasso, SelectionValue::polygon(&lasso, &pid("u"), &pid("v"), &ring)?)?;
    // The chart's step: voxel counts of the lasso's points, the largest dense region.
    let counts = mem.table("pts").await?.filter(members().predicate(&drawn)?)?
        .aggregate(vec![gx.cell_expr(col("x")).alias("i"), gy.cell_expr(col("y")).alias("j"), gz.cell_expr(col("z")).alias("k")],
                   vec![count(lit(1)).alias("n")])?
        .collect().await?;
    let mut per_voxel = HashMap::new();
    for b in &counts {
        let (i, j, k, c) = (i64s(b, "i"), i64s(b, "j"), i64s(b, "k"), i64s(b, "n"));
        for r in 0..b.num_rows() {
            per_voxel.insert((i.value(r), j.value(r), k.value(r)), c.value(r) as usize);
        }
    }
    let (region, regions) = lidar_common::largest_region(&per_voxel, STRUCTURE);
    let cells = region.iter().map(|c| vec![c.0, c.1, c.2]);
    let cloud = drawn.set(&voxels, SelectionValue::cells(&voxels, &[&pid("vx"), &pid("vy"), &pid("vz")], cells)?)?;
    let t_select = ms(t0);

    // Every point with its two memberships, through the crate's predicates.
    let t0 = Instant::now();
    let rows = mem.table("pts").await?
        .select(vec![col("x"), col("y"), col("z"), linear_expr(&a).alias("u"),
            members().predicate(&drawn)?.alias("in_lasso"), members().predicate(&cloud)?.alias("in_cloud")])?
        .collect().await?;
    let t_rows = ms(t0);
    // Three layers per view: tilted (u, v), from above, and from the side
    // (u against height, so it lines up with the tilted view).
    let side_h = 200.0;
    let mut views: [[Vec<[f32; 2]>; 3]; 3] = Default::default();
    let (mut n_lasso, mut n_cloud) = (0, 0);
    for bt in &rows {
        let (x, y, z, u) = (f64s(bt, "x"), f64s(bt, "y"), f64s(bt, "z"), f64s(bt, "u"));
        let flag = |n: &str| bt.column_by_name(n).unwrap().as_any().downcast_ref::<BooleanArray>().unwrap().clone();
        let (l, c) = (flag("in_lasso"), flag("in_cloud"));
        for r in 0..bt.num_rows() {
            let layer = if c.is_valid(r) && c.value(r) { 2 } else if l.is_valid(r) && l.value(r) { 1 } else { 0 };
            n_lasso += (layer > 0) as usize;
            n_cloud += (layer == 2) as usize;
            let (px, py, pz) = (x.value(r), y.value(r), z.value(r));
            let v = b[0] + b[1] * px + b[2] * py + b[3] * pz;
            views[0][layer].push([u.value(r) as f32, v as f32]);
            views[1][layer].push([((px - WIN[0]) / (WIN[1] - WIN[0]) * P) as f32, (P - (py - WIN[2]) / (WIN[3] - WIN[2]) * P) as f32]);
            views[2][layer].push([u.value(r) as f32, (side_h - (pz - hz.0) / (hz.1 - hz.0) * side_h) as f32]);
        }
    }
    println!("avenger-selection: the lasso {n_lasso} points; CloudLasso the largest of {regions} dense regions, {} voxels, {n_cloud} points; \
              {t_select:.0} ms to select, {t_rows:.0} ms to read both memberships", region.len());

    // Experiment 7's algorithm as plain Rust on the same points, as the probe checks it.
    let cell = |v: f64, lo: f64, hi: f64, n: f64| ((v - lo) / (hi - lo) * n).floor() as i64;
    let (mut rc, mut inside) = (HashMap::new(), Vec::new());
    for bt in &points {
        let (x, y, z) = (f64s(bt, "x"), f64s(bt, "y"), f64s(bt, "z"));
        for r in 0..bt.num_rows() {
            let p = [x.value(r), y.value(r), z.value(r)];
            let q = [a[0] + a[1] * p[0] + a[2] * p[1] + a[3] * p[2], b[0] + b[1] * p[0] + b[2] * p[1] + b[3] * p[2]];
            if in_ring([q[0].floor() + 0.5, q[1].floor() + 0.5], &ring) {
                let k = (cell(p[0], WIN[0], WIN[1], VOXELS[0]), cell(p[1], WIN[2], WIN[3], VOXELS[1]), cell(p[2], hz.0, hz.1, VOXELS[2]));
                *rc.entry(k).or_insert(0usize) += 1;
                inside.push(k);
            }
        }
    }
    let (rbest, rregions) = lidar_common::largest_region(&rc, STRUCTURE);
    let rn = inside.iter().filter(|k| rbest.contains(k)).count();
    let apart = 100.0 * n_cloud.abs_diff(rn) as f64 / rn.max(1) as f64;
    println!("experiment 7's Rust: the lasso {} points; the largest of {rregions} regions, {} voxels, {rn} points ({apart:.2} % apart)",
        inside.len(), rbest.len());

    // The figure: tilted and from above side by side, the side view under the tilted one.
    let step = (n / 400_000).max(1);
    let ring32: Vec<[f32; 2]> = ring.iter().chain(ring.first()).map(|p| [p[0] as f32, p[1] as f32]).collect();
    let (left, right, top) = (80.0, 80.0 + P as f32 + 110.0, 110.0);
    let mut marks = vec![
        text("CloudLasso through avenger-selection", 24.0, 30.0, 20.0, INK),
        text(format!("{} points in a 500 m window · the lasso drawn in the tilted view (yaw {YAW}°, elevation {ELEVATION}°) takes {} points, from the ground up (orange)",
            thousands(n), thousands(n_lasso)), 24.0, 50.0, 11.0, MUTED),
        text(format!("CloudLasso keeps the largest of {regions} regions of voxels at least {STRUCTURE} times as dense as the densest: {} voxels, {} points (blue) · \
            experiment 7's Rust: {} points", region.len(), thousands(n_cloud), thousands(rn)), 24.0, 65.0, 11.0, MUTED),
    ];
    let (pf, pw) = (P as f32, side_h as f32);
    marks.push(panel([left, top], [&views[0][0], &views[0][1], &views[0][2]], step, vec![line(&ring32, RED, 2.0)],
        "The tilted view, with the lasso"));
    marks.push(panel([right, top], [&views[1][0], &views[1][1], &views[1][2]], step, vec![
        axis([WIN[0], WIN[1]], (0.0, pf), AxisOrientation::Bottom, "easting (m)", pf, pf)?,
        axis([WIN[2], WIN[3]], (pf, 0.0), AxisOrientation::Left, "northing (m)", pf, pf)?,
    ], "From above"));
    let side_top = top + pf + 60.0;
    marks.push(panel([left, side_top], [&views[2][0], &views[2][1], &views[2][2]], step, vec![
        axis([hz.0, hz.1], (pw, 0.0), AxisOrientation::Left, "height (m)", pf, pw)?,
    ], "From the side, across the tilted view"));
    for (i, (colour, label)) in [(GREY, "not in the lasso"), (LASSO, "in the lasso, left out by CloudLasso"), (BLUE, "CloudLasso: the largest dense region")]
        .into_iter().enumerate() {
        let y = side_top + 40.0 + i as f32 * 24.0;
        marks.push(SceneRectMark { len: 1, x: right.into(), y: (y - 10.0).into(), width: Some(12.0.into()), height: Some(12.0.into()),
            fill: ColorOrGradient::Color(colour).into(), interactive: false, ..Default::default() }.into());
        marks.push(text(label, right + 20.0, y, 12.0, INK));
    }
    render(marks, [right + pf + 40.0, side_top + pw + 40.0], &format!("{out}/cloudlasso.png")).await
}
