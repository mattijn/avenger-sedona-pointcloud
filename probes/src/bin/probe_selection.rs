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

struct View {
    name: &'static str,
    /// px = a[0] + a[1] x + a[2] y + a[3] z, and py likewise.
    a: [f64; 4],
    b: [f64; 4],
    /// The window the chart queries, in metres.
    window: Option<[f64; 4]>,
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
        window: None,
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
        window: Some(win),
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
    let (u, v) = (ProjectionId::new("u").unwrap(), ProjectionId::new("v").unwrap());
    let (gu, gv) = (grid(size), grid(size));
    let sel = SelectionId::new("lasso").unwrap();
    let producer = ProducerDefinition::new(
        sel.clone(),
        ProducerId::new("lasso").unwrap(),
        ViewId::new("map").unwrap(),
        [Projection::new(u.clone(), view.px(&view.a)).unwrap(), Projection::new(v.clone(), view.px(&view.b)).unwrap()],
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
    let (exact_ms, exact_n) = best(3, || count(mem.table("pts").await.unwrap().filter(exact.clone()).unwrap())).await?;
    println!("exact point in polygon   Rust loop {rust_ms:>8.1} ms   {rust_n} points");
    println!("                         DataFusion {exact_ms:>7.1} ms   {exact_n} points");

    for size in [1.0, 2.0, 4.0] {
        let t0 = Instant::now();
        let (pred, tuples) = selection(view, size);
        let build_ms = ms(t0);
        let (sel_ms, sel_n) = best(3, || count(mem.table("pts").await.unwrap().filter(pred.clone()).unwrap())).await?;
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

    // 3: from the file, flat lasso at 1 px. A bounding box in data units,
    // one cell wider than the lasso, derived from the ring through the view.
    let (pred, _) = selection(&view, 1.0);
    let ring = view.ring();
    let inv = |c: &[f64; 4], i: usize, p: f64| (p - c[0]) / c[i];
    let (xs, ys): (Vec<f64>, Vec<f64>) = ring.iter().map(|p| (inv(&view.a, 1, p[0]), inv(&view.b, 2, p[1]))).unzip();
    let pad = 1.0 / view.a[1];
    let bbox = col("x").between(lit(xs.iter().cloned().fold(f64::MAX, f64::min) - pad), lit(xs.iter().cloned().fold(f64::MIN, f64::max) + pad))
        .and(col("y").between(lit(ys.iter().cloned().fold(f64::MAX, f64::min) - pad), lit(ys.iter().cloned().fold(f64::MIN, f64::max) + pad)));
    println!("\n## from the file, flat lasso at 1 px");
    let file = |ctx: SessionContext, f: Expr| {
        let q = pts("");
        async move { count(ctx.sql(&q).await?.filter(f)?).await }
    };
    let (t, n) = best(2, || file(ctx.clone(), pred.clone())).await?;
    println!("predicate alone, full scan              {t:>7.0} ms   {n} points");
    for stmt in ["SET las.collect_statistics = 'true'", "SET las.parallel_statistics_extraction = 'true'"] {
        ctx.sql(stmt).await?.collect().await?;
    }
    let t0 = Instant::now();
    let n = file(ctx.clone(), bbox.clone()).await?;
    println!("first query with chunk statistics         {:>7.0} ms   {n} points (builds the statistics)", ms(t0));
    let (t, n) = best(3, || file(ctx.clone(), pred.clone())).await?;
    println!("predicate alone, chunk statistics         {t:>7.0} ms   {n} points");
    let (t, n) = best(3, || file(ctx.clone(), bbox.clone().and(pred.clone()))).await?;
    println!("bounding box and predicate, statistics    {t:>7.0} ms   {n} points");
    Ok(())
}
