//! Mosaic's Cross-Filter Flights (10M), the dataset of Jon's
//! `examples/winit-mosaic-flights`, drawn headlessly with Avenger, with the
//! selections this repo added to `avenger-selection`: a brush, a lasso on a
//! density panel, a soft brush, and a line brush and a timebox over series
//! the figures aggregate. Each figure's histograms come from the
//! crate's predicates over the 10M rows; the run ends with the time each
//! redraw takes directly and through the preaggregation split.
//!
//! Usage: cargo run --release -p lidar-flights --bin flights -- <flights-10m.parquet> [out-dir]

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
    ConsumerFilter, EmptySelection, PixelGrid, ProducerDefinition, ProducerId, Projection, ProjectionId, Resolution, SelectionFilter,
    SelectionId, SelectionSet, SelectionValue, SeriesTest, ValueTest, ViewId,
};
use avenger_wgpu::canvas::{Canvas, PngCanvas};
use datafusion::common::Result as DFResult;
use datafusion::datasource::MemTable;
use datafusion::functions_aggregate::expr_fn::{avg, count, sum};
use datafusion::logical_expr::{cast, col, ident, lit, Expr};
use datafusion::prelude::{DataFrame, ParquetReadOptions, SessionContext};

type Error = Box<dyn std::error::Error>;

/// Jon's panels: 600 × 200 px, with his domains and display bins.
const W: f32 = 600.0;
const H: f32 = 200.0;
const BLUE: [f32; 4] = [70. / 255., 130. / 255., 180. / 255., 1.];
const GREY: [f32; 4] = [0.86, 0.87, 0.89, 1.];
const INK: [f32; 4] = [0.25, 0.27, 0.3, 1.];

struct Plot {
    name: &'static str,
    title: &'static str,
    domain: [f64; 2],
    step: f64,
}
const PLOTS: [Plot; 3] = [
    Plot { name: "delay", title: "Arrival Delay (min)", domain: [-60., 190.], step: 10. },
    Plot { name: "time", title: "Departure Time (hour)", domain: [0., 24.], step: 1. },
    Plot { name: "distance", title: "Flight Distance (miles)", domain: [0., 5000.], step: 200. },
];
impl Plot {
    fn px(&self, v: f64, width: f32) -> f32 {
        ((v - self.domain[0]) / (self.domain[1] - self.domain[0])) as f32 * width
    }
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    s.as_bytes().rchunks(3).rev().map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join(",")
}
fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}
fn pid(s: &str) -> ProjectionId {
    ProjectionId::new(s).unwrap()
}
fn grid(domain: [f64; 2], range: [f64; 2]) -> PixelGrid {
    let a = |v: [f64; 2]| -> ArrayRef { Arc::new(Float64Array::from(v.to_vec())) };
    PixelGrid::new(BuiltinScale::Linear, a(domain), a(range), Default::default(), 0.0, 1.0).unwrap()
}
/// The brush of one panel, as Jon's example defines it: its column on a 1 px grid.
fn brush_producer(p: &Plot) -> ProducerDefinition {
    ProducerDefinition::new(SelectionId::new("brush").unwrap(), ProducerId::new(p.name).unwrap(), ViewId::new(p.name).unwrap(),
        [Projection::new(pid("value"), col(p.name)).unwrap()]).unwrap()
        .with_pixel_grids([(pid("value"), grid(p.domain, [0.0, W as f64]))]).unwrap()
}
/// What panel `i` shows: every selection but its own.
fn cross(i: usize) -> ConsumerFilter {
    ConsumerFilter::new(ViewId::new(PLOTS[i].name).unwrap(), SelectionFilter::cross_filter([&SelectionId::new("brush").unwrap()]))
}
fn empty() -> SelectionSet {
    SelectionSet::new([(SelectionId::new("brush").unwrap(), Resolution::Intersect)]).unwrap()
}

