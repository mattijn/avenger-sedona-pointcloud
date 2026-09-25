//! Phase C: three deciders behind one interface, with a response cache.
//!
//! - `Rules`: keywords and statistics; no network, the floor.
//! - `Jev`: TypeSafe's classifier through OpenRouter's Decisions endpoint.
//! - `Llm`: a general model (Claude Haiku 4.5) through OpenRouter's chat
//!   endpoint, asked to answer the same questions as JSON.
//!
//! The API key is read from `OPENROUTER_API_KEY` in the environment and only
//! ever placed in the request header. Responses are cached under
//! `experiments/07-chart-decisions/cache/`, keyed by a hash of the decider,
//! the observation and the questions, so reruns are reproducible and free.

use std::path::PathBuf;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

#[derive(Clone, Debug, Default)]
pub struct Decision {
    pub answers: Map<String, Value>,
    /// Confidence of the `action` answer, where the decider gives one.
    pub confidence: Option<f64>,
    pub ms: f64,
    pub cost: f64,
    pub input_tokens: u64,
    pub cached: bool,
    /// Probability per option of the `action` answer (Jev only).
    pub probs: Vec<(String, f64)>,
}

fn action_probs(raw: &Value) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = raw["answers"]["action"]["probabilities"]
        .as_object()
        .map(|m| m.iter().map(|(k, p)| (k.clone(), p.as_f64().unwrap_or(0.0))).collect())
        .unwrap_or_default();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

#[async_trait]
pub trait Decider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn decide(&self, observation: &Value, questions: &Value) -> Result<Decision, String>;
}

// ---------------------------------------------------------------------------

/// FNV-1a: a stable hash across runs and Rust versions, for cache keys.
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn cache_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("cache")
}

fn cache_path(decider: &str, observation: &Value, questions: &Value) -> PathBuf {
    let key = fnv(&format!("{decider}\n{observation}\n{questions}"));
    cache_dir().join(format!("{decider}-{key:016x}.json"))
}

/// Cache entries read or written by this process, so stale ones can be
/// pruned after a full run.
static USED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn mark_used(p: &std::path::Path) {
    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
        USED.lock().unwrap().push(name.to_string());
    }
}

/// The cache files this process used.
pub fn used_cache() -> Vec<String> {
    let mut v = USED.lock().unwrap().clone();
    v.sort();
    v.dedup();
    v
}

fn from_cache(p: &PathBuf) -> Option<Decision> {
    mark_used(p);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
    Some(Decision {
        answers: v["answers"].as_object()?.clone(),
        confidence: v["confidence"].as_f64(),
        ms: v["ms"].as_f64().unwrap_or(0.0),
        cost: v["cost"].as_f64().unwrap_or(0.0),
        input_tokens: v["input_tokens"].as_u64().unwrap_or(0),
        cached: true,
        probs: action_probs(&v["raw"]),
    })
}

fn to_cache(p: &PathBuf, d: &Decision, raw: &Value) {
    mark_used(p);
    let _ = std::fs::create_dir_all(cache_dir());
    let v = json!({
        "answers": d.answers, "confidence": d.confidence, "ms": d.ms,
        "cost": d.cost, "input_tokens": d.input_tokens, "raw": raw,
    });
    let _ = std::fs::write(p, serde_json::to_string_pretty(&v).unwrap());
}

fn api_key() -> Result<String, String> {
    std::env::var("OPENROUTER_API_KEY").map_err(|_| "OPENROUTER_API_KEY is not set".to_string())
}

// ---------------------------------------------------------------------------

pub struct Jev {
    pub model: &'static str,
    pub client: reqwest::Client,
}

#[async_trait]
impl Decider for Jev {
    fn name(&self) -> &'static str {
        "jev"
    }
    async fn decide(&self, observation: &Value, questions: &Value) -> Result<Decision, String> {
        let path = cache_path(self.name(), observation, questions);
        if let Some(d) = from_cache(&path) {
            return Ok(d);
        }
        let body = json!({"model": self.model, "state": observation, "questions": questions});
        let t = Instant::now();
        let resp = self
            .client
            .post("https://openrouter.ai/api/alpha/decisions")
            .bearer_auth(api_key()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let raw: Value = resp.json().await.map_err(|e| e.to_string())?;
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let answers = raw["answers"]
            .as_object()
            .ok_or_else(|| format!("jev: {raw}"))?;
        let mut out = Map::new();
        for (k, a) in answers {
            if let Some(c) = a["choice"].as_str() {
                out.insert(k.clone(), json!(c));
            }
        }
        let d = Decision {
            answers: out,
            confidence: answers.get("action").and_then(|a| a["confidence"].as_f64()),
            ms,
            cost: raw["usage"]["cost"].as_f64().unwrap_or(0.0),
            input_tokens: raw["usage"]["input_tokens"].as_u64().unwrap_or(0),
            cached: false,
            probs: action_probs(&raw),
        };
        to_cache(&path, &d, &raw);
        Ok(d)
    }
}

// ---------------------------------------------------------------------------

pub struct Llm {
    pub model: &'static str,
    pub client: reqwest::Client,
}

