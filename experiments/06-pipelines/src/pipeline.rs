//! A GDAL-style pipeline: `read x ! filter --vega "..." ! sql "..." ! count`.
//!
//! Every step has a kind. Transforms only extend one DataFusion
//! `LogicalPlan`; nothing executes until a sink (or `materialize`), so the
//! optimiser sees the whole chain, like a GDAL VRT that is only materialised
//! at the end. Commands change the chart and are logged; queries read the
//! data or the chart and change nothing (CQRS).
//!
//! Steps live in a registry, so a package can add steps just as it can add
//! functions to the session.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use datafusion::common::DFSchema;
use datafusion::datasource::{MemTable, ViewTable};
use datafusion::error::{DataFusionError, Result};
use datafusion::logical_expr::LogicalPlan;
use datafusion::physical_plan::displayable;
use datafusion::prelude::{DataFrame, SessionContext};

use crate::vega::{parse, Compiler};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Starts the data: `read`.
    Source,
    /// Extends the lazy plan: `filter`, `calc`, `sql`, `bin`.
    Transform,
    /// Changes the chart, and is appended to the command log.
    Command,
    /// Reads the data or the chart; changes nothing.
    Query,
    /// Executes: `count`, `head`, `write`, `explain`.
    Sink,
}

/// One step as written: `name positional... --flag value`.
#[derive(Clone, Debug, Default)]
pub struct Call {
    pub name: String,
    pub args: Vec<String>,
    pub flags: BTreeMap<String, String>,
}

impl Call {
    pub fn flag(&self, k: &str) -> Option<&str> {
        self.flags.get(k).map(String::as_str)
    }
    pub fn arg(&self, i: usize) -> Result<&str> {
        self.args
            .get(i)
            .map(String::as_str)
            .ok_or_else(|| err(format!("{}: missing argument {}", self.name, i + 1)))
    }
    pub fn render(&self) -> String {
        let mut s = self.name.clone();
        for a in &self.args {
            s += &format!(" {}", quote(a));
        }
        for (k, v) in &self.flags {
            s += &format!(" --{k} {}", quote(v));
        }
        s
    }
}

