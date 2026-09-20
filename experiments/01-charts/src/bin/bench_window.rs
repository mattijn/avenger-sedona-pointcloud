//! Compare ways to fetch the points inside a map window from a COPC tile.
//!
//! Usage: cargo run --release -p lidar-charts --bin bench_window -- <tile.copc.laz>

use lidar_common::las_context;
use std::time::Instant;

use copc_rs::{Bounds, BoundsSelection, CopcReader, LodSelection, Vector};
use datafusion::prelude::SessionContext;

const X0: f64 = 657_000.0;
const Y0: f64 = 6_867_000.0;

fn window_sql(from: &str, w: [f64; 4]) -> String {
    format!(
        "SELECT x, y, classification FROM {from}
         WHERE x >= {} AND x < {} AND y >= {} AND y < {}",
        X0 + w[0],
        X0 + w[1],
        Y0 + w[2],
        Y0 + w[3]
    )
}

async fn timed_rows(
    ctx: &SessionContext,
    label: &str,
    sql: &str,
) -> datafusion::error::Result<usize> {
    let t = Instant::now();
    let batches = ctx.sql(sql).await?.collect().await?;
    let n = batches.iter().map(|b| b.num_rows()).sum();
    println!("  {label:<46} {n:>9} rows  {:>9.1?}", t.elapsed());
    Ok(n)
}

fn copc_rows(tile: &str, label: &str, lod: LodSelection, w: Option<[f64; 4]>) -> usize {
    let t = Instant::now();
    let mut reader = CopcReader::from_path(tile).unwrap();
    let bounds = match w {
        None => BoundsSelection::All,
        Some(w) => BoundsSelection::Within(Bounds {
            min: Vector {
                x: X0 + w[0],
                y: Y0 + w[2],
                z: -1.0e4,
            },
            max: Vector {
                x: X0 + w[1],
                y: Y0 + w[3],
                z: 1.0e4,
            },
        }),
    };
    let (mut xs, mut ys, mut cs) = (Vec::new(), Vec::new(), Vec::new());
    for p in reader.points(lod, bounds).unwrap() {
        let p = p.unwrap();
        xs.push((p.x - X0) as f32);
        ys.push((p.y - Y0) as f32);
        cs.push(u8::from(p.classification));
    }
    println!("  {label:<46} {:>9} rows  {:>9.1?}", xs.len(), t.elapsed());
    xs.len()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tile = std::env::args().nth(1).expect("tile path");
    let url = format!("'{tile}'");
    let small = [700.0, 820.0, 340.0, 460.0]; // 120 × 120 m
    let medium = [250.0, 750.0, 250.0, 750.0]; // 500 × 500 m

    // COPC hierarchy facts
    let reader = CopcReader::from_path(&tile)?;
    let info = reader.copc_info();
    println!(
        "COPC: {} hierarchy entries, root halfsize {:.1} m, root spacing {:.2} m",
        reader.num_entries(),
        info.halfsize,
        info.spacing
    );

    println!("\n1. sedona-pointcloud, no statistics (full scan)");
    let ctx = las_context();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?;
    timed_rows(&ctx, "120 m window", &window_sql(&url, small)).await?;
    timed_rows(&ctx, "500 m window", &window_sql(&url, medium)).await?;

    println!("\n2. sedona-pointcloud with chunk statistics");
    let _ = std::fs::remove_file(format!("{tile}.stats"));
    let ctx = las_context();
    for s in [
        "SET las.geometry_encoding = 'plain'",
        "SET las.collect_statistics = 'true'",
        "SET las.parallel_statistics_extraction = 'true'",
        "SET las.persist_statistics = 'true'",
    ] {
        ctx.sql(s).await?;
    }
    timed_rows(
        &ctx,
        "120 m window, first query (builds stats)",
        &window_sql(&url, small),
    )
    .await?;
    timed_rows(&ctx, "120 m window, same session", &window_sql(&url, small)).await?;
    timed_rows(
        &ctx,
        "500 m window, same session",
        &window_sql(&url, medium),
    )
    .await?;
    let ctx = las_context();
    for s in [
        "SET las.geometry_encoding = 'plain'",
        "SET las.collect_statistics = 'true'",
    ] {
        ctx.sql(s).await?;
    }
    timed_rows(
        &ctx,
        "120 m window, new session (sidecar)",
        &window_sql(&url, small),
    )
    .await?;

    println!("\n3. COPC octree via copc-rs (single thread)");
    copc_rows(
        &tile,
        "120 m window, all levels",
        LodSelection::All,
        Some(small),
    );
    copc_rows(
        &tile,
        "500 m window, all levels",
        LodSelection::All,
        Some(medium),
    );
    copc_rows(
        &tile,
        "500 m window, resolution 1 m",
        LodSelection::Resolution(1.0),
        Some(medium),
    );
    copc_rows(
        &tile,
        "whole tile, resolution 2 m",
        LodSelection::Resolution(2.0),
        None,
    );
    copc_rows(
        &tile,
        "whole tile, resolution 1 m",
        LodSelection::Resolution(1.0),
        None,
    );
    copc_rows(&tile, "whole tile, all levels", LodSelection::All, None);

    println!("\n4. In-memory Arrow table (DataFusion MemTable)");
    let ctx = las_context();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?;
    let t = Instant::now();
    ctx.sql(&format!(
        "CREATE TABLE pts AS SELECT x, y, z, classification FROM {url}"
    ))
    .await?
    .collect()
    .await?;
    println!(
        "  {:<46} {:>9}       {:>9.1?}",
        "load whole tile into memory",
        "",
        t.elapsed()
    );
    timed_rows(&ctx, "120 m window", &window_sql("pts", small)).await?;
    timed_rows(&ctx, "120 m window, again", &window_sql("pts", small)).await?;
    timed_rows(&ctx, "500 m window", &window_sql("pts", medium)).await?;
    Ok(())
}
