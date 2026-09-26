//! Layer 4: the data. Each `sql` step is planned by DataFusion against a
//! table with no rows that has the schema reaching the step, and each
//! channel must name a field of the schema it sees. Plans are cached by SQL
//! text and input schema: an LLM writer retries, and most retries repeat the
//! data stages.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use datafusion::prelude::SessionContext;

use crate::spec::StepSpec;
use crate::syntax::{Call, Span};
use crate::types::ArgType;

/// The LAS point columns the LiDAR pipelines read, with their LAS types.
pub fn las_schema() -> SchemaRef {
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

pub struct DataLayer {
    source: SchemaRef,
    ctx: SessionContext,
    cache: Mutex<HashMap<(String, String), Result<SchemaRef, String>>>,
}

/// The data as it flows through one pipeline.
pub struct Flow<'a> {
    layer: &'a DataLayer,
    schema: Option<SchemaRef>,
}

fn fingerprint(s: &Schema) -> String {
    s.fields().iter().map(|f| format!("{}:{}", f.name(), f.data_type())).collect::<Vec<_>>().join(",")
}

impl DataLayer {
    pub fn new(source: SchemaRef) -> DataLayer {
        DataLayer { source, ctx: SessionContext::new(), cache: Mutex::new(HashMap::new()) }
    }

    pub fn start(&self) -> Flow<'_> {
        Flow { layer: self, schema: None }
    }

    /// The output schema of `sql` over a table `input` with schema `input`.
    fn plan(&self, sql: &str, input: &SchemaRef) -> Result<SchemaRef, String> {
        let key = (sql.to_string(), fingerprint(input));
        if let Some(r) = self.cache.lock().unwrap().get(&key) {
            return r.clone();
        }
        let _ = self.ctx.deregister_table("input");
        let table = MemTable::try_new(input.clone(), vec![vec![]]).map_err(|e| e.to_string())?;
        self.ctx.register_table("input", Arc::new(table)).map_err(|e| e.to_string())?;
        // Planning a query over an in-memory table does no I/O, so this
        // needs no async runtime of its own.
        let r = futures::executor::block_on(self.ctx.sql(sql))
            .map(|df| Arc::new(df.schema().as_arrow().clone()) as SchemaRef)
            .map_err(|e| e.to_string().lines().next().unwrap_or("").trim_start_matches("Error during planning: ").to_string());
        self.cache.lock().unwrap().insert(key, r.clone());
        r
    }
}

impl Flow<'_> {
    /// Issues for one step, as `(argument, span, message)`; updates the
    /// schema the next step sees.
    pub fn step(&mut self, c: &Call, step: &StepSpec, args: &BTreeMap<&str, &str>) -> Vec<(Option<String>, Option<Span>, String)> {
        let mut out = vec![];
        match c.name.as_str() {
            "read" => self.schema = Some(self.layer.source.clone()),
            "sql" => {
                if let (Some(q), Some(input)) = (args.get("query"), &self.schema) {
                    match self.layer.plan(q, input) {
                        Ok(s) => self.schema = Some(s),
                        Err(e) => {
                            out.push((Some("query".into()), None, format!("SQL: {e}")));
                            // Later steps cannot be checked against a schema that is unknown.
                            self.schema = None;
                        }
                    }
                }
            }
            "filter" => {
                if let (Some(w), Some(input)) = (args.get("sql"), &self.schema) {
                    if let Err(e) = self.layer.plan(&format!("SELECT * FROM input WHERE {w}"), input) {
                        out.push((Some("--sql".into()), None, format!("SQL: {e}")));
                    }
                }
            }
            "calc" => {
                if let (Some(name), Some(input)) = (args.get("name"), self.schema.clone()) {
                    let added = match args.get("sql") {
                        Some(e) => match self.layer.plan(&format!("SELECT *, {e} AS \"{}\" FROM input", name.replace('"', "\"\"")), &input) {
                            Ok(s) => Some(s),
                            Err(e) => {
                                out.push((Some("--sql".into()), None, format!("SQL: {e}")));
                                None
                            }
                        },
                        // A Vega calculation's type is not known here.
                        None => {
                            let mut f: Vec<Field> = input.fields().iter().map(|f| f.as_ref().clone()).collect();
                            f.push(Field::new(*name, DataType::Null, true));
                            Some(Arc::new(Schema::new(f)))
                        }
                    };
                    self.schema = added;
                }
            }
            _ => {}
        }
        if let Some(s) = &self.schema {
            let named = step.positional.iter().map(|a| (a.name.clone(), a.name.clone(), &a.ty)).chain(step.flags.iter().map(|(k, t)| (k.clone(), format!("--{k}"), t)));
            for (key, label, ty) in named {
                let ArgType::Pattern { name, .. } = ty else { continue };
                if name != "channel" {
                    continue;
                }
                if let Some(v) = args.get(key.as_str()).filter(|v| !v.starts_with('#')) {
                    let f = v.split(':').next().unwrap_or("");
                    if s.field_with_name(f).is_err() {
                        let have = s.fields().iter().map(|f| f.name().as_str()).collect::<Vec<_>>().join(", ");
                        out.push((Some(label), None, format!("no field `{f}` in the data here ({have})")));
                    }
                }
            }
        }
        out
    }
}
