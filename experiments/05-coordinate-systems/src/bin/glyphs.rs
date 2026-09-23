//! vega-lite#7848, "Composing cartesian and polar coordinates": a pie chart
//! at every point of a scatter plot, or of a map.
//!
//! Two pieces, from experiments 5 and 6:
//! - `Nested`: an outer coordinate system × a polar glyph. Each position is
//!   (outer channels…, θ, r); only the centre goes through the outer system.
//! - per-group normalisation in SQL, done by two Avenger pipelines
//!   (`pipelines/glyphs_*.json`): `sum(n) OVER (PARTITION BY gx, gy)`, the
//!   "facet first, then compute the theta scale" the issue asks for.
//!
//! This binary only draws. Its input is the slices the pipelines write:
//!
//!     cargo run --release -p lidar-coords --bin glyphs -- <issue.parquet> <tile.parquet> <out.png>

use arrow::array::{AsArray, RecordBatch};
use arrow::compute::cast;
use arrow::datatypes::{DataType, Float64Type};
use avenger_color::ColorOrGradient;
use avenger_common::canvas::CanvasDimensions;
use avenger_geo::ProjectionKind;
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::rect::SceneRectMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_wgpu::canvas::{Canvas, PngCanvas};
use datafusion::prelude::SessionContext;
use lidar_common::{CLASSES, INK, MUTED, OTHER};
use lidar_coords::coords::{Cartesian, CoordinateSystem, Nested, Spatial};
use lidar_coords::draw::{self, Tick};
use lidar_coords::lambert93_to_lonlat;
use lidar_coords::specs::{GRID, TILE_E, TILE_N};

type Error = Box<dyn std::error::Error>;

const P: f64 = 380.0; // plot size
const EDGE: [f32; 4] = [1.0, 1.0, 1.0, 0.9];

/// Slices from SQL: one row per (group, category) with the running start and
/// the group total, computed per group by a window function.
struct Slice {
    gx: f64,
    gy: f64,
    cat: i32,
    n: f64,
    start: f64,
    total: f64,
}

/// Slices written by a pipeline: gx, gy, cat, n, start, total.
async fn slices(ctx: &SessionContext, parquet: &str) -> Result<Vec<Slice>, Error> {
    let b = ctx
        .sql(&format!(
            "SELECT gx, gy, cat, n, start, total FROM '{parquet}' ORDER BY gx, gy, cat"
        ))
        .await?
        .collect()
        .await?;
    let (gx, gy, cat, n, start, total) = (
        f64s(&b, "gx"),
        f64s(&b, "gy"),
        f64s(&b, "cat"),
        f64s(&b, "n"),
        f64s(&b, "start"),
        f64s(&b, "total"),
    );
    Ok((0..gx.len())
        .map(|i| Slice {
            gx: gx[i],
            gy: gy[i],
            cat: cat[i] as i32,
            n: n[i],
            start: start[i],
            total: total[i],
        })
        .collect())
}

fn f64s(b: &[RecordBatch], name: &str) -> Vec<f64> {
    b.iter()
        .flat_map(|b| {
            let a = cast(b.column_by_name(name).unwrap(), &DataType::Float64).unwrap();
            a.as_primitive::<Float64Type>().values().to_vec()
        })
        .collect()
}

/// Pie slices as rings in (outer…, θ, r) input space. `radius_of` gives the
/// glyph's r for a group total (1 = full `Nested::radius`).
fn pies(
    cs: &dyn CoordinateSystem,
    s: &[Slice],
    outer: &dyn Fn(f64, f64) -> Vec<f64>,
    radius_of: &dyn Fn(f64) -> f64,
    color: &dyn Fn(i32) -> [f32; 4],
) -> SceneMark {
    let rings: Vec<Vec<Vec<f64>>> = s
        .iter()
        .map(|sl| {
            let (t0, t1) = (sl.start / sl.total, (sl.start + sl.n) / sl.total);
            let r = radius_of(sl.total);
            let o = outer(sl.gx, sl.gy);
            let at = |t: f64, r: f64| o.iter().copied().chain([t, r]).collect::<Vec<f64>>();
            vec![at(t0, 0.0), at(t1, 0.0), at(t1, r), at(t0, r)]
        })
        .collect();
    let fill: Vec<[f32; 4]> = s.iter().map(|sl| color(sl.cat)).collect();
    draw::rings(cs, &rings, &fill, EDGE).0
}

fn panel(origin: [f32; 2], title: &str, subtitle: &str, mut marks: Vec<SceneMark>) -> SceneMark {
    marks.push(draw::title(
        &[(title, 14.0, true, INK), (subtitle, 10.5, false, MUTED)],
        [-40.0, -82.0],
    ));
    draw::group(origin, marks)
}

