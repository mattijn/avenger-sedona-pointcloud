//! Measures a lasso expressed through `avenger-selection` (the copy in
//! crates/, with `SelectionValue::polygon`) on the tile's raw points, against
//! the lasso experiment 7 draws with its own code:
//!
//! 1. agreement: the crate selects a point when its pixel cell's centre lies
//!    in the lasso; an exact test uses the point itself. Every point where
//!    the two differ should lie within one cell of the lasso's outline;
//! 2. cost, in memory: the crate's predicate in DataFusion, an exact
//!    point-in-polygon function in DataFusion, and a plain Rust loop;
//! 3. cost, from the file: the predicate alone, and with a bounding box that
//!    lets chunk statistics skip most of the tile.
//!
//! The two lassos are experiment 7's (bin/layer_roundtrip.rs): one on the
//! flat map of the whole tile, one in the tilted view of a 500 m window.
//!
//! Usage: cargo run --release -p lidar-probes --bin probe_selection -- <tile>

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{Array, ArrayRef, BooleanArray, Float64Array, RecordBatch};
use arrow::datatypes::DataType;
use avenger_scales_datafusion::BuiltinScale;
use avenger_selection::{
    ConsumerFilter, EmptySelection, PixelGrid, ProducerDefinition, ProducerId, Projection, ProjectionId, Resolution, SelectionFilter,
    SelectionId, SelectionSet, SelectionValue, ViewId,
};
use datafusion::common::Result as DFResult;
use datafusion::datasource::MemTable;
use datafusion::logical_expr::{
    col, lit, ColumnarValue, Expr, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use datafusion::prelude::{DataFrame, SessionContext};
use lidar_common::las_context;

/// Experiment 7's plot size in logical pixels.
const P: f64 = 400.0;

// ---------------------------------------------------------------------------
// The exact test, as a DataFusion function: is (u, v) inside the ring?

#[derive(Debug, PartialEq, Eq, Hash)]
struct InPolygon {
    ring: Vec<[u64; 2]>,
    signature: Signature,
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
fn in_polygon(ring: &[[f64; 2]], u: Expr, v: Expr) -> Expr {
    ScalarUDF::from(InPolygon {
        ring: ring.iter().map(|p| [p[0].to_bits(), p[1].to_bits()]).collect(),
        signature: Signature::exact(vec![DataType::Float64, DataType::Float64], Volatility::Immutable),
    })
    .call(vec![u, v])
}
impl ScalarUDFImpl for InPolygon {
    fn name(&self) -> &str {
        "probe_in_polygon"
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> DFResult<DataType> {
        Ok(DataType::Boolean)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> DFResult<ColumnarValue> {
        let ring: Vec<[f64; 2]> = self.ring.iter().map(|p| [f64::from_bits(p[0]), f64::from_bits(p[1])]).collect();
        let n = args.number_rows;
        let u = args.args[0].clone().into_array(n)?;
        let v = args.args[1].clone().into_array(n)?;
        let (u, v) = (u.as_any().downcast_ref::<Float64Array>().unwrap(), v.as_any().downcast_ref::<Float64Array>().unwrap());
        let out: BooleanArray = (0..n).map(|i| Some(u.is_valid(i) && v.is_valid(i) && in_ring([u.value(i), v.value(i)], &ring))).collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
    }
}

// ---------------------------------------------------------------------------
// A view: screen pixels as linear expressions of x, y and z.

#[derive(Clone)]
struct View {
    name: &'static str,
    /// px = a[0] + a[1] x + a[2] y + a[3] z, and py likewise.
    a: [f64; 4],
    b: [f64; 4],
    /// The lasso in experiment 7's unit square, y up.
    unit_ring: Vec<[f64; 2]>,
}
impl View {
    fn ring(&self) -> Vec<[f64; 2]> {
        self.unit_ring.iter().map(|q| [q[0] * P, (1.0 - q[1]) * P]).collect()
    }
    fn px(&self, c: &[f64; 4]) -> Expr {
        lit(c[0]) + lit(c[1]) * col("x") + lit(c[2]) * col("y") + lit(c[3]) * col("z")
    }
    fn at(&self, c: &[f64; 4], x: f64, y: f64, z: f64) -> f64 {
        c[0] + c[1] * x + c[2] * y + c[3] * z
    }
}

fn unit_ring(s: &str) -> Vec<[f64; 2]> {
    s.split(';').map(|p| {
        let (a, b) = p.split_once(',').unwrap();
        [a.parse().unwrap(), b.parse().unwrap()]
    }).collect()
}

/// The flat map: x and y scaled to the plot, screen y down.
fn flat(ext: [f64; 4]) -> View {
    let [x0, x1, y0, y1] = ext;
    View {
        name: "flat map, whole tile",
        a: [-x0 / (x1 - x0) * P, P / (x1 - x0), 0.0, 0.0],
        b: [P + y0 / (y1 - y0) * P, 0.0, -P / (y1 - y0), 0.0],
        unit_ring: unit_ring("0.6,0.35;0.8,0.35;0.8,0.55;0.6,0.55"),
    }
}

/// Experiment 7's tilt (layer/draw.rs `tilt`), unrolled into coefficients.
fn tilted(win: [f64; 4], hlo: f64, hhi: f64, yaw: f64, elevation: f64) -> View {
    let [x0, x1, y0, y1] = win;
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (se, ce) = elevation.to_radians().sin_cos();
    let s = P / std::f64::consts::SQRT_2 * 0.98;
    // Unit coordinates, centred: X = x/W - x0/W - 0.5, Y likewise, Z = 0.35 h.
    let (w, h, dz) = (x1 - x0, y1 - y0, hhi - hlo);
    let xc = [-x0 / w - 0.5, 1.0 / w, 0.0, 0.0];
    let yc = [-y0 / h - 0.5, 0.0, 1.0 / h, 0.0];
    let zc = [-0.35 * hlo / dz, 0.0, 0.0, 0.35 / dz];
    let lin = |f: &dyn Fn(usize) -> f64| [f(0), f(1), f(2), f(3)];
    let xr = lin(&|i| xc[i] * cy - yc[i] * sy);
    let yr = lin(&|i| xc[i] * sy + yc[i] * cy);
    let mut a = lin(&|i| s * xr[i]);
    a[0] += P / 2.0;
    let mut b = lin(&|i| -s * (yr[i] * se + zc[i] * ce));
    b[0] += P * 0.62;
    View {
        name: "tilted view, 500 m window (yaw 30, elevation 35)",
        a,
        b,
        unit_ring: unit_ring("0.42,0.52;0.75,0.55;0.78,0.32;0.45,0.28"),
    }
}

// ---------------------------------------------------------------------------

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}
async fn count(df: DataFrame) -> DFResult<usize> {
    df.count().await
}
async fn in_mem(mem: SessionContext, f: Expr) -> DFResult<usize> {
    count(mem.table("pts").await?.filter(f)?).await
}
/// The best of `n` runs, and the value the last one returned.
async fn best<F, Fut>(n: usize, mut f: F) -> DFResult<(f64, usize)>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = DFResult<usize>>,
{
    let (mut t, mut v) = (f64::MAX, 0);
    for _ in 0..n {
        let t0 = Instant::now();
        v = f().await?;
        t = t.min(ms(t0));
    }
    Ok((t, v))
}

fn grid(size: f64) -> PixelGrid {
    let a: ArrayRef = Arc::new(Float64Array::from(vec![0.0, P]));
    PixelGrid::new(BuiltinScale::Linear, a.clone(), a, HashMap::new(), 0.0, size).unwrap()
}

/// The lasso as avenger-selection sees it: two projections (screen pixels),
/// each on a pixel grid, and the polygon as tuples of cells.
fn selection(view: &View, size: f64) -> (Expr, usize) {
    selection_on(view, size, view.px(&view.a), view.px(&view.b))
}
fn selection_on(view: &View, size: f64, pu: Expr, pv: Expr) -> (Expr, usize) {
    let (u, v) = (ProjectionId::new("u").unwrap(), ProjectionId::new("v").unwrap());
    let (gu, gv) = (grid(size), grid(size));
    let sel = SelectionId::new("lasso").unwrap();
    let producer = ProducerDefinition::new(
        sel.clone(),
        ProducerId::new("lasso").unwrap(),
        ViewId::new("map").unwrap(),
        [Projection::new(u.clone(), pu).unwrap(), Projection::new(v.clone(), pv).unwrap()],
    )
    .unwrap()
    .with_pixel_grids([(u.clone(), gu.clone()), (v.clone(), gv.clone())])
    .unwrap();
    let value = SelectionValue::polygon((&u, &gu), (&v, &gv), &view.ring()).unwrap();
    let tuples = value.as_tuples().len();
    let state = SelectionSet::new([(sel.clone(), Resolution::Intersect)]).unwrap().set(&producer, value).unwrap();
    let filter = ConsumerFilter::new(ViewId::new("points").unwrap(), SelectionFilter::membership(&sel, EmptySelection::MatchNone));
    (filter.predicate(&state).unwrap(), tuples)
}

/// A plain two-dimensional brush on u and v, in pixels, at 1 px cells.
fn one_box(u0: f64, u1: f64, v0: f64, v1: f64) -> Expr {
    use avenger_selection::ValueTest;
    let (u, v) = (ProjectionId::new("u").unwrap(), ProjectionId::new("v").unwrap());
    let sel = SelectionId::new("lasso").unwrap();
    let producer = ProducerDefinition::new(sel.clone(), ProducerId::new("box").unwrap(), ViewId::new("map").unwrap(),
        [Projection::new(u.clone(), col("u")).unwrap(), Projection::new(v.clone(), col("v")).unwrap()]).unwrap()
        .with_pixel_grids([(u.clone(), grid(1.0)), (v.clone(), grid(1.0))]).unwrap();
    let value = SelectionValue::tuple([(u, ValueTest::range(u0..=u1)), (v, ValueTest::range(v0..=v1))]);
    let state = SelectionSet::new([(sel.clone(), Resolution::Intersect)]).unwrap().set(&producer, value).unwrap();
    ConsumerFilter::new(ViewId::new("points").unwrap(), SelectionFilter::membership(&sel, EmptySelection::MatchNone)).predicate(&state).unwrap()
}

fn column(b: &RecordBatch, name: &str) -> Float64Array {
    let c = b.column_by_name(name).unwrap();
    arrow::compute::cast(c, &DataType::Float64).unwrap().as_any().downcast_ref::<Float64Array>().unwrap().clone()
}

fn dist_to_ring(p: [f64; 2], ring: &[[f64; 2]]) -> f64 {
    (0..ring.len()).map(|i| {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
        let t = (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
        (p[0] - a[0] - t * vx).hypot(p[1] - a[1] - t * vy)
    }).fold(f64::MAX, f64::min)
}

/// Soft selection (experiment 7's `--soft`): the crate's `degree()` against
/// experiment 7's formula, which measures the continuous distance from the
/// brush; the crate measures from the row's cell. Both in pixels here.
async fn soft(view: &View, points: &[RecordBatch]) -> DFResult<()> {
    let width = 0.05 * P; // experiment 7's widths are in the unit square
    let (x0, x1, y0, y1) = (0.6 * P, 0.8 * P, 0.45 * P, 0.65 * P);
    println!("\n## soft selection on the flat map: brush {:.0}..{:.0} x {:.0}..{:.0} px, width {width} px", x0, x1, y0, y1);
    let mem = SessionContext::new();
    mem.register_table("pts", Arc::new(MemTable::try_new(points[0].schema(), vec![points.to_vec()])?))?;
    let (u, v) = (ProjectionId::new("u").unwrap(), ProjectionId::new("v").unwrap());
    let sel = SelectionId::new("brush").unwrap();
    for (label, size) in [("1 px", 1.0), ("4 px", 4.0)] {
        let producer = ProducerDefinition::new(sel.clone(), ProducerId::new("brush").unwrap(), ViewId::new("map").unwrap(),
            [Projection::new(u.clone(), view.px(&view.a)).unwrap(), Projection::new(v.clone(), view.px(&view.b)).unwrap()]).unwrap()
            .with_pixel_grids([(u.clone(), grid(size)), (v.clone(), grid(size))]).unwrap();
        let value = SelectionValue::tuple([(u.clone(), avenger_selection::ValueTest::range(x0..=x1)), (v.clone(), avenger_selection::ValueTest::range(y0..=y1))]);
        let state = SelectionSet::new([(sel.clone(), Resolution::Intersect)]).unwrap().set(&producer, value).unwrap();
        let filter = ConsumerFilter::new(ViewId::new("points").unwrap(), SelectionFilter::membership(&sel, EmptySelection::MatchNone));
        let degree = filter.degree(&state, width).unwrap();
        let (t_degree, _) = best(3, || async { let df = mem.table("pts").await?.select(vec![degree.clone().alias("d")])?; count(df.filter(col("d").gt(lit(0.0)))?).await }).await?;
        let out = mem.table("pts").await?.select(vec![degree.alias("d"), view.px(&view.a).alias("u"), view.px(&view.b).alias("v")])?.collect().await?;
        let pred = filter.predicate(&state).unwrap();
        let (t_pred, _) = best(3, || in_mem(mem.clone(), pred.clone())).await?;
        let (mut worst, mut faded, mut half_crate, mut half_e7) = (0.0_f64, 0usize, 0usize, 0usize);
        for b in &out {
            let (d, uu, vv) = (column(b, "d"), column(b, "u"), column(b, "v"));
            for i in 0..b.num_rows() {
                let (pu, pv) = (uu.value(i), vv.value(i));
                let dist = (x0 - pu).max(pu - x1).max(0.0).hypot((y0 - pv).max(pv - y1).max(0.0));
                let e7 = (1.0 - dist / width).clamp(0.0, 1.0);
                worst = worst.max((d.value(i) - e7).abs());
                faded += (d.value(i) > 0.0 && d.value(i) < 1.0) as usize;
                half_crate += (d.value(i) >= 0.5) as usize;
                half_e7 += (e7 >= 0.5) as usize;
            }
        }
        println!("cells of {label}: degree for every point {t_degree:>6.0} ms (the predicate alone {t_pred:.0} ms); \
                  {faded} points faded; largest difference from experiment 7's formula {worst:.3} \
                  (a cell diagonal over the width is {:.3}); degree >= 0.5: {half_crate} here, {half_e7} there",
                 size * std::f64::consts::SQRT_2 / width);
    }
    // A soft lasso: the distance to its nearest run of cells, 80 of them.
    let (_, tuples) = selection(view, 1.0);
    let (uid, vid) = (ProjectionId::new("u").unwrap(), ProjectionId::new("v").unwrap());
    let lasso = ProducerDefinition::new(sel.clone(), ProducerId::new("lasso").unwrap(), ViewId::new("map").unwrap(),
        [Projection::new(uid.clone(), view.px(&view.a)).unwrap(), Projection::new(vid.clone(), view.px(&view.b)).unwrap()]).unwrap()
        .with_pixel_grids([(uid.clone(), grid(1.0)), (vid.clone(), grid(1.0))]).unwrap();
    let value = SelectionValue::polygon((&uid, &grid(1.0)), (&vid, &grid(1.0)), &view.ring()).unwrap();
    let state = SelectionSet::new([(sel.clone(), Resolution::Intersect)]).unwrap().set(&lasso, value).unwrap();
    let degree = ConsumerFilter::new(ViewId::new("points").unwrap(), SelectionFilter::membership(&sel, EmptySelection::MatchNone)).degree(&state, width).unwrap();
    let (t, _) = best(3, || async { let df = mem.table("pts").await?.select(vec![degree.clone().alias("d")])?; count(df.filter(col("d").gt(lit(0.0)))?).await }).await?;
    println!("soft lasso, {tuples} runs of 1 px cells: degree for every point {t:.0} ms");
    Ok(())
}

/// Series selections on experiment 7's flight lines (layer/data.rs): points
/// per half second of each flight line. A line brush and a timebox with the
/// values of bin/layer_roundtrip.rs find their lines in one query each; the
/// keys then select those lines' raw points. On a growing tile, as a stream
/// delivers it, the chart rebuilds the lines and the keys.
async fn series(ctx: &SessionContext, tile: &str) -> DFResult<()> {
    use avenger_selection::SeriesTest;
    println!("\n## series: experiment 7's flight lines");
    let raw = ctx.sql(&format!("SELECT point_source_id, gps_time FROM '{tile}' ORDER BY gps_time")).await?.collect().await?;
    let mem = SessionContext::new();
    mem.register_table("input", Arc::new(MemTable::try_new(raw[0].schema(), vec![raw.clone()])?))?;
    let flight_sql = |upto: Option<f64>| {
        let w = upto.map_or(String::new(), |t| format!(" WHERE gps_time <= {t}"));
        format!("WITH i AS (SELECT * FROM input{w}) \
                 SELECT point_source_id AS line, floor((gps_time - t0) * 2) / 2 AS t, CAST(count(*) AS DOUBLE) AS n \
                 FROM i JOIN (SELECT point_source_id AS l, min(gps_time) AS t0 FROM i GROUP BY point_source_id) m \
                 ON i.point_source_id = m.l GROUP BY point_source_id, floor((gps_time - t0) * 2) / 2")
    };
    let tests = [
        ("line brush 18.9,185000 → 21.6,145000", SeriesTest::Crosses { from: [18.9, 185000.0], to: [21.6, 145000.0] }),
        ("timebox t 14..18, n 120000..200000", SeriesTest::Within { x: (14.0, 18.0), y: (120000.0, 200000.0) }),
        // Experiment 7's timebox takes no line of this tile; this one takes two of four.
        ("timebox t 14..18, n 80000..130000", SeriesTest::Within { x: (14.0, 18.0), y: (80000.0, 130000.0) }),
    ];
    let t0 = Instant::now();
    let flight = mem.sql(&flight_sql(None)).await?.collect().await?;
    let t_build = ms(t0);
    let n_rows: usize = flight.iter().map(|b| b.num_rows()).sum();
    // Experiment 7's own loop, on the same rows.
    let mut lines: std::collections::BTreeMap<i64, Vec<[f64; 2]>> = Default::default();
    for b in &flight {
        let l = arrow::compute::cast(b.column_by_name("line").unwrap(), &DataType::Int64)?;
        let l = l.as_any().downcast_ref::<arrow::array::Int64Array>().unwrap();
        let (t, n) = (column(b, "t"), column(b, "n"));
        for i in 0..b.num_rows() {
            lines.entry(l.value(i)).or_default().push([t.value(i), n.value(i)]);
        }
    }
    for pts in lines.values_mut() {
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    }
    println!("{} flight lines, {n_rows} rows, built from {} points in {t_build:.0} ms", lines.len(), raw.iter().map(|b| b.num_rows()).sum::<usize>());
    for (l, pts) in &lines {
        let w: Vec<f64> = pts.iter().filter(|p| p[0] >= 14.0 && p[0] <= 18.0).map(|p| p[1]).collect();
        println!("  line {l}: t 0..{:.1}, n over t 14..18: {:.0}..{:.0}", pts.last().unwrap()[0],
                 w.iter().cloned().fold(f64::MAX, f64::min), w.iter().cloned().fold(f64::MIN, f64::max));
    }
    let fm = SessionContext::new();
    fm.register_table("flight", Arc::new(MemTable::try_new(flight[0].schema(), vec![flight.clone()])?))?;
    let o = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    for (label, test) in &tests {
        let want: Vec<i64> = lines.iter().filter(|(_, pts)| match test {
            SeriesTest::Crosses { from, to } => pts.windows(2).any(|w| o(*from, *to, w[0]) * o(*from, *to, w[1]) <= 0.0 && o(w[0], w[1], *from) * o(w[0], w[1], *to) <= 0.0),
            SeriesTest::Within { x, y } => {
                let within: Vec<&[f64; 2]> = pts.iter().filter(|p| p[0] >= x.0 && p[0] <= x.1).collect();
                !within.is_empty() && within.iter().all(|p| p[1] >= y.0 && p[1] <= y.1)
            }
        }).map(|(l, _)| *l).collect();
        let mut keys = Vec::new();
        let mut t_keys = f64::MAX;
        for _ in 0..3 {
            let t0 = Instant::now();
            keys = test.keys(fm.table("flight").await?, col("line"), col("t"), col("n")).await.unwrap();
            t_keys = t_keys.min(ms(t0));
        }
        let got: Vec<i64> = keys.iter().map(|k| match k.cast_to(&DataType::Int64).unwrap() { datafusion::common::ScalarValue::Int64(Some(v)) => v, _ => -1 }).collect();
        // The keys select the raw points of those lines.
        let key = ProjectionId::new("line").unwrap();
        let sel = SelectionId::new("series").unwrap();
        let producer = ProducerDefinition::new(sel.clone(), ProducerId::new("brush").unwrap(), ViewId::new("lines").unwrap(),
            [Projection::new(key.clone(), col("point_source_id")).unwrap()]).unwrap();
        let state = SelectionSet::new([(sel.clone(), Resolution::Intersect)]).unwrap().set(&producer, SeriesTest::value(&key, keys.clone())).unwrap();
        let pred = ConsumerFilter::new(ViewId::new("points").unwrap(), SelectionFilter::cross_filter([&sel])).predicate(&state).unwrap();
        let (t_pts, n_pts) = best(3, || async { count(mem.table("input").await?.filter(pred.clone())?).await }).await?;
        println!("{label}: lines {got:?} in {t_keys:.1} ms; experiment 7's loop agrees: {}; their raw points: {n_pts} in {t_pts:.0} ms",
                 if got == want { "yes".to_string() } else { format!("NO, it has {want:?}") });
    }
    // As a stream would deliver the tile, in gps_time order: rebuild the
    // lines and the keys after each quarter of the points. (Quarters of the
    // flight time would not do: the lines are minutes apart.)
    let times: Vec<f64> = raw.iter().flat_map(|b| { let c = column(b, "gps_time"); (0..b.num_rows()).map(move |i| c.value(i)).collect::<Vec<_>>() }).collect();
    for q in [0.25, 0.5, 0.75, 1.0] {
        let upto = times[((q * times.len() as f64) as usize).min(times.len() - 1)];
        let t0 = Instant::now();
        let fl = mem.sql(&flight_sql(Some(upto))).await?.collect().await?;
        let t_b = ms(t0);
        let n_lines = { let mut ls: Vec<String> = Vec::new(); for b in &fl { let c = b.column_by_name("line").unwrap(); for i in 0..b.num_rows() { ls.push(arrow::util::display::array_value_to_string(c, i)?); } } ls.sort(); ls.dedup(); ls.len() };
        let n_fl: usize = fl.iter().map(|b| b.num_rows()).sum();
        let f2 = SessionContext::new();
        f2.register_table("flight", Arc::new(MemTable::try_new(fl[0].schema(), vec![fl])?))?;
        let t0 = Instant::now();
        let mut k = Vec::new();
        for (_, test) in &tests {
            k.push(test.keys(f2.table("flight").await?, col("line"), col("t"), col("n")).await.unwrap().len());
        }
        println!("first {:>3.0} % of the points: {n_lines} lines in {n_fl} rows, rebuilt in {t_b:>5.0} ms; the three key sets in {:>5.1} ms ({:?} lines)", q * 100.0, ms(t0), k);
    }
    Ok(())
}

async fn measure(ctx: &SessionContext, view: &View, points: &[RecordBatch]) -> DFResult<()> {
    let ring = view.ring();
    let n_points: usize = points.iter().map(|b| b.num_rows()).sum();
    println!("\n## {} — {} points, lasso of {} vertices", view.name, n_points, ring.len());

    let mem = SessionContext::new();
    mem.register_table("pts", Arc::new(MemTable::try_new(points[0].schema(), vec![points.to_vec()])?))?;
    let exact = in_polygon(&ring, view.px(&view.a), view.px(&view.b));

    // A plain Rust loop over the same arrays: the floor for any engine.
    let t0 = Instant::now();
    let mut rust_n = 0;
    for b in points {
        let (x, y, z) = (column(b, "x"), column(b, "y"), column(b, "z"));
        for i in 0..b.num_rows() {
            let (xi, yi, zi) = (x.value(i), y.value(i), z.value(i));
            rust_n += in_ring([view.at(&view.a, xi, yi, zi), view.at(&view.b, xi, yi, zi)], &ring) as usize;
        }
    }
    let rust_ms = ms(t0);
    let (exact_ms, exact_n) = best(3, || in_mem(mem.clone(), exact.clone())).await?;
    println!("exact point in polygon   Rust loop {rust_ms:>8.1} ms   {rust_n} points");
    println!("                         DataFusion {exact_ms:>7.1} ms   {exact_n} points");

    for size in [1.0, 2.0, 4.0] {
        let t0 = Instant::now();
        let (pred, tuples) = selection(view, size);
        let build_ms = ms(t0);
        let (sel_ms, sel_n) = best(3, || in_mem(mem.clone(), pred.clone())).await?;
        // Where the crate and the exact test disagree, and how far from the outline.
        let differ = mem.table("pts").await?
            .select(vec![view.px(&view.a).alias("u"), view.px(&view.b).alias("v"), pred.clone().alias("sel"), exact.clone().alias("exact")])?
            .filter(col("sel").not_eq(col("exact")))?
            .collect()
            .await?;
        let mut worst: f64 = 0.0;
        let mut n_differ = 0;
        for b in &differ {
            let (u, v) = (column(b, "u"), column(b, "v"));
            for i in 0..b.num_rows() {
                worst = worst.max(dist_to_ring([u.value(i), v.value(i)], &ring));
                n_differ += 1;
            }
        }
        println!(
            "avenger-selection {size} px  {tuples:>4} tuples, built in {build_ms:.2} ms   DataFusion {sel_ms:>7.1} ms   {sel_n} points; \
             {n_differ} differ from exact ({:.3} %), all within {worst:.2} px of the outline (a cell's diagonal is {:.2} px)",
            100.0 * n_differ as f64 / exact_n.max(1) as f64,
            size * std::f64::consts::SQRT_2
        );
    }
    // The same, with the screen coordinates computed once as columns: does
    // the time go into repeating the projection in every tuple?
    let with_uv = mem.table("pts").await?.select(vec![view.px(&view.a).alias("u"), view.px(&view.b).alias("v")])?.collect().await?;
    let uvm = SessionContext::new();
    uvm.register_table("pts", Arc::new(MemTable::try_new(with_uv[0].schema(), vec![with_uv])?))?;
    let (pred, tuples) = selection_on(view, 1.0, col("u"), col("v"));
    let (t, n) = best(3, || in_mem(uvm.clone(), pred.clone())).await?;
    let (t0, _) = best(3, || in_mem(uvm.clone(), in_polygon(&ring, col("u"), col("v")))).await?;
    println!("u, v as columns, 1 px   {tuples:>4} tuples   DataFusion {t:>7.1} ms   {n} points   (exact on the same columns {t0:.1} ms)");
    // The cost of one tuple: the ring's bounding box as one two-range tuple.
    let (bu, bv) = (ring.iter().map(|p| p[0]), ring.iter().map(|p| p[1]));
    let (u0, u1) = (bu.clone().fold(f64::MAX, f64::min), bu.fold(f64::MIN, f64::max));
    let (v0, v1) = (bv.clone().fold(f64::MAX, f64::min), bv.fold(f64::MIN, f64::max));
    let pred = one_box(u0, u1, v0, v1);
    let (t, n) = best(3, || in_mem(uvm.clone(), pred.clone())).await?;
    println!("one tuple (the ring's bounding box)       DataFusion {t:>7.1} ms   {n} points");
    let text = format!("{}", selection_on(view, 1.0, col("u"), col("v")).0);
    println!("in the 1 px predicate: {} calls of the pixel-cell function, {} of the finite check",
             text.matches("avenger_selection_pixel").count(), text.matches("avenger_selection_finite").count());
    let _ = ctx;
    Ok(())
}

#[tokio::main]
async fn main() -> DFResult<()> {
    let tile = std::env::args().nth(1).unwrap_or_else(|| "data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz".into());
    let ctx = las_context();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?.collect().await?;
    let pts = |w: &str| format!("SELECT x, y, CAST(z AS DOUBLE) AS z FROM '{tile}'{w}");

    let e = ctx.sql(&format!("SELECT min(x), max(x), min(y), max(y) FROM '{tile}'")).await?.collect().await?;
    let ext = [0, 1, 2, 3].map(|i| column(&e[0], e[0].schema().field(i).name()).value(0));
    println!("# A lasso through avenger-selection, on {tile}");
    println!("extent x {:.0}..{:.0}, y {:.0}..{:.0}; plot {P} px", ext[0], ext[1], ext[2], ext[3]);

    // 1 and 2: in memory.
    let t0 = Instant::now();
    let all = ctx.sql(&pts("")).await?.collect().await?;
    println!("loaded the tile into memory in {:.0} ms", ms(t0));
    let view = flat(ext);
    measure(&ctx, &view, &all).await?;
    soft(&view, &all).await?;
    drop(all);

    let win = [657500.0, 658000.0, 6867250.0, 6867750.0];
    let wsql = format!(" WHERE x BETWEEN {} AND {} AND y BETWEEN {} AND {}", win[0], win[1], win[2], win[3]);
    let part = ctx.sql(&pts(&wsql)).await?.collect().await?;
    let (mut hlo, mut hhi) = (f64::MAX, f64::MIN);
    for b in &part {
        let z = column(b, "z");
        for i in 0..b.num_rows() {
            hlo = hlo.min(z.value(i));
            hhi = hhi.max(z.value(i));
        }
    }
    let tilt = tilted(win, hlo, hhi, 30.0, 35.0);
    measure(&ctx, &tilt, &part).await?;
    drop(part);

    series(&ctx, &tile).await?;

    // 3: from the file, flat lasso at 1 px. A bounding box in data units,
    // one cell wider than the lasso, derived from the ring through the view.
    let (pred, _) = selection(&view, 1.0);
    let ring = view.ring();
    let inv = |c: &[f64; 4], i: usize, p: f64| (p - c[0]) / c[i];
    let (xs, ys): (Vec<f64>, Vec<f64>) = ring.iter().map(|p| (inv(&view.a, 1, p[0]), inv(&view.b, 2, p[1]))).unzip();
    let pad = 1.0 / view.a[1];
    // Plain comparisons, as bench_window writes them, for the chunk pruning.
    let bbox = col("x").gt_eq(lit(xs.iter().cloned().fold(f64::MAX, f64::min) - pad))
        .and(col("x").lt_eq(lit(xs.iter().cloned().fold(f64::MIN, f64::max) + pad)))
        .and(col("y").gt_eq(lit(ys.iter().cloned().fold(f64::MAX, f64::min) - pad)))
        .and(col("y").lt_eq(lit(ys.iter().cloned().fold(f64::MIN, f64::max) + pad)));
    println!("\n## from the file, flat lasso at 1 px");
    let file = |ctx: SessionContext, f: Expr| {
        let q = pts("");
        async move { count(ctx.sql(&q).await?.filter(f)?).await }
    };
    let (t, n) = best(2, || file(ctx.clone(), col("z").gt(lit(-1e9)))).await?;
    println!("no filter, full scan                      {t:>7.0} ms   {n} points");
    let (t, n) = best(2, || file(ctx.clone(), pred.clone())).await?;
    println!("predicate alone, full scan              {t:>7.0} ms   {n} points");
    // Statistics persist in a sidecar (<tile>.stats, git-ignored); without it
    // every query by path builds them again.
    // A new session: one that has read the tile without statistics keeps
    // using what it listed then.
    let _ = std::fs::remove_file(format!("{tile}.stats"));
    let ctx = las_context();
    for stmt in ["SET las.geometry_encoding = 'plain'", "SET las.collect_statistics = 'true'", "SET las.parallel_statistics_extraction = 'true'", "SET las.persist_statistics = 'true'"] {
        ctx.sql(stmt).await?.collect().await?;
    }
    let t0 = Instant::now();
    let n = file(ctx.clone(), bbox.clone()).await?;
    println!("first query with chunk statistics         {:>7.0} ms   {n} points (builds the statistics)", ms(t0));
    let (t, n) = best(3, || file(ctx.clone(), bbox.clone())).await?;
    println!("bounding box alone, chunk statistics      {t:>7.0} ms   {n} points");
    let (t, n) = best(3, || file(ctx.clone(), pred.clone())).await?;
    println!("predicate alone, chunk statistics         {t:>7.0} ms   {n} points");
    let (t, n) = best(3, || file(ctx.clone(), bbox.clone().and(pred.clone()))).await?;
    println!("bounding box and predicate, statistics    {t:>7.0} ms   {n} points");
    Ok(())
}
