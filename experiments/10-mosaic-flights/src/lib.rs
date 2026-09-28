//! What experiment 10's figures and its window share: Jon's three panels,
//! the flights loaded as his example transforms them, the brush, lasso and
//! series producers, the histogram query, and the drawing of a panel.

use std::sync::Arc;
use std::time::Instant;

use arrow::array::{ArrayRef, Float64Array, RecordBatch};
use arrow::datatypes::DataType;
use avenger_color::ColorOrGradient;
use avenger_common::canvas::CanvasDimensions;
use avenger_guides::axis::opts::{AxisConfig, AxisOrientation};
use avenger_scales::scales::linear::LinearScale;
use avenger_scales_datafusion::BuiltinScale;
use avenger_scenegraph::marks::{group::SceneGroup, line::SceneLineMark, mark::SceneMark, rect::SceneRectMark, text::SceneTextMark};
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_selection::{
    ConsumerFilter, PixelGrid, ProducerDefinition, ProducerId, Projection, ProjectionId, Resolution, SelectionFilter,
    SelectionId, SelectionSet, ViewId,
};
use avenger_wgpu::canvas::{Canvas, PngCanvas};
use datafusion::common::Result as DFResult;
use datafusion::datasource::MemTable;
use datafusion::functions_aggregate::expr_fn::{avg, count, sum};
use datafusion::logical_expr::{cast, col, ident, lit, Expr};
use datafusion::prelude::{DataFrame, ParquetReadOptions, SessionContext};

pub type Error = Box<dyn std::error::Error>;

/// Jon's panels: 600 × 200 px, with his domains and display bins.
pub const W: f32 = 600.0;
pub const H: f32 = 200.0;
pub const BLUE: [f32; 4] = [70. / 255., 130. / 255., 180. / 255., 1.];
pub const GREY: [f32; 4] = [0.86, 0.87, 0.89, 1.];
pub const INK: [f32; 4] = [0.25, 0.27, 0.3, 1.];

pub struct Plot {
    pub name: &'static str,
    pub title: &'static str,
    pub domain: [f64; 2],
    pub step: f64,
}
pub const PLOTS: [Plot; 3] = [
    Plot { name: "delay", title: "Arrival Delay (min)", domain: [-60., 190.], step: 10. },
    Plot { name: "time", title: "Departure Time (hour)", domain: [0., 24.], step: 1. },
    Plot { name: "distance", title: "Flight Distance (miles)", domain: [0., 5000.], step: 200. },
];
impl Plot {
    pub fn px(&self, v: f64, width: f32) -> f32 {
        ((v - self.domain[0]) / (self.domain[1] - self.domain[0])) as f32 * width
    }
}

pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    s.as_bytes().rchunks(3).rev().map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join(",")
}
pub fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}
pub fn pid(s: &str) -> ProjectionId {
    ProjectionId::new(s).unwrap()
}
pub fn grid(domain: [f64; 2], range: [f64; 2]) -> PixelGrid {
    let a = |v: [f64; 2]| -> ArrayRef { Arc::new(Float64Array::from(v.to_vec())) };
    PixelGrid::new(BuiltinScale::Linear, a(domain), a(range), Default::default(), 0.0, 1.0).unwrap()
}
/// The brush of one panel, as Jon's example defines it: its column on a 1 px grid.
pub fn brush_producer(p: &Plot) -> ProducerDefinition {
    ProducerDefinition::new(SelectionId::new("brush").unwrap(), ProducerId::new(p.name).unwrap(), ViewId::new(p.name).unwrap(),
        [Projection::new(pid("value"), col(p.name)).unwrap()]).unwrap()
        .with_pixel_grids([(pid("value"), grid(p.domain, [0.0, W as f64]))]).unwrap()
}
/// What panel `i` shows: every selection but its own.
pub fn cross(i: usize) -> ConsumerFilter {
    ConsumerFilter::new(ViewId::new(PLOTS[i].name).unwrap(), SelectionFilter::cross_filter([&SelectionId::new("brush").unwrap()]))
}
pub fn empty() -> SelectionSet {
    SelectionSet::new([(SelectionId::new("brush").unwrap(), Resolution::Intersect)]).unwrap()
}

