//! The data and the three chart specifications, shared by the figure
//! binary and the live viewer. No specification knows which coordinate
//! system it is drawn in.

use arrow::array::{AsArray, RecordBatch};
use arrow::datatypes::{Float64Type, Int32Type};
use avenger_scenegraph::marks::mark::SceneMark;
use datafusion::prelude::SessionContext;
use lidar_common::{CLASSES, INK, MUTED, OTHER};

use crate::coords::CoordinateSystem;
use crate::draw::{self, Cost, Tick};
use crate::lambert93_to_lonlat;

pub const GRID: [f32; 4] = [0.82, 0.84, 0.87, 1.0];
pub const EDGE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub const TILE_E: f64 = 657_000.0;
pub const TILE_N: f64 = 6_867_000.0;

/// A cell: easting and northing in the tile (m), unit height, colour.
pub type Cell = (f64, f64, f64, [f32; 4]);

pub struct Classes {
    pub labels: Vec<&'static str>,
    pub counts: Vec<f64>,
    pub colors: Vec<[f32; 4]>,
}

fn f64s(b: &[RecordBatch], i: usize) -> Vec<f64> {
    b.iter()
        .flat_map(|b| b.column(i).as_primitive::<Float64Type>().values().to_vec())
        .collect()
}

fn i32s(b: &[RecordBatch], i: usize) -> Vec<i32> {
    b.iter()
        .flat_map(|b| b.column(i).as_primitive::<Int32Type>().values().to_vec())
        .collect()
}

fn class_color(c: i32) -> [f32; 4] {
    CLASSES
        .iter()
        .find(|(k, _, _)| *k as i32 == c)
        .map_or(OTHER, |(_, _, rgba)| *rgba)
}

/// Points per class, and the tile as `cell_m` cells with the highest point's
/// height (scaled to unit, 0.5th–99.5th percentile) and class.
pub async fn load(
    ctx: &SessionContext,
    tile: &str,
    cell_m: f64,
) -> datafusion::error::Result<(Classes, Vec<Cell>, (f64, f64))> {
    ctx.sql("SET las.geometry_encoding = 'plain'").await?;
    let counts = ctx
        .sql(&format!(
            "SELECT CAST(classification AS INT) AS c, CAST(count(*) AS DOUBLE) AS n
             FROM '{tile}' GROUP BY 1 ORDER BY 1"
        ))
        .await?
        .collect()
        .await?;
    let half = cell_m / 2.0;
    let cells = ctx
        .sql(&format!(
            "SELECT CAST(floor((x - {TILE_E}) / {cell_m}) * {cell_m} + {half} AS DOUBLE) AS cx,
                    CAST(floor((y - {TILE_N}) / {cell_m}) * {cell_m} + {half} AS DOUBLE) AS cy,
                    CAST(max(z) AS DOUBLE) AS zmax,
                    CAST(first_value(classification ORDER BY z DESC) AS INT) AS cls
             FROM '{tile}' GROUP BY 1, 2"
        ))
        .await?
        .collect()
        .await?;

    let mut classes = Classes {
        labels: vec![
            "Ground",
            "Low veg.",
            "Med. veg.",
            "High veg.",
            "Building",
            "Other",
        ],
        counts: vec![0.0; 6],
        colors: CLASSES.iter().map(|c| c.2).chain([OTHER]).collect(),
    };
    for (c, n) in i32s(&counts, 0).into_iter().zip(f64s(&counts, 1)) {
        let i = CLASSES.iter().position(|k| k.0 as i32 == c).unwrap_or(5);
        classes.counts[i] += n;
    }

    let (cx, cy, zmax, cls) = (
        f64s(&cells, 0),
        f64s(&cells, 1),
        f64s(&cells, 2),
        i32s(&cells, 3),
    );
    let mut z_sorted = zmax.clone();
    z_sorted.sort_by(f64::total_cmp);
    let (z_lo, z_hi) = (
        z_sorted[z_sorted.len() / 200],
        z_sorted[z_sorted.len() * 995 / 1000],
    );
    let cells = (0..cx.len())
        .map(|i| {
            let z = ((zmax[i] - z_lo) / (z_hi - z_lo)).clamp(0.0, 1.2);
            (cx[i], cy[i], z, class_color(cls[i]))
        })
        .collect();
    Ok((classes, cells, (z_lo, z_hi)))
}

/// Point count per class: x is a band per class, y the count.
pub fn bars(cs: &dyn CoordinateSystem, c: &Classes) -> (Vec<SceneMark>, Cost) {
    let n = c.counts.len() as f64;
    let step = 2e6;
    let max = (c.counts.iter().cloned().fold(0.0, f64::max) / step).ceil() * step;
    let lo: Vec<[f64; 2]> = (0..c.counts.len())
        .map(|i| [(i as f64 + 0.15) / n, 0.0])
        .collect();
    let hi: Vec<[f64; 2]> = c
        .counts
        .iter()
        .enumerate()
        .map(|(i, v)| [(i as f64 + 0.85) / n, v / max])
        .collect();
    let x_ticks: Vec<Tick> = c
        .labels
        .iter()
        .enumerate()
        .map(|(i, l)| Tick::label((i as f64 + 0.5) / n, *l))
        .collect();
    let y_ticks: Vec<Tick> = (0..=(max / step) as usize)
        .map(|k| Tick::new(k as f64 * step / max, format!("{}M", k * 2)))
        .collect();
    let mut marks = draw::grid(cs, [&x_ticks, &y_ticks], GRID, MUTED);
    let (m, cost) = draw::rects(cs, &lo, &hi, &c.colors, EDGE);
    marks.push(m);
    (marks, cost)
}

