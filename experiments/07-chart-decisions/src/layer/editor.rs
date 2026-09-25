//! Editor mode: the chart written as a pipeline by hand.
//!
//! The text is run through a fresh pipeline with the `layer` package, the
//! same as the autopilot's lines. The drawn table is built from the
//! pipeline's own result, so an edited `sql` stage shows in the chart. When
//! the data stages are exactly those of one of the four known tables, the
//! cached Parquet file is read instead of the tile (the same result, in
//! milliseconds instead of seconds).

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

/// Run a pipeline text. Nothing changes unless all of it runs and folds.
pub async fn apply(text: &str, base: &Data) -> Result<Applied, String> {
    let t = std::time::Instant::now();
    let calls = parse_pipeline(text).map_err(|e| e.to_string())?;
    let (mut p, _) = layer_pipeline().await.map_err(|e| e.to_string())?;
    let kind = |c: &Call| p.steps.get(&c.name).map(|(_, s)| s.kind());
    let split = calls.iter().position(|c| kind(c) == Some(Kind::Command)).ok_or("no chart command (bars, pie, line, heatmap, map)")?;
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