/// Counts (or the sum of `weight`) per display bin of `p`, over the rows `f` keeps.
pub async fn histogram(ctx: &SessionContext, p: &Plot, f: Expr, weight: Option<Expr>) -> DFResult<Vec<(i64, f64)>> {
    let bin = cast(datafusion::functions::math::expr_fn::floor((col(p.name) - lit(p.domain[0])) / lit(p.step)), DataType::Int64);
    let value = match weight {
        None => cast(count(lit(1)), DataType::Float64),
        Some(w) => sum(w),
    };
    let b = ctx.table("flights").await?.filter(f)?.aggregate(vec![bin.alias("bin")], vec![value.alias("v")])?.collect().await?;
    let mut out: Vec<(i64, f64)> = b.iter().flat_map(|b| {
        let (k, v) = (i64s(b, "bin"), f64s(b, "v"));
        (0..b.num_rows()).map(move |i| (k.value(i), v.value(i)))
    }).collect();
    out.sort_by_key(|x| x.0);
    Ok(out)
}
pub fn f64s(b: &RecordBatch, n: &str) -> Float64Array {
    let c = arrow::compute::cast(b.column_by_name(n).unwrap(), &DataType::Float64).unwrap();
    c.as_any().downcast_ref::<Float64Array>().unwrap().clone()
}
pub fn i64s(b: &RecordBatch, n: &str) -> arrow::array::Int64Array {
    let c = arrow::compute::cast(b.column_by_name(n).unwrap(), &DataType::Int64).unwrap();
    c.as_any().downcast_ref::<arrow::array::Int64Array>().unwrap().clone()
}

// ---------------------------------------------------------------------------
// Drawing

pub fn text(s: impl Into<String>, x: f32, y: f32, size: f32, color: [f32; 4]) -> SceneMark {
    SceneTextMark { text: s.into().into(), x: x.into(), y: y.into(), font_size: size.into(), color: ColorOrGradient::Color(color).into(),
        interactive: false, ..Default::default() }.into()
}
pub fn rects(xywh: &[[f32; 4]], fill: Vec<[f32; 4]>) -> SceneMark {
    let col = |i: usize| xywh.iter().map(|r| r[i]).collect::<Vec<_>>();
    SceneRectMark { len: xywh.len() as u32, x: col(0).into(), y: col(1).into(), width: Some(col(2).into()), height: Some(col(3).into()),
        fill: fill.into_iter().map(ColorOrGradient::Color).collect::<Vec<_>>().into(), interactive: false, ..Default::default() }.into()
}
pub fn outline(points: &[[f32; 2]], color: [f32; 4], width: f32) -> SceneMark {
    SceneLineMark { len: points.len() as u32, x: points.iter().map(|p| p[0]).collect::<Vec<_>>().into(),
        y: points.iter().map(|p| p[1]).collect::<Vec<_>>().into(), stroke: ColorOrGradient::Color(color), stroke_width: width,
        interactive: false, ..Default::default() }.into()
}
pub fn axes(x: [f64; 2], y: [f64; 2], x_title: &str, y_title: &str, origin: [f32; 2], w: f32, h: f32) -> Result<Vec<SceneMark>, Error> {
    let mut out = Vec::new();
    for (domain, range, orientation, title, format) in [
        (x, (0.0, w), AxisOrientation::Bottom, x_title, ",.0f"),
        (y, (h, 0.0), AxisOrientation::Left, y_title, ".2~s"),
    ] {
        let scale = LinearScale::configured((domain[0] as f32, domain[1] as f32), range);
        let config = AxisConfig { dimensions: [w, h], orientation, tick_count: Some(5.), format_number: Some(format.into()), ..Default::default() };
        out.push(lidar_common::make_numeric_axis_marks(&scale, title, origin, &config)?.into());
    }
    Ok(out)
}
/// A histogram panel: grey for all flights, then each layer of bars in
/// front, and the brush if the panel has one.
/// A shaded range behind a panel's bars: a brush, or a soft brush's reach.
pub struct Shade {
    pub range: [f64; 2],
    pub alpha: f32,
    pub outline: bool,
}
pub fn panel(p: &Plot, origin: [f32; 2], all: &[(i64, f64)], layers: &[(&[(i64, f64)], Vec<[f32; 4]>)], shades: &[Shade], max: f64)
    -> Result<SceneMark, Error> {
    let bars = |bins: &[(i64, f64)]| -> Vec<[f32; 4]> {
        bins.iter().map(|(b, v)| {
            let x0 = p.px(p.domain[0] + *b as f64 * p.step, W);
            let x1 = p.px(p.domain[0] + (*b + 1) as f64 * p.step, W);
            let h = (*v / max) as f32 * H;
            [x0 + 0.5, H - h, (x1 - x0 - 1.0).max(0.0), h]
        }).collect()
    };
    let mut marks = Vec::new();
    for s in shades {
        let (a, b) = (p.px(s.range[0].max(p.domain[0]), W), p.px(s.range[1].min(p.domain[1]), W));
        marks.push(rects(&[[a, 0.0, b - a, H]], vec![[0.0, 0.0, 0.0, s.alpha]]));
        if s.outline {
            marks.push(outline(&[[a, 0.0], [a, H], [b, H], [b, 0.0], [a, 0.0]], INK, 1.0));
        }
    }
    marks.push(rects(&bars(all), vec![GREY; all.len()]));
    for (bins, colours) in layers {
        marks.push(rects(&bars(bins), colours.clone()));
    }
    let mut group = axes(p.domain, [0.0, max], p.title, "Count", [0.0, 0.0], W, H)?;
    group.extend(marks);
    Ok(SceneGroup { origin, marks: group, ..Default::default() }.into())
}
pub async fn render(marks: Vec<SceneMark>, size: [f32; 2], path: &str) -> Result<(), Error> {
    let mut scene_marks = vec![rects(&[[0.0, 0.0, size[0], size[1]]], vec![[1.0; 4]])];
    scene_marks.extend(marks);
    let scene = SceneGraph { width: size[0], height: size[1], origin: [0.0; 2], marks: vec![SceneGroup { marks: scene_marks, ..Default::default() }.into()] };
    let mut canvas = PngCanvas::new(CanvasDimensions { size, scale: 2.0 }, Default::default()).await?;
    canvas.set_scene(&scene)?;
    canvas.render().await?.save(path)?;
    println!("  -> {path}");
    Ok(())
}
pub fn heading(title: &str, sub: &str) -> Vec<SceneMark> {
    vec![text(title, 24.0, 30.0, 20.0, INK), text(sub, 24.0, 50.0, 11.0, [0.42, 0.45, 0.49, 1.])]
}
pub const TOP: f32 = 80.0;
pub const ROW: f32 = H + 80.0;
pub fn size(rows: f32) -> [f32; 2] {
    [W + 120.0, TOP + rows * ROW]
}

