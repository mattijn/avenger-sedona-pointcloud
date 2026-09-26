//! Layers 1 and 2 as CEL, for programs without this library. The bundle is
//! JSON: per step, a CEL check over `v` for each argument, the `requires`
//! rule over `state` and `step`, and what the step `sets`. Run it as
//! [`crate::Validator`] does:
//!
//! 1. an unknown step is an issue; each positional in order: missing (and not
//!    optional) is an issue, else its `check` must be true; more positionals
//!    than declared is an issue; each flag must be declared and pass its
//!    `check`;
//! 2. `requires` must be true, with `state` as it is before the step and
//!    `step` the arguments by name; then apply `sets` (`$name` is the
//!    argument `name`).
//!
//! Layers 3 (Vega) and 4 (SQL) are not in CEL.

use serde_json::{json, Map, Value};

use crate::spec::Spec;

pub fn cel_bundle(spec: &Spec) -> Value {
    let mut steps = Map::new();
    for (name, s) in &spec.steps {
        let positional: Vec<Value> = s
            .positional
            .iter()
            .map(|a| json!({"name": a.name, "type": a.ty.name(), "check": a.ty.cel(), "optional": a.optional}))
            .collect();
        let flags: Map<String, Value> = s.flags.iter().map(|(k, t)| (k.clone(), json!({"type": t.name(), "check": t.cel()}))).collect();
        let sets: Map<String, Value> = s.sets.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        steps.insert(name.clone(), json!({"kind": s.kind, "positional": positional, "flags": flags, "requires": s.requires, "message": s.message, "sets": sets}));
    }
    json!({
        "format": "avenger-validate/cel",
        "version": 1,
        "about": "Layers 1 (arguments) and 2 (order and state) of avenger-validate as CEL. Input: a list of steps, [{\"step\": name, \"args\": [..], \"flags\": {..}}], all values strings. For each step: an unknown step is an issue; each positional in order must be present (unless optional) and pass `check` with v bound to it; extra positionals are an issue; each flag must be declared and pass `check`; then `requires` must be true with `state` (before the step) and `step` (its arguments by name) bound, and `sets` is applied ($name is the argument `name`). Layers 3 (Vega expressions) and 4 (SQL against the schema) are not in CEL.",
        "state": spec.state,
        "steps": steps,
    })
}
