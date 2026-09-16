//! Export a LAS/LAZ tile to Parquet for the `.avenger` chart in `lang/`.
//! Coordinates become tile-relative metres (origin at the tile's south-west
//! kilometre corner).
//!
//! Usage: cargo run --release --bin export_parquet -- <tile.copc.laz> [out.parquet]

use std::sync::Arc;
use std::time::Instant;

use arrow::array::AsArray;
use arrow::datatypes::Float64Type;
use datafusion::execution::SessionStateBuilder;
use datafusion::prelude::{SessionConfig, SessionContext};
use sedona_pointcloud::las::format::{Extension, LasFormatFactory};
use sedona_pointcloud::las::options::LasOptions;

#[tokio::main]
async fn main() -> datafusion::error::Result<()> {
    let mut args = std::env::args().skip(1);
    let tile = args
        .next()
        .expect("usage: export_parquet <tile.copc.laz> [out.parquet]");
    let out = args
        .next()
        .unwrap_or_else(|| "lang/pantin_points.parquet".to_string());

    let config = SessionConfig::new().with_option_extension(LasOptions::default());
    let mut state = SessionStateBuilder::new()
        .with_config(config)
        .with_default_features()
        .build();
    state.register_file_format(Arc::new(LasFormatFactory::new(Extension::Laz)), true)?;
    let ctx = SessionContext::new_with_state(state).enable_url_table();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?;

    let t = Instant::now();
    let bounds = ctx
        .sql(&format!("SELECT min(x), min(y) FROM '{tile}'"))
        .await?
        .collect()
        .await?;
    let corner = |i: usize| {
        (bounds[0].column(i).as_primitive::<Float64Type>().value(0) / 1000.0).floor() * 1000.0
    };
    let (x0, y0) = (corner(0), corner(1));

    ctx.sql(&format!(
        "COPY (
           SELECT x - {x0} AS x, y - {y0} AS y, z, CAST(classification AS INT) AS classification
           FROM '{tile}'
         ) TO '{out}' STORED AS PARQUET"
    ))
    .await?
    .collect()
    .await?;
    println!("wrote {out} (origin {x0}, {y0}) in {:.2?}", t.elapsed());
    Ok(())
}
