//! Validate a chart pipeline before it runs, in four layers:
//!
//! 1. **arguments**: each value against its type (a pattern, a range, a choice);
//! 2. **order and state**: each step's precondition on the pipeline state,
//!    written in CEL (`highlight` needs a mark with emphasis);
//! 3. **expressions**: the embedded Vega expressions parse;
//! 4. **data**: each `sql` step plans against the schema that reaches it, and
//!    each channel names a field of it (feature `sql`, DataFusion).
//!
//! Layer 0 parses the text. Every issue carries its layer, step, argument and
//! byte range. Layers 1 and 2 export as CEL ([`export::cel_bundle`]), so a
//! program without this library can run them with any CEL implementation.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

pub mod export;
pub mod spec;
pub mod syntax;
pub mod types;
pub mod vega;

#[cfg(feature = "sql")]
pub mod sql;
#[cfg(feature = "python")]
mod python;
#[cfg(feature = "wasm")]
mod wasm;

pub use spec::Spec;
pub use syntax::{parse, Call, Span};

/// One problem found in a pipeline.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Issue {
    /// 0 syntax, 1 argument, 2 order and state, 3 expression, 4 data.
    pub layer: u8,
    /// The step's index, from 0.
    pub step: Option<usize>,
    /// The step's name.
    pub name: Option<String>,
    /// The argument: a positional's name, or `--flag`.
    pub arg: Option<String>,
    pub message: String,
    /// Byte range in the text, when the pipeline was given as text.
    pub span: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    pub valid: bool,
    pub steps: usize,
    /// The layers that ran; 4 needs the `sql` feature.
    pub layers: Vec<u8>,
    pub issues: Vec<Issue>,
}

/// Time spent per layer, summed over calls, in seconds.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Timings {
    pub parse: f64,
    pub arguments: f64,
    pub state: f64,
    pub expressions: f64,
    pub data: f64,
}

struct Rules {
    base: cel::Context<'static>,
    /// Each distinct `requires`, compiled, and whether it reads `step`.
    programs: HashMap<String, (cel::Program, bool)>,
    /// A rule is a pure function of the state and the step's arguments, and
    /// the state is small, so its outcome is kept: the same `(rule, state,
    /// arguments)` gives the same answer without running CEL again.
    memo: std::sync::Mutex<HashMap<String, Result<bool, String>>>,
}

impl Rules {
    fn eval(&self, rule: &str, state: &BTreeMap<String, String>, args: &BTreeMap<&str, &str>) -> Result<bool, String> {
        let (program, uses_step) = &self.programs[rule];
        let mut key = String::with_capacity(rule.len() + 64);
        key.push_str(rule);
        for (k, v) in state {
            key.push('\u{1}');
            key.push_str(k);
            key.push('=');
            key.push_str(v);
        }
        if *uses_step {
            key.push('\u{2}');
            for (k, v) in args {
                key.push('\u{1}');
                key.push_str(k);
                key.push('=');
                key.push_str(v);
            }
        }
        if let Some(r) = self.memo.lock().unwrap().get(&key) {
            return r.clone();
        }
        let mut cx = self.base.new_inner_scope();
        cx.add_variable_from_value("state", state.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<HashMap<String, String>>());
        cx.add_variable_from_value("step", args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<String, String>>());
        let r = match program.execute(&cx) {
            Ok(cel::Value::Bool(b)) => Ok(b),
            Ok(other) => Err(format!("the rule gave {other:?}, not a Boolean")),
            Err(e) => Err(e.to_string()),
        };
        self.memo.lock().unwrap().insert(key, r.clone());
        r
    }
}

/// A validator for one spec. Build it once; checking is then cheap.
pub struct Validator {
    spec: Spec,
    checker: types::Checker,
    rules: Rules,
    #[cfg(feature = "sql")]
    data: Option<sql::DataLayer>,
}

impl Default for Validator {
    fn default() -> Self {
        Validator::new(Spec::builtin())
    }
}

