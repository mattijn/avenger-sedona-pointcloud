//! Measures the selections of experiment 7 through `avenger-selection` (the
//! copy in crates/, with this repo's additions) on the tile's raw points,
//! against experiment 7's own algorithms run as plain Rust on the same
//! points: lasso, brush, soft brush and soft lasso, CloudLasso, line brush
//! and timebox (hard and soft), the preaggregation split, the selection log,
//! and a lasso read from the file. Each section checks that the two agree;
//! a table of the timings closes the run.
//!
//! Usage: cargo run --release -p lidar-probes --bin probe_selection -- <tile>

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch};
use arrow::datatypes::DataType;
use avenger_scales_datafusion::BuiltinScale;
use avenger_selection::{
    ConsumerFilter, EmptySelection, Gesture, LogEntry, PixelGrid, ProducerDefinition, ProducerId, Producers, Projection,
    ProjectionId, Resolution, SelectionFilter, SelectionId, SelectionSet, SelectionUpdate, SelectionValue, SeriesTest, ValueTest,
    ViewId,
};
use datafusion::common::{Result as DFResult, ScalarValue};
use datafusion::datasource::MemTable;
use datafusion::functions_aggregate::expr_fn::{count as count_agg, sum as total};
use datafusion::logical_expr::{
    col, lit, ColumnarValue, Expr, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use datafusion::prelude::SessionContext;
use lidar_common::las_context;

/// Experiment 7's plot size in logical pixels.
const P: f64 = 400.0;

// ---------------------------------------------------------------------------
// Small pieces

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}
/// The best time of `n` runs, and what the last run returned.
async fn best<T, F, Fut>(n: usize, mut f: F) -> DFResult<(f64, T)>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = DFResult<T>>,
{
    let mut t = f64::MAX;
    let mut v = None;
    for _ in 0..n {
        let t0 = Instant::now();
        v = Some(f().await?);
        t = t.min(ms(t0));
    }
    Ok((t, v.expect("n > 0")))
}
fn f64s(b: &RecordBatch, name: &str) -> Float64Array {
    let c = arrow::compute::cast(b.column_by_name(name).unwrap(), &DataType::Float64).unwrap();
    c.as_any().downcast_ref::<Float64Array>().unwrap().clone()
}
fn i64s(b: &RecordBatch, name: &str) -> Int64Array {
    let c = arrow::compute::cast(b.column_by_name(name).unwrap(), &DataType::Int64).unwrap();
    c.as_any().downcast_ref::<Int64Array>().unwrap().clone()
}
fn rows(points: &[RecordBatch]) -> usize {
    points.iter().map(|b| b.num_rows()).sum()
}
fn int(k: &ScalarValue) -> i64 {
    match k.cast_to(&DataType::Int64) {
        Ok(ScalarValue::Int64(Some(v))) => v,
        _ => -1,
    }
}
fn yes(ok: bool) -> &'static str {
    if ok { "yes" } else { "NO" }
}

/// A session holding `points` as the table `name`.
fn table(name: &str, points: &[RecordBatch]) -> DFResult<SessionContext> {
    let ctx = SessionContext::new();
    ctx.register_table(name, Arc::new(MemTable::try_new(points[0].schema(), vec![points.to_vec()])?))?;
    Ok(ctx)
}
async fn count_where(ctx: &SessionContext, name: &str, f: Expr) -> DFResult<usize> {
    ctx.table(name).await?.filter(f)?.count().await
}

fn pid(name: &str) -> ProjectionId {
    ProjectionId::new(name).unwrap()
}
fn sid(name: &str) -> SelectionId {
    SelectionId::new(name).unwrap()
}
/// A producer of `selection`, named `name`, in the view `map`.
fn producer(selection: &str, name: &str, projections: Vec<(&str, Expr)>, grids: Vec<(&str, PixelGrid)>) -> ProducerDefinition {
    let p = ProducerDefinition::new(sid(selection), ProducerId::new(name).unwrap(), ViewId::new("map").unwrap(),
        projections.into_iter().map(|(n, e)| Projection::new(pid(n), e).unwrap())).unwrap();
    p.with_pixel_grids(grids.into_iter().map(|(n, g)| (pid(n), g))).unwrap()
}
fn set(p: &ProducerDefinition, v: SelectionValue) -> SelectionSet {
    SelectionSet::new([(p.selection().clone(), Resolution::Intersect)]).unwrap().set(p, v).unwrap()
}
/// Membership, as a view other than the producer's reads it.
fn members(selection: &str) -> ConsumerFilter {
    ConsumerFilter::new(ViewId::new("points").unwrap(), SelectionFilter::membership(&sid(selection), EmptySelection::MatchNone))
}
fn linear(domain: [f64; 2], range: [f64; 2], size: f64) -> PixelGrid {
    let a = |v: [f64; 2]| -> ArrayRef { Arc::new(Float64Array::from(v.to_vec())) };
    PixelGrid::new(BuiltinScale::Linear, a(domain), a(range), HashMap::new(), 0.0, size).unwrap()
}
/// Screen pixels, one grid cell per `size` pixels.
fn screen(size: f64) -> PixelGrid {
    linear([0.0, P], [0.0, P], size)
}

/// The timings the run ends with: what experiment 7's Rust takes, and the crate.
#[derive(Default)]
struct Summary(Vec<(String, String, f64, f64, String)>);
impl Summary {
    fn add(&mut self, what: &str, data: impl Into<String>, rust: f64, krate: f64, agree: impl Into<String>) {
        self.0.push((what.into(), data.into(), rust, krate, agree.into()));
    }
    fn print(&self) {
        println!("\n## Timings: experiment 7's Rust against avenger-selection, on the same data");
        println!("| Selection | Data | Rust | avenger-selection | Agree |\n|---|---|---|---|---|");
        for (w, d, r, k, a) in &self.0 {
            let t = |x: f64| match x {
                x if x.is_nan() => "—".into(),
                x if x < 0.1 => format!("{:.0} µs", x * 1e3),
                x if x < 10.0 => format!("{x:.1} ms"),
                x => format!("{x:.0} ms"),
            };
            println!("| {w} | {d} | {} | {} | {a} |", t(*r), t(*k));
        }
    }
}

