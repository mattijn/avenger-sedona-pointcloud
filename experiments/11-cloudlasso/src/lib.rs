//! What experiment 11's figure and its window share: the 500 m window of the
//! tile, the two producers of one selection (the lasso on the screen's 1 px
//! cells, and the voxels), CloudLasso's step from the lasso to the largest
//! dense region, and the drawing helpers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{ArrayRef, Float64Array, Int64Array, RecordBatch};
use arrow::datatypes::DataType;
use avenger_color::ColorOrGradient;
use avenger_guides::axis::opts::{AxisConfig, AxisOrientation};
use avenger_scales::scales::linear::LinearScale;
use avenger_scales_datafusion::BuiltinScale;
use avenger_scenegraph::marks::{line::SceneLineMark, mark::SceneMark, symbol::SceneSymbolMark, text::SceneTextMark};
use avenger_selection::{
    ConsumerFilter, EmptySelection, PixelGrid, ProducerDefinition, ProducerId, Projection, ProjectionId, Resolution, SelectionFilter,
    SelectionId, SelectionSet, SelectionValue, ViewId,
};
use datafusion::datasource::MemTable;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::logical_expr::{col, lit, Expr};
use datafusion::prelude::SessionContext;
use lidar_common::las_context;

pub type Error = Box<dyn std::error::Error>;

/// Experiment 7's plot, its window, view and lasso (as the probe measures them).
pub const P: f64 = 400.0;
pub const WIN: [f64; 4] = [657500.0, 658000.0, 6867250.0, 6867750.0];
pub const YAW: f64 = 30.0;
pub const ELEVATION: f64 = 35.0;
pub const RING: [[f64; 2]; 4] = [[0.42, 0.52], [0.75, 0.55], [0.78, 0.32], [0.45, 0.28]];
pub const STRUCTURE: f64 = 0.3;
/// Voxels: 1/40 of the window across, 1/10 of its height range up.
pub const VOXELS: [f64; 3] = [40.0, 40.0, 10.0];

pub const GREY: [f32; 4] = [0.8, 0.82, 0.85, 1.0];
pub const LASSO: [f32; 4] = [0.96, 0.66, 0.35, 1.0];
pub const BLUE: [f32; 4] = [70. / 255., 130. / 255., 180. / 255., 1.];
pub const RED: [f32; 4] = [0.85, 0.2, 0.15, 1.0];

pub fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    s.as_bytes().rchunks(3).rev().map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join(",")
}
pub fn f64s(b: &RecordBatch, n: &str) -> Float64Array {
    let c = arrow::compute::cast(b.column_by_name(n).unwrap(), &DataType::Float64).unwrap();
    c.as_any().downcast_ref::<Float64Array>().unwrap().clone()
}
pub fn i64s(b: &RecordBatch, n: &str) -> Int64Array {
    let c = arrow::compute::cast(b.column_by_name(n).unwrap(), &DataType::Int64).unwrap();
    c.as_any().downcast_ref::<Int64Array>().unwrap().clone()
}
pub fn pid(s: &str) -> ProjectionId {
    ProjectionId::new(s).unwrap()
}
pub fn linear(domain: [f64; 2], range: [f64; 2]) -> PixelGrid {
    let a = |v: [f64; 2]| -> ArrayRef { Arc::new(Float64Array::from(v.to_vec())) };
    PixelGrid::new(BuiltinScale::Linear, a(domain), a(range), Default::default(), 0.0, 1.0).unwrap()
}
pub fn producer(name: &str, projections: Vec<(&str, Expr, PixelGrid)>) -> ProducerDefinition {
    let (projections, grids): (Vec<_>, Vec<_>) =
        projections.into_iter().map(|(n, e, g)| (Projection::new(pid(n), e).unwrap(), (pid(n), g))).unzip();
    ProducerDefinition::new(SelectionId::new("cloud").unwrap(), ProducerId::new(name).unwrap(), ViewId::new("map").unwrap(), projections)
        .unwrap()
        .with_pixel_grids(grids)
        .unwrap()
}
/// What another view shows of the selection: its members, nothing when empty.
pub fn members() -> ConsumerFilter {
    ConsumerFilter::new(ViewId::new("points").unwrap(), SelectionFilter::membership(&SelectionId::new("cloud").unwrap(), EmptySelection::MatchNone))
}
pub fn linear_expr(c: &[f64; 4]) -> Expr {
    lit(c[0]) + lit(c[1]) * col("x") + lit(c[2]) * col("y") + lit(c[3]) * col("z")
}
pub fn in_ring(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let mut inside = false;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + ring.len() - 1) % ring.len()]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