/// Departure time against arrival delay, the density panel of figure 2.
pub const DENSITY_H: f32 = 300.0;
pub fn density_producer() -> ProducerDefinition {
    let (t, d) = (&PLOTS[1], &PLOTS[0]);
    ProducerDefinition::new(SelectionId::new("brush").unwrap(), ProducerId::new("lasso").unwrap(), ViewId::new("density").unwrap(),
        [Projection::new(pid("u"), col("time")).unwrap(), Projection::new(pid("v"), col("delay")).unwrap()]).unwrap()
        .with_pixel_grids([(pid("u"), grid(t.domain, [0.0, W as f64])), (pid("v"), grid(d.domain, [DENSITY_H as f64, 0.0]))]).unwrap()
}
/// The lasso of figure 2, in the density panel's pixels: evening departures
/// that arrive an hour or more late.
pub fn the_lasso() -> Vec<[f64; 2]> {
    [[395.0, 175.0], [470.0, 60.0], [560.0, 25.0], [595.0, 70.0], [590.0, 180.0], [520.0, 205.0], [440.0, 215.0]].to_vec()
}

/// Figures 4 and 5's series. The flights have none, so the figures choose an
/// aggregation: one line per 200-mile band of distance (Jon's display bins),
/// the mean arrival delay per hour of departure, over hours with at least
/// 1,000 flights.
pub const BAND: f64 = 200.0;
pub const SERIES_H: f32 = 300.0;
pub const MEAN: [f64; 2] = [-10.0, 120.0];
pub fn band() -> Expr {
    cast(datafusion::functions::math::expr_fn::floor(col("distance") / lit(BAND)), DataType::Int64)
}
pub async fn series_rows(ctx: &SessionContext) -> DFResult<DataFrame> {
    let hour = cast(datafusion::functions::math::expr_fn::floor(col("time")), DataType::Float64) + lit(0.5);
    ctx.table("flights").await?
        .aggregate(vec![band().alias("band"), hour.alias("x")], vec![avg(col("delay")).alias("y"), count(lit(1)).alias("n")])?
        .filter(col("n").gt_eq(lit(1000)))
}
/// The series as vertices by band, in x order.
pub async fn series_lines(ctx: &SessionContext) -> DFResult<std::collections::BTreeMap<i64, Vec<[f64; 2]>>> {
    let mut lines = std::collections::BTreeMap::<i64, Vec<[f64; 2]>>::new();
    for b in series_rows(ctx).await?.collect().await? {
        let (k, x, y) = (i64s(&b, "band"), f64s(&b, "x"), f64s(&b, "y"));
        for i in 0..b.num_rows() {
            lines.entry(k.value(i)).or_default().push([x.value(i), y.value(i)]);
        }
    }
    lines.values_mut().for_each(|l| l.sort_by(|a, b| a[0].total_cmp(&b[0])));
    Ok(lines)
}
pub fn series_producer() -> ProducerDefinition {
    ProducerDefinition::new(SelectionId::new("brush").unwrap(), ProducerId::new("series").unwrap(), ViewId::new("series").unwrap(),
        [Projection::new(pid("band"), band()).unwrap()]).unwrap()
}

