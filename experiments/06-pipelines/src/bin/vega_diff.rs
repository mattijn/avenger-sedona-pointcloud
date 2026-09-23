//! Phase B: differential test. The same expressions over the same rows, in
//! real Vega (reference/vega_eval.mjs) and compiled to DataFusion.
//!
//!     (cd experiments/06-pipelines/reference && npm install && \
//!      TZ=UTC node vega_eval.mjs ../corpus/differential.json > ../../../out/vega_reference.json)
//!     cargo run --release -p lidar-pipeline --bin vega_diff -- \
//!       experiments/06-pipelines/corpus/differential.json out/vega_reference.json

use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, Float64Array, RecordBatch, StringArray,
};
use arrow::compute::cast;
use arrow::datatypes::{DataType, Field, Float64Type, Schema};
use datafusion::common::DFSchema;
use datafusion::datasource::MemTable;
use datafusion::prelude::SessionContext;
use lidar_pipeline::packages::vega_format;
use lidar_pipeline::vega::{parse, Compiler};
use serde_json::{json, Value};

/// A value in the common JSON encoding shared with vega_eval.mjs.
fn decode_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Object(o) => match o.get("$").and_then(Value::as_str) {
            Some("NaN") => Some(f64::NAN),
            Some("Infinity") => Some(f64::INFINITY),
            Some("-Infinity") => Some(f64::NEG_INFINITY),
            _ => None,
        },
        _ => None,
    }
}

fn encode_num(x: f64) -> Value {
    if x.is_nan() {
        json!({"$": "NaN"})
    } else if x.is_infinite() {
        json!({"$": if x > 0.0 { "Infinity" } else { "-Infinity" }})
    } else {
        json!(x)
    }
}

fn to_json(a: &ArrayRef) -> Vec<Value> {
    let t = a.data_type().clone();
    let (n, a) = (a.len(), a.clone());
    if t == DataType::Boolean {
        let b = a.as_boolean();
        return (0..n)
            .map(|i| {
                if b.is_null(i) {
                    Value::Null
                } else {
                    json!(b.value(i))
                }
            })
            .collect();
    }
    if t.is_numeric() {
        let f = cast(&a, &DataType::Float64).unwrap();
        let f = f.as_primitive::<Float64Type>();
        return (0..n)
            .map(|i| {
                if f.is_null(i) {
                    Value::Null
                } else {
                    encode_num(f.value(i))
                }
            })
            .collect();
    }
    if t == DataType::Null {
        return vec![Value::Null; n];
    }
    let s = cast(&a, &DataType::Utf8).unwrap();
    let s = s.as_string::<i32>();
    (0..n)
        .map(|i| {
            if s.is_null(i) {
                Value::Null
            } else {
                json!(s.value(i))
            }
        })
        .collect()
}

fn same(v: &Value, d: &Value) -> bool {
    let undefined = |x: &Value| x.get("$").and_then(Value::as_str) == Some("undefined");
    match (v, d) {
        (Value::Null, Value::Null) => true,
        (x, Value::Null) if undefined(x) => true,
        _ => match (decode_num(v), decode_num(d)) {
            (Some(a), Some(b)) => {
                (a.is_nan() && b.is_nan()) || a == b || (a - b).abs() <= 1e-9 * a.abs().max(1.0)
            }
            _ => v == d,
        },
    }
}