/// The window's points, registered in memory as `pts`, and their height range.
pub async fn load(tile: &str) -> Result<(SessionContext, Vec<RecordBatch>, usize, (f64, f64)), Error> {
    let ctx = las_context();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?.collect().await?;
    let points = ctx.sql(&format!("SELECT x, y, CAST(z AS DOUBLE) AS z FROM '{tile}' WHERE x BETWEEN {} AND {} AND y BETWEEN {} AND {}",
        WIN[0], WIN[1], WIN[2], WIN[3])).await?.collect().await?;
    let n: usize = points.iter().map(|b| b.num_rows()).sum();
    let hz = points.iter().flat_map(|b| f64s(b, "z").values().to_vec()).fold((f64::MAX, f64::MIN), |(a, b), z| (a.min(z), b.max(z)));
    let mem = SessionContext::new();
    mem.register_table("pts", Arc::new(MemTable::try_new(points[0].schema(), vec![points.clone()])?))?;
    Ok((mem, points, n, hz))
}

/// A lasso and what CloudLasso keeps of it.
pub struct Selected {
    /// The lasso alone: every point whose screen cell falls inside the ring.
    pub drawn: SelectionSet,
    /// The lasso and the largest dense region's voxels; the lasso alone without `structure`.
    pub cloud: SelectionSet,
    pub voxels: usize,
    pub regions: usize,
}

/// The chart's two producers of one selection, for a lasso `ring` drawn in
/// the plot's pixels in the view (`yaw`, `elevation`); with `structure`,
/// CloudLasso's step: voxel counts of the lasso's points, the largest region
/// of voxels at least that share as dense as the densest.
pub async fn select(mem: &SessionContext, hz: (f64, f64), yaw: f64, elevation: f64, ring: &[[f64; 2]], structure: Option<f64>)
    -> Result<Selected, Error> {
    let (a, b) = lidar_common::tilt(WIN, hz, yaw, elevation, P);
    let screen = linear([0.0, P], [0.0, P]);
    let lasso = producer("lasso", vec![("u", linear_expr(&a), screen.clone()), ("v", linear_expr(&b), screen)]);
    let grids = [linear([WIN[0], WIN[1]], [0.0, VOXELS[0]]), linear([WIN[2], WIN[3]], [0.0, VOXELS[1]]), linear([hz.0, hz.1], [0.0, VOXELS[2]])];
    let [gx, gy, gz] = &grids;
    let voxels = producer("voxels", vec![("vx", col("x"), gx.clone()), ("vy", col("y"), gy.clone()), ("vz", col("z"), gz.clone())]);

    let empty = SelectionSet::new([(SelectionId::new("cloud").unwrap(), Resolution::Intersect)])?;
    let drawn = empty.set(&lasso, SelectionValue::polygon(&lasso, &pid("u"), &pid("v"), ring)?)?;
    let Some(structure) = structure else {
        return Ok(Selected { cloud: drawn.clone(), drawn, voxels: 0, regions: 0 });
    };
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
    let (region, regions) = lidar_common::largest_region(&per_voxel, structure);
    let cells = region.iter().map(|c| vec![c.0, c.1, c.2]);
    let cloud = drawn.set(&voxels, SelectionValue::cells(&voxels, &[&pid("vx"), &pid("vy"), &pid("vz")], cells)?)?;
    Ok(Selected { drawn, cloud, voxels: region.len(), regions })
}

// ---------------------------------------------------------------------------
// Drawing

pub fn text(s: impl Into<String>, x: f32, y: f32, size: f32, color: [f32; 4]) -> SceneMark {
    SceneTextMark { text: s.into().into(), x: x.into(), y: y.into(), font_size: size.into(), color: ColorOrGradient::Color(color).into(),
        interactive: false, ..Default::default() }.into()
}
pub fn line(points: &[[f32; 2]], color: [f32; 4], width: f32) -> SceneMark {
    SceneLineMark { len: points.len() as u32, x: points.iter().map(|p| p[0]).collect::<Vec<_>>().into(),
        y: points.iter().map(|p| p[1]).collect::<Vec<_>>().into(), stroke: ColorOrGradient::Color(color), stroke_width: width,
        interactive: false, ..Default::default() }.into()
}
pub fn dots(points: &[[f32; 2]], color: [f32; 4]) -> SceneMark {
    SceneSymbolMark { len: points.len() as u32, x: points.iter().map(|p| p[0]).collect::<Vec<_>>().into(),
        y: points.iter().map(|p| p[1]).collect::<Vec<_>>().into(), fill: ColorOrGradient::Color(color).into(), size: 1.5.into(),
        stroke_width: 0.0f32.into(), interactive: false, ..Default::default() }.into()
}
pub fn axis(domain: [f64; 2], range: (f32, f32), orientation: AxisOrientation, title: &str, w: f32, h: f32) -> Result<SceneMark, Error> {
    let scale = LinearScale::configured((domain[0] as f32, domain[1] as f32), range);
    let config = AxisConfig { dimensions: [w, h], orientation, tick_count: Some(4.), format_number: Some(",.0f".into()), ..Default::default() };
    Ok(lidar_common::make_numeric_axis_marks(&scale, title, [0.0, 0.0], &config)?.into())
}
