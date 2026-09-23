//! Phase A: how much of the Vega expression language compiles to DataFusion?
//!
//!     cargo run --release -p lidar-pipeline --bin vega_coverage -- experiments/06-pipelines/corpus/vega_test_data.json
//!
//! Every expression in the corpus is parsed, analysed, and, when it is a row
//! expression, compiled against a schema of its `datum` fields and planned by
//! DataFusion. Then every function in the Vega expression reference is probed
//! with numeric arguments.

use std::collections::{BTreeMap, BTreeSet};

use arrow::datatypes::{DataType, Field, Schema};
use datafusion::common::DFSchema;
use datafusion::logical_expr::ExprSchemable;
use datafusion::prelude::SessionContext;
use lidar_pipeline::vega::{analyze, parse, Blocker, Compiler};

/// The function list of <https://vega.github.io/vega/docs/expressions/>, by section.
const REFERENCE: [(&str, &str); 22] = [
    ("Type checking", "isArray isBoolean isDate isDefined isNumber isObject isRegExp isString isValid"),
    ("Type coercion", "toBoolean toDate toNumber toString"),
    ("Control flow", "if"),
    ("Math", "isNaN isFinite abs acos asin atan atan2 ceil clamp cos exp floor hypot log max min pow random round sin sqrt tan"),
    ("Easing", "easeLinear easeQuad easeQuadIn easeQuadOut easeQuadInOut easeCubic easeCubicIn easeCubicOut easeCubicInOut easePoly easePolyIn easePolyOut easePolyInOut easeSin easeSinIn easeSinOut easeSinInOut easeExp easeExpIn easeExpOut easeExpInOut easeCircle easeCircleIn easeCircleOut easeCircleInOut easeBounce easeBounceIn easeBounceOut easeBounceInOut easeBack easeBackIn easeBackOut easeBackInOut easeElastic easeElasticIn easeElasticOut easeElasticInOut"),
    ("Statistical", "sampleNormal cumulativeNormal densityNormal quantileNormal sampleLogNormal cumulativeLogNormal densityLogNormal quantileLogNormal sampleUniform cumulativeUniform densityUniform quantileUniform"),
    ("Date-time", "now datetime date day dayofyear year quarter month week isoweek hours minutes seconds milliseconds time timezoneoffset timeOffset timeSequence utc utcdate utcday utcdayofyear utcyear utcquarter utcmonth utcweek utcisoweek utchours utcminutes utcseconds utcmilliseconds utcOffset utcSequence"),
    ("Array", "extent clampRange inrange join lerp interpolateLinear peek pluck reverse sequence sort span"),
    ("String", "indexof lastindexof length lower pad parseFloat parseInt replace slice split substring trim truncate upper btoa atob encodeURIComponent"),
    ("Object", "merge"),
    ("Formatting", "dayFormat dayAbbrevFormat format monthFormat monthAbbrevFormat timeUnitSpecifier timeFormat timeParse utcFormat utcParse"),
    ("RegExp", "regexp test"),
    ("Color", "rgb hsl lab hcl luminance contrast"),
    ("Event", "item group xy x y pinchDistance pinchAngle inScope"),
    ("Data", "data indata"),
    ("Scale and projection", "scale invert copy domain range bandwidth bandspace gradient panLinear panLog panPow panSymlog zoomLinear zoomLog zoomPow zoomSymlog"),
    ("Geographic", "geoArea geoBounds geoCentroid geoScale geoTranslate"),
    ("Tree", "treePath treeAncestors"),
    ("Browser", "containerSize screen windowSize"),
    ("Logging", "warn info debug"),
    ("Selection (Vega-Lite)", "vlSelectionTest vlSelectionIdTest vlSelectionResolve vlSelectionTuples"),
    ("Constants", "NaN E LN2 LN10 LOG2E LOG10E MAX_VALUE MIN_VALUE PI SQRT1_2 SQRT2"),
];

#[derive(Default)]
struct Tally {
    n: usize,
    examples: Vec<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: vega_coverage <corpus.json>");
    let corpus: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let ctx = SessionContext::new();
    // With --package vega-format, register the d3 formatting functions first.
    if std::env::args().any(|a| a == "vega-format") {
        for f in lidar_pipeline::packages::vega_format::functions() {
            ctx.register_udf(f);
        }
    }
    let state = ctx.state();

