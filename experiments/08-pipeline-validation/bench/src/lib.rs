//! A four-layer validator for experiment 7's pipelines. The step vocabulary
//! comes from `spec/steps.json`, the same file the Python validator reads.
//! With the `python` feature this crate is also a Python module (pyo3).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};


use cel::{Context, Program};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use datafusion::prelude::SessionContext;
use serde_json::{json, Value};

#[path = "../../../06-pipelines/src/vega/parse.rs"]
#[allow(dead_code)]
mod vega;

// ---------------------------------------------------------------------------
// Layer 0: the text into steps. A copy of experiment 6's `parse_pipeline`.

#[derive(Debug, Default)]
pub struct Call {
    name: String,
    args: Vec<String>,
    flags: BTreeMap<String, String>,
}

pub fn parse_pipeline(src: &str) -> Result<Vec<Call>, String> {
    let mut words: Vec<(String, bool)> = vec![];
    let mut cur = String::new();
    let (mut quoted, mut in_quote) = (false, None::<char>);
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match (in_quote, c) {
            (Some(q), c) if c == q => in_quote = None,
            (Some(_), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n)
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                in_quote = Some(c);
                quoted = true;
            }
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() || quoted {
                    words.push((std::mem::take(&mut cur), quoted));
                }
                quoted = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if in_quote.is_some() {
        return Err("unterminated quote in pipeline".into());
    }
    if !cur.is_empty() || quoted {
        words.push((cur, quoted));
    }
    let mut calls = vec![];
    for group in words.split(|(w, q)| w == "!" && !q) {
        let mut it = group.iter().peekable();
        let Some((name, _)) = it.next() else { continue };
        let mut call = Call { name: name.clone(), ..Default::default() };
        while let Some((w, q)) = it.next() {
            if let (Some(k), false) = (w.strip_prefix("--"), *q) {
                let v = match it.peek() {
                    Some((v, vq)) if *vq || !v.starts_with("--") => it.next().unwrap().0.clone(),
                    _ => "true".into(),
                };
                call.flags.insert(k.to_string(), v);
            } else {
                call.args.push(w.clone());
            }
        }
        calls.push(call);
    }
    Ok(calls)
}

// ---------------------------------------------------------------------------
// Layer 1: one argument against its type.

fn is_hex(v: &str) -> bool {
    v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn unit(v: &str) -> bool {
    v.parse::<f64>().is_ok_and(|x| (0.0..=1.0).contains(&x))
}

fn check_type(ty: &Value, v: &str) -> Result<(), String> {
    if let Some(choices) = ty.as_array() {
        return if choices.iter().any(|c| c == v) { Ok(()) } else { Err(format!("`{v}` is not one of {ty}")) };
    }
    let ok = match ty.as_str().unwrap_or("text") {
        "hex" => is_hex(v),
        "unit" => unit(v),
        "unit_pair" => v.split(',').filter(|s| unit(s)).count() == 2 && v.split(',').count() == 2,
        "number" => v.parse::<f64>().is_ok(),
        "flag" => matches!(v, "true" | "false"),
        "channel" => {
            is_hex(v)
                || match v.split_once(':') {
                    Some((f, t)) => !f.is_empty() && matches!(t, "N" | "O" | "Q" | "T"),
                    None => !v.is_empty(),
                }
        }
        _ => true,
    };
    if ok { Ok(()) } else { Err(format!("`{v}` is not a {ty}")) }
}

// ---------------------------------------------------------------------------

struct Step {
    positional: Vec<(String, Value)>,
    flags: BTreeMap<String, Value>,
    requires: Program,
    requires_src: String,
    sets: BTreeMap<String, String>,
}

pub struct Spec {
    steps: BTreeMap<String, Step>,
    state: Value,
}

pub fn load_spec(path: &str) -> Spec {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut steps = BTreeMap::new();
    for (name, s) in v["steps"].as_object().unwrap() {
        let requires_src = s["requires"].as_str().unwrap_or("true").to_string();
        steps.insert(
            name.clone(),
            Step {
                positional: s["positional"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|p| (p[0].as_str().unwrap().to_string(), p[1].clone()))
                    .collect(),
                flags: s["flags"].as_object().into_iter().flatten().map(|(k, t)| (k.clone(), t.clone())).collect(),
                requires: Program::compile(&requires_src).unwrap(),
                requires_src,
                sets: s["sets"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, t)| (k.clone(), t.as_str().unwrap().to_string()))
                    .collect(),
            },
        );
    }
    Spec { steps, state: v["state"].clone() }
}

pub fn las_schema() -> SchemaRef {
    // The columns the tile's pipelines use, with the types of the LAS format.
    let f = |n: &str, t: DataType| Field::new(n, t, true);
    Arc::new(Schema::new(vec![
        f("x", DataType::Float64),
        f("y", DataType::Float64),
        f("z", DataType::Float64),
        f("intensity", DataType::UInt16),
        f("return_number", DataType::UInt8),
        f("number_of_returns", DataType::UInt8),
        f("classification", DataType::UInt8),
        f("scan_angle", DataType::Float32),
        f("user_data", DataType::UInt8),
        f("point_source_id", DataType::UInt16),
        f("gps_time", DataType::Float64),
        f("red", DataType::UInt16),
        f("green", DataType::UInt16),
        f("blue", DataType::UInt16),
    ]))
}

fn set_input(ctx: &SessionContext, schema: SchemaRef) {
    let _ = ctx.deregister_table("input");
    ctx.register_table("input", Arc::new(MemTable::try_new(schema, vec![vec![]]).unwrap())).unwrap();
}

#[derive(Default)]
pub struct Timings {
    pub parse: Duration,
    pub l1: Duration,
    pub l2: Duration,
    pub l3: Duration,
    pub l4: Duration,
}

