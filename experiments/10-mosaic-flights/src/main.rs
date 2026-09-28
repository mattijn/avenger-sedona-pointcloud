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

use arrow::array::RecordBatch;
use arrow::datatypes::DataType;
use avenger_scenegraph::marks::group::SceneGroup;
use avenger_selection::{ConsumerFilter, EmptySelection, SelectionFilter, SelectionId, SelectionSet, SelectionValue, SeriesTest, ValueTest, ViewId};
use datafusion::datasource::MemTable;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::logical_expr::{cast, col, lit};
use datafusion::prelude::SessionContext;
use lidar_flights::*;


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


/// 2. A lasso on a density panel, filtering the three histograms.
async fn lasso(ctx: &SessionContext, all: &[Vec<(i64, f64)>], n: usize, out: &str) -> Result<(), Error> {
    let p = density_producer();
    let ring = the_lasso();
    let value = SelectionValue::polygon(&p, &pid("u"), &pid("v"), &ring)?;
    let tuples = value.as_tuples().len();
    let state = empty().set(&p, value)?;
    let (t, d) = (&PLOTS[1], &PLOTS[0]);
    let (heat, fill) = density(ctx).await?;
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
    let n = load(&ctx, &data).await?;
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