    let mut outcome: BTreeMap<&str, Tally> = BTreeMap::new();
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut add = |k: &'static str, e: &str| {
        let t = outcome.entry(k).or_default();
        t.n += 1;
        if t.examples.len() < 3 {
            t.examples.push(e.chars().take(90).collect());
        }
    };
    for item in &corpus {
        let src = item["expr"].as_str().unwrap();
        let ast = match parse(src) {
            Ok(a) => a,
            Err(e) => {
                *reasons.entry(format!("parse: {}", e.message)).or_default() += 1;
                add("parse error", src);
                continue;
            }
        };
        let a = analyze(&ast);
        if let Some(b) = a.blockers.iter().next() {
            for b in &a.blockers {
                *reasons.entry(b.to_string()).or_default() += 1;
            }
            add(
                match b {
                    Blocker::ChartQuery(_) => "reads the chart (query)",
                    Blocker::Interaction(_) => "interaction (command)",
                    Blocker::Unsupported(_) => "unsupported",
                },
                src,
            );
            continue;
        }
        // Row expression: every datum field as Float64 (the corpus has no data).
        let schema = Schema::new(
            a.fields
                .iter()
                .map(|f| Field::new(f, DataType::Float64, true))
                .collect::<Vec<_>>(),
        );
        let df_schema = DFSchema::try_from(schema)?;
        let compiler = Compiler {
            schema: &df_schema,
            registry: &state,
            vega: false,
        };
        match compiler.compile(&ast) {
            Ok(expr) => match expr.get_type(&df_schema) {
                Ok(_) if a.signals.is_empty() => add("row expression, compiled", src),
                Ok(_) => add("row expression with signals, compiled", src),
                Err(e) => {
                    *reasons
                        .entry(format!("type: {e}").chars().take(100).collect())
                        .or_default() += 1;
                    add("compiled, fails to type", src)
                }
            },
            Err(b) => {
                *reasons.entry(b.to_string()).or_default() += 1;
                add("unsupported", src);
            }
        }
    }

    println!("## Corpus: {} unique expressions\n", corpus.len());
    println!("| Outcome | Expressions | Example |\n|---|---|---|");
    for (k, t) in &outcome {
        println!(
            "| {k} | {} | `{}` |",
            t.n,
            t.examples[0].replace('|', "\\|")
        );
    }
    println!("\n### Reasons (an expression can have several)\n");
    let mut r: Vec<_> = reasons.into_iter().collect();
    r.sort_by(|a, b| b.1.cmp(&a.1));
    for (k, n) in r.iter().take(25) {
        println!("- {n} × {k}");
    }

    // Probe every documented function with numeric arguments.
    let schema = Schema::new(vec![
        Field::new("a", DataType::Float64, true),
        Field::new("b", DataType::Float64, true),
        Field::new("c", DataType::Float64, true),
    ]);
    let df_schema = DFSchema::try_from(schema)?;
    let compiler = Compiler {
        schema: &df_schema,
        registry: &state,
        vega: false,
    };
    println!("\n## Vega expression reference\n");
    println!("| Section | Functions | Compile | Chart query | Interaction | Not yet |\n|---|---|---|---|---|---|");
    let (mut tot, mut ok_all) = (0, 0);
    let mut missing_all = BTreeSet::new();
    for (section, names) in REFERENCE {
        let (mut ok, mut chart, mut inter, mut missing) = (0, 0, 0, vec![]);
        for name in names.split_whitespace() {
            let src = if section == "Constants" {
                name.to_string()
            } else {
                format!("{name}(datum.a, datum.b, datum.c)")
            };
            let ast = parse(&src)?;
            let blocked = analyze(&ast).blockers.into_iter().next();
            match blocked
                .map(Err)
                .unwrap_or_else(|| compiler.compile(&ast).map(|_| ()))
            {
                Ok(()) => ok += 1,
                Err(Blocker::ChartQuery(_)) => chart += 1,
                Err(Blocker::Interaction(_)) => inter += 1,
                Err(Blocker::Unsupported(_)) => {
                    missing.push(name);
                    missing_all.insert(name);
                }
            }
        }
        let n = names.split_whitespace().count();
        tot += n;
        ok_all += ok;
        println!(
            "| {section} | {n} | {ok} | {chart} | {inter} | {} |",
            if missing.is_empty() {
                "–".to_string()
            } else {
                missing.join(", ")
            }
        );
    }
    println!("\n{ok_all} of {tot} documented names compile to DataFusion.");
    Ok(())
}
