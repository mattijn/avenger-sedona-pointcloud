//! `avenger-validate check "<pipeline>"` (or `-` for stdin) prints the report
//! as JSON; `export-cel` prints layers 1 and 2 as CEL; `corpus <dir> [n]`
//! runs every pipeline an LLM writer produced in experiment 7 (`n` times)
//! and prints verdicts and timings; `corpus <dir> --jsonl` prints each
//! pipeline as data with its layer 1 and 2 issues, for checking the CEL
//! export elsewhere.

use std::io::Read;

use avenger_validate::{calls_to_json, export, parse, Timings, Validator};
use serde_json::Value;

/// Every writer attempt in experiment 7's results: the pipeline and whether
/// the chart refused it.
fn corpus(dir: &str) -> Vec<(String, bool)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("a results directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with("decisions.json"))
        .collect();
    files.sort();
    let mut out = vec![];
    for f in files {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        for x in v.as_array().into_iter().flatten() {
            for (_, route) in x.as_object().into_iter().flatten() {
                for a in route["attempts"].as_array().into_iter().flatten() {
                    if let Some(p) = a["pipeline"].as_str() {
                        out.push((p.to_string(), !a["refused"].is_null()));
                    }
                }
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("check") => {
            let text = match args.get(2).map(String::as_str) {
                None | Some("-") => {
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s).unwrap();
                    s
                }
                Some(t) => t.to_string(),
            };
            let r = Validator::default().check(&text);
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
            std::process::exit(if r.valid { 0 } else { 1 });
        }
        Some("export-cel") => println!("{}", serde_json::to_string_pretty(&export::cel_bundle(&Validator::default().spec().clone())).unwrap()),
        Some("corpus") => {
            let dir = args.get(2).map(String::as_str).unwrap_or("experiments/07-chart-decisions/results");
            let pipes = corpus(dir);
            if args.iter().any(|a| a == "--jsonl") {
                let v = Validator::default().without_data();
                for (p, refused) in &pipes {
                    let Ok(calls) = parse(p) else { continue };
                    let r = v.check_calls(&calls);
                    let issues: Vec<Value> = r.issues.iter().filter(|i| i.layer <= 2).map(|i| serde_json::json!({"layer": i.layer, "step": i.step, "arg": i.arg})).collect();
                    println!("{}", serde_json::json!({"steps": calls_to_json(&calls), "refused": refused, "issues": issues}));
                }
                return;
            }
            let n: usize = args.get(3).and_then(|n| n.parse().ok()).unwrap_or(20);
            let t0 = std::time::Instant::now();
            let v = Validator::default();
            let start = t0.elapsed();
            let (mut caught, mut false_pos, mut refused_n) = (0, 0, 0);
            let mut by_layer = [0usize; 5];
            for (p, refused) in &pipes {
                let r = v.check(p);
                refused_n += *refused as usize;
                match (*refused, r.valid) {
                    (true, false) => caught += 1,
                    (false, false) => {
                        false_pos += 1;
                        eprintln!("flagged but accepted: {p}\n  {:?}", r.issues.iter().map(|i| &i.message).collect::<Vec<_>>());
                    }
                    _ => {}
                }
                for i in &r.issues {
                    by_layer[i.layer as usize] += 1;
                }
            }
            let distinct: std::collections::HashSet<&String> = pipes.iter().map(|(p, _)| p).collect();
            println!("{} pipelines ({} distinct), {refused_n} refused by the chart", pipes.len(), distinct.len());
            println!("refused and flagged: {caught} of {refused_n}; accepted but flagged: {false_pos}");
            println!("issues by layer 0..4: {by_layer:?}");
            println!("start-up: {:.2} ms", start.as_secs_f64() * 1e3);
            // Timings: a cold pass (empty plan cache), then n warm passes.
            for (label, reps, fresh) in [("cold, first pass", 1, true), ("warm", n, false)] {
                let v = if fresh { Validator::default() } else { Validator::default() };
                if !fresh {
                    for (p, _) in &pipes {
                        v.check(p);
                    }
                }
                let mut t = Timings::default();
                let w = std::time::Instant::now();
                for _ in 0..reps {
                    for (p, _) in &pipes {
                        v.check_timed(p, &mut t);
                    }
                }
                let k = (reps * pipes.len()) as f64 / 1e6;
                println!(
                    "{label}: {:.1} µs a pipeline (parse {:.1}, arguments {:.1}, state {:.1}, expressions {:.1}, data {:.1})",
                    w.elapsed().as_secs_f64() / k,
                    t.parse / k,
                    t.arguments / k,
                    t.state / k,
                    t.expressions / k,
                    t.data / k
                );
            }
        }
        _ => eprintln!("usage: avenger-validate check \"<pipeline>\" | export-cel | corpus <dir> [n] [--jsonl]"),
    }
}
