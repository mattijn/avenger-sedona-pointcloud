//! The four datasets, each one aggregated query over the tile through
//! experiment 6's pipeline. Cached as Parquet under `out/layer/` so later
//! runs start in milliseconds.

use arrow::array::{AsArray, RecordBatch};
use arrow::datatypes::{DataType, Float64Type};
use datafusion::error::Result;
use lidar_common::{CLASSES, OTHER};

use super::model::Data;
use crate::{pipeline, TILE};

fn class_case() -> String {
    let mut s = String::from("CASE classification");
    for (code, label, _) in CLASSES {
        s += &format!(" WHEN {code} THEN '{label}'");
    }
    s + " ELSE 'Other' END"
}

fn class_order() -> String {
    let mut s = String::from("CASE classification");
    for (i, (code, _, _)) in CLASSES.iter().enumerate() {
        s += &format!(" WHEN {code} THEN {i}");
    }
    s + &format!(" ELSE {} END", CLASSES.len())
}

async fn table(name: &str, steps: &str) -> Result<Vec<RecordBatch>> {
    let cache = format!("out/layer/{name}.parquet");
    let mut p = pipeline().await?;
    if std::path::Path::new(&cache).exists() {
        p.run(&format!("read {cache}")).await?;
    } else {
        std::fs::create_dir_all("out/layer")?;
        p.run(&format!("read {TILE} --statistics ! {steps} ! write {cache}")).await?;
        p.run(&format!("read {cache}")).await?;
    }
    p.dataframe()?.collect().await
}

fn f64s(b: &[RecordBatch], name: &str) -> Vec<f64> {
    b.iter()
        .flat_map(|b| {
            let a = arrow::compute::cast(b.column_by_name(name).unwrap(), &DataType::Float64).unwrap();
            a.as_primitive::<Float64Type>().values().to_vec()
        })
        .collect()
}

fn strings(b: &[RecordBatch], name: &str) -> Vec<String> {
    b.iter()
        .flat_map(|b| {
            let a = arrow::compute::cast(b.column_by_name(name).unwrap(), &DataType::Utf8).unwrap();
            a.as_string::<i32>().iter().map(|s| s.unwrap_or_default().to_string()).collect::<Vec<_>>()
        })
        .collect()
}

pub async fn load() -> Result<Data> {
    let (case, order) = (class_case(), class_order());
    let classes = table(
        "classes",
        &format!("sql \"SELECT {case} AS label, min({order}) AS o, CAST(count(*) AS DOUBLE) AS n FROM input GROUP BY {case} ORDER BY o\""),
    )
    .await?;
    let flight = table(
        "flight",
        "sql \"SELECT point_source_id AS line, floor((gps_time - t0) * 2) / 2 AS t, CAST(count(*) AS DOUBLE) AS n \
         FROM input JOIN (SELECT point_source_id AS l, min(gps_time) AS t0 FROM input GROUP BY point_source_id) m \
         ON input.point_source_id = m.l GROUP BY point_source_id, floor((gps_time - t0) * 2) / 2 ORDER BY line, t\"",
    )
    .await?;
    let class_height = table(
        "class_height",
        &format!(
            "sql \"SELECT {case} AS label, floor((z - 42) / 4) * 4 AS band, CAST(count(*) AS DOUBLE) AS n FROM input \
             WHERE z >= 42 AND z < 78 GROUP BY {case}, floor((z - 42) / 4) * 4\""
        ),
    )
    .await?;
    let cells = table(
        "cells",
        "filter --vega \"datum.classification == 6\" ! sql \"SELECT floor(x/5)*5 AS cx, floor(y/5)*5 AS cy, max(z) AS h \
         FROM input WHERE x < 658000 AND y < 6868000 GROUP BY floor(x/5)*5, floor(y/5)*5\"",
    )
    .await?;
    let labels = strings(&classes, "label");
    let counts = f64s(&classes, "n");
    let colour = |l: &str| CLASSES.iter().find(|c| c.1 == l).map_or(OTHER, |c| c.2);
    Ok(Data {
        classes: labels.iter().zip(&counts).map(|(l, n)| (l.clone(), colour(l), *n)).collect(),
        flight: {
            let (l, t, n) = (f64s(&flight, "line"), f64s(&flight, "t"), f64s(&flight, "n"));
            (0..l.len()).map(|i| (l[i] as i64, t[i], n[i])).collect()
        },
        class_height: {
            let (l, b, n) = (strings(&class_height, "label"), f64s(&class_height, "band"), f64s(&class_height, "n"));
            (0..l.len()).map(|i| (l[i].clone(), b[i], n[i])).collect()
        },
        cells: {
            let (x, y, h) = (f64s(&cells, "cx"), f64s(&cells, "cy"), f64s(&cells, "h"));
            (0..x.len()).map(|i| (x[i], y[i], h[i])).collect()
        },
    })
}
