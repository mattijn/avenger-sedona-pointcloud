//! Phase E: decide on every word while an instruction is being typed.
//! How often would the chart switch before the sentence is complete, and
//! how much does a confidence threshold with hysteresis help?
//!
//!     set -a; source <env>/.env; set +a
//!     cargo run --release -p lidar-decide --bin typing

use std::sync::Arc;

use lidar_decide::deciders::{Decider, Jev, Llm, Rules};
use lidar_decide::options::{self, Policy};
use lidar_decide::{build, observe, short, steps};
use serde_json::Value;

type Error = Box<dyn std::error::Error>;

const IDS: [&str; 6] = ["i01", "i05", "i09", "i10", "i12", "i14"];
/// Act only when the decider is at least this sure (Jev reports confidence;
/// rules and the LLM count as sure).
const THRESHOLD: f64 = 0.6;

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
    println!("| Instruction | Decider | Decision after each word | Chart changes | … with threshold {THRESHOLD} | Right from word |\n|---|---|---|---|---|---|");
    let mut totals = std::collections::BTreeMap::<&str, (usize, usize, usize)>::new();
    for id in IDS {
        let c = cases["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap();
        let base = &cases["bases"][c["base"].as_str().unwrap()];
        let mut p = build(&steps(&base["data"]), &steps(&base["commands"])).await?;
        let stats = observe::stats(&p).await?;
        let describe: Value = serde_json::from_str(&p.run("describe").await?.join(""))?;
        let q = options::questions(Policy::Free, &p.chart, &stats);
        let text = c["text"].as_str().unwrap();
        let words: Vec<&str> = text.split_whitespace().collect();
        for d in &deciders {
            let mut seq = vec![];
            for k in 1..=words.len() {
                let prefix = words[..k].join(" ");
                let obs = observe::observation(
                    Policy::Free.text(),
                    &describe,
                    &stats,
                    None,
                    Some(&prefix),
                );
                let dec = d
                    .decide(&obs, &q)
                    .await
                    .map_err(|e| format!("{} on {id}/{k}: {e}", d.name()))?;
                seq.push((
                    short(&dec.answers),
                    dec.confidence.unwrap_or(1.0),
                    dec.answers,
                ));
            }
            // Only decisions that become valid commands can change the chart;
            // an incomplete intent (zoom, but no region yet) does not.
            let applicable = |a: &serde_json::Map<String, Value>| {
                options::commands(a, &p.chart, &stats).is_ok_and(|c| !c.is_empty())
            };
            // Raw: the chart follows every applicable decision.
            let mut shown = "no_change".to_string();
            let mut raw = 0;
            for (s, _, a) in &seq {
                if applicable(a) && *s != shown {
                    raw += 1;
                    shown = s.clone();
                }
            }
            // With a threshold: act only on sure decisions.
            let mut shown_t = "no_change".to_string();
            let mut damped = 0;
            for (s, conf, a) in &seq {
                if applicable(a) && *s != shown_t && *conf >= THRESHOLD {
                    damped += 1;
                    shown_t = s.clone();
                }
            }
            let ok = |a: &serde_json::Map<String, Value>| {
                c["expect"].as_object().unwrap().iter().all(|(k, v)| {
                    let got = a.get(k).and_then(Value::as_str).unwrap_or("");
                    match v {
                        Value::Array(o) => o.iter().any(|x| x.as_str() == Some(got)),
                        v => v.as_str() == Some(got),
                    }
                })
            };
            let right_from = (0..seq.len()).find(|&i| seq[i..].iter().all(|(_, _, a)| ok(a)));
            let trail: Vec<String> = seq
                .iter()
                .map(|(s, conf, _)| {
                    if d.name() == "jev" {
                        format!("{s} ({conf:.2})")
                    } else {
                        s.clone()
                    }
                })
                .collect();
            println!(
                "| {text} | {} | {} | {raw} | {damped} | {} of {} |",
                d.name(),
                trail.join(" → "),
                right_from.map_or("never".into(), |i| (i + 1).to_string()),
                words.len()
            );
            let t = totals.entry(d.name()).or_default();
            t.0 += raw;
            t.1 += damped;
            t.2 += right_from.is_some() as usize;
        }
        eprintln!("{id} done");
    }
    println!("\n| Decider | Chart changes | … with threshold {THRESHOLD} | Instructions ending right |\n|---|---|---|---|");
    for (name, (raw, damped, ok)) in totals {
        println!("| {name} | {raw} | {damped} | {ok} of {} |", IDS.len());
    }
    std::fs::create_dir_all(format!("{}/results", env!("CARGO_MANIFEST_DIR")))?;
    std::fs::write(
        format!(
            "{}/results/cache_used_typing.txt",
            env!("CARGO_MANIFEST_DIR")
        ),
        lidar_decide::deciders::used_cache().join("\n"),
    )?;
    Ok(())
}