/// The flights as Jon's transform (dataflow.rs) gives them, registered as
/// `flights`: delay clipped to −60…180, hours and miles as they are.
pub async fn load(ctx: &SessionContext, data: &str) -> Result<usize, Error> {
    let raw = ctx.read_parquet(data, ParquetReadOptions::default()).await?;
    println!("{} ({:?})", data, raw.schema().fields().iter().map(|f| format!("{}: {}", f.name(), f.data_type())).collect::<Vec<_>>());
    let delay = datafusion::functions::core::expr_fn::greatest(vec![lit(-60.0), datafusion::functions::core::expr_fn::least(vec![cast(ident("ARR_DELAY"), DataType::Float64), lit(180.0)])]);
    let rows = raw.select(vec![delay.alias("delay"), cast(ident("DEP_TIME"), DataType::Float64).alias("time"), cast(ident("DISTANCE"), DataType::Float64).alias("distance")])?
        .collect().await?;
    let n: usize = rows.iter().map(|b| b.num_rows()).sum();
    ctx.register_table("flights", Arc::new(MemTable::try_new(rows[0].schema(), vec![rows])?))?;
    Ok(n)
}

/// The density panel's cells, 15 minutes of departure by 5 minutes of delay,
/// tinted on a logarithmic scale: their rectangles and fills, in the panel's pixels.
pub async fn density(ctx: &SessionContext) -> Result<(Vec<[f32; 4]>, Vec<[f32; 4]>), Error> {
    let (t, d) = (&PLOTS[1], &PLOTS[0]);
    let (tb, db) = (0.25, 5.0);
    let cells = ctx.table("flights").await?.aggregate(
        vec![cast(datafusion::functions::math::expr_fn::floor(col("time") / lit(tb)), DataType::Int64).alias("i"),
             cast(datafusion::functions::math::expr_fn::floor((col("delay") - lit(d.domain[0])) / lit(db)), DataType::Int64).alias("j")],
        vec![count(lit(1)).alias("n")])?.collect().await?;
    let mut heat: Vec<[f32; 4]> = Vec::new();
    let mut fill = Vec::new();
    let top = cells.iter().flat_map(|b| f64s(b, "n").values().to_vec()).fold(1.0, f64::max).ln();
    for b in &cells {
        let (i, j, c) = (i64s(b, "i"), i64s(b, "j"), f64s(b, "n"));
        for r in 0..b.num_rows() {
            let (x0, x1) = (t.px(i.value(r) as f64 * tb, W), t.px((i.value(r) + 1) as f64 * tb, W));
            let (v0, v1) = (d.domain[0] + j.value(r) as f64 * db, d.domain[0] + (j.value(r) + 1) as f64 * db);
            let (y1, y0) = (DENSITY_H - d.px(v0, DENSITY_H), DENSITY_H - d.px(v1, DENSITY_H));
            if x1 <= 0.0 || y1 <= 0.0 || y0 >= DENSITY_H {
                continue;
            }
            let k = (c.value(r).ln() / top) as f32;
            heat.push([x0, y0.max(0.0), x1 - x0, y1.min(DENSITY_H) - y0.max(0.0)]);
            fill.push([1.0 - k * (1.0 - BLUE[0]), 1.0 - k * (1.0 - BLUE[1]), 1.0 - k * (1.0 - BLUE[2]), 1.0]);
        }
    }
    Ok((heat, fill))
}
