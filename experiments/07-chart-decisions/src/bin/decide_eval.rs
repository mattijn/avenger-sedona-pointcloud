//! Phases C, D and F: every case, every decider, both policies where a case
//! has them. Scores the answers against the expected ones, sends the chosen
//! commands through experiment 6's task validation, and renders the charts
//! Jev's decisions produce.
//!
//!     set -a; source <env>/.env; set +a
//!     cargo run --release -p lidar-decide --bin decide_eval

use std::collections::BTreeMap;
use std::sync::Arc;

use lidar_decide::deciders::{Decider, Decision, Jev, Llm, Rules};
use lidar_decide::observe::{self, Stats};
use lidar_decide::options::{self, NotApplied, Policy};
use lidar_decide::{build, steps};
use lidar_pipeline::packages::chart;
use lidar_pipeline::pipeline::Pipeline;
use serde_json::{json, Map, Value};

type Error = Box<dyn std::error::Error>;

/// Does a decision match the expected answers? Only the keys the case names.
fn matches(expect: &Value, answers: &Map<String, Value>) -> bool {
    expect.as_object().unwrap().iter().all(|(k, v)| {
        let got = answers.get(k).and_then(Value::as_str).unwrap_or("");
        match v {
            Value::Array(options) => options.iter().any(|o| o.as_str() == Some(got)),
            v => v.as_str() == Some(got),
        }
    })
}

fn short(d: &Decision) -> String {
    lidar_decide::short(&d.answers)
}

struct Row {
    case: String,
    kind: &'static str,
    decider: &'static str,
    correct: bool,
    decision: Decision,
    applied: String,
}

/// Send a decision's commands through the pipeline's task validation, then
/// undo them so the next decider starts from the same chart.
async fn apply(p: &mut Pipeline, d: &Decision, stats: &Stats) -> String {
    match options::commands(&d.answers, &p.chart, stats) {
        Err(NotApplied::NeedsText) => "needs free text".into(),
        Err(NotApplied::Unfit(m)) => format!("unfit: {m}"),
        Ok(cmds) if cmds.is_empty() => "nothing to do".into(),
        Ok(cmds) => {
            let mut accepted = 0;
            let mut msg = String::new();
            for c in &cmds {
                match p.run(c).await {
                    Ok(_) => accepted += 1,
                    Err(e) => msg = format!("rejected: {e}"),
                }
            }
            for _ in 0..accepted {
                let _ = p.run("undo").await;
            }
            if msg.is_empty() {
                format!("accepted: {}", cmds.join(" ; "))
            } else {
                msg
            }
        }
    }
}

