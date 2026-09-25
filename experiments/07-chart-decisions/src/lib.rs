//! Experiment 7: charts driven by decisions.

pub mod deciders;
pub mod layer;
pub mod observe;
pub mod options;

use lidar_common::las_context;
use lidar_pipeline::packages::{self, chart};
use lidar_pipeline::pipeline::Pipeline;

pub const TILE: &str = "data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz";

/// A pipeline with experiment 6's packages, Vega semantics on.
pub async fn pipeline() -> datafusion::error::Result<Pipeline> {
    let ctx = las_context();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?;
    let mut p = Pipeline::new(ctx);
    p.vega_semantics = true;
    for pkg in [
        packages::vega_format(),
        packages::vega_compat(),
        packages::sedona::package(),
        chart::package(),
    ] {
        p.install(pkg);
    }
    Ok(p)
}

/// Pipeline steps from a JSON array, with the tile path filled in.
pub fn steps(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().replace("{TILE}", TILE))
        .collect()
}

/// A pipeline with the given data steps and chart commands applied.
pub async fn build(data: &[String], commands: &[String]) -> datafusion::error::Result<Pipeline> {
    let mut p = pipeline().await?;
    p.run(&data.join(" ! ")).await?;
    for c in commands {
        p.run(c).await?;
    }
    Ok(p)
}

/// A decision as `action/argument`, for tables.
pub fn short(answers: &serde_json::Map<String, serde_json::Value>) -> String {
    let get = |k: &str| answers.get(k).and_then(serde_json::Value::as_str);
    let action = get("action").unwrap_or("?");
    let arg = match action {
        "color" => get("colour"),
        "zoom" => get("region"),
        "rotate_labels" => get("angle"),
        "highlight" => get("subset"),
        "mark" => get("mark"),
        _ => None,
    };
    match arg {
        Some(x) => format!("{action}/{x}"),
        None => action.to_string(),
    }
}