#[async_trait]
impl Decider for Llm {
    fn name(&self) -> &'static str {
        "llm"
    }
    async fn decide(&self, observation: &Value, questions: &Value) -> Result<Decision, String> {
        let path = cache_path(self.name(), observation, questions);
        if let Some(d) = from_cache(&path) {
            return Ok(d);
        }
        let prompt = format!(
            "You drive a chart. Read the state, then answer every question by choosing exactly one of \
             its option keys. Reply with a JSON object only, mapping each question id to the chosen key.\n\n\
             State:\n{}\n\nQuestions:\n{}",
            serde_json::to_string_pretty(observation).unwrap(),
            serde_json::to_string_pretty(questions).unwrap()
        );
        let body = json!({
            "model": self.model,
            "temperature": 0,
            "messages": [{"role": "user", "content": prompt}],
            "usage": {"include": true},
        });
        let t = Instant::now();
        let resp = self
            .client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .bearer_auth(api_key()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let raw: Value = resp.json().await.map_err(|e| e.to_string())?;
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let text = raw["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| format!("llm: {raw}"))?;
        let (a, b) = (
            text.find('{').unwrap_or(0),
            text.rfind('}').map_or(text.len(), |i| i + 1),
        );
        let answers: Map<String, Value> =
            serde_json::from_str(&text[a..b]).map_err(|e| format!("llm json: {e}: {text}"))?;
        let d = Decision {
            answers,
            confidence: None,
            ms,
            cost: raw["usage"]["cost"].as_f64().unwrap_or(0.0),
            input_tokens: raw["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            cached: false,
            probs: vec![],
        };
        to_cache(&path, &d, &raw);
        Ok(d)
    }
}

// ---------------------------------------------------------------------------

/// Keywords for instructions, statistics for data changes.
pub struct Rules;

fn has(t: &str, words: &[&str]) -> bool {
    words.iter().any(|w| t.contains(w))
}

#[async_trait]
impl Decider for Rules {
    fn name(&self) -> &'static str {
        "rules"
    }
    async fn decide(&self, o: &Value, questions: &Value) -> Result<Decision, String> {
        let t0 = Instant::now();
        let mut a = Map::new();
        let set = |a: &mut Map<String, Value>, k: &str, v: &str| {
            a.insert(k.into(), json!(v));
        };
        let free = questions.get("mark").is_some();
        // The chart layer's marks (pilot.rs), only offered there.
        let layer = questions["mark"]["criteria"].get("pie").is_some();
        if let Some(i) = o["instruction"].as_str() {
            let t = i.to_lowercase();
            let colour = ["red", "blue", "orange", "green", "grey", "gray"]
                .into_iter()
                .find(|c| t.contains(c));
            let region = if has(&t, &["north-east", "north east", "northeast", "top right"]) {
                Some("north_east")
            } else if has(&t, &["north-west", "north west", "northwest", "top left"]) {
                Some("north_west")
            } else if has(
                &t,
                &["south-east", "south east", "southeast", "bottom right"],
            ) {
                Some("south_east")
            } else if has(
                &t,
                &["south-west", "south west", "southwest", "bottom left"],
            ) {
                Some("south_west")
            } else if has(
                &t,
                &["everything", "whole", "reset", "all of it", "zoom out"],
            ) {
                Some("all")
            } else {
                None
            };
            if has(&t, &["don't", "do not", "nothing", "no change", "leave it"]) {
                set(&mut a, "action", "no_change");
            } else if let Some(c) = colour {
                set(&mut a, "action", "color");
                set(&mut a, "colour", if c == "gray" { "grey" } else { c });
            } else if let Some(r) = region {
                set(&mut a, "action", "zoom");
                set(&mut a, "region", r);
            } else if has(&t, &["label"])
                && has(&t, &["rotate", "tilt", "angle", "vertical", "turn"])
            {
                set(&mut a, "action", "rotate_labels");
                set(
                    &mut a,
                    "angle",
                    if has(&t, &["vertical", "90"]) {
                        "vertical"
                    } else {
                        "tilt"
                    },
                );
            } else if has(&t, &["outlier"]) {
                set(&mut a, "action", "highlight");
                set(&mut a, "subset", "outliers");
            } else if has(&t, &["highlight", "emphas", "tallest", "highest", "tall "]) {
                set(&mut a, "action", "highlight");
                set(&mut a, "subset", "top_10");
            } else if has(&t, &["title"]) {
                set(&mut a, "action", "title");
            } else if layer && has(&t, &["pie", "donut", "share"]) {
                set(&mut a, "action", "mark");
                set(&mut a, "mark", "pie");
            } else if layer && has(&t, &["heatmap", "heat map"]) {
                set(&mut a, "action", "mark");
                set(&mut a, "mark", "heatmap");
            } else if layer && has(&t, &["time series", "over time", "timeline"]) {
                set(&mut a, "action", "mark");
                set(&mut a, "mark", "line");
            } else if free && has(&t, &["map"]) {
                set(&mut a, "action", "mark");
                set(&mut a, "mark", "map");
            } else if free && has(&t, &["bar"]) {
                set(&mut a, "action", "mark");
                set(&mut a, "mark", "bars");
            } else if free && has(&t, &["scatter"]) {
                set(&mut a, "action", "mark");
                set(&mut a, "mark", "points");
            } else {
                set(&mut a, "action", "no_change");
            }
        } else {
            let change = o["data_change"].as_str().unwrap_or("");
            if free && has(change, &["(coordinate)", "(geometry"]) && has(change, &["new column"]) {
                set(&mut a, "action", "mark");
                set(&mut a, "mark", "map");
            } else if has(change, &["extreme value"]) {
                set(&mut a, "action", "highlight");
                set(&mut a, "subset", "outliers");
            } else {
                set(&mut a, "action", "no_change");
            }
        }
        Ok(Decision {
            answers: a,
            confidence: Some(1.0),
            ms: t0.elapsed().as_secs_f64() * 1e3,
            ..Default::default()
        })
    }
}
