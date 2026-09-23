//! Phase C: is the chain one lazy plan, and does a late Vega filter still
//! reach the LAZ scan? Same pipeline, four ways, best of three runs each.
//!
//!     cargo run --release -p lidar-pipeline --bin pipeline_bench -- data/<tile>.copc.laz

use std::time::Instant;

use lidar_common::las_context;
use lidar_pipeline::packages::{vega_compat, vega_format};
use lidar_pipeline::pipeline::Pipeline;

async fn pipeline(vega: bool) -> Result<Pipeline, Box<dyn std::error::Error>> {
    let ctx = las_context();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?;
    for f in vega_format::functions()
        .into_iter()
        .chain(vega_compat::functions())
    {
        ctx.register_udf(f);
    }
    let mut p = Pipeline::new(ctx);
    p.vega_semantics = vega;
    Ok(p)
}

/// The scan node and any filter directly on the scanned columns.
fn scan_lines(explain: &str) -> Vec<String> {
    let physical = explain.split("physical plan:").nth(1).unwrap_or("");
    physical
        .lines()
        .filter(|l| l.contains("DataSourceExec") || l.contains("FilterExec"))
        .map(|l| {
            let l = l.trim();
            if l.len() > 230 {
                format!("{}…", &l[..230])
            } else {
                l.to_string()
            }
        })
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tile = std::env::args()
        .nth(1)
        .expect("usage: pipeline_bench <tile.copc.laz>");
    // A 120 m window, written as a Vega filter after a SQL step.
    let chain = |materialize: bool| {
        let m = if materialize { " ! materialize" } else { "" };
        format!(
            "read {tile} --statistics \
             ! calc h --vega \"datum.z - 42\"{m} \
             ! sql \"SELECT x, y, h, classification FROM input WHERE classification = 6\"{m} \
             ! filter --vega \"datum.x < 657120 && datum.y < 6867120\""
        )
    };
    let variants = [
        ("lazy, SQL semantics", false, false),
        ("lazy, Vega semantics", true, false),
        (
            "eager (materialize after each step), SQL semantics",
            false,
            true,
        ),
    ];
    for (name, vega, eager) in variants {
        let mut best = f64::MAX;
        let mut rows = String::new();
        for _ in 0..3 {
            let mut p = pipeline(vega).await?;
            let t = Instant::now();
            p.run(&chain(eager)).await?;
            rows = p.run("count").await?.join("");
            best = best.min(t.elapsed().as_secs_f64());
        }
        println!("\n## {name}: {rows}, best of 3: {:.0} ms", best * 1e3);
        let mut p = pipeline(vega).await?;
        p.run(&chain(eager)).await?;
        for l in scan_lines(&p.explain().await?) {
            println!("    {l}");
        }
    }
    Ok(())
}
