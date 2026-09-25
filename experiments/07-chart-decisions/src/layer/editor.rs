//! Editor mode: the chart written as a pipeline by hand.
//!
//! The text is run through a fresh pipeline with the `layer` package, the
//! same as the autopilot's lines. The drawn table is built from the
//! pipeline's own result, so an edited `sql` stage shows in the chart. When
//! the data stages are exactly those of one of the four known tables, the
//! cached Parquet file is read instead of the tile (the same result, in
//! milliseconds instead of seconds).

use arrow::util::display::{ArrayFormatter, FormatOptions};
use lidar_pipeline::pipeline::{parse_pipeline, Call, Kind, Pipeline};

use super::model::{Data, Dataset, State};
use super::{data, package};
use crate::layer_pipeline;

pub struct Applied {
    pub pipeline: Pipeline,
    pub data: Data,
    pub state: State,
    /// The data stages as written, one per entry.
    pub data_stages: Vec<String>,
    /// How the data was read.
    pub note: String,
    pub ms: f64,
}

fn quote(s: &str) -> String {
    if s.contains([' ', '"', '!']) || s.is_empty() {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Calls as text, as `Call::render` writes them, except that a flag without
/// a value (`--statistics`) stays bare.
fn render(calls: &[Call]) -> Vec<String> {
    calls
        .iter()
        .map(|c| {
            let mut s = c.name.clone();
            for a in &c.args {
                s += &format!(" {}", quote(a));
            }
            for (k, v) in &c.flags {
                s += &if v == "true" { format!(" --{k}") } else { format!(" --{k} {}", quote(v)) };
            }
            s
        })
        .collect()
}

/// What `apply` says of a text without a chart command: `query` runs it.
pub const NO_CHART: &str = "no chart command (bars, pie, line, heatmap, map)";

/// The rows a text without a chart command ends in, or what its queries
/// printed.
#[derive(Clone)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Lines printed by queries and sinks (`schema`, `count`, `explain`).
    pub text: Vec<String>,
    pub note: String,
    pub ms: f64,
}

/// Rows shown when the text ends in data without `head`.
const ROWS: usize = 20;

/// Batches as column names and text cells.
fn grid(batches: &[arrow::record_batch::RecordBatch]) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let columns = batches.first().map_or(vec![], |b| b.schema().fields().iter().map(|f| f.name().to_string()).collect());
    let opts = FormatOptions::default().with_null("null");
    let mut rows = vec![];
    for b in batches {
        let fmts: Vec<ArrayFormatter> = b.columns().iter().map(|c| ArrayFormatter::try_new(c.as_ref(), &opts)).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
        for i in 0..b.num_rows() {
            rows.push(fmts.iter().map(|f| f.value(i).to_string()).collect());
        }
    }
    Ok((columns, rows))
}

/// Run a text without a chart command, and keep what it shows. A trailing
/// `head N` becomes a table rather than text, and so does a text that ends
/// in data. The chart and its pipeline are not touched.
pub async fn query(text: &str) -> Result<Table, String> {
    let t = std::time::Instant::now();
    let first = text.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
    // `overview`, or plain SQL over the named tables and information_schema.
    if first == "OVERVIEW" {
        let (p, _) = layer_pipeline().await.map_err(|e| e.to_string())?;
        return super::catalog::overview(&p.ctx, None).await;
    }
    if matches!(first.as_str(), "SELECT" | "WITH" | "SHOW" | "DESCRIBE" | "EXPLAIN") {
        let (p, _) = layer_pipeline().await.map_err(|e| e.to_string())?;
        let batches = p.ctx.sql(text).await.map_err(|e| e.to_string())?.limit(0, Some(200)).map_err(|e| e.to_string())?.collect().await.map_err(|e| e.to_string())?;
        let (columns, rows) = grid(&batches)?;
        return Ok(Table { note: format!("SQL: {} rows", rows.len()), columns, rows, text: vec![], ms: t.elapsed().as_secs_f64() * 1e3 });
    }
    let mut calls = parse_pipeline(text).map_err(|e| e.to_string())?;
    let (mut p, _) = layer_pipeline().await.map_err(|e| e.to_string())?;
    let head = match calls.last() {
        Some(c) if c.name == "head" => {
            let n = c.args.first().and_then(|v| v.parse().ok()).unwrap_or(5);
            calls.pop();
            Some(n)
        }
        _ => None,
    };
    let mut out = vec![];
    for c in &calls {
        if let Some(s) = p.run_call(c).await.map_err(|e| format!("{}: {e}", c.name))? {
            out.extend(s.lines().map(String::from));
        }
    }
    let kind = |c: &Call| p.steps.get(&c.name).map(|(_, s)| s.kind());
    let ends_in_data = head.is_some() || calls.last().is_some_and(|c| matches!(kind(c), Some(Kind::Source | Kind::Transform)));
    let (mut columns, mut rows, mut note): (Vec<String>, Vec<Vec<String>>, String) = (vec![], vec![], String::new());
    if ends_in_data {
        let n = head.unwrap_or(ROWS);
        let batches = p.dataframe().map_err(|e| e.to_string())?.limit(0, Some(n)).map_err(|e| e.to_string())?.collect().await.map_err(|e| e.to_string())?;
        (columns, rows) = grid(&batches)?;
        note = match head {
            Some(n) => format!("head {n}: {} rows", rows.len()),
            None if rows.len() < ROWS => format!("all {} rows", rows.len()),
            None => format!("the first {} rows; end with `head N` for another number", rows.len()),
        };
    }
    Ok(Table { columns, rows, text: out, note, ms: t.elapsed().as_secs_f64() * 1e3 })
}

/// Run a pipeline text. Nothing changes unless all of it runs and folds.
pub async fn apply(text: &str, base: &Data) -> Result<Applied, String> {
    let t = std::time::Instant::now();
    let calls = parse_pipeline(text).map_err(|e| e.to_string())?;
    let (mut p, _) = layer_pipeline().await.map_err(|e| e.to_string())?;
    let kind = |c: &Call| p.steps.get(&c.name).map(|(_, s)| s.kind());
    let split = calls.iter().position(|c| kind(c) == Some(Kind::Command)).ok_or(NO_CHART)?;
    if let Some(c) = calls[split..].iter().find(|c| kind(c) != Some(Kind::Command)) {
        return Err(format!("`{}` comes after the chart commands; data stages go first", c.name));
    }
    let written = render(&calls[..split]);
    let known = Dataset::ALL.iter().map(|d| d.0).find(|ds| {
        let canonical = parse_pipeline(&package::data_stages(*ds).join(" ! ")).map(|c| render(&c)).unwrap_or_default();
        canonical == written
    });
    let (data_text, note) = match known {
        Some(ds) => (format!("read {}", data::source(ds)), format!("data stages are the {} table: read from its cache, {}", ds.id(), data::source(ds))),
        None => (written.join(" ! "), "data stages run from the tile".to_string()),
    };
    p.run(&data_text).await.map_err(|e| e.to_string())?;
    for c in &calls[split..] {
        p.run_call(c).await.map_err(|e| format!("{}: {e}", c.name))?;
    }
    let batches = p.dataframe().map_err(|e| e.to_string())?.collect().await.map_err(|e| e.to_string())?;
    let d = data::from_pipeline(&p.chart, &batches, base)?;
    let state = package::state(&p.chart, &d)?;
    Ok(Applied { pipeline: p, data: d, state, data_stages: written, note, ms: t.elapsed().as_secs_f64() * 1e3 })
}
