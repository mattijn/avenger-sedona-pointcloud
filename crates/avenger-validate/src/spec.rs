//! The step vocabulary as data: per step its positional arguments and flags
//! with a type, a CEL precondition on the pipeline state (`requires`), and
//! what the step sets. The built-in spec is `spec/steps.json`.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::types::ArgType;

/// The built-in spec: the steps of the LiDAR experiments' pipelines.
pub const STEPS_JSON: &str = include_str!("../spec/steps.json");

#[derive(Clone, Debug)]
pub struct Arg {
    pub name: String,
    pub ty: ArgType,
    /// A positional argument that may be left out (`select --soft 0.2`).
    pub optional: bool,
}

#[derive(Clone, Debug)]
pub struct StepSpec {
    pub kind: String,
    pub positional: Vec<Arg>,
    pub flags: BTreeMap<String, ArgType>,
    /// CEL over `state` (before the step) and `step` (its arguments by name).
    pub requires: String,
    /// Said when `requires` fails, if the spec gives it.
    pub message: Option<String>,
    /// State keys to set; a `$name` value is the step's argument `name`.
    pub sets: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct Spec {
    pub steps: BTreeMap<String, StepSpec>,
    pub state: BTreeMap<String, String>,
    pub about: String,
}

impl Spec {
    pub fn builtin() -> Spec {
        Spec::from_json(STEPS_JSON).expect("the built-in spec parses")
    }

    pub fn from_json(text: &str) -> Result<Spec, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("spec: {e}"))?;
        let mut steps = BTreeMap::new();
        for (name, s) in v["steps"].as_object().ok_or("spec: no `steps` object")? {
            let ty = |t: &Value| ArgType::from_json(t).map_err(|e| format!("spec: step `{name}`: {e}"));
            let mut positional = vec![];
            for p in s["positional"].as_array().into_iter().flatten() {
                let arg = p[0].as_str().ok_or_else(|| format!("spec: step `{name}`: a positional needs a name"))?;
                positional.push(Arg { name: arg.to_string(), ty: ty(&p[1])?, optional: p.get(2).and_then(Value::as_str) == Some("optional") });
            }
            let mut flags = BTreeMap::new();
            for (k, t) in s["flags"].as_object().into_iter().flatten() {
                flags.insert(k.clone(), ty(t)?);
            }
            steps.insert(
                name.clone(),
                StepSpec {
                    kind: s["kind"].as_str().unwrap_or("command").to_string(),
                    positional,
                    flags,
                    requires: s["requires"].as_str().unwrap_or("true").to_string(),
                    message: s["message"].as_str().map(String::from),
                    sets: s["sets"].as_object().into_iter().flatten().map(|(k, t)| (k.clone(), t.as_str().unwrap_or("").to_string())).collect(),
                },
            );
        }
        let state = v["state"].as_object().into_iter().flatten().map(|(k, t)| (k.clone(), t.as_str().unwrap_or("").to_string())).collect();
        Ok(Spec { steps, state, about: v["about"].as_str().unwrap_or("").to_string() })
    }
}