async fn render_with(
    p: &mut Pipeline,
    d: &Decision,
    stats: &Stats,
    path: &str,
) -> Result<(), Error> {
    let mut n = 0;
    if let Ok(cmds) = options::commands(&d.answers, &p.chart, stats) {
        for c in cmds {
            if p.run(&c).await.is_ok() {
                n += 1;
            }
        }
    }
    let (def, _) = chart::build_definition(p).await?;
    std::fs::write(path, chart::render_blocking(def)?)?;
    for _ in 0..n {
        let _ = p.run("undo").await;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let root = env!("CARGO_MANIFEST_DIR");
    let cases: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{root}/cases.json"))?)?;
    let client = reqwest::Client::new();
    let deciders: Vec<Arc<dyn Decider>> = vec![
        Arc::new(Rules),
        Arc::new(Jev {
            model: "typesafe/jev-1.13",
            client: client.clone(),
        }),
        Arc::new(Llm {
            model: "anthropic/claude-haiku-4.5",
            client,
        }),
    ];
    let gallery = format!("{root}/images/cases");
    std::fs::create_dir_all(&gallery)?;
    let mut rows: Vec<Row> = vec![];
    let mut log = vec![];

    // ---- typed instructions ------------------------------------------------
    for c in cases["instructions"].as_array().unwrap() {
        let base = &cases["bases"][c["base"].as_str().unwrap()];
        let mut p = build(&steps(&base["data"]), &steps(&base["commands"])).await?;
        let stats = observe::stats(&p).await?;
        let policy = if c["policy"] == "narrow" {
            Policy::Narrow
        } else {
            Policy::Free
        };
        let describe: Value = serde_json::from_str(&p.run("describe").await?.join(""))?;
        let text = c["text"].as_str().unwrap();
        let obs = observe::observation(policy.text(), &describe, &stats, None, Some(text));
        let q = options::questions(policy, &p.chart, &stats);
        let id = c["id"].as_str().unwrap();
        for d in &deciders {
            let decision = d
                .decide(&obs, &q)
                .await
                .map_err(|e| format!("{} on {id}: {e}", d.name()))?;
            let correct = matches(&c["expect"], &decision.answers);
            let applied = apply(&mut p, &decision, &stats).await;
            if d.name() == "jev" {
                render_with(&mut p, &decision, &stats, &format!("{gallery}/{id}.png")).await?;
            }
            log.push(json!({"case": id, "text": text, "policy": policy.name(), "decider": d.name(), "answers": decision.answers, "confidence": decision.confidence, "correct": correct, "applied": applied, "ms": decision.ms, "cost": decision.cost, "input_tokens": decision.input_tokens, "cached": decision.cached}));
            rows.push(Row {
                case: format!("{id} {text}"),
                kind: "instruction",
                decider: d.name(),
                correct,
                decision,
                applied,
            });
        }
        eprintln!("{id} done");
    }

    // ---- data changes, under both policies ------------------------------------
    for c in cases["data_changes"].as_array().unwrap() {
        let base = &cases["bases"][c["base"].as_str().unwrap()];
        let pick = |k: &str| {
            if c[k] == "base" {
                steps(&base["data"])
            } else {
                steps(&c[k])
            }
        };
        let commands = steps(&base["commands"]);
        let before_p = build(&pick("before"), &commands).await?;
        let before = observe::stats(&before_p).await?;
        let mut p = build(&pick("after"), &commands).await?;
        let after = observe::stats(&p).await?;
        let change = observe::diff_data(&before, &after);
        let describe: Value = serde_json::from_str(&p.run("describe").await?.join(""))?;
        let id = c["id"].as_str().unwrap();
        for policy in [Policy::Free, Policy::Narrow] {
            let obs = observe::observation(policy.text(), &describe, &after, Some(&change), None);
            let q = options::questions(policy, &p.chart, &after);
            let expect = &c["expect"][policy.name()];
            for d in &deciders {
                let decision = d
                    .decide(&obs, &q)
                    .await
                    .map_err(|e| format!("{} on {id}: {e}", d.name()))?;
                let correct = matches(expect, &decision.answers);
                let applied = apply(&mut p, &decision, &after).await;
                if d.name() == "jev" && policy == Policy::Free {
                    render_with(&mut p, &decision, &after, &format!("{gallery}/{id}.png")).await?;
                }
                log.push(json!({"case": id, "change": change, "policy": policy.name(), "decider": d.name(), "answers": decision.answers, "confidence": decision.confidence, "correct": correct, "applied": applied, "ms": decision.ms, "cost": decision.cost, "input_tokens": decision.input_tokens, "cached": decision.cached}));
                rows.push(Row {
                    case: format!("{id} {} ({})", c["name"].as_str().unwrap(), policy.name()),
                    kind: "data change",
                    decider: d.name(),
                    correct,
                    decision,
                    applied,
                });
            }
        }
        eprintln!("{id} done: {change}");
    }

    // ---- report ------------------------------------------------------------
    std::fs::create_dir_all(format!("{root}/results"))?;
    std::fs::write(
        format!("{root}/results/decisions.json"),
        serde_json::to_string_pretty(&log)?,
    )?;

    let names: Vec<&str> = deciders.iter().map(|d| d.name()).collect();
    println!(
        "| Case | {} |\n|---|{}",
        names.join(" | "),
        "---|".repeat(names.len())
    );
    let mut by_case: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
    for r in &rows {
        by_case.entry(r.case.clone()).or_default().push(r);
    }
    for (case, rs) in &by_case {
        let cells: Vec<String> = rs
            .iter()
            .map(|r| {
                let conf = r
                    .decision
                    .confidence
                    .filter(|_| r.decider == "jev")
                    .map(|c| format!(" ({c:.2})"))
                    .unwrap_or_default();
                format!(
                    "{}{}{}",
                    if r.correct { "" } else { "**✗** " },
                    short(&r.decision),
                    conf
                )
            })
            .collect();
        println!("| {case} | {} |", cells.join(" | "));
    }
    println!("\n| Decider | Instructions correct | Data changes correct | Latency p50 / p95 | Cost, all decisions | Input tokens (mean) | Accepted / needs text / unfit / rejected |\n|---|---|---|---|---|---|---|");
    for name in &names {
        let rs: Vec<&Row> = rows.iter().filter(|r| r.decider == *name).collect();
        let ok = |k: &str| {
            (
                rs.iter().filter(|r| r.kind == k && r.correct).count(),
                rs.iter().filter(|r| r.kind == k).count(),
            )
        };
        let (ci, ni) = ok("instruction");
        let (cd, nd) = ok("data change");
        let mut ms: Vec<f64> = rs.iter().map(|r| r.decision.ms).collect();
        ms.sort_by(f64::total_cmp);
        let pct = |p: f64| ms[((ms.len() - 1) as f64 * p).round() as usize];
        let cost: f64 = rs.iter().map(|r| r.decision.cost).sum();
        let tokens = rs.iter().map(|r| r.decision.input_tokens).sum::<u64>() / rs.len() as u64;
        let count = |pre: &str| rs.iter().filter(|r| r.applied.starts_with(pre)).count();
        println!(
            "| {name} | {ci}/{ni} | {cd}/{nd} | {:.0} / {:.0} ms | ${cost:.5} | {tokens} | {} / {} / {} / {} |",
            pct(0.5), pct(0.95), count("accepted"), count("needs"), count("unfit"), count("rejected")
        );
    }
    std::fs::create_dir_all(format!("{}/results", env!("CARGO_MANIFEST_DIR")))?;
    std::fs::write(
        format!(
            "{}/results/cache_used_decide_eval.txt",
            env!("CARGO_MANIFEST_DIR")
        ),
        lidar_decide::deciders::used_cache().join("\n"),
    )?;
    Ok(())
}