fn legend(origin: [f32; 2], items: &[(&str, [f32; 4])]) -> SceneMark {
    let mut marks = vec![];
    let mut x = 0.0f32;
    for (label, c) in items {
        marks.push(
            SceneRectMark {
                interactive: false,
                len: 1,
                x: x.into(),
                y: 0.0.into(),
                width: Some(11.0.into()),
                height: Some(11.0.into()),
                fill: ColorOrGradient::Color(*c).into(),
                ..Default::default()
            }
            .into(),
        );
        marks.push(draw::title(&[(label, 11.0, false, INK)], [x + 16.0, -3.5]));
        x += 22.0 + label.len() as f32 * 6.2;
    }
    draw::group(origin, marks)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().collect();
    let (issue_path, tile_path, out) = (&args[1], &args[2], &args[3]);
    let ctx = SessionContext::new().enable_url_table();

    // ---- the issue's own data --------------------------------------------
    let issue = slices(&ctx, issue_path).await?;
    let tableau = [
        [0.31, 0.47, 0.66, 1.0],
        [0.95, 0.56, 0.17, 1.0],
        [0.88, 0.34, 0.35, 1.0],
        [0.46, 0.72, 0.70, 1.0],
        [0.35, 0.63, 0.31, 1.0],
        [0.93, 0.79, 0.28, 1.0],
    ];
    let cart = Cartesian {
        width: P,
        height: P,
    };
    let nested = Nested {
        outer: &cart,
        split: 2,
        radius: 42.0,
    };
    let unit3 = |v: f64| v / 3.0;
    let ticks3: Vec<Tick> = (0..=3)
        .map(|k| Tick::new(unit3(k as f64), format!("{k}")))
        .collect();
    let mut m1 = draw::grid(&cart, [&ticks3, &ticks3], GRID, MUTED);
    m1.push(pies(
        &nested,
        &issue,
        &|x, y| vec![unit3(x), unit3(y)],
        &|_| 1.0,
        &|c| tableau[(c as usize - 1) % 6],
    ));

    // ---- the tile: class mix per 100 m block ------------------------------
    let blocks = slices(&ctx, tile_path).await?;
    let max_total = blocks.iter().map(|s| s.total).fold(0.0, f64::max);
    println!(
        "{} slices in {} blocks",
        blocks.len(),
        blocks
            .iter()
            .map(|s| (s.gx as i64, s.gy as i64))
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
    );
    let class_color = |c: i32| {
        CLASSES
            .iter()
            .find(|k| k.0 as i32 == c)
            .map_or(OTHER, |k| k.2)
    };
    // Area proportional to the block's point count (the thread's sqrt(total)).
    let radius_of = |total: f64| (total / max_total).sqrt();

    let (d0, d1) = (-60.0, 1060.0);
    let unit = |v: f64| (v - d0) / (d1 - d0);
    let metres: Vec<Tick> = (0..=5)
        .map(|k| Tick::new(unit(k as f64 * 200.0), format!("{}", k * 200)))
        .collect();
    let mut m2 = draw::grid(&cart, [&metres, &metres], GRID, MUTED);
    let nested_map = Nested {
        outer: &cart,
        split: 2,
        radius: 17.0,
    };
    m2.push(pies(
        &nested_map,
        &blocks,
        &|x, y| vec![unit(x), unit(y)],
        &radius_of,
        &class_color,
    ));

    // Same pies, outer system a conic projection of lon/lat.
    let lonlat = |e: f64, n: f64| {
        let (lon, lat) = lambert93_to_lonlat(TILE_E + e, TILE_N + n);
        vec![lon, lat]
    };
    let ring: Vec<Vec<f64>> = (0..=40)
        .flat_map(|k| {
            let t = d0 + (d1 - d0) * k as f64 / 40.0;
            [lonlat(t, d0), lonlat(t, d1), lonlat(d0, t), lonlat(d1, t)]
        })
        .collect();
    let min = |i: usize| ring.iter().map(|p| p[i]).fold(f64::MAX, f64::min);
    let max = |i: usize| ring.iter().map(|p| p[i]).fold(f64::MIN, f64::max);
    let extent = [[min(0), min(1)], [max(0), max(1)]];
    let conic = Spatial::fit(
        "conic conformal",
        ProjectionKind::ConicConformal {
            parallels: (44.0, 49.0),
        },
        [-3.0, 0.0, 0.0],
        extent,
        P,
        P,
    );
    let deg = |lo: f64, hi: f64, suffix: &str| -> Vec<Tick> {
        let mut v = (lo / 0.005).ceil() * 0.005;
        let mut t = vec![];
        while v <= hi {
            t.push(Tick::new(v, format!("{v:.3}°{suffix}")));
            v += 0.005;
        }
        t
    };
    let (lon_t, lat_t) = (
        deg(extent[0][0], extent[1][0], "E"),
        deg(extent[0][1], extent[1][1], "N"),
    );
    let mut m3 = draw::grid(&conic, [&lon_t, &lat_t], GRID, MUTED);
    let nested_conic = Nested {
        outer: &conic,
        split: 2,
        radius: 17.0,
    };
    m3.push(pies(
        &nested_conic,
        &blocks,
        &lonlat,
        &radius_of,
        &class_color,
    ));

    // ---- figure --------------------------------------------------------------
    let cell = P as f32 + 130.0;
    let mut legend_items: Vec<(&str, [f32; 4])> = CLASSES.iter().map(|c| (c.1, c.2)).collect();
    legend_items.push(("Other", OTHER));
    let scene = SceneGraph {
        marks: vec![
            panel(
                [70.0, 100.0],
                "The issue's own example",
                "Nested { outer: cartesian } · θ normalised per (x, y) in SQL",
                m1,
            ),
            panel(
                [70.0 + cell, 100.0],
                "Class mix per 100 m block",
                "cartesian, Lambert-93 metres · area ∝ points in block",
                m2,
            ),
            panel(
                [70.0 + 2.0 * cell, 100.0],
                "The same pies on a projected map",
                "Nested { outer: spatial conic } · pies stay round",
                m3,
            ),
            legend([70.0 + cell, P as f32 + 150.0], &legend_items),
        ],
        width: 3.0 * cell + 20.0,
        height: P as f32 + 180.0,
        origin: [0.0; 2],
    };
    let mut canvas = PngCanvas::new(
        CanvasDimensions {
            size: [scene.width, scene.height],
            scale: 2.0,
        },
        Default::default(),
    )
    .await?;
    canvas.set_scene(&scene)?;
    canvas.render().await?.save(out)?;
    println!("wrote {out}");
    Ok(())
}
