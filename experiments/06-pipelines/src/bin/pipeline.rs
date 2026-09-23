//! Run a pipeline on the command line, GDAL style.
//!
//!     cargo run --release -p lidar-pipeline --bin pipeline -- \
//!       'read data/tile.copc.laz ! filter --vega "datum.classification == 6" ! count'
//!
//! Flags before the pipeline: --semantics vega|sql (default sql),
//! --steps (list the registered steps), --packages (install report),
//! --trace (time per step).

use lidar_common::las_context;
use lidar_pipeline::packages;
use lidar_pipeline::pipeline::Pipeline;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ctx = las_context();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?;
    let mut p = Pipeline::new(ctx);
    let before = p.functions.len();
    for package in [
        packages::vega_format(),
        packages::vega_compat(),
        packages::sedona::package(),
        packages::terrain::package(),
        packages::chart::package(),
    ] {
        let (name, nf, ns) = (package.name, package.functions.len(), package.steps.len());
        let taken = p.install(package);
        if args.iter().any(|a| a == "--packages") {
            println!(
                "installed {name}: {nf} functions, {ns} steps; {} names already taken{}",
                taken.len(),
                if taken.is_empty() {
                    String::new()
                } else {
                    format!(
                        ": {}",
                        taken
                            .iter()
                            .map(|(n, o)| format!("{n} (was {o})"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            );
        }
    }
    if args.iter().any(|a| a == "--packages") {
        println!(
            "{before} DataFusion built-ins, {} functions in total",
            p.functions.len()
        );
        return Ok(());
    }
    p.vega_semantics = args
        .windows(2)
        .any(|w| w[0] == "--semantics" && w[1] == "vega");
    if args.iter().any(|a| a == "--steps") {
        for (name, (package, step)) in &p.steps {
            println!(
                "{package:>6}  {name:<12} {:?}  {}",
                step.kind(),
                step.help()
            );
        }
        return Ok(());
    }
    // Replay a saved pipeline, then run the given steps (e.g. a render).
    if let Some(i) = args.iter().position(|a| a == "--replay") {
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&args[i + 1])?)?;
        p.replay(&saved).await?;
        println!(
            "replayed {} data steps and {} commands",
            saved["data"].as_array().map_or(0, |a| a.len()),
            p.log.len()
        );
    }
    // Render a serialised chart definition with no pipeline at all.
    if let Some(i) = args.iter().position(|a| a == "--render-avc") {
        let png = packages::chart::render_avc(std::fs::read(&args[i + 1])?)?;
        std::fs::write(&args[i + 2], &png)?;
        println!(
            "rendered {} ({} bytes) from {}",
            args[i + 2],
            png.len(),
            args[i + 1]
        );
        return Ok(());
    }
    let src = args
        .last()
        .expect("usage: pipeline [--semantics vega] '<step> ! <step> ...'");
    for out in p.run(src).await? {
        println!("{out}\n");
    }
    if args.iter().any(|a| a == "--trace") {
        for (call, kind, ms) in &p.trace {
            println!("{ms:>9.1} ms  {kind:?}  {call}");
        }
    }
    Ok(())
}