fn quote(s: &str) -> String {
    if s.contains([' ', '"', '!']) || s.is_empty() {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

pub fn err(m: impl Into<String>) -> DataFusionError {
    DataFusionError::Plan(m.into())
}

/// Split a pipeline string into steps at ` ! `, honouring quotes.
pub fn parse_pipeline(src: &str) -> Result<Vec<Call>> {
    let mut words: Vec<(String, bool)> = vec![]; // (word, was quoted)
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
        return Err(err("unterminated quote in pipeline"));
    }
    if !cur.is_empty() || quoted {
        words.push((cur, quoted));
    }
    let mut calls = vec![];
    for group in words.split(|(w, q)| w == "!" && !q) {
        let mut it = group.iter().peekable();
        let Some((name, _)) = it.next() else { continue };
        let mut call = Call {
            name: name.clone(),
            ..Default::default()
        };
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

/// A bundle of functions and steps, registered together.
pub struct Package {
    pub name: &'static str,
    pub functions: Vec<datafusion::logical_expr::ScalarUDF>,
    pub steps: Vec<(&'static str, Arc<dyn Step>)>,
}

#[async_trait]
pub trait Step: Send + Sync {
    fn kind(&self) -> Kind;
    fn help(&self) -> &'static str;
    /// Run the step. Queries and sinks return what they print.
    async fn run(&self, p: &mut Pipeline, call: &Call) -> Result<Option<String>>;
}

pub struct Pipeline {
    pub ctx: SessionContext,
    pub plan: Option<LogicalPlan>,
    /// Compile Vega expressions with JavaScript semantics.
    pub vega_semantics: bool,
    pub steps: BTreeMap<String, (String, Arc<dyn Step>)>,
    /// Which package registered each function name.
    pub functions: BTreeMap<String, String>,
    /// Commands applied so far, in order: the chart's history.
    pub log: Vec<String>,
    pub chart: serde_json::Value,
    /// Per step: (rendered call, kind, milliseconds).
    pub trace: Vec<(String, Kind, f64)>,
}

impl Pipeline {
    pub fn new(ctx: SessionContext) -> Self {
        let mut p = Pipeline {
            ctx,
            plan: None,
            vega_semantics: false,
            steps: BTreeMap::new(),
            functions: BTreeMap::new(),
            log: vec![],
            chart: serde_json::json!({}),
            trace: vec![],
        };
        for name in p.ctx.state().scalar_functions().keys() {
            p.functions.insert(name.clone(), "datafusion".into());
        }
        crate::steps::register_core(&mut p);
        p
    }

    /// Install a package: its functions into the session, its steps into
    /// the step registry. Returns the names that were already taken, with
    /// their previous owner. The newer package wins, as DataFusion's
    /// `register_udf` does; the report makes that visible.
    pub fn install(&mut self, package: Package) -> Vec<(String, String)> {
        let mut taken = vec![];
        for f in package.functions {
            let names: Vec<String> = std::iter::once(f.name().to_string())
                .chain(f.aliases().iter().cloned())
                .collect();
            for n in &names {
                if let Some(prev) = self.functions.insert(n.clone(), package.name.to_string()) {
                    if prev != package.name {
                        taken.push((n.clone(), prev));
                    }
                }
            }
            self.ctx.register_udf(f);
        }
        for (name, step) in package.steps {
            if let Some((prev, _)) = self.steps.get(name) {
                taken.push((format!("step {name}"), prev.clone()));
            }
            self.register_step(package.name, name, step);
        }
        taken
    }

    /// Register a step under a package name.
    pub fn register_step(&mut self, package: &str, name: &str, step: Arc<dyn Step>) {
        self.steps
            .insert(name.to_string(), (package.to_string(), step));
    }

    pub fn plan(&self) -> Result<&LogicalPlan> {
        self.plan
            .as_ref()
            .ok_or_else(|| err("no data yet: start the pipeline with `read`"))
    }

    pub fn dataframe(&self) -> Result<DataFrame> {
        Ok(DataFrame::new(self.ctx.state(), self.plan()?.clone()))
    }

    /// Compile a Vega expression against the current data.
    pub fn vega(&self, src: &str) -> Result<datafusion::logical_expr::Expr> {
        let schema: &DFSchema = self.plan()?.schema();
        let state = self.ctx.state();
        let ast = parse(src).map_err(|e| err(format!("vega: {e}")))?;
        Compiler {
            schema,
            registry: &state,
            vega: self.vega_semantics,
        }
        .compile(&ast)
        .map_err(|e| err(format!("vega: {e}")))
    }

    /// Expose the plan so far to SQL as the view `input`. DataFusion's
    /// analyzer inlines the view, so the chain stays one plan.
    pub fn expose_input(&self) -> Result<()> {
        let _ = self.ctx.deregister_table("input");
        let view = ViewTable::new(self.plan()?.clone(), None);
        self.ctx.register_table("input", Arc::new(view))?;
        Ok(())
    }

    /// Execute the plan now and continue from an in-memory table.
    pub async fn materialize(&mut self) -> Result<usize> {
        let df = self.dataframe()?;
        let schema = Arc::new(df.schema().as_arrow().clone());
        let batches = df.collect().await?;
        let rows = batches.iter().map(|b| b.num_rows()).sum();
        let name = format!("materialized_{}", self.trace.len());
        self.ctx
            .register_table(&name, Arc::new(MemTable::try_new(schema, vec![batches])?))?;
        self.plan = Some(self.ctx.table(&name).await?.into_unoptimized_plan());
        Ok(rows)
    }

    pub async fn run_call(&mut self, call: &Call) -> Result<Option<String>> {
        let (_, step) = self
            .steps
            .get(&call.name)
            .cloned()
            .ok_or_else(|| err(format!("unknown step `{}`", call.name)))?;
        let t = Instant::now();
        let out = step.run(self, call).await?;
        // `undo` edits the log itself and is not logged.
        if step.kind() == Kind::Command && call.name != "undo" {
            self.log.push(call.render());
        }
        self.trace
            .push((call.render(), step.kind(), t.elapsed().as_secs_f64() * 1e3));
        Ok(out)
    }

    pub async fn run(&mut self, src: &str) -> Result<Vec<String>> {
        let mut out = vec![];
        for call in parse_pipeline(src)? {
            if let Some(s) = self.run_call(&call).await? {
                out.push(s);
            }
        }
        Ok(out)
    }

    /// The pipeline as data, like GDAL's `.gdalg.json`: the steps that build
    /// the data (sources and transforms, in order) and the chart's command
    /// log. Queries are not part of it: they change nothing.
    pub fn to_json(&self) -> serde_json::Value {
        let data: Vec<&String> = self
            .trace
            .iter()
            .filter(|(_, k, _)| matches!(k, Kind::Source | Kind::Transform))
            .map(|(c, _, _)| c)
            .collect();
        serde_json::json!({
            "semantics": if self.vega_semantics { "vega" } else { "sql" },
            "data": data,
            "commands": self.log,
        })
    }

    /// Rebuild a pipeline from `to_json` output: rerun the data steps, then
    /// replay the commands.
    pub async fn replay(&mut self, saved: &serde_json::Value) -> Result<()> {
        self.vega_semantics = saved["semantics"] == "vega";
        for key in ["data", "commands"] {
            for line in saved[key].as_array().into_iter().flatten() {
                self.run(line.as_str().unwrap_or_default()).await?;
            }
        }
        Ok(())
    }

    /// The optimised logical plan and the physical plan, as text.
    pub async fn explain(&self) -> Result<String> {
        let state = self.ctx.state();
        let optimized = state.optimize(self.plan()?)?;
        let physical = state.create_physical_plan(self.plan()?).await?;
        Ok(format!(
            "optimised logical plan:\n{}\n\nphysical plan:\n{}",
            optimized.display_indent(),
            displayable(physical.as_ref()).indent(true)
        ))
    }
}