fn short(v: &Value) -> String {
    match v {
        Value::Object(o) => o
            .get("$")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string(),
        Value::String(s) => format!("{s:?}"),
        other => other.to_string(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(&args[1])?)?;
    let reference: Value = serde_json::from_str(&std::fs::read_to_string(&args[2])?)?;
    let rows = cases["rows"].as_array().unwrap();

    let a: Float64Array = rows.iter().map(|r| decode_num(&r["a"])).collect();
    let b: Float64Array = rows.iter().map(|r| decode_num(&r["b"])).collect();
    let t: Float64Array = rows.iter().map(|r| decode_num(&r["t"])).collect();
    let s: StringArray = rows.iter().map(|r| r["s"].as_str()).collect();
    let f: BooleanArray = rows.iter().map(|r| r["f"].as_bool()).collect();
    let schema = Arc::new(Schema::new(vec![
        Field::new("a", DataType::Float64, true),
        Field::new("b", DataType::Float64, true),
        Field::new("s", DataType::Utf8, true),
        Field::new("f", DataType::Boolean, true),
        Field::new("t", DataType::Float64, true),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(a),
            Arc::new(b),
            Arc::new(s),
            Arc::new(f),
            Arc::new(t),
        ],
    )?;
    let ctx = SessionContext::new();
    for udf in vega_format::functions() {
        ctx.register_udf(udf);
    }
    ctx.register_table(
        "t",
        Arc::new(MemTable::try_new(schema.clone(), vec![vec![batch]])?),
    )?;
    for udf in lidar_pipeline::packages::vega_compat::functions() {
        ctx.register_udf(udf);
    }
    let state = ctx.state();
    let df_schema = DFSchema::try_from(schema.as_ref().clone())?;

    // Evaluate every expression under one semantics: per expression, either
    // the rows that differ from Vega, or why it could not run.
    let expressions: Vec<&str> = cases["expressions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();
    let mut results: Vec<[Result<Vec<String>, String>; 2]> = vec![];
    for e in &expressions {
        let r = &reference["results"][*e];
        let Some(expected) = r.get("values").and_then(Value::as_array) else {
            let msg = format!(
                "Vega throws: {}",
                r["error"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(50)
                    .collect::<String>()
            );
            results.push([Err(msg.clone()), Err(msg)]);
            continue;
        };
        let mut pair = vec![];
        for vega in [false, true] {
            let compiler = Compiler {
                schema: &df_schema,
                registry: &state,
                vega,
            };
            let got = match parse(e)
                .map_err(|x| x.to_string())
                .and_then(|ast| compiler.compile(&ast).map_err(|x| x.to_string()))
            {
                Ok(expr) => match ctx.table("t").await?.select(vec![expr.alias("out")]) {
                    Ok(df) => df
                        .collect()
                        .await
                        .map(|b| {
                            b.iter()
                                .flat_map(|b| to_json(b.column(0)))
                                .collect::<Vec<_>>()
                        })
                        .map_err(|x| x.to_string()),
                    Err(x) => Err(x.to_string()),
                },
                Err(x) => Err(x),
            };
            pair.push(got.map(|got| {
                expected
                    .iter()
                    .zip(&got)
                    .enumerate()
                    .filter(|(_, (v, d))| !same(v, d))
                    .map(|(i, (v, d))| format!("{i}: {} → {}", short(v), short(d)))
                    .collect()
            }));
        }
        let vega_mode = pair.pop().unwrap();
        results.push([pair.pop().unwrap(), vega_mode]);
    }

    let n = cases["rows"].as_array().unwrap().len();
    println!("| Expression | SQL semantics | Vega semantics | Remaining differences, Vega semantics (row: Vega → here) |\n|---|---|---|---|");
    let mut totals = [(0usize, 0usize, 0usize); 2]; // rows agreeing, rows compared, expressions fully agreeing
    for (e, pair) in expressions.iter().zip(&results) {
        let cell = |r: &Result<Vec<String>, String>| match r {
            Ok(d) => format!("{}/{n}", n - d.len()),
            Err(x) if x.starts_with("Vega throws") => "Vega throws".into(),
            Err(_) => "fails".into(),
        };
        for (k, r) in pair.iter().enumerate() {
            if let Ok(d) = r {
                totals[k].0 += n - d.len();
                totals[k].1 += n;
                totals[k].2 += d.is_empty() as usize;
            }
        }
        let rest = match &pair[1] {
            Ok(d) => d.join(", "),
            Err(x) => x.chars().take(70).collect(),
        };
        println!(
            "| `{}` | {} | {} | {} |",
            e.replace('|', "\\|"),
            cell(&pair[0]),
            cell(&pair[1]),
            rest.replace('|', "/")
        );
    }
    for (k, name) in ["SQL semantics", "Vega semantics"].iter().enumerate() {
        let (ok, all, full) = totals[k];
        println!("\n{name}: {ok}/{all} rows agree ({:.1} %), {full} of {} expressions agree on every row.", 100.0 * ok as f64 / all as f64, expressions.len());
    }
    Ok(())
}
