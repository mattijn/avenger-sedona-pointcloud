//! Validate every pipeline experiment 7's writers produced, and time each
//! layer apart.
//!
//! `cargo run --release -- [repeats]`

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use datafusion::prelude::SessionContext;
use lidar_validate::{corpus, load_spec, validate, Timings};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let here = env!("CARGO_MANIFEST_DIR");
    let repeats: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20);
    let t = Instant::now();
    let spec = load_spec(&format!("{here}/../spec/steps.json"));
    let load = t.elapsed();
    let t = Instant::now();
    let ctx = SessionContext::new();
    let session = t.elapsed();
    let pipelines = corpus(&format!("{here}/../../07-chart-decisions/results"));

    // One pass for the verdicts, then timed passes.
    let mut agree = BTreeMap::<(bool, bool), usize>::new();
    let mut by_layer = BTreeMap::<String, usize>::new();
    let mut dummy = Timings::default();
    let mut flagged = vec![];
    for (p, refused) in &pipelines {
        let errs = validate(&spec, &ctx, p, &mut dummy).await;
        *agree.entry((*refused, !errs.is_empty())).or_default() += 1;
        for e in &errs {
            *by_layer.entry(e[..2].to_string()).or_default() += 1;
        }
        if !errs.is_empty() {
            flagged.push(format!("{}\n    -> {}", p.replace('\n', " "), errs.join("\n    -> ")));
        }
    }
    let mut t = Timings::default();
    let wall = Instant::now();
    for _ in 0..repeats {
        for (p, _) in &pipelines {
            validate(&spec, &ctx, p, &mut t).await;
        }
    }
    let wall = wall.elapsed();
    let n = (repeats * pipelines.len()) as f64;
    let us = |d: Duration| d.as_secs_f64() * 1e6 / n;

    println!("rust: {} pipelines x {repeats} repeats", pipelines.len());
    println!("  load spec (once)       {:>9.1} µs", load.as_secs_f64() * 1e6);
    println!("  SessionContext (once)  {:>9.1} µs", session.as_secs_f64() * 1e6);
    println!("  per pipeline, mean:");
    println!("    L0 parse             {:>9.2} µs", us(t.parse));
    println!("    L1 arguments         {:>9.2} µs", us(t.l1));
    println!("    L2 order and state   {:>9.2} µs", us(t.l2));
    println!("    L3 Vega expressions  {:>9.2} µs", us(t.l3));
    println!("    L4 SQL and fields    {:>9.2} µs", us(t.l4));
    println!("    total (wall)         {:>9.2} µs", wall.as_secs_f64() * 1e6 / n);
    println!("  verdicts (recorded refused, flagged here): {agree:?}");
    println!("  errors by layer: {by_layer:?}");
    std::fs::write(format!("{here}/../results/rust_flagged.txt"), flagged.join("\n")).unwrap();
}