/// Validate one pipeline; returns the errors, each tagged with its layer.
pub async fn validate(spec: &Spec, ctx: &SessionContext, src: &str, t: &mut Timings) -> Vec<String> {
    let mut errs = vec![];
    let t0 = Instant::now();
    let calls = match parse_pipeline(src) {
        Ok(c) => c,
        Err(e) => return vec![format!("L0 {e}")],
    };
    t.parse += t0.elapsed();
    let mut state = spec.state.clone();
    let mut schema: Option<SchemaRef> = None;
    for (i, c) in calls.iter().enumerate() {
        let Some(step) = spec.steps.get(&c.name) else {
            errs.push(format!("L1 step {i}: unknown step `{}`", c.name));
            continue;
        };
        // Layer 1: each argument against its type.
        let t1 = Instant::now();
        let mut args: BTreeMap<&str, &str> = BTreeMap::new();
        for (k, (name, ty)) in step.positional.iter().enumerate() {
            if let Some(v) = c.args.get(k) {
                if let Err(e) = check_type(ty, v) {
                    errs.push(format!("L1 {} {name}: {e}", c.name));
                }
                args.insert(name, v);
            }
        }
        for (k, v) in &c.flags {
            match step.flags.get(k) {
                Some(ty) => {
                    if let Err(e) = check_type(ty, v) {
                        errs.push(format!("L1 {} --{k}: {e}", c.name));
                    }
                    args.insert(k, v);
                }
                None => errs.push(format!("L1 {}: no flag --{k}", c.name)),
            }
        }
        t.l1 += t1.elapsed();

        // Layer 2: the step's precondition on the state, then its effect.
        let t2 = Instant::now();
        let mut cx = Context::default();
        cx.add_variable("state", state.clone()).unwrap();
        cx.add_variable("step", json!(args)).unwrap();
        match step.requires.execute(&cx) {
            Ok(cel::Value::Bool(true)) => {}
            _ => errs.push(format!("L2 {}: needs {} (state {state})", c.name, step.requires_src)),
        }
        for (k, v) in &step.sets {
            state[k] = match v.strip_prefix('$') {
                Some(a) => json!(args.get(a).copied().unwrap_or("")),
                None => json!(v),
            };
        }
        t.l2 += t2.elapsed();

        // Layer 3: embedded Vega expressions.
        let t3 = Instant::now();
        for (name, ty) in step.positional.iter().map(|(n, t)| (n.as_str(), t)).chain(step.flags.iter().map(|(n, t)| (n.as_str(), t))) {
            if ty == "vega" {
                if let Some(src) = args.get(name) {
                    if let Err(e) = vega::parse(src) {
                        errs.push(format!("L3 {}: vega: {e}", c.name));
                    }
                }
            }
        }
        t.l3 += t3.elapsed();

        // Layer 4: the data. Plan the SQL against a table with no rows, and
        // check the channels' fields against the schema that reaches them.
        let t4 = Instant::now();
        if c.name == "read" {
            schema = Some(las_schema());
            set_input(ctx, las_schema());
        }
        if c.name == "sql" {
            if let Some(q) = args.get("query") {
                match ctx.sql(q).await {
                    Ok(df) => {
                        let s: SchemaRef = Arc::new(df.schema().as_arrow().clone());
                        set_input(ctx, s.clone());
                        schema = Some(s);
                    }
                    Err(e) => errs.push(format!("L4 sql: {}", e.to_string().lines().next().unwrap_or(""))),
                }
            }
        }
        if let Some(s) = &schema {
            for (name, ty) in step.positional.iter().map(|(n, t)| (n.as_str(), t)).chain(step.flags.iter().map(|(n, t)| (n.as_str(), t))) {
                if ty != "channel" {
                    continue;
                }
                if let Some(v) = args.get(name).filter(|v| !is_hex(v)) {
                    let f = v.split(':').next().unwrap();
                    if s.field_with_name(f).is_err() {
                        errs.push(format!("L4 {} --{name}: no field `{f}`", c.name));
                    }
                }
            }
        }
        t.l4 += t4.elapsed();
    }
    errs
}

pub fn corpus(dir: &str) -> Vec<(String, bool)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with("decisions.json"))
        .collect();
    files.sort();
    let mut out = vec![];
    for f in files {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        for x in v.as_array().into_iter().flatten() {
            for (_, route) in x.as_object().unwrap() {
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


#[cfg(feature = "python")]
mod python {
    use pyo3::prelude::*;

    /// `Validator(spec_path)`; `check(text)` returns the errors of one
    /// pipeline, `check_many(texts)` those of each.
    #[pyclass(unsendable)]
    struct Validator {
        spec: super::Spec,
        ctx: super::SessionContext,
        rt: tokio::runtime::Runtime,
    }

    #[pymethods]
    impl Validator {
        #[new]
        fn new(spec_path: &str) -> Self {
            Validator {
                spec: super::load_spec(spec_path),
                ctx: super::SessionContext::new(),
                rt: tokio::runtime::Builder::new_current_thread().build().unwrap(),
            }
        }
        fn check(&self, src: &str) -> Vec<String> {
            let mut t = super::Timings::default();
            self.rt.block_on(super::validate(&self.spec, &self.ctx, src, &mut t))
        }
        fn check_many(&self, srcs: Vec<String>) -> Vec<Vec<String>> {
            let mut t = super::Timings::default();
            srcs.iter().map(|s| self.rt.block_on(super::validate(&self.spec, &self.ctx, s, &mut t))).collect()
        }
    }

    #[pymodule]
    fn lidar_validate(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add_class::<Validator>()
    }
}