/// Counts (or the sum of `weight`) per display bin of `p`, over the rows `f` keeps.
async fn histogram(ctx: &SessionContext, p: &Plot, f: Expr, weight: Option<Expr>) -> DFResult<Vec<(i64, f64)>> {
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
fn f64s(b: &RecordBatch, n: &str) -> Float64Array {
    let c = arrow::compute::cast(b.column_by_name(n).unwrap(), &DataType::Float64).unwrap();
    c.as_any().downcast_ref::<Float64Array>().unwrap().clone()
}
fn i64s(b: &RecordBatch, n: &str) -> arrow::array::Int64Array {
    let c = arrow::compute::cast(b.column_by_name(n).unwrap(), &DataType::Int64).unwrap();
    c.as_any().downcast_ref::<arrow::array::Int64Array>().unwrap().clone()
}

// ---------------------------------------------------------------------------
// Drawing

fn text(s: impl Into<String>, x: f32, y: f32, size: f32, color: [f32; 4]) -> SceneMark {
    SceneTextMark { text: s.into().into(), x: x.into(), y: y.into(), font_size: size.into(), color: ColorOrGradient::Color(color).into(),
        interactive: false, ..Default::default() }.into()
}
fn rects(xywh: &[[f32; 4]], fill: Vec<[f32; 4]>) -> SceneMark {
    let col = |i: usize| xywh.iter().map(|r| r[i]).collect::<Vec<_>>();
    SceneRectMark { len: xywh.len() as u32, x: col(0).into(), y: col(1).into(), width: Some(col(2).into()), height: Some(col(3).into()),
        fill: fill.into_iter().map(ColorOrGradient::Color).collect::<Vec<_>>().into(), interactive: false, ..Default::default() }.into()
}
fn outline(points: &[[f32; 2]], color: [f32; 4], width: f32) -> SceneMark {
    SceneLineMark { len: points.len() as u32, x: points.iter().map(|p| p[0]).collect::<Vec<_>>().into(),
        y: points.iter().map(|p| p[1]).collect::<Vec<_>>().into(), stroke: ColorOrGradient::Color(color), stroke_width: width,
        interactive: false, ..Default::default() }.into()
}
fn axes(x: [f64; 2], y: [f64; 2], x_title: &str, y_title: &str, origin: [f32; 2], w: f32, h: f32) -> Result<Vec<SceneMark>, Error> {
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
struct Shade {
    range: [f64; 2],
    alpha: f32,
    outline: bool,
}
fn panel(p: &Plot, origin: [f32; 2], all: &[(i64, f64)], layers: &[(&[(i64, f64)], Vec<[f32; 4]>)], shades: &[Shade], max: f64)
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
async fn render(marks: Vec<SceneMark>, size: [f32; 2], path: &str) -> Result<(), Error> {
    let mut scene_marks = vec![rects(&[[0.0, 0.0, size[0], size[1]]], vec![[1.0; 4]])];
    scene_marks.extend(marks);
    let scene = SceneGraph { width: size[0], height: size[1], origin: [0.0; 2], marks: vec![SceneGroup { marks: scene_marks, ..Default::default() }.into()] };
    let mut canvas = PngCanvas::new(CanvasDimensions { size, scale: 2.0 }, Default::default()).await?;
    canvas.set_scene(&scene)?;
    canvas.render().await?.save(path)?;
    println!("  -> {path}");
    Ok(())
}
fn heading(title: &str, sub: &str) -> Vec<SceneMark> {
    vec![text(title, 24.0, 30.0, 20.0, INK), text(sub, 24.0, 50.0, 11.0, [0.42, 0.45, 0.49, 1.])]
}
const TOP: f32 = 80.0;
const ROW: f32 = H + 80.0;
fn size(rows: f32) -> [f32; 2] {
    [W + 120.0, TOP + rows * ROW]
}

// ---------------------------------------------------------------------------
// The figures

/// 1. Jon's figure: a brush on arrival delay, cross-filtering the other two.
async fn crossfilter(ctx: &SessionContext, all: &[Vec<(i64, f64)>], n: usize, out: &str) -> Result<(), Error> {
    let brush = [60.0, 180.0];
    let delay = brush_producer(&PLOTS[0]);
    let state = empty().set(&delay, SelectionValue::tuple([(pid("value"), ValueTest::range(brush[0]..brush[1]))]))?;
    let mut marks = heading("Cross-Filter Flights", &format!("{} flights · arrival delay brushed from {} to {} min · each panel applies the other panels' brushes", thousands(n), brush[0], brush[1]));
    for (i, p) in PLOTS.iter().enumerate() {
        let t = Instant::now();
        let bins = histogram(ctx, p, cross(i).predicate(&state)?, None).await?;
        println!("  {}: {:.0} ms", p.name, ms(t));
        let max = all[i].iter().map(|x| x.1).fold(1.0, f64::max);
        let shade: Vec<Shade> = if i == 0 { vec![Shade { range: brush, alpha: 0.06, outline: true }] } else { vec![] };
        marks.push(panel(p, [80.0, TOP + i as f32 * ROW], &all[i], &[(&bins, vec![BLUE; bins.len()])], &shade, max)?);
    }
    render(marks, size(3.0), &format!("{out}/crossfilter.png")).await
}

/// Departure time against arrival delay, the density panel of figure 2.
const DENSITY_H: f32 = 300.0;
fn density_producer() -> ProducerDefinition {
    let (t, d) = (&PLOTS[1], &PLOTS[0]);
    ProducerDefinition::new(SelectionId::new("brush").unwrap(), ProducerId::new("lasso").unwrap(), ViewId::new("density").unwrap(),
        [Projection::new(pid("u"), col("time")).unwrap(), Projection::new(pid("v"), col("delay")).unwrap()]).unwrap()
        .with_pixel_grids([(pid("u"), grid(t.domain, [0.0, W as f64])), (pid("v"), grid(d.domain, [DENSITY_H as f64, 0.0]))]).unwrap()
}
/// The lasso of figure 2, in the density panel's pixels: evening departures
/// that arrive an hour or more late.
fn the_lasso() -> Vec<[f64; 2]> {
    [[395.0, 175.0], [470.0, 60.0], [560.0, 25.0], [595.0, 70.0], [590.0, 180.0], [520.0, 205.0], [440.0, 215.0]].to_vec()
}

/// 2. A lasso on a density panel, filtering the three histograms.
async fn lasso(ctx: &SessionContext, all: &[Vec<(i64, f64)>], n: usize, out: &str) -> Result<(), Error> {
    let p = density_producer();
    let ring = the_lasso();
    let value = SelectionValue::polygon(&p, &pid("u"), &pid("v"), &ring)?;
    let tuples = value.as_tuples().len();
    let state = empty().set(&p, value)?;
    // Counts per density cell: 15 minutes of departure by 5 minutes of delay.
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
    let mut marks = heading("A lasso on the density of departure time and arrival delay",
        &format!("{} flights · the lasso as {tuples} runs of 1 px cells (SelectionValue::polygon) · each histogram shows the flights inside it", thousands(n)));
    let mut group = axes(t.domain, d.domain, t.title, d.title, [0.0, 0.0], W, DENSITY_H)?;
    group.push(rects(&heat, fill));
    let ring32: Vec<[f32; 2]> = ring.iter().chain(ring.first()).map(|p| [p[0] as f32, p[1] as f32]).collect();
    group.push(outline(&ring32, [0.85, 0.2, 0.15, 1.0], 2.0));
    marks.push(SceneGroup { origin: [80.0, TOP], marks: group, ..Default::default() }.into());
    let base = TOP + DENSITY_H + 80.0;
    for (i, p) in PLOTS.iter().enumerate() {
        let t0 = Instant::now();
        let bins = histogram(ctx, p, cross(i).predicate(&state)?, None).await?;
        println!("  {}: {:.0} ms", p.name, ms(t0));
        let max = all[i].iter().map(|x| x.1).fold(1.0, f64::max);
        marks.push(panel(p, [80.0, base + i as f32 * ROW], &all[i], &[(&bins, vec![BLUE; bins.len()])], &[], max)?);
    }
    render(marks, [W + 120.0, base + 3.0 * ROW], &format!("{out}/lasso.png")).await
}

/// 3. A soft brush on distance: the other panels count each flight by its
/// degree, beside the hard brush.
async fn soft(ctx: &SessionContext, all: &[Vec<(i64, f64)>], n: usize, out: &str) -> Result<(), Error> {
    let brush = [1500.0, 2500.0];
    let width = 60.0;
    let dist = brush_producer(&PLOTS[2]);
    let state = empty().set(&dist, SelectionValue::tuple([(pid("value"), ValueTest::range(brush[0]..brush[1]))]))?;
    let light = [0.6, 0.75, 0.88, 1.0];
    let mut marks = heading("A soft brush on distance",
        &format!("{} flights · brush {}–{} miles, soft over {width} px (degree()) · light: counted by degree · dark: inside", thousands(n), brush[0], brush[1]));
    let reach = width as f64 * (PLOTS[2].domain[1] - PLOTS[2].domain[0]) / W as f64;
    // Grey to blue by degree.
    let tint = |k: f32| [0, 1, 2, 3].map(|c| GREY[c] + k * (BLUE[c] - GREY[c]));
    for (i, p) in PLOTS.iter().enumerate() {
        let t0 = Instant::now();
        let max = all[i].iter().map(|x| x.1).fold(1.0, f64::max);
        let origin = [80.0, TOP + i as f32 * ROW];
        if i == 2 {
            // The brushed panel: each bar tinted by the degree of its centre.
            let (a, z) = (p.px(brush[0], W) as f64, p.px(brush[1], W) as f64);
            let colours: Vec<[f32; 4]> = all[2].iter().map(|(b, _)| {
                let c = p.px(p.domain[0] + (*b as f64 + 0.5) * p.step, W) as f64;
                tint((1.0 - (a - c).max(c - z).max(0.0) / width).clamp(0.0, 1.0) as f32)
            }).collect();
            let shades = [Shade { range: [brush[0] - reach, brush[1] + reach], alpha: 0.03, outline: false }, Shade { range: brush, alpha: 0.06, outline: true }];
            marks.push(panel(p, origin, &all[2], &[(&all[2], colours)], &shades, max)?);
            continue;
        }
        let hard = histogram(ctx, p, cross(i).predicate(&state)?, None).await?;
        let faded = histogram(ctx, p, lit(true), Some(cross(i).degree(&state, width)?)).await?;
        println!("  {}: {:.0} ms", p.name, ms(t0));
        marks.push(panel(p, origin, &all[i], &[(&faded, vec![light; faded.len()]), (&hard, vec![BLUE; hard.len()])], &[], max)?);
    }
    render(marks, size(3.0), &format!("{out}/soft.png")).await
}

/// Figures 4 and 5's series. The flights have none, so the figures choose an
/// aggregation: one line per 200-mile band of distance (Jon's display bins),
/// the mean arrival delay per hour of departure, over hours with at least
/// 1,000 flights.
const BAND: f64 = 200.0;
const SERIES_H: f32 = 300.0;
const MEAN: [f64; 2] = [-10.0, 120.0];
fn band() -> Expr {
    cast(datafusion::functions::math::expr_fn::floor(col("distance") / lit(BAND)), DataType::Int64)
}
async fn series_rows(ctx: &SessionContext) -> DFResult<DataFrame> {
    let hour = cast(datafusion::functions::math::expr_fn::floor(col("time")), DataType::Float64) + lit(0.5);
    ctx.table("flights").await?
        .aggregate(vec![band().alias("band"), hour.alias("x")], vec![avg(col("delay")).alias("y"), count(lit(1)).alias("n")])?
        .filter(col("n").gt_eq(lit(1000)))
}
/// The series as vertices by band, in x order.
async fn series_lines(ctx: &SessionContext) -> DFResult<std::collections::BTreeMap<i64, Vec<[f64; 2]>>> {
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
fn series_producer() -> ProducerDefinition {
    ProducerDefinition::new(SelectionId::new("brush").unwrap(), ProducerId::new("series").unwrap(), ViewId::new("series").unwrap(),
        [Projection::new(pid("band"), band()).unwrap()]).unwrap()
}

/// 4 and 5. A test over whole series (a line brush or a timebox) on the
/// line chart, its keys as a selection that filters the three histograms.
async fn series(ctx: &SessionContext, all: &[Vec<(i64, f64)>], n: usize, test: SeriesTest, file: &str, out: &str) -> Result<(), Error> {
    let lines = series_lines(ctx).await?;
    let t0 = Instant::now();
    let keys = test.keys(series_rows(ctx).await?, col("band"), col("x"), col("y")).await?;
    println!("  {} series of {}: {:.0} ms", keys.len(), lines.len(), ms(t0));
    let chosen: std::collections::HashSet<i64> = keys.iter().map(|k| match k {
        datafusion::common::ScalarValue::Int64(Some(v)) => *v,
        other => panic!("a band: {other:?}"),
    }).collect();
    let state = empty().set(&series_producer(), test.value(&pid("band"), keys))?;
    let (t, d) = (&PLOTS[1], Plot { name: "mean", title: "Mean Arrival Delay (min)", domain: MEAN, step: 1.0 });
    let at = |p: [f64; 2]| [t.px(p[0], W), SERIES_H - d.px(p[1], SERIES_H)];
    let (title, what) = match &test {
        SeriesTest::Crosses { from, to } => ("A line brush on the delay by hour of each distance band",
            format!("the bands whose line crosses the segment ({}, {}) to ({}, {})", from[0], from[1], to[0], to[1])),
        SeriesTest::Within { x, y } => ("A timebox on the delay by hour of each distance band",
            format!("the bands whose line stays between {} and {} min from {}:00 to {}:00", y.0, y.1, x.0, x.1)),
    };
    let mut marks = heading(title, &format!("{} flights · one line per {BAND}-mile band of distance, the mean arrival delay per hour of departure", thousands(n)));
    marks.push(text(format!("{what} (SeriesTest): {} of {} bands · each histogram shows their flights", chosen.len(), lines.len()),
        24.0, 65.0, 11.0, [0.42, 0.45, 0.49, 1.]));
    let mut group = axes(t.domain, d.domain, t.title, d.title, [0.0, 0.0], W, SERIES_H)?;
    let red = [0.85, 0.2, 0.15, 1.0];
    if let SeriesTest::Within { x, y } = &test {
        let (a, b) = (at([x.0, y.1]), at([x.1, y.0]));
        group.push(rects(&[[a[0], a[1], b[0] - a[0], b[1] - a[1]]], vec![[0.85, 0.2, 0.15, 0.08]]));
        group.push(outline(&[a, [b[0], a[1]], b, [a[0], b[1]], a], red, 1.5));
    }
    for selected in [false, true] {
        for (k, l) in lines.iter().filter(|(k, _)| chosen.contains(k) == selected) {
            let pts: Vec<[f32; 2]> = l.iter().map(|p| at(*p)).collect();
            group.push(outline(&pts, if selected { BLUE } else { GREY }, if selected { 2.0 } else { 1.0 }));
            if selected {
                let end = pts.last().unwrap();
                group.push(text(format!("{}–{} mi", k * BAND as i64, (k + 1) * BAND as i64), end[0] + 4.0, end[1] + 3.0, 9.0, BLUE));
            }
        }
    }
    if let SeriesTest::Crosses { from, to } = &test {
        group.push(outline(&[at(*from), at(*to)], red, 2.5));
    }
    marks.push(SceneGroup { origin: [80.0, TOP], marks: group, ..Default::default() }.into());
    let base = TOP + SERIES_H + 80.0;
    for (i, p) in PLOTS.iter().enumerate() {
        let t0 = Instant::now();
        let bins = histogram(ctx, p, cross(i).predicate(&state)?, None).await?;
        println!("  {}: {:.0} ms", p.name, ms(t0));
        let max = all[i].iter().map(|x| x.1).fold(1.0, f64::max);
        marks.push(panel(p, [80.0, base + i as f32 * ROW], &all[i], &[(&bins, vec![BLUE; bins.len()])], &[], max)?);
    }
    render(marks, [W + 160.0, base + 3.0 * ROW], &format!("{out}/{file}")).await
}

/// 4. Redraws of one histogram (arrival delay) as the lasso moves, directly
/// and through the preaggregation split, checked against each other.
async fn timings(ctx: &SessionContext) -> Result<(), Error> {
    use avenger_datafusion_preaggregate::{BoundQuery, FilterQuery, PreaggregatePlanner};
    use datafusion::logical_expr::{LogicalPlan, LogicalPlanBuilder};
    let p = density_producer();
    let d = &PLOTS[0];
    let histogram = |rows: LogicalPlan| {
        let bin = cast(datafusion::functions::math::expr_fn::floor((col("delay") - lit(d.domain[0])) / lit(d.step)), DataType::Int64);
        LogicalPlanBuilder::from(rows).aggregate(vec![bin.alias("bin")], vec![count(lit(1_i64)).alias("n")])?.build()
    };
    let filter = ConsumerFilter::new(ViewId::new("delay").unwrap(), SelectionFilter::membership(&SelectionId::new("brush").unwrap(), EmptySelection::MatchAll));
    let states: Vec<SelectionSet> = [[0.0, 0.0], [-40.0, 0.0], [-80.0, 20.0], [-120.0, 40.0]].iter().map(|o| {
        let ring: Vec<[f64; 2]> = the_lasso().iter().map(|q| [q[0] + o[0], q[1] + o[1]]).collect();
        empty().set(&p, SelectionValue::polygon(&p, &pid("u"), &pid("v"), &ring).unwrap()).unwrap()
    }).collect();
    let source = ctx.table("flights").await?.into_unoptimized_plan();
    let split = filter.predicates(&states[0], &p)?.split().expect("a split").clone();
    let t0 = Instant::now();
    let prepared = PreaggregatePlanner::default().prepare(FilterQuery::new(source.clone(), histogram)?, split.dimensions().to_vec())?;
    let m = prepared.materialization_plan().expect("preaggregated").clone();
    let stored = ctx.execute_logical_plan(m.clone()).await?.collect().await?;
    let n_stored: usize = stored.iter().map(|b| b.num_rows()).sum();
    ctx.register_table("states", Arc::new(MemTable::try_new(Arc::new(m.schema().as_arrow().clone()), vec![stored])?))?;
    println!("  warm-up: {n_stored} stored rows in {:.0} ms", ms(t0));
    let stored = ctx.table("states").await?.into_unoptimized_plan();
    let direct = FilterQuery::new(source, histogram)?;
    for (k, state) in states.iter().enumerate() {
        let pr = filter.predicates(state, &p)?;
        let t0 = Instant::now();
        let a = ctx.execute_logical_plan(direct.direct(pr.full().clone())?).await?.collect().await?;
        let t_direct = ms(t0);
        let BoundQuery::Preaggregated { rollup, .. } = prepared.bind(pr.split().expect("a split").changing().clone())? else { panic!("direct") };
        let t0 = Instant::now();
        let b = ctx.execute_logical_plan(rollup.with_materialization(stored.clone())?).await?.collect().await?;
        let t_roll = ms(t0);
        let flat = |x: Vec<RecordBatch>| {
            let mut v: Vec<(i64, i64)> = x.iter().flat_map(|b| { let (k, n) = (i64s(b, "bin"), i64s(b, "n")); (0..b.num_rows()).map(move |i| (k.value(i), n.value(i))) }).collect();
            v.sort();
            v
        };
        println!("  lasso position {}: direct {t_direct:.0} ms, rollup {t_roll:.1} ms, same histogram: {}", k + 1, flat(a) == flat(b));
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let data = args.next().unwrap_or_else(|| "data/flights-10m.parquet".into());
    let out = args.next().unwrap_or_else(|| "experiments/10-mosaic-flights/images".into());
    std::fs::create_dir_all(&out)?;
    let ctx = SessionContext::new();
    let t0 = Instant::now();
    let raw = ctx.read_parquet(&data, ParquetReadOptions::default()).await?;
    println!("{} ({:?})", data, raw.schema().fields().iter().map(|f| format!("{}: {}", f.name(), f.data_type())).collect::<Vec<_>>());
    // Jon's transform (dataflow.rs): delay clipped to −60…180, hours and miles as they are.
    let delay = datafusion::functions::core::expr_fn::greatest(vec![lit(-60.0), datafusion::functions::core::expr_fn::least(vec![cast(ident("ARR_DELAY"), DataType::Float64), lit(180.0)])]);
    let rows = raw.select(vec![delay.alias("delay"), cast(ident("DEP_TIME"), DataType::Float64).alias("time"), cast(ident("DISTANCE"), DataType::Float64).alias("distance")])?
        .collect().await?;
    let n: usize = rows.iter().map(|b| b.num_rows()).sum();
    ctx.register_table("flights", Arc::new(MemTable::try_new(rows[0].schema(), vec![rows])?))?;
    println!("{n} flights in memory in {:.0} ms", ms(t0));
    let mut all = Vec::new();
    for p in &PLOTS {
        all.push(histogram(&ctx, p, lit(true), None).await?);
    }
    println!("figure 1, cross-filter:");
    crossfilter(&ctx, &all, n, &out).await?;
    println!("figure 2, lasso:");
    lasso(&ctx, &all, n, &out).await?;
    println!("figure 3, soft brush:");
    soft(&ctx, &all, n, &out).await?;
    println!("figure 4, line brush:");
    series(&ctx, &all, n, SeriesTest::Crosses { from: [21.6, 45.0], to: [22.6, 55.0] }, "linebrush.png", &out).await?;
    println!("figure 5, timebox:");
    series(&ctx, &all, n, SeriesTest::Within { x: (6.0, 12.0), y: (-5.0, 5.0) }, "timebox.png", &out).await?;
    println!("redraws of the arrival-delay histogram as the lasso moves:");
    timings(&ctx).await?;
    Ok(())
}
