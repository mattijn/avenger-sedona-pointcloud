//! The `terrain` step package: whole-dataset tools, the kind geolibre and
//! whitebox provide. A hillshade needs each cell's neighbours, so it is a
//! step over the table, not a row function. It executes the plan so far:
//! a materialisation point in an otherwise lazy chain, as GDAL's
//! `materialize` step is.

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{AsArray, Float64Array, RecordBatch};
use arrow::compute::cast;
use arrow::datatypes::{DataType, Field, Float64Type, Schema};
use async_trait::async_trait;
use datafusion::datasource::MemTable;
use datafusion::error::Result;

use crate::pipeline::{err, Call, Kind, Package, Pipeline, Step};

struct Hillshade;

fn column(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>> {
    let mut out = vec![];
    for b in batches {
        let a = b
            .column_by_name(name)
            .ok_or_else(|| err(format!("hillshade: no column `{name}`")))?;
        let a = cast(a, &DataType::Float64)?;
        out.extend(
            a.as_primitive::<Float64Type>()
                .iter()
                .map(|v| v.unwrap_or(f64::NAN)),
        );
    }
    Ok(out)
}

#[async_trait]
impl Step for Hillshade {
    fn kind(&self) -> Kind {
        Kind::Transform
    }
    fn help(&self) -> &'static str {
        "hillshade --x cx --y cy --z h [--cell 1] [--azimuth 315] [--altitude 45]: Horn's method on a regular grid"
    }
    async fn run(&self, p: &mut Pipeline, c: &Call) -> Result<Option<String>> {
        let (xn, yn, zn) = (
            c.flag("x").unwrap_or("cx"),
            c.flag("y").unwrap_or("cy"),
            c.flag("z").unwrap_or("h"),
        );
        let cell: f64 = c.flag("cell").and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let az = c
            .flag("azimuth")
            .and_then(|v| v.parse().ok())
            .unwrap_or(315.0f64)
            .to_radians();
        let alt = c
            .flag("altitude")
            .and_then(|v| v.parse().ok())
            .unwrap_or(45.0f64)
            .to_radians();
        let batches = p.dataframe()?.collect().await?;
        let (x, y, z) = (
            column(&batches, xn)?,
            column(&batches, yn)?,
            column(&batches, zn)?,
        );
        let key = |x: f64, y: f64| ((x / cell).round() as i64, (y / cell).round() as i64);
        let grid: HashMap<(i64, i64), f64> = x
            .iter()
            .zip(&y)
            .zip(&z)
            .map(|((x, y), z)| (key(*x, *y), *z))
            .collect();
        let shade: Vec<f64> = x
            .iter()
            .zip(&y)
            .map(|(x, y)| {
                let (i, j) = key(*x, *y);
                let centre = grid[&(i, j)];
                let at = |di: i64, dj: i64| *grid.get(&(i + di, j + dj)).unwrap_or(&centre);
                // Horn (1981): 3×3 weighted gradients.
                let dzdx = ((at(1, 1) + 2.0 * at(1, 0) + at(1, -1))
                    - (at(-1, 1) + 2.0 * at(-1, 0) + at(-1, -1)))
                    / (8.0 * cell);
                let dzdy = ((at(-1, 1) + 2.0 * at(0, 1) + at(1, 1))
                    - (at(-1, -1) + 2.0 * at(0, -1) + at(1, -1)))
                    / (8.0 * cell);
                let slope = dzdx.hypot(dzdy).atan();
                let aspect = dzdy.atan2(-dzdx);
                (alt.sin() * slope.cos() + alt.cos() * slope.sin() * (az - aspect).cos()).max(0.0)
            })
            .collect();
        let schema = Arc::new(Schema::new(vec![
            Field::new(xn, DataType::Float64, false),
            Field::new(yn, DataType::Float64, false),
            Field::new(zn, DataType::Float64, true),
            Field::new("hillshade", DataType::Float64, false),
        ]));
        let n = x.len();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Float64Array::from(x)),
                Arc::new(Float64Array::from(y)),
                Arc::new(Float64Array::from(z)),
                Arc::new(Float64Array::from(shade)),
            ],
        )?;
        let name = format!("hillshade_{}", p.trace.len());
        p.ctx.register_table(
            &name,
            Arc::new(MemTable::try_new(schema, vec![vec![batch]])?),
        )?;
        p.plan = Some(p.ctx.table(&name).await?.into_unoptimized_plan());
        Ok(Some(format!("hillshade over {n} cells")))
    }
}

struct Png;

#[async_trait]
impl Step for Png {
    fn kind(&self) -> Kind {
        Kind::Sink
    }
    fn help(&self) -> &'static str {
        "png <path> --x cx --y cy --value hillshade [--cell 1]: a grey raster of one value per cell"
    }
    async fn run(&self, p: &mut Pipeline, c: &Call) -> Result<Option<String>> {
        let path = c.arg(0)?;
        let cell: f64 = c.flag("cell").and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let batches = p.dataframe()?.collect().await?;
        let x = column(&batches, c.flag("x").unwrap_or("cx"))?;
        let y = column(&batches, c.flag("y").unwrap_or("cy"))?;
        let v = column(&batches, c.flag("value").unwrap_or("hillshade"))?;
        let (x0, y1) = (
            x.iter().cloned().fold(f64::MAX, f64::min),
            y.iter().cloned().fold(f64::MIN, f64::max),
        );
        let w = ((x.iter().cloned().fold(f64::MIN, f64::max) - x0) / cell) as u32 + 1;
        let h = ((y1 - y.iter().cloned().fold(f64::MAX, f64::min)) / cell) as u32 + 1;
        let (lo, hi) = v
            .iter()
            .filter(|v| v.is_finite())
            .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        let mut img = image::GrayImage::new(w, h);
        for ((x, y), v) in x.iter().zip(&y).zip(&v) {
            let (i, j) = (((x - x0) / cell) as u32, ((y1 - y) / cell) as u32);
            let g = ((v - lo) / (hi - lo).max(1e-12) * 255.0).clamp(0.0, 255.0) as u8;
            img.put_pixel(i.min(w - 1), j.min(h - 1), image::Luma([g]));
        }
        img.save(path).map_err(|e| err(e.to_string()))?;
        Ok(Some(format!("wrote {path} ({w}×{h})")))
    }
}

pub fn package() -> Package {
    Package {
        name: "terrain",
        functions: vec![],
        steps: vec![("hillshade", Arc::new(Hillshade)), ("png", Arc::new(Png))],
    }
}