/// The same counts as one stacked bar of shares: x is the running share,
/// and the bar occupies the middle of y. In polar, that middle is the ring
/// of a donut; nothing else changes.
pub fn stack(cs: &dyn CoordinateSystem, c: &Classes) -> (Vec<SceneMark>, Cost) {
    let total: f64 = c.counts.iter().sum();
    let mut acc = 0.0;
    let (mut lo, mut hi) = (vec![], vec![]);
    for v in &c.counts {
        lo.push([acc / total, 0.35]);
        acc += v;
        hi.push([acc / total, 0.65]);
    }
    let x_ticks: Vec<Tick> = (0..4)
        .map(|k| Tick::new(k as f64 * 0.25, format!("{}%", k * 25)))
        .collect();
    let y_ticks = [Tick::line(0.35), Tick::line(0.65)];
    let mut marks = draw::grid(cs, [&x_ticks, &y_ticks], GRID, MUTED);
    let (m, cost) = draw::rects(cs, &lo, &hi, &c.colors, EDGE);
    marks.push(m);
    (marks, cost)
}

/// The map's data in one system's inputs, prepared once: converting
/// Lambert-93 to lon/lat is data preparation, not projection.
pub struct MapLayers {
    pub pos: Vec<Vec<f64>>,
    pub fill: Vec<[f32; 4]>,
    pub footprint: Vec<Vec<f64>>,
}

impl MapLayers {
    /// Both encodings side by side, for a `Paired` transition.
    pub fn paired(a: &MapLayers, b: &MapLayers) -> MapLayers {
        let cat = |x: &[Vec<f64>], y: &[Vec<f64>]| {
            x.iter()
                .zip(y)
                .map(|(p, q)| p.iter().chain(q).copied().collect())
                .collect()
        };
        MapLayers {
            pos: cat(&a.pos, &b.pos),
            fill: a.fill.clone(),
            footprint: cat(&a.footprint, &b.footprint),
        }
    }

    pub fn new(cells: &[Cell], to_input: &dyn Fn(f64, f64, f64) -> Vec<f64>) -> Self {
        let edge = |a: [f64; 2], b: [f64; 2]| {
            (0..40).map(move |k| {
                let t = k as f64 / 40.0;
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
            })
        };
        let corners = [[0.0, 0.0], [1000.0, 0.0], [1000.0, 1000.0], [0.0, 1000.0]];
        MapLayers {
            pos: cells.iter().map(|c| to_input(c.0, c.1, c.2)).collect(),
            fill: cells.iter().map(|c| c.3).collect(),
            footprint: (0..4)
                .flat_map(|i| edge(corners[i], corners[(i + 1) % 4]))
                .map(|p| to_input(p[0], p[1], 0.0))
                .collect(),
        }
    }
}

/// The tile as a map: cells as a scatter, the tile footprint as a geoshape,
/// and the grid.
pub fn map(
    cs: &dyn CoordinateSystem,
    layers: &MapLayers,
    ticks: [&[Tick]; 2],
    size: f32,
) -> (Vec<SceneMark>, Cost) {
    let MapLayers {
        pos,
        fill,
        footprint,
    } = layers;
    let mut marks = draw::grid(cs, ticks, GRID, MUTED);
    let ring = std::slice::from_ref(footprint);
    marks.push(draw::shape(cs, ring, [0.93, 0.93, 0.95, 1.0], MUTED, 0.0));
    marks.push(draw::points(cs, pos, fill, size));
    marks.push(draw::shape(cs, ring, [0.0; 4], INK, 1.2));
    let cost = Cost {
        instances: pos.len(),
        vertices: pos.len(),
        as_rects: false,
    };
    (marks, cost)
}

/// The map's inputs and ticks for every system: unit inputs (with and
/// without height) for the planar systems, lon/lat for `Spatial`.
pub struct MapSetup {
    pub in_unit: MapLayers,
    pub in_unit_2d: MapLayers,
    pub in_lonlat: MapLayers,
    pub metres: Vec<Tick>,
    pub lon_ticks: Vec<Tick>,
    pub lat_ticks: Vec<Tick>,
    /// Lon/lat bounds of the tile plus margin.
    pub extent: [[f64; 2]; 2],
}

impl MapSetup {
    pub fn new(cells: &[Cell]) -> Self {
        // Lambert-93 metres relative to the tile, with a margin.
        let (d0, d1) = (-60.0, 1060.0);
        let unit = |v: f64| (v - d0) / (d1 - d0);
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
        let degree_ticks = |lo: f64, hi: f64, suffix: &str| -> Vec<Tick> {
            let step = 0.005;
            let mut v = (lo / step).ceil() * step;
            let mut t = vec![];
            while v <= hi {
                t.push(Tick::new(v, format!("{v:.3}°{suffix}")));
                v += step;
            }
            t
        };
        MapSetup {
            in_unit: MapLayers::new(cells, &|e, n, z| vec![unit(e), unit(n), z]),
            in_unit_2d: MapLayers::new(cells, &|e, n, _| vec![unit(e), unit(n)]),
            in_lonlat: MapLayers::new(cells, &|e, n, _| lonlat(e, n)),
            metres: (0..=5)
                .map(|k| Tick::new(unit(k as f64 * 200.0), format!("{}", k * 200)))
                .collect(),
            lon_ticks: degree_ticks(extent[0][0], extent[1][0], "E"),
            lat_ticks: degree_ticks(extent[0][1], extent[1][1], "N"),
            extent,
        }
    }
}
