//! Phases E and F: do commands and queries behave as CQRS promises, and do
//! the two serialised forms reproduce the chart exactly?
//!
//!     cargo run --release -p lidar-pipeline --bin cqrs_check -- data/<tile>.copc.laz out
//!
//! Then, in fresh processes (see the README), `pipeline --replay` and
//! `pipeline --render-avc` render the saved forms for a byte comparison.

use std::time::Instant;

use lidar_common::las_context;
use lidar_pipeline::packages::{self, chart};
use lidar_pipeline::pipeline::Pipeline;

async fn pipeline() -> Result<Pipeline, Box<dyn std::error::Error>> {
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

fn snapshot(p: &Pipeline) -> String {
    format!(
        "{}\n{}",
        p.chart,
        p.plan
            .as_ref()
            .map(|x| x.display_indent().to_string())
            .unwrap_or_default()
    )
}

fn check(name: &str, ok: bool, detail: String) {
    println!(
        "| {name} | {} | {detail} |",
        if ok { "yes" } else { "**no**" }
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (tile, out) = (&args[1], &args[2]);
    let data = format!(
        "read {tile} --statistics \
         ! filter --vega \"datum.classification == 6\" \
         ! calc height --vega \"floor((datum.z - 42) / 2) * 2\" \
         ! sql \"SELECT height, count(*) AS points FROM input WHERE height BETWEEN 0 AND 40 GROUP BY height ORDER BY height\" \
         ! calc label --vega \"toString(datum.height) + ' m'\""
    );
    let commands = [
        "chart bar --x label --y points",
        "set title \"Building points per 2 m of height above 42 m\"",
        "set x.title \"height above 42 m\"",
        "set x.labelAngle -45",
        "set fill #c44e52",
    ];
    let queries = [
        "get",
        "get title",
        "domain y",
        "history",
        "count",
        "schema",
        "head 2",
        "explain",
    ];

    let mut p = pipeline().await?;
    p.run(&data).await?;
    println!("| Check | Holds | Detail |\n|---|---|---|");

    // Queries between every command change nothing.
    let (mut unchanged, mut ran) = (0, 0);
    for c in commands {
        p.run(c).await?;
        for q in queries {
            let before = snapshot(&p);
            p.run(q).await?;
            ran += 1;
            unchanged += (snapshot(&p) == before) as usize;
        }
    }
    check(
        "Queries leave chart state and plan unchanged",
        unchanged == ran,
        format!("{unchanged} of {ran} queries"),
    );
    check(
        "Queries are not in the command log",
        p.log.len() == commands.len(),
        format!("{} commands logged", p.log.len()),
    );

    // Undo refolds the log without its last command.
    let before = p.chart.clone();
    p.run("set fill #4c78a8").await?;
    let changed = p.chart != before;
    p.run("undo").await?;
    check(
        "Undo restores the previous state",
        changed && p.chart == before,
        "set fill, then undo".into(),
    );

    // Render twice: is rendering itself deterministic?
    let t = Instant::now();
    let (definition, rows) = chart::build_definition(&p).await?;
    let avc = definition.to_bytes()?;
    let a = chart::render_blocking(definition)?;
    let first_ms = t.elapsed().as_secs_f64() * 1e3;
    let (definition, _) = chart::build_definition(&p).await?;
    let a2 = chart::render_blocking(definition)?;
    check(
        "Rendering twice gives the same PNG",
        a == a2,
        format!("{} bytes, {rows} rows, {first_ms:.0} ms", a.len()),
    );
    std::fs::write(format!("{out}/cqrs_original.png"), &a)?;

    // Replay the saved pipeline in a fresh Pipeline.
    let saved = p.to_json();
    std::fs::write(
        format!("{out}/cqrs_pipeline.json"),
        serde_json::to_string_pretty(&saved)?,
    )?;
    let mut q = pipeline().await?;
    q.replay(&saved).await?;
    let (definition, _) = chart::build_definition(&q).await?;
    let b = chart::render_blocking(definition)?;
    check(
        "Replaying data steps + command log gives the same PNG",
        a == b,
        format!(
            "{} data steps, {} commands",
            saved["data"].as_array().unwrap().len(),
            saved["commands"].as_array().unwrap().len()
        ),
    );
    check(
        "… and the same chart state",
        q.chart == p.chart,
        String::new(),
    );

    // Option 2: task commands instead of setters, for the same chart.
    let setters = [
        "chart bar --x label --y points",
        "set title \"Heights\"",
        "set x.labelAngle -45",
        "set fill #c44e52",
    ];
    let tasks = [
        "bars points --by label",
        "title \"Heights\"",
        "rotate-labels x",
        "color #c44e52",
    ];
    let mut o1 = pipeline().await?;
    o1.run(&data).await?;
    for c in setters {
        o1.run(c).await?;
    }
    let mut o2 = pipeline().await?;
    o2.run(&data).await?;
    for c in tasks {
        o2.run(c).await?;
    }
    let (d1, _) = chart::build_definition(&o1).await?;
    let (d2, _) = chart::build_definition(&o2).await?;
    let same_png = chart::render_blocking(d1)? == chart::render_blocking(d2)?;
    check(
        "Option 2 (tasks) builds the same chart as option 1 (setters)",
        o1.chart == o2.chart && same_png,
        "same state and byte-identical PNG".into(),
    );

    // A rejected task leaves no trace: not logged, state unchanged.
    let before = (o2.chart.clone(), o2.log.clone());
    let bad = [
        "bars nope --by label",
        "zoom 700000..800000",
        "rotate-labels z",
        "color salmon",
        "highlight \"st_point(datum.height) > 1\"",
    ];
    let rejected = {
        let mut n = 0;
        for c in bad {
            n += o2.run(c).await.is_err() as usize;
        }
        n
    };
    check(
        "Invalid tasks are rejected and leave no trace",
        rejected == bad.len() && (o2.chart.clone(), o2.log.clone()) == before,
        format!("{rejected} of {} rejected", bad.len()),
    );

    // The vocabulary gap: option 1 can set any key, option 2 only has verbs.
    let axis_title_setter = o1.run("set x.title \"height above 42 m\"").await.is_ok();
    let axis_title_task = o2.run("axis-title x \"height above 42 m\"").await.is_ok();
    check(
        "An axis title: option 1 can say it, option 2 has no verb for it yet",
        axis_title_setter && !axis_title_task,
        "`set x.title …` works; `axis-title` is not a step".into(),
    );

    // The chart definition artifact: native plans and a data snapshot.
    std::fs::write(format!("{out}/cqrs_chart.avc"), &avc)?;
    let c = chart::render_avc(avc.clone())?;
    check(
        "Rendering the serialised chart definition gives the same PNG",
        a == c,
        format!("{} bytes of definition", avc.len()),
    );
    Ok(())
}
