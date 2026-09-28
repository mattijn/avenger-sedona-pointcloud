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
use std::time::Instant;

use arrow::array::{Array, BooleanArray};
use avenger_color::ColorOrGradient;
use avenger_common::canvas::CanvasDimensions;
use avenger_guides::axis::opts::AxisOrientation;
use avenger_scenegraph::marks::{group::SceneGroup, mark::SceneMark, rect::SceneRectMark};
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_wgpu::canvas::{Canvas, PngCanvas};
use datafusion::logical_expr::col;
use lidar_cloudlasso::*;
use lidar_common::{INK, MUTED};

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
    let t0 = Instant::now();
    let (mem, points, n, hz) = load(&tile).await?;
    println!("{n} points in the window, heights {:.1}–{:.1} m, read in {:.0} ms", hz.0, hz.1, ms(t0));

    let (a, b) = lidar_common::tilt(WIN, hz, YAW, ELEVATION, P);
    let ring: Vec<[f64; 2]> = RING.iter().map(|q| [q[0] * P, (1.0 - q[1]) * P]).collect();
    let t0 = Instant::now();
    let Selected { drawn, cloud, voxels, regions } = select(&mem, hz, YAW, ELEVATION, &ring, Some(STRUCTURE)).await?;
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
              {t_select:.0} ms to select, {t_rows:.0} ms to read both memberships", voxels);

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
            experiment 7's Rust: {} points", voxels, thousands(n_cloud), thousands(rn)), 24.0, 65.0, 11.0, MUTED),
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