/// A clock that is a no-op where there is none (wasm32).
#[derive(Clone, Copy)]
struct Clock(#[cfg(not(target_arch = "wasm32"))] std::time::Instant);

impl Clock {
    fn now() -> Clock {
        Clock(
            #[cfg(not(target_arch = "wasm32"))]
            std::time::Instant::now(),
        )
    }
    fn secs(self) -> f64 {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.elapsed().as_secs_f64();
        #[cfg(target_arch = "wasm32")]
        0.0
    }
}

impl Validator {
    pub fn new(spec: Spec) -> Validator {
        let checker = types::Checker::new(spec.steps.values().flat_map(|s| s.positional.iter().map(|a| &a.ty).chain(s.flags.values())));
        let mut programs = HashMap::new();
        for s in spec.steps.values() {
            programs.entry(s.requires.clone()).or_insert_with(|| {
                let p = cel::Program::compile(&s.requires).expect("a step's `requires` compiles");
                let uses_step = p.references().has_variable("step");
                (p, uses_step)
            });
        }
        Validator {
            spec,
            checker,
            rules: Rules { base: cel::Context::default(), programs, memo: Default::default() },
            #[cfg(feature = "sql")]
            data: Some(sql::DataLayer::new(sql::las_schema())),
        }
    }

    /// Layer 4 against this source schema (what `read` gives), instead of the
    /// LAS columns.
    #[cfg(feature = "sql")]
    pub fn with_source_schema(mut self, schema: datafusion::arrow::datatypes::SchemaRef) -> Validator {
        self.data = Some(sql::DataLayer::new(schema));
        self
    }

    /// Without layer 4.
    #[allow(unused_mut)]
    pub fn without_data(mut self) -> Validator {
        #[cfg(feature = "sql")]
        {
            self.data = None;
        }
        self
    }

    pub fn spec(&self) -> &Spec {
        &self.spec
    }

    pub fn check(&self, src: &str) -> Report {
        self.check_timed(src, &mut Timings::default())
    }

    pub fn check_timed(&self, src: &str, t: &mut Timings) -> Report {
        let c = Clock::now();
        let calls = match syntax::parse(src) {
            Ok(c) => c,
            Err(e) => {
                return Report {
                    valid: false,
                    steps: 0,
                    layers: vec![0],
                    issues: vec![Issue { layer: 0, step: None, name: None, arg: None, message: e.message, span: Some((e.offset, e.offset + 1)) }],
                }
            }
        };
        t.parse += c.secs();
        self.check_calls_timed(&calls, t)
    }

    /// Steps given as data: `[{"step": "chart", "args": ["bar"], "flags": {"x": "a:N"}}]`,
    /// the form the CEL export validates.
    pub fn check_json(&self, steps: &serde_json::Value) -> Report {
        match calls_from_json(steps) {
            Ok(calls) => self.check_calls(&calls),
            Err(message) => Report { valid: false, steps: 0, layers: vec![0], issues: vec![Issue { layer: 0, step: None, name: None, arg: None, message, span: None }] },
        }
    }

    pub fn check_calls(&self, calls: &[Call]) -> Report {
        self.check_calls_timed(calls, &mut Timings::default())
    }

    pub fn check_calls_timed(&self, calls: &[Call], t: &mut Timings) -> Report {
        let mut issues = vec![];
        let has_spans = calls.iter().any(|c| c.span != (0, 0));
        let mut state: BTreeMap<String, String> = self.spec.state.clone();
        #[cfg(feature = "sql")]
        let mut data = self.data.as_ref().map(|d| d.start());
        for (i, c) in calls.iter().enumerate() {
            let issue = |layer: u8, arg: Option<String>, span: Option<Span>, message: String| Issue {
                layer,
                step: Some(i),
                name: Some(c.name.clone()),
                arg,
                message,
                span: if has_spans { span.or(Some(c.span)) } else { None },
            };
            let Some(step) = self.spec.steps.get(&c.name) else {
                issues.push(issue(1, None, None, format!("unknown step `{}`", c.name)));
                continue;
            };
            // Layer 1: each value against its type; the arguments by name.
            let c1 = Clock::now();
            let mut args: BTreeMap<&str, &str> = BTreeMap::new();
            for (k, a) in step.positional.iter().enumerate() {
                match c.args.get(k) {
                    Some(v) => {
                        if let Err(e) = self.checker.check(&a.ty, v) {
                            issues.push(issue(1, Some(a.name.clone()), c.arg_spans.get(k).copied(), e));
                        }
                        args.insert(&a.name, v);
                    }
                    None if a.optional => {}
                    None => issues.push(issue(1, Some(a.name.clone()), None, format!("`{}` needs its {} ({})", c.name, a.name, a.ty.name()))),
                }
            }
            for (k, _) in c.args.iter().enumerate().skip(step.positional.len()) {
                issues.push(issue(1, None, c.arg_spans.get(k).copied(), format!("`{}` takes {} positional argument(s), not {}", c.name, step.positional.len(), c.args.len())));
                break;
            }
            for (k, v) in &c.flags {
                match step.flags.get(k) {
                    Some(ty) => {
                        if let Err(e) = self.checker.check(ty, v) {
                            issues.push(issue(1, Some(format!("--{k}")), c.flag_spans.get(k).copied(), e));
                        }
                        args.insert(k, v);
                    }
                    None => issues.push(issue(1, Some(format!("--{k}")), c.flag_spans.get(k).copied(), format!("`{}` has no flag --{k}", c.name))),
                }
            }
            t.arguments += c1.secs();

            // Layer 2: the step's precondition on the state, then its effect.
            let c2 = Clock::now();
            match self.rules.eval(&step.requires, &state, &args) {
                Ok(true) => {}
                r => {
                    let now = state.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
                    let why = match &step.message {
                        Some(m) => m.clone(),
                        None => format!("`{}` needs {}", c.name, step.requires),
                    };
                    let err = if let Err(e) = r { format!(" (the rule failed: {e})") } else { String::new() };
                    issues.push(issue(2, None, None, format!("{why}; the pipeline has {}{err}", if now.is_empty() { "nothing yet".into() } else { now })));
                }
            }
            for (k, v) in &step.sets {
                let v = match v.strip_prefix('$') {
                    Some(a) => args.get(a).copied().unwrap_or("").to_string(),
                    None => v.clone(),
                };
                state.insert(k.clone(), v);
            }
            t.state += c2.secs();

            // Layer 3: embedded Vega expressions.
            let c3 = Clock::now();
            let named = step.positional.iter().enumerate().map(|(k, a)| (a.name.clone(), &a.ty, c.arg_spans.get(k).copied())).chain(step.flags.iter().map(|(k, ty)| (k.clone(), ty, c.flag_spans.get(k).copied())));
            for (name, ty, span) in named {
                if *ty != types::ArgType::Vega {
                    continue;
                }
                let key = name.as_str();
                if let Some(src) = args.get(key) {
                    if let Err(e) = vega::parse(src) {
                        let at = span.map(|s| (s.0, s.1));
                        issues.push(issue(3, Some(arg_label(step, key)), at, format!("Vega expression: {e}")));
                    }
                }
            }
            t.expressions += c3.secs();

            // Layer 4: the data.
            #[cfg(feature = "sql")]
            if let Some(d) = data.as_mut() {
                let c4 = Clock::now();
                for (arg, span, message) in d.step(c, step, &args) {
                    let span = arg.as_deref().and_then(|a| span_of(c, step, a)).or(span);
                    issues.push(issue(4, arg, span, message));
                }
                t.data += c4.secs();
            }
        }
        #[allow(unused_mut)]
        let mut layers = vec![0, 1, 2, 3];
        #[cfg(feature = "sql")]
        if self.data.is_some() {
            layers.push(4);
        }
        Report { valid: issues.is_empty(), steps: calls.len(), layers, issues }
    }
}

fn arg_label(step: &spec::StepSpec, name: &str) -> String {
    if step.positional.iter().any(|a| a.name == name) {
        name.to_string()
    } else {
        format!("--{name}")
    }
}

#[cfg(feature = "sql")]
fn span_of(c: &Call, step: &spec::StepSpec, arg: &str) -> Option<Span> {
    match arg.strip_prefix("--") {
        Some(f) => c.flag_spans.get(f).copied(),
        None => step.positional.iter().position(|a| a.name == arg).and_then(|k| c.arg_spans.get(k).copied()),
    }
}

/// Steps as data, into calls (without spans).
pub fn calls_from_json(steps: &serde_json::Value) -> Result<Vec<Call>, String> {
    let list = steps.as_array().ok_or("a pipeline as data is a list of steps")?;
    let mut out = vec![];
    for (i, s) in list.iter().enumerate() {
        let name = s["step"].as_str().ok_or_else(|| format!("step {i}: no `step` name"))?;
        let text = |v: &serde_json::Value| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out.push(Call {
            name: name.to_string(),
            args: s["args"].as_array().into_iter().flatten().map(text).collect(),
            flags: s["flags"].as_object().into_iter().flatten().map(|(k, v)| (k.clone(), text(v))).collect(),
            ..Default::default()
        });
    }
    Ok(out)
}

/// Calls as data: the form [`Validator::check_json`] and the CEL export take.
pub fn calls_to_json(calls: &[Call]) -> serde_json::Value {
    serde_json::Value::Array(calls.iter().map(|c| serde_json::json!({"step": c.name, "args": c.args, "flags": c.flags})).collect())
}
