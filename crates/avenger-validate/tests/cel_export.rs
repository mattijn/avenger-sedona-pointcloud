//! The CEL export must say what the native layers 1 and 2 say: per type on
//! a list of values, and per pipeline on experiment 7's corpus, run through
//! the bundle by a driver that follows `export`'s documented algorithm.

use std::collections::HashMap;

use avenger_validate::{export, parse, types, Spec, Validator};
use serde_json::Value;

fn eval(src: &str, vars: &[(&str, cel::Value)]) -> bool {
    let p = cel::Program::compile(src).unwrap_or_else(|e| panic!("`{src}` compiles: {e:?}"));
    let mut cx = cel::Context::default();
    for (k, v) in vars {
        cx.add_variable_from_value(*k, v.clone());
    }
    matches!(p.execute(&cx), Ok(cel::Value::Bool(true)))
}

#[test]
fn every_type_agrees_with_its_cel() {
    let values = [
        "", "0", "1", "0.5", ".5", "1.0", "1.5", "-0.1", "0.", "00.5", "1e-1", "+0.5", "abc", "#c44e52", "#C44E52", "#c44e5", "#c44e52f", "green",
        "0.2,0.8", "0.2, 0.8", "0.2,1.2", "1,0", "12", "-3.5e2", "3.", "1..2 3..4", "1..2", "label", "label:N", "label:Z", "a:b:Q", "a:", ":N",
        "true", "false", "bar", "Bar", "tilt",
    ];
    let spec = Spec::builtin();
    let all: Vec<&types::ArgType> = spec.steps.values().flat_map(|s| s.positional.iter().map(|a| &a.ty).chain(s.flags.values())).collect();
    let checker = types::Checker::new(all.iter().copied());
    for ty in &all {
        for v in values {
            let native = checker.check(ty, v).is_ok();
            let cel = eval(&ty.cel(), &[("v", v.into())]);
            assert_eq!(native, cel, "type {} on `{v}`: native {native}, CEL {cel} ({})", ty.name(), ty.cel());
        }
    }
}

/// Layers 1 and 2 by the bundle alone: `(layer, step)` for each issue.
fn by_bundle(bundle: &Value, steps: &Value) -> Vec<(u8, usize)> {
    let mut out = vec![];
    let mut state: HashMap<String, String> = bundle["state"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
    for (i, s) in steps.as_array().unwrap().iter().enumerate() {
        let Some(spec) = bundle["steps"].get(s["step"].as_str().unwrap()) else {
            out.push((1, i));
            continue;
        };
        let mut args: HashMap<String, String> = HashMap::new();
        let given: Vec<&str> = s["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
        let pos = spec["positional"].as_array().unwrap();
        for (k, p) in pos.iter().enumerate() {
            match given.get(k) {
                Some(v) => {
                    if !eval(p["check"].as_str().unwrap(), &[("v", (*v).into())]) {
                        out.push((1, i));
                    }
                    args.insert(p["name"].as_str().unwrap().into(), v.to_string());
                }
                None if p["optional"].as_bool().unwrap() => {}
                None => out.push((1, i)),
            }
        }
        if given.len() > pos.len() {
            out.push((1, i));
        }
        for (k, v) in s["flags"].as_object().unwrap() {
            let v = v.as_str().unwrap();
            match spec["flags"].get(k) {
                Some(f) => {
                    if !eval(f["check"].as_str().unwrap(), &[("v", v.into())]) {
                        out.push((1, i));
                    }
                    args.insert(k.clone(), v.to_string());
                }
                None => out.push((1, i)),
            }
        }
        if !eval(spec["requires"].as_str().unwrap(), &[("state", state.clone().into()), ("step", args.clone().into())]) {
            out.push((2, i));
        }
        for (k, v) in spec["sets"].as_object().unwrap() {
            let v = v.as_str().unwrap();
            let v = v.strip_prefix('$').map_or(v.to_string(), |a| args.get(a).cloned().unwrap_or_default());
            state.insert(k.clone(), v);
        }
    }
    out
}

#[test]
fn the_bundle_agrees_with_the_validator_on_the_corpus() {
    let v = Validator::default().without_data();
    let bundle = export::cel_bundle(v.spec());
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../experiments/07-chart-decisions/results");
    let mut n = 0;
    for f in std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.to_string_lossy().ends_with("decisions.json")) {
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        for x in doc.as_array().into_iter().flatten() {
            for (_, route) in x.as_object().into_iter().flatten() {
                for a in route["attempts"].as_array().into_iter().flatten() {
                    let Some(p) = a["pipeline"].as_str() else { continue };
                    let calls = parse(p).unwrap();
                    let native: Vec<(u8, usize)> = v.check_calls(&calls).issues.iter().filter(|i| i.layer <= 2).map(|i| (i.layer, i.step.unwrap())).collect();
                    let cel = by_bundle(&bundle, &avenger_validate::calls_to_json(&calls));
                    assert_eq!(native, cel, "{p}");
                    n += 1;
                }
            }
        }
    }
    assert!(n > 200, "the corpus has {n} pipelines");
}