// ---------------------------------------------------------------------------
// Experiment 7's geometry, as plain Rust (layer/model.rs)

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
/// Experiment 7's `crosses`: segments that share a point.
fn crosses(p1: [f64; 2], p2: [f64; 2], q1: [f64; 2], q2: [f64; 2]) -> bool {
    let o = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let within = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        c[0] >= a[0].min(b[0]) && c[0] <= a[0].max(b[0]) && c[1] >= a[1].min(b[1]) && c[1] <= a[1].max(b[1])
    };
    let (d1, d2, d3, d4) = (o(q1, q2, p1), o(q1, q2, p2), o(p1, p2, q1), o(p1, p2, q2));
    (d1 * d2 < 0.0 && d3 * d4 < 0.0)
        || (d1 == 0.0 && within(q1, q2, p1))
        || (d2 == 0.0 && within(q1, q2, p2))
        || (d3 == 0.0 && within(p1, p2, q1))
        || (d4 == 0.0 && within(p1, p2, q2))
}
fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
    let t = (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / (vx * vx + vy * vy).max(1e-12)).clamp(0.0, 1.0);
    (p[0] - a[0] - t * vx).hypot(p[1] - a[1] - t * vy)
}
fn to_ring(p: [f64; 2], ring: &[[f64; 2]]) -> f64 {
    (0..ring.len()).map(|i| to_segment(p, ring[i], ring[(i + 1) % ring.len()])).fold(f64::MAX, f64::min)
}
/// The regions of dense voxels, joined across faces, edges and corners, and
/// the one with the most points (layer/model.rs `lasso_take`).
fn largest_region(count: &HashMap<(i64, i64, i64), usize>, structure: f64) -> (HashSet<(i64, i64, i64)>, usize) {
    let top = count.values().copied().max().unwrap_or(0);
    let dense: HashSet<_> = count.iter().filter(|(_, c)| **c as f64 >= structure * top as f64).map(|(v, _)| *v).collect();
    let (mut seen, mut best, mut best_n, mut regions) = (HashSet::new(), HashSet::new(), 0, 0);
    for v in &dense {
        if !seen.insert(*v) {
            continue;
        }
        regions += 1;
        let (mut stack, mut region, mut n) = (vec![*v], HashSet::new(), 0);
        while let Some(c) = stack.pop() {
            region.insert(c);
            n += count[&c];
            for d in 0..27 {
                let q = (c.0 + d % 3 - 1, c.1 + d / 3 % 3 - 1, c.2 + d / 9 - 1);
                if dense.contains(&q) && seen.insert(q) {
                    stack.push(q);
                }
            }
        }
        if n > best_n {
            (best, best_n) = (region, n);
        }
    }
    (best, regions)
}

/// The exact test as a DataFusion function: is (u, v) inside the ring?
#[derive(Debug, PartialEq, Eq, Hash)]
struct InPolygon {
    ring: Vec<[u64; 2]>,
    signature: Signature,
}
fn in_polygon(ring: &[[f64; 2]], u: Expr, v: Expr) -> Expr {
    ScalarUDF::from(InPolygon {
        ring: ring.iter().map(|p| p.map(f64::to_bits)).collect(),
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
        let ring: Vec<[f64; 2]> = self.ring.iter().map(|p| p.map(f64::from_bits)).collect();
        let n = args.number_rows;
        let (u, v) = (args.args[0].clone().into_array(n)?, args.args[1].clone().into_array(n)?);
        let (u, v) = (u.as_any().downcast_ref::<Float64Array>().unwrap(), v.as_any().downcast_ref::<Float64Array>().unwrap());
        let out: BooleanArray = (0..n).map(|i| Some(u.is_valid(i) && v.is_valid(i) && in_ring([u.value(i), v.value(i)], &ring))).collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
    }
}

// ---------------------------------------------------------------------------
// Views: screen pixels as linear expressions of x, y and z

#[derive(Clone)]
struct View {
    name: &'static str,
    /// px = a[0] + a[1] x + a[2] y + a[3] z, and py likewise.
    a: [f64; 4],
    b: [f64; 4],
    /// Experiment 7's lasso for this view, in its unit square (y up).
    unit_ring: Vec<[f64; 2]>,
}
impl View {
    fn ring(&self) -> Vec<[f64; 2]> {
        self.unit_ring.iter().map(|q| [q[0] * P, (1.0 - q[1]) * P]).collect()
    }
    fn u(&self) -> Expr {
        Self::expr(&self.a)
    }
    fn v(&self) -> Expr {
        Self::expr(&self.b)
    }
    fn expr(c: &[f64; 4]) -> Expr {
        lit(c[0]) + lit(c[1]) * col("x") + lit(c[2]) * col("y") + lit(c[3]) * col("z")
    }
    fn at(&self, x: f64, y: f64, z: f64) -> [f64; 2] {
        let f = |c: &[f64; 4]| c[0] + c[1] * x + c[2] * y + c[3] * z;
        [f(&self.a), f(&self.b)]
    }
    /// The lasso's producer: u and v on the screen's grid.
    fn lasso(&self, selection: &str, size: f64) -> ProducerDefinition {
        producer(selection, "lasso", vec![("u", self.u()), ("v", self.v())], vec![("u", screen(size)), ("v", screen(size))])
    }
    /// Screen positions of the rows, in plain Rust.
    fn each(&self, points: &[RecordBatch], mut f: impl FnMut([f64; 2], [f64; 3])) {
        for b in points {
            let (x, y, z) = (f64s(b, "x"), f64s(b, "y"), f64s(b, "z"));
            for i in 0..b.num_rows() {
                let p = [x.value(i), y.value(i), z.value(i)];
                f(self.at(p[0], p[1], p[2]), p);
            }
        }
    }
}
fn unit_ring(s: &str) -> Vec<[f64; 2]> {
    s.split(';').map(|p| p.split(',').map(|v| v.parse().unwrap()).collect::<Vec<f64>>()).map(|v| [v[0], v[1]]).collect()
}
/// The flat map of the whole tile: x and y scaled to the plot, screen y down.
fn flat(ext: [f64; 4]) -> View {
    let [x0, x1, y0, y1] = ext;
    View {
        name: "flat map, whole tile",
        a: [-x0 / (x1 - x0) * P, P / (x1 - x0), 0.0, 0.0],
        b: [P + y0 / (y1 - y0) * P, 0.0, -P / (y1 - y0), 0.0],
        unit_ring: unit_ring("0.6,0.35;0.8,0.35;0.8,0.55;0.6,0.55"),
    }
}
/// Experiment 7's tilt (layer/draw.rs `tilt`) over a window, as coefficients.
fn tilted(win: [f64; 4], hz: (f64, f64), yaw: f64, elevation: f64) -> View {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (se, ce) = elevation.to_radians().sin_cos();
    let s = P / std::f64::consts::SQRT_2 * 0.98;
    let (w, h, dz) = (win[1] - win[0], win[3] - win[2], hz.1 - hz.0);
    // Centred unit coordinates, and height at 0.35 of the plot.
    let x = [-win[0] / w - 0.5, 1.0 / w, 0.0, 0.0];
    let y = [-win[2] / h - 0.5, 0.0, 1.0 / h, 0.0];
    let z = [-0.35 * hz.0 / dz, 0.0, 0.0, 0.35 / dz];
    let mut a = [0.0; 4].map(|_| 0.0);
    let mut b = a;
    for i in 0..4 {
        a[i] = s * (x[i] * cy - y[i] * sy);
        b[i] = -s * ((x[i] * sy + y[i] * cy) * se + z[i] * ce);
    }
    a[0] += P / 2.0;
    b[0] += P * 0.62;
    View { name: "tilted view, 500 m window (yaw 30, elevation 35)", a, b, unit_ring: unit_ring("0.42,0.52;0.75,0.55;0.78,0.32;0.45,0.28") }
}

// ---------------------------------------------------------------------------
// 1. The lasso

async fn lasso(view: &View, points: &[RecordBatch], sum: &mut Summary) -> DFResult<()> {
    let ring = view.ring();
    println!("\n## lasso: {} ({} points)", view.name, rows(points));
    let mem = table("pts", points)?;
    let t0 = Instant::now();
    let mut rust_n = 0;
    view.each(points, |q, _| rust_n += in_ring(q, &ring) as usize);
    let rust = ms(t0);
    let exact = in_polygon(&ring, view.u(), view.v());
    let (t_exact, exact_n) = best(3, || count_where(&mem, "pts", exact.clone())).await?;
    println!("exact point in polygon: Rust {rust:.0} ms, a DataFusion function {t_exact:.0} ms; {rust_n} points");
    for size in [1.0, 2.0, 4.0] {
        let p = view.lasso("lasso", size);
        let value = SelectionValue::polygon(&p, &pid("u"), &pid("v"), &ring).unwrap();
        let tuples = value.as_tuples().len();
        let pred = members("lasso").predicate(&set(&p, value)).unwrap();
        let (t, n) = best(3, || count_where(&mem, "pts", pred.clone())).await?;
        // Where the cell-centre rule and the exact test disagree, and how far out.
        let differ = mem.table("pts").await?
            .select(vec![view.u().alias("u"), view.v().alias("v"), pred.clone().alias("a"), exact.clone().alias("b")])?
            .filter(col("a").not_eq(col("b")))?.collect().await?;
        let (mut n_diff, mut worst) = (0, 0.0_f64);
        for b in &differ {
            let (u, v) = (f64s(b, "u"), f64s(b, "v"));
            for i in 0..b.num_rows() {
                worst = worst.max(to_ring([u.value(i), v.value(i)], &ring));
                n_diff += 1;
            }
        }
        let calls = format!("{pred}").matches("avenger_selection_pixel_cell").count();
        println!("avenger-selection, {size} px cells: {tuples} runs, {t:.0} ms, {n} points; {n_diff} differ from exact \
                  ({:.2} %), all within {worst:.2} px of the outline; {calls} pixel-cell calls per row",
                 100.0 * n_diff as f64 / exact_n.max(1) as f64);
        if size == 1.0 {
            let within = worst <= size * std::f64::consts::SQRT_2;
            sum.add("lasso", format!("{} points, {}", rows(points), view.name), rust, t, format!("{} (±1 cell at the edge)", yes(within)));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Brushes, hard and soft, and a soft lasso

async fn brushes(view: &View, points: &[RecordBatch], sum: &mut Summary) -> DFResult<()> {
    let width = 0.05 * P; // experiment 7's `--soft 0.05`, in pixels
    let (x0, x1, y0, y1) = (0.6 * P, 0.8 * P, 0.45 * P, 0.65 * P);
    println!("\n## brush {x0:.0}..{x1:.0} × {y0:.0}..{y1:.0} px on the flat map, soft width {width} px");
    let mem = table("pts", points)?;
    let dist = |q: [f64; 2]| (x0 - q[0]).max(q[0] - x1).max(0.0).hypot((y0 - q[1]).max(q[1] - y1).max(0.0));
    // The brush as a 1 px grid has it: whole cells, its edge cells included.
    let t0 = Instant::now();
    let mut rust_in = 0;
    let inside = |c: f64, lo: f64, hi: f64| c.floor() >= lo.floor() && c.floor() <= hi.floor();
    view.each(points, |q, _| rust_in += (inside(q[0], x0, x1) && inside(q[1], y0, y1)) as usize);
    let rust_hard = ms(t0);
    let t0 = Instant::now();
    let mut soft_n = 0;
    view.each(points, |q, _| soft_n += ((1.0 - dist(q) / width).clamp(0.0, 1.0) > 0.0) as usize);
    let rust_soft = ms(t0);
    for size in [1.0, 4.0] {
        let p = producer("brush", "brush", vec![("u", view.u()), ("v", view.v())], vec![("u", screen(size)), ("v", screen(size))]);
        let st = set(&p, SelectionValue::tuple([(pid("u"), ValueTest::range(x0..=x1)), (pid("v"), ValueTest::range(y0..=y1))]));
        let (pred, degree) = (members("brush").predicate(&st).unwrap(), members("brush").degree(&st, width).unwrap());
        let (t_hard, n_hard) = best(3, || count_where(&mem, "pts", pred.clone())).await?;
        let (t_soft, _) = best(3, || count_where(&mem, "pts", degree.clone().gt(lit(0.0)))).await?;
        // Experiment 7's degree for the same rows, from their u and v.
        let mut worst = 0.0_f64;
        for b in mem.table("pts").await?.select(vec![degree.alias("d"), view.u().alias("u"), view.v().alias("v")])?.collect().await? {
            let (d, u, v) = (f64s(&b, "d"), f64s(&b, "u"), f64s(&b, "v"));
            for i in 0..b.num_rows() {
                worst = worst.max((d.value(i) - (1.0 - dist([u.value(i), v.value(i)]) / width).clamp(0.0, 1.0)).abs());
            }
        }
        let bound = size * std::f64::consts::SQRT_2 / width;
        println!("{size} px cells: brush {t_hard:.0} ms ({n_hard} points; Rust at 1 px {rust_in}), soft {t_soft:.0} ms (Rust {soft_n} points reached); \
                  largest difference from experiment 7's degree {worst:.3}, within a cell's diagonal ({bound:.3}): {}", yes(worst <= bound + 1e-9));
        if size == 1.0 {
            sum.add("brush", format!("{} points", rows(points)), rust_hard, t_hard, format!("{} ({n_hard} and {rust_in} points)", yes(n_hard.abs_diff(rust_in) * 1000 <= rust_in)));
            sum.add("soft brush", format!("{} points", rows(points)), rust_soft, t_soft, format!("{} (≤ {worst:.3})", yes(worst <= bound + 1e-9)));
        }
    }
    let p = view.lasso("brush", 1.0);
    let degree = members("brush").degree(&set(&p, SelectionValue::polygon(&p, &pid("u"), &pid("v"), &view.ring()).unwrap()), width).unwrap();
    let (t, _) = best(3, || count_where(&mem, "pts", degree.clone().gt(lit(0.0)))).await?;
    let ring = view.ring();
    let t0 = Instant::now();
    let mut faded = 0;
    view.each(points, |q, _| faded += (!in_ring(q, &ring) && to_ring(q, &ring) < width) as usize);
    println!("soft lasso: {t:.0} ms (Rust, distance to the outline: {:.0} ms)", ms(t0));
    sum.add("soft lasso", format!("{} points", rows(points)), ms(t0), t, "not compared here (tests: degree 1 = predicate)");
    Ok(())
}

// ---------------------------------------------------------------------------
// 3. CloudLasso

/// The chart's side of CloudLasso: voxel counts of what the state selects,
/// the largest dense region, and its voxels as a value carrying the lasso
/// as a `"cloudlasso"` gesture.
#[derive(Clone)]
struct Cloud {
    mem: SessionContext,
    voxels: ProducerDefinition,
    grids: [PixelGrid; 3],
    ring: Vec<[f64; 2]>,
    structure: f64,
}
impl Cloud {
    async fn voxels(&self, state: &SelectionSet) -> DFResult<(SelectionValue, usize)> {
        let [gx, gy, gz] = &self.grids;
        let counts = self.mem.table("pts").await?
            .filter(members("cloud").predicate(state).unwrap())?
            .aggregate(vec![gx.cell_expr(col("x")).alias("i"), gy.cell_expr(col("y")).alias("j"), gz.cell_expr(col("z")).alias("k")],
                       vec![count_agg(lit(1)).alias("n")])?
            .collect().await?;
        let mut count = HashMap::new();
        for b in &counts {
            let (i, j, k, n) = (i64s(b, "i"), i64s(b, "j"), i64s(b, "k"), i64s(b, "n"));
            for r in 0..b.num_rows() {
                count.insert((i.value(r), j.value(r), k.value(r)), n.value(r) as usize);
            }
        }
        let (best, regions) = largest_region(&count, self.structure);
        let cells = best.iter().map(|c| vec![c.0, c.1, c.2]);
        let gesture = Gesture::new("cloudlasso", self.ring.iter().copied()).on([pid("u"), pid("v")]).with_param("structure", self.structure);
        Ok((SelectionValue::cells(&self.voxels, &[&pid("vx"), &pid("vy"), &pid("vz")], cells).unwrap().with_gesture(gesture), regions))
    }
}

async fn cloud_lasso(view: &View, win: [f64; 4], hz: (f64, f64), points: &[RecordBatch], sum: &mut Summary)
    -> DFResult<(Cloud, Vec<ProducerDefinition>, Vec<SelectionUpdate>)> {
    let structure = 0.3;
    println!("\n## CloudLasso in the tilted view (structure {structure}; voxels 1/40 of the window across, 1/10 of its height)");
    let ring = view.ring();
    let (gx, gy, gz) = (linear([win[0], win[1]], [0.0, 40.0], 1.0), linear([win[2], win[3]], [0.0, 40.0], 1.0), linear([hz.0, hz.1], [0.0, 10.0], 1.0));
    let voxels = producer("cloud", "voxels", vec![("vx", col("x")), ("vy", col("y")), ("vz", col("z"))],
                          vec![("vx", gx.clone()), ("vy", gy.clone()), ("vz", gz.clone())]);
    let lasso = view.lasso("cloud", 1.0);
    let cloud = Cloud { mem: table("pts", points)?, voxels: voxels.clone(), grids: [gx, gy, gz], ring: ring.clone(), structure };
    let t0 = Instant::now();
    let drawn = SelectionValue::polygon(&lasso, &pid("u"), &pid("v"), &ring).unwrap();
    let gesture = drawn.gesture().unwrap().clone().with_param("structure", structure);
    let set_lasso = SelectionUpdate::set(&lasso, drawn.with_gesture(gesture));
    let state = set(&lasso, SelectionValue::default()).apply(set_lasso.clone()).unwrap();
    let (value, regions) = cloud.voxels(&state).await?;
    let n_vox = value.as_tuples().len();
    let set_voxels = SelectionUpdate::set(&voxels, value);
    let n = count_where(&cloud.mem, "pts", members("cloud").predicate(&state.apply(set_voxels.clone()).unwrap()).unwrap()).await?;
    let t = ms(t0);
    // Experiment 7's algorithm in plain Rust, on the same points.
    let t0 = Instant::now();
    let (mut rc, mut inside) = (HashMap::new(), Vec::new());
    let cell = |v: f64, lo: f64, hi: f64, n: f64| ((v - lo) / (hi - lo) * n).floor() as i64;
    view.each(points, |q, p| {
        if in_ring([q[0].floor() + 0.5, q[1].floor() + 0.5], &ring) {
            let k = (cell(p[0], win[0], win[1], 40.0), cell(p[1], win[2], win[3], 40.0), cell(p[2], hz.0, hz.1, 10.0));
            *rc.entry(k).or_insert(0usize) += 1;
            inside.push(k);
        }
    });
    let (rbest, rregions) = largest_region(&rc, structure);
    let rn = inside.iter().filter(|k| rbest.contains(k)).count();
    let rust = ms(t0);
    println!("avenger-selection and the chart: {regions} dense regions, the largest {n_vox} voxels, {n} points, {t:.0} ms");
    println!("experiment 7's Rust:             {rregions} dense regions, the largest {} voxels, {rn} points, {rust:.0} ms", rbest.len());
    sum.add("CloudLasso", format!("{} points, tilted", rows(points)), rust, t, format!("{:.2} % apart (Float32 voxel edges)", 100.0 * n.abs_diff(rn) as f64 / rn as f64));
    Ok((cloud, vec![lasso, voxels], vec![set_lasso, set_voxels]))
}

// ---------------------------------------------------------------------------
// 4. The preaggregation split

/// A class histogram cross-filtered by a focus, through the planner of
/// avenger-datafusion-preaggregate as avenger-selection's own example uses
/// it: counts stored per class and interaction dimension at warm-up, each
/// redraw rolled up from them and checked against the direct query.
async fn split(label: &str, points: &[RecordBatch], focus: &ProducerDefinition, states: &[SelectionSet], fade: bool, sum: &mut Summary) -> DFResult<()> {
    use avenger_datafusion_preaggregate::{BoundQuery, FilterQuery, PreaggregatePlanner};
    use datafusion::logical_expr::{LogicalPlan, LogicalPlanBuilder};
    let ctx = table("pts", points)?;
    let histogram = |rows: LogicalPlan| LogicalPlanBuilder::from(rows).aggregate(vec![col("class")], vec![count_agg(lit(1_i64)).alias("n")])?.build();
    let sorted = |b: Vec<RecordBatch>| {
        let mut out: Vec<(i64, f64)> = b.iter().flat_map(|x| {
            let (k, v) = (i64s(x, x.schema().field(0).name()), f64s(x, x.schema().field(1).name()));
            (0..x.num_rows()).map(move |i| (k.value(i), v.value(i)))
        }).collect();
        out.sort_by_key(|x| x.0);
        out
    };
    let run = |plan: LogicalPlan| {
        let ctx = ctx.clone();
        async move { ctx.execute_logical_plan(plan).await?.collect().await }
    };
    let filter = ConsumerFilter::new(ViewId::new("classes").unwrap(), SelectionFilter::cross_filter([focus.selection()]));
    let source = ctx.table("pts").await?.into_unoptimized_plan();
    let direct = FilterQuery::new(source.clone(), histogram)?;
    let split = filter.predicates(&states[0], focus).unwrap().split().expect("a split").clone();
    let fixed = split.fixed().clone();
    let prepared = PreaggregatePlanner::default()
        .prepare(FilterQuery::new(source, |rows| histogram(LogicalPlanBuilder::from(rows).filter(fixed)?.build()?))?, split.dimensions().to_vec())?;
    let materialization = prepared.materialization_plan().expect("preaggregated").clone();
    let t0 = Instant::now();
    let stored = run(materialization.clone()).await?;
    let t_warm = ms(t0);
    println!("{label}: warm-up stored {} rows from {} in {t_warm:.0} ms", rows(&stored), rows(points));
    ctx.register_table("states", Arc::new(MemTable::try_new(Arc::new(materialization.schema().as_arrow().clone()), vec![stored])?))?;
    let stored = ctx.table("states").await?.into_unoptimized_plan();
    let (mut roll, mut dir, mut same) = (Vec::new(), Vec::new(), true);
    for state in states {
        let p = filter.predicates(state, focus).unwrap();
        let rollup = match prepared.bind(p.split().expect("the same split").changing().clone())? {
            BoundQuery::Preaggregated { rollup, .. } => rollup.with_materialization(stored.clone())?,
            BoundQuery::Direct { .. } => panic!("the planner fell back to a direct query"),
        };
        let (tr, r) = best(1, || run(rollup.clone())).await?;
        let (td, d) = best(1, || run(direct.direct(p.full().clone()).unwrap())).await?;
        same &= sorted(r) == sorted(d);
        roll.push(tr);
        dir.push(td);
    }
    let span = |v: &[f64]| format!("{:.1}–{:.1} ms", v.iter().cloned().fold(f64::MAX, f64::min), v.iter().cloned().fold(0.0, f64::max));
    println!("  {} redraws: rollup {}, direct {}; same histograms: {}", states.len(), span(&roll), span(&dir), yes(same));
    sum.add(&format!("split: {label}"), format!("{} redraws", states.len()), f64::NAN, roll.iter().sum::<f64>() / roll.len() as f64,
            format!("{} (direct {:.0} ms)", yes(same), dir.iter().sum::<f64>() / dir.len() as f64));
    if fade {
        // A fade: the focus's degree over stored pixel cells, times the counts.
        let dims = split.dimensions().to_vec();
        let warm = ctx.table("pts").await?.filter(split.fixed().clone())?
            .aggregate([col("class")].into_iter().chain(dims.iter().enumerate().map(|(i, d)| d.clone().alias(format!("d{i}")))).collect(),
                       vec![count_agg(lit(1_i64)).alias("n")])?.collect().await?;
        ctx.register_table("fade", Arc::new(MemTable::try_new(warm[0].schema(), vec![warm])?))?;
        let (mut worst, mut tr, mut td) = (0.0_f64, 0.0, 0.0);
        for state in states {
            let keys = (0..dims.len()).map(|k| col(format!("d{k}"))).collect();
            let deg = filter.focus_degree(state, focus, 20.0, keys).unwrap();
            let n = datafusion::logical_expr::cast(col("n"), DataType::Float64);
            let (t1, r) = best(1, || async { ctx.table("fade").await?.aggregate(vec![col("class")], vec![total(n.clone() * deg.clone())])?.collect().await }).await?;
            let full = filter.degree(state, 20.0).unwrap();
            let (t2, d) = best(1, || async { ctx.table("pts").await?.aggregate(vec![col("class")], vec![total(full.clone())])?.collect().await }).await?;
            for (a, b) in sorted(r).iter().zip(sorted(d)) {
                worst = worst.max((a.1 - b.1).abs() / b.1.abs().max(1.0));
            }
            (tr, td) = (tr + t1 / states.len() as f64, td + t2 / states.len() as f64);
        }
        println!("  a fade (soft 20 px): {tr:.1} ms from stored cells, {td:.0} ms over the points; largest relative difference {worst:.0e}");
        sum.add("split: fade", format!("{} redraws", states.len()), f64::NAN, tr, format!("{} (direct {td:.0} ms)", yes(worst < 1e-9)));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 5. Series: experiment 7's flight lines

/// Experiment 7's flight lines (layer/data.rs): points per half second of
/// each line, from the points delivered up to `upto` in gps_time.
fn flight_sql(upto: Option<f64>) -> String {
    let w = upto.map_or(String::new(), |t| format!(" WHERE gps_time <= {t}"));
    format!("WITH i AS (SELECT * FROM input{w}) \
             SELECT point_source_id AS line, floor((gps_time - t0) * 2) / 2 AS t, CAST(count(*) AS DOUBLE) AS n \
             FROM i JOIN (SELECT point_source_id AS l, min(gps_time) AS t0 FROM i GROUP BY point_source_id) m \
             ON i.point_source_id = m.l GROUP BY point_source_id, floor((gps_time - t0) * 2) / 2")
}

/// Experiment 7's loops over its lines: selected, or a degree when soft.
fn e7_series(test: &SeriesTest, pts: &[[f64; 2]], soft: Option<(f64, [f64; 2])>) -> f64 {
    match test {
        SeriesTest::Crosses { from, to } => {
            if pts.windows(2).any(|w| crosses(w[0], w[1], *from, *to)) {
                return 1.0;
            }
            let Some((width, s)) = soft else { return 0.0 };
            let unit = |p: [f64; 2]| [p[0] * s[0], p[1] * s[1]];
            (1.0 - pts.iter().map(|p| to_segment(unit(*p), unit(*from), unit(*to))).fold(f64::MAX, f64::min) / width).clamp(0.0, 1.0)
        }
        SeriesTest::Within { x, y } => {
            let within: Vec<_> = pts.iter().filter(|p| p[0] >= x.0 && p[0] <= x.1).collect();
            let inside = within.iter().filter(|p| p[1] >= y.0 && p[1] <= y.1).count();
            match soft {
                _ if within.is_empty() => 0.0,
                Some(_) => inside as f64 / within.len() as f64,
                None => (inside == within.len()) as u8 as f64,
            }
        }
    }
}

async fn series(ctx: &SessionContext, tile: &str, sum: &mut Summary) -> DFResult<(SessionContext, Vec<ProducerDefinition>, Vec<SelectionUpdate>)> {
    println!("\n## series: experiment 7's flight lines");
    let raw = ctx.sql(&format!("SELECT point_source_id, gps_time FROM '{tile}' ORDER BY gps_time")).await?.collect().await?;
    let input = table("input", &raw)?;
    let t0 = Instant::now();
    let flight = input.sql(&flight_sql(None)).await?.collect().await?;
    println!("{} rows of lines from {} points in {:.0} ms", rows(&flight), rows(&raw), ms(t0));
    let fm = table("flight", &flight)?;
    let mut lines: BTreeMap<i64, Vec<[f64; 2]>> = BTreeMap::new();
    for b in &flight {
        let (l, t, n) = (i64s(b, "line"), f64s(b, "t"), f64s(b, "n"));
        for i in 0..b.num_rows() {
            lines.entry(l.value(i)).or_default().push([t.value(i), n.value(i)]);
        }
    }
    lines.values_mut().for_each(|p| p.sort_by(|a, b| a[0].total_cmp(&b[0])));
    // Experiment 7's unit square: t over 0..ceil(max t), n over nice(0, max n).
    let (tmax, nmax) = lines.values().flatten().fold((0.0_f64, 0.0_f64), |(a, b), p| (a.max(p[0]), b.max(p[1])));
    let step = [1.0, 2.0, 5.0, 10.0].map(|m| m * 10f64.powf((nmax / 5.0).log10().floor())).into_iter().find(|s| nmax / s <= 6.0).unwrap();
    let scale = [1.0 / tmax.ceil(), 1.0 / ((nmax / step).ceil() * step)];
    let brush = SeriesTest::Crosses { from: [18.9, 185000.0], to: [21.6, 145000.0] };
    let tests = [
        ("line brush", brush.clone(), None),
        // Experiment 7's timebox, which takes no line of this tile, then one that takes two of four.
        ("timebox", SeriesTest::Within { x: (14.0, 18.0), y: (120000.0, 200000.0) }, None),
        ("timebox", SeriesTest::Within { x: (14.0, 18.0), y: (80000.0, 130000.0) }, None),
        ("soft line brush", brush, Some(0.2)),
        ("soft timebox", SeriesTest::Within { x: (14.0, 18.0), y: (120000.0, 200000.0) }, Some(0.1)),
    ];
    let key = pid("line");
    let p = ProducerDefinition::new(sid("series"), ProducerId::new("brush").unwrap(), ViewId::new("lines").unwrap(),
        [Projection::new(key.clone(), col("point_source_id")).unwrap()]).unwrap();
    let mut updates = Vec::new();
    for (label, test, soft) in &tests {
        let t0 = Instant::now();
        let want: Vec<(i64, f64)> = lines.iter().map(|(l, pts)| (*l, e7_series(test, pts, soft.map(|w| (w, scale))))).filter(|x| x.1 > 0.0).collect();
        let rust = ms(t0);
        let rows_ = || fm.table("flight");
        let (t, value) = match soft {
            None => best(3, || async { Ok(test.value(&key, test.keys(rows_().await?, col("line"), col("t"), col("n")).await.unwrap())) }).await?,
            Some(w) => best(3, || async {
                let d = test.degrees(rows_().await?, col("line"), col("t"), col("n"), scale, *w).await.unwrap();
                Ok(test.soft_value(&key, d, scale, *w))
            }).await?,
        };
        let state = set(&p, value.clone());
        let degree = ConsumerFilter::new(ViewId::new("points").unwrap(), SelectionFilter::cross_filter([&sid("series")])).degree(&state, 1.0).unwrap();
        let got: Vec<(i64, f64)> = {
            let mut g: Vec<(i64, f64)> = value.as_tuples().iter().flatten().flat_map(|(_, t)| match t {
                ValueTest::OneOf(vs) => vs.iter().map(|v| (int(v), 1.0)).collect(),
                _ => vec![],
            }).collect();
            g.extend(value.partial().map_or(vec![], |(_, d)| d.iter().map(|(k, v)| (int(k), *v)).collect()));
            g.sort_by_key(|x| x.0);
            g
        };
        let agree = got.len() == want.len() && got.iter().zip(&want).all(|(a, b)| a.0 == b.0 && (a.1 - b.1).abs() < 1e-9);
        let (t_pts, n) = best(3, || count_where(&input, "input", degree.clone().gt(lit(0.0)))).await?;
        println!("{label} {}: {} in {t:.1} ms (Rust {:.0} µs), agree: {}; {n} raw points reached in {t_pts:.0} ms",
                 serde_json::to_string(&test.gesture().points()).unwrap(),
                 got.iter().map(|(l, d)| if *d == 1.0 { format!("{l}") } else { format!("{l}: {d:.3}") }).collect::<Vec<_>>().join(", "),
                 rust * 1e3, yes(agree));
        if !(label.starts_with("timebox") && updates.len() == 2) {
            sum.add(label, format!("{} lines, {} rows", lines.len(), rows(&flight)), rust, t, yes(agree));
        }
        updates.push(SelectionUpdate::set(&p, value));
    }
    // As a stream would deliver the tile, in gps_time order: the lines and the
    // keys again after each quarter of the points (quarters of the flight time
    // would not do: the lines are minutes apart).
    let times: Vec<f64> = raw.iter().flat_map(|b| f64s(b, "gps_time").values().to_vec()).collect();
    for q in [0.25, 0.5, 0.75, 1.0] {
        let t0 = Instant::now();
        let fl = input.sql(&flight_sql(Some(times[((q * times.len() as f64) as usize).min(times.len() - 1)]))).await?.collect().await?;
        let t_lines = ms(t0);
        let f2 = table("flight", &fl)?;
        let t0 = Instant::now();
        let mut k = Vec::new();
        for (_, test, _) in &tests[..3] {
            k.push(test.keys(f2.table("flight").await?, col("line"), col("t"), col("n")).await.unwrap().len());
        }
        println!("first {:>3.0} % of the points: lines {t_lines:.0} ms, the three key sets {:.1} ms ({k:?} lines)", q * 100.0, ms(t0));
    }
    Ok((fm, vec![p], updates))
}

// ---------------------------------------------------------------------------
// 6. The selection log

/// CloudLasso's and the series' updates, a clear and a toggle, written to
/// out/selection_log.jsonl, read back and replayed: the chart draws its own
/// gestures again. The replay must give the same contributions and points.
async fn log(cloud: &Cloud, defs: &[ProducerDefinition], mut updates: Vec<SelectionUpdate>, flight: &SessionContext) -> DFResult<()> {
    println!("\n## a selection log");
    updates.push(SelectionUpdate::clear(&defs[1]));
    updates.push(updates[1].clone());
    updates.push(SelectionUpdate::toggle(&defs[2], SelectionValue::tuple([(pid("line"), ValueTest::equal(33_i64))])));
    let lines: Vec<String> = updates.iter().map(|u| serde_json::to_string(&u.to_json().unwrap()).unwrap()).collect();
    std::fs::create_dir_all("out").ok();
    std::fs::write("out/selection_log.jsonl", lines.join("\n") + "\n").ok();
    let producers = Producers::new(defs.iter().cloned());
    let start = SelectionSet::new([(sid("cloud"), Resolution::Intersect), (sid("series"), Resolution::Intersect)]).unwrap();
    let (mut a, mut b, mut same, mut drawn) = (start.clone(), start, true, 0);
    let t0 = Instant::now();
    for (u, line) in updates.into_iter().zip(std::fs::read_to_string("out/selection_log.jsonl").unwrap().lines()) {
        a = a.apply(u).unwrap();
        let r = match LogEntry::from_json(&serde_json::from_str(line).unwrap(), &producers).unwrap() {
            LogEntry::Update(u) => u,
            LogEntry::Drawn(d) => {
                drawn += 1;
                let key = pid("line");
                let rows_ = flight.table("flight").await?;
                let value = match (d.gesture.kind(), SeriesTest::from_gesture(&d.gesture), d.gesture.param("soft")) {
                    ("cloudlasso", _, _) => cloud.voxels(&b).await?.0,
                    (_, Some(test), None) => test.value(&key, test.keys(rows_, col("line"), col("t"), col("n")).await.unwrap()),
                    (_, Some(test), Some(w)) => {
                        let s = [d.gesture.param("sx").unwrap(), d.gesture.param("sy").unwrap()];
                        test.soft_value(&key, test.degrees(rows_, col("line"), col("t"), col("n"), s, w).await.unwrap(), s, w)
                    }
                    (other, _, _) => panic!("no redraw for {other}"),
                };
                d.redraw(value)
            }
        };
        b = b.apply(r).unwrap();
        for sel in ["cloud", "series"] {
            same &= a.contributions(&sid(sel)).unwrap().collect::<Vec<_>>() == b.contributions(&sid(sel)).unwrap().collect::<Vec<_>>();
        }
    }
    let t_replay = ms(t0);
    let (na, nb) = (count_where(&cloud.mem, "pts", members("cloud").predicate(&a).unwrap()).await?,
                    count_where(&cloud.mem, "pts", members("cloud").predicate(&b).unwrap()).await?);
    println!("{} updates in {} bytes (the lasso {}, CloudLasso {}); replayed in {t_replay:.0} ms, {drawn} drawn again by the chart; \
              same contributions: {}; CloudLasso points {na} recorded, {nb} replayed",
             lines.len(), lines.iter().map(|l| l.len()).sum::<usize>(), lines[0].len(), lines[1].len(), yes(same && na == nb));
    Ok(())
}

// ---------------------------------------------------------------------------
// 7. From the file

async fn from_file(tile: &str, view: &View, sum: &mut Summary) -> DFResult<()> {
    println!("\n## the flat lasso, read from the file");
    let p = view.lasso("lasso", 1.0);
    let pred = members("lasso").predicate(&set(&p, SelectionValue::polygon(&p, &pid("u"), &pid("v"), &view.ring()).unwrap())).unwrap();
    // A bounding box one cell wider than the lasso, in plain comparisons on x
    // and y, which chunk statistics can prune by.
    let (xs, ys): (Vec<f64>, Vec<f64>) = view.ring().iter().map(|q| ((q[0] - view.a[0]) / view.a[1], (q[1] - view.b[0]) / view.b[2])).unzip();
    let (lo, hi) = (|v: &[f64]| v.iter().cloned().fold(f64::MAX, f64::min), |v: &[f64]| v.iter().cloned().fold(f64::MIN, f64::max));
    let pad = 1.0 / view.a[1];
    let bbox = col("x").gt_eq(lit(lo(&xs) - pad)).and(col("x").lt_eq(lit(hi(&xs) + pad)))
        .and(col("y").gt_eq(lit(lo(&ys) - pad))).and(col("y").lt_eq(lit(hi(&ys) + pad)));
    let q = format!("SELECT x, y, CAST(z AS DOUBLE) AS z FROM '{tile}'");
    let file = |ctx: SessionContext, f: Expr| {
        let q = q.clone();
        async move { ctx.sql(&q).await?.filter(f)?.count().await }
    };
    // A new session with statistics kept in a sidecar (<tile>.stats, not in
    // git): a session that has listed the tile without them keeps using that.
    let _ = std::fs::remove_file(format!("{tile}.stats"));
    let ctx = las_context();
    for s in ["geometry_encoding = 'plain'", "collect_statistics = 'true'", "parallel_statistics_extraction = 'true'", "persist_statistics = 'true'"] {
        ctx.sql(&format!("SET las.{s}")).await?.collect().await?;
    }
    let (t_full, _) = best(2, || file(ctx.clone(), col("z").gt(lit(-1e9)))).await?;
    let (t_pred, n) = best(2, || file(ctx.clone(), pred.clone())).await?;
    let (t_both, n2) = best(3, || file(ctx.clone(), bbox.clone().and(pred.clone()))).await?;
    println!("full scan {t_full:.0} ms; the lasso alone {t_pred:.0} ms ({n} points); with its bounding box {t_both:.0} ms ({n2} points)");
    sum.add("lasso from the file", "105 MB tile", f64::NAN, t_both, format!("{} (lasso alone {t_pred:.0} ms)", yes(n == n2)));
    Ok(())
}

// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> DFResult<()> {
    let tile = std::env::args().nth(1).unwrap_or_else(|| "data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz".into());
    let ctx = las_context();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?.collect().await?;
    let pts = |w: &str| format!("SELECT x, y, CAST(z AS DOUBLE) AS z, CAST(classification AS BIGINT) AS class FROM '{tile}'{w}");
    let e = ctx.sql(&format!("SELECT min(x) a, max(x) b, min(y) c, max(y) d FROM '{tile}'")).await?.collect().await?;
    let ext = ["a", "b", "c", "d"].map(|c| f64s(&e[0], c).value(0));
    println!("# Selections through avenger-selection, on {tile} (plot {P} px)");
    let mut sum = Summary::default();

    let all = ctx.sql(&pts("")).await?.collect().await?;
    let map = flat(ext);
    lasso(&map, &all, &mut sum).await?;
    brushes(&map, &all, &mut sum).await?;

    let win = [657500.0, 658000.0, 6867250.0, 6867750.0];
    let part = ctx.sql(&pts(&format!(" WHERE x BETWEEN {} AND {} AND y BETWEEN {} AND {}", win[0], win[1], win[2], win[3]))).await?.collect().await?;
    let hz = part.iter().flat_map(|b| f64s(b, "z").values().to_vec()).fold((f64::MAX, f64::MIN), |(a, b), z| (a.min(z), b.max(z)));
    let tilt = tilted(win, hz, 30.0, 35.0);
    lasso(&tilt, &part, &mut sum).await?;
    let (cloud, mut defs, mut updates) = cloud_lasso(&tilt, win, hz, &part, &mut sum).await?;

    println!("\n## the preaggregation split: a class histogram, cross-filtered");
    let lasso_p = map.lasso("lasso", 1.0);
    let moved: Vec<SelectionSet> = [[0.0, 0.0], [20.0, 0.0], [0.0, 20.0]].iter().map(|d| {
        let ring: Vec<[f64; 2]> = map.ring().iter().map(|q| [q[0] + d[0], q[1] + d[1]]).collect();
        set(&lasso_p, SelectionValue::polygon(&lasso_p, &pid("u"), &pid("v"), &ring).unwrap())
    }).collect();
    split("lasso, three positions", &all, &lasso_p, &moved, true, &mut sum).await?;
    drop(all);
    let with_lasso = set(&defs[0], SelectionValue::default()).apply(updates[0].clone()).unwrap();
    let mut densities = Vec::new();
    for structure in [0.3, 0.5, 0.15] {
        let (v, _) = Cloud { structure, ..cloud.clone() }.voxels(&with_lasso).await?;
        densities.push(with_lasso.set(&defs[1], v).unwrap());
    }
    split("CloudLasso, three densities", &part, &defs[1], &densities, false, &mut sum).await?;
    drop(part);

    let (flight, series_defs, series_updates) = series(&ctx, &tile, &mut sum).await?;
    defs.extend(series_defs);
    updates.extend(series_updates);
    log(&cloud, &defs, updates, &flight).await?;
    from_file(&tile, &map, &mut sum).await?;
    sum.print();
    Ok(())
}
