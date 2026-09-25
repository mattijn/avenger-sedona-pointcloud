//! The `layer` package: the chart layer's marks as experiment 6 pipeline
//! commands, and the fold from the pipeline's chart state back to a layer
//! `State`.
//!
//! So a decision is not applied to the chart directly. It becomes pipeline
//! lines (`read out/layer/flight.parquet ! line n --x t --series line`,
//! `zoom 0..13 0..100000`, `highlight "datum.n >= 3321708"`). The pipeline
//! validates each against the data, exactly as experiment 6's task commands,
//! and logs it. The chart that is drawn is the fold of that log.
//!
//! Experiment 6's `color`, `zoom`, `reset-zoom`, `highlight` and `title` are
//! used as they are. The marks are new; each one starts a fresh chart (its
//! colour, zoom and emphasis are set by the commands after it). `bars` takes
//! over experiment 6's `bars`, with the same syntax, so that it starts a
//! fresh chart too.

use std::sync::Arc;

use async_trait::async_trait;
use datafusion::error::Result;
use lidar_pipeline::pipeline::{err, Call, Kind, Package, Pipeline, Step};
use serde_json::{json, Value};

use super::model::{base_domains, emphasis, quarter_domains, Data, Dataset, Mark, Quarter, State};
use crate::options::COLOURS;

/// `(step, chart mark, positional measure, flags)`.
const MARKS: [(&str, &str, bool, &[&str]); 5] = [
    ("bars", "bar", true, &["by"]),
    ("pie", "arc", true, &["by"]),
    ("line", "line", true, &["x", "series"]),
    ("heatmap", "heatmap", true, &["x", "y"]),
    ("map", "map", false, &["x", "y", "value"]),
];

fn fields(p: &Pipeline) -> Result<Vec<String>> {
    Ok(p.plan()?.schema().fields().iter().map(|f| f.name().to_string()).collect())
}

struct MarkStep;

#[async_trait]
impl Step for MarkStep {
    fn kind(&self) -> Kind {
        Kind::Command
    }
    fn help(&self) -> &'static str {
        "bars <n> --by <f> · pie <n> --by <f> · line <n> --x <f> --series <f> · heatmap <n> --x <f> --y <f> · map --x <f> --y <f> --value <f>"
    }
    async fn run(&self, p: &mut Pipeline, c: &Call) -> Result<Option<String>> {
        let (_, mark, measure, flags) = MARKS.iter().find(|m| m.0 == c.name).unwrap();
        let have = fields(p)?;
        let known = |f: &str| {
            if have.iter().any(|x| x == f) {
                Ok(f.to_string())
            } else {
                Err(err(format!("{}: no field `{f}` in the data (fields: {})", c.name, have.join(", "))))
            }
        };
        // A new chart: nothing of the previous one carries over.
        let mut state = json!({"mark": mark});
        if *measure {
            state["value"] = json!(known(c.arg(0)?)?);
        }
        for f in *flags {
            let v = known(c.flag(f).ok_or_else(|| err(format!("{} needs --{f}", c.name)))?)?;
            state[*f] = json!(v);
        }
        // The axes experiment 6's `zoom` checks against.
        let (x, y) = match *mark {
            "bar" | "arc" => (state["by"].clone(), state["value"].clone()),
            "line" => (state["x"].clone(), state["value"].clone()),
            "heatmap" => (state["x"].clone(), state["y"].clone()),
            _ => (state["x"].clone(), state["y"].clone()),
        };
        state["x"] = json!({"field": x});
        state["y"] = json!({"field": y});
        p.chart = state;
        Ok(None)
    }
}

struct ClearHighlight;

#[async_trait]
impl Step for ClearHighlight {
    fn kind(&self) -> Kind {
        Kind::Command
    }
    fn help(&self) -> &'static str {
        "clear-highlight: remove the emphasis"
    }
    async fn run(&self, p: &mut Pipeline, _c: &Call) -> Result<Option<String>> {
        if let Some(o) = p.chart.as_object_mut() {
            o.remove("highlight");
        }
        Ok(None)
    }
}

pub fn package() -> Package {
    let mut steps: Vec<(&'static str, Arc<dyn Step>)> =
        MARKS.iter().map(|m| (m.0, Arc::new(MarkStep) as Arc<dyn Step>)).collect();
    steps.push(("clear-highlight", Arc::new(ClearHighlight)));
    Package { name: "layer", functions: vec![], steps }
}

// ---------------------------------------------------------------------------
// Decisions → pipeline lines.

fn hex_of(c: [f32; 4]) -> Option<&'static str> {
    COLOURS.iter().find(|(n, _)| super::pilot::colour_name(Some(c)) == *n).map(|(_, h)| *h)
}

fn mark_line(m: Mark) -> &'static str {
    match m {
        Mark::Bars => "bars n --by label",
        Mark::Pie => "pie n --by label",
        Mark::Line => "line n --x t --series line",
        Mark::Heatmap => "heatmap n --x band --y label",
        Mark::Map => "map --x cx --y cy --value h",
    }
}

fn zoom_line(m: Mark, q: Quarter, d: &Data) -> String {
    let (bx, by) = base_domains(m, d).unwrap();
    let ((x0, x1), (y0, y1)) = quarter_domains(Some(q), bx, by);
    format!("zoom {x0}..{x1} {y0}..{y1}")
}

fn highlight_line(n: &State, d: &Data) -> String {
    let (f, t) = emphasis(n.mark, d).unwrap();
    format!("highlight \"datum.{f} >= {}\"", n.threshold.unwrap_or(t))
}

fn zoom_of(n: &State, d: &Data) -> Option<String> {
    match (n.range, n.zoom) {
        (Some(((x0, x1), (y0, y1))), _) => Some(format!("zoom {x0}..{x1} {y0}..{y1}")),
        (None, Some(q)) => Some(zoom_line(n.mark, q, d)),
        (None, None) => None,
    }
}

/// A fresh mark (after an optional `read`), then what it carries.
fn fresh(read: bool, n: &State, d: &Data) -> Vec<String> {
    let read = if read { format!("read {} ! ", super::data::source(n.dataset)) } else { String::new() };
    let mut out = vec![format!("{read}{}", mark_line(n.mark))];
    out.extend(n.color.and_then(hex_of).map(|h| format!("color {h}")));
    if n.highlight {
        out.push(highlight_line(n, d));
    }
    out.extend(zoom_of(n, d));
    out
}

/// The pipeline lines that take the chart from `s` to `n`.
pub fn lines(s: &State, n: &State, d: &Data) -> Vec<String> {
    // A mark starts a fresh chart; that is also the only way back to the
    // default colour, since `color` sets one and nothing unsets it.
    if n.mark != s.mark || n.dataset != s.dataset || (n.color.is_none() && s.color.is_some()) {
        return fresh(n.dataset != s.dataset, n, d);
    }
    let mut out = vec![];
    if n.color != s.color {
        out.extend(n.color.and_then(hex_of).map(|h| format!("color {h}")));
    }
    if (n.highlight, n.threshold) != (s.highlight, s.threshold) {
        out.push(if n.highlight { highlight_line(n, d) } else { "clear-highlight".into() });
    }
    if (n.zoom, n.range) != (s.zoom, s.range) {
        out.push(zoom_of(n, d).unwrap_or_else(|| "reset-zoom".into()));
    }
    out
}

/// The stages that build a dataset from the tile.
pub fn data_stages(ds: Dataset) -> Vec<String> {
    let mut v = vec![format!("read {} --statistics", crate::TILE)];
    v.extend(super::data::origin(ds).split(" ! ").map(String::from));
    v
}

/// The commands that draw `s` on its data: the mark, then what it carries.
pub fn commands(s: &State, d: &Data) -> Vec<String> {
    fresh(false, s, d)
}

/// The whole pipeline behind `s`, from the tile: its stages in order, to be
/// joined with ` ! `. No render sink; the chart state is its result.
pub fn full(s: &State, d: &Data) -> Vec<String> {
    let mut v = data_stages(s.dataset);
    v.extend(commands(s, d));
    v
}

/// The pipeline that shows `s` from scratch.
pub fn initial(s: &State, d: &Data) -> Vec<String> {
    fresh(true, s, d)
}

// ---------------------------------------------------------------------------
// The pipeline's chart state → the layer's state.

fn domain(v: &Value) -> Option<(f64, f64)> {
    let s = v["domain"].as_str()?;
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// Fold the pipeline's chart state into what the layer draws. Anything the
/// layer cannot draw faithfully is an error, not an approximation.
pub fn state(chart: &Value, d: &Data) -> std::result::Result<State, String> {
    let mark = match chart["mark"].as_str() {
        Some("bar") => Mark::Bars,
        Some("arc") => Mark::Pie,
        Some("line") => Mark::Line,
        Some("heatmap") => Mark::Heatmap,
        Some("map") => Mark::Map,
        other => return Err(format!("no layer mark in the chart ({other:?})")),
    };
    let dataset = Dataset::ALL.iter().map(|x| x.0).find(|x| mark.fits(*x)).unwrap();
    let mut s = State::new(dataset);
    s.mark = mark;
    s.title = chart["title"].as_str().map_or_else(|| s.default_title(), String::from);
    if let Some(h) = chart["fill"].as_str() {
        match COLOURS.iter().find(|c| c.1.eq_ignore_ascii_case(h)) {
            Some((name, _)) => {
                s.color = super::pilot::colour_of(name);
            }
            None => return Err(format!("colour {h} is not one of the palette")),
        }
    }
    if let Some(w) = chart["highlight"]["where"].as_str() {
        let (f, top) = emphasis(mark, d).ok_or("this mark has no emphasis")?;
        let t: f64 = w
            .strip_prefix(&format!("datum.{f} >= "))
            .and_then(|t| t.trim().parse().ok())
            .ok_or_else(|| format!("the layer draws `datum.{f} >= <number>` only, not `{w}`"))?;
        s.highlight = true;
        s.threshold = (t != top).then_some(t);
    }
    match (domain(&chart["x"]), domain(&chart["y"])) {
        (None, None) => {}
        (Some(x), Some(y)) => {
            let base = base_domains(mark, d).ok_or("this mark has no zoom")?;
            match [Quarter::NorthEast, Quarter::NorthWest, Quarter::SouthEast, Quarter::SouthWest]
                .into_iter()
                .find(|q| quarter_domains(Some(*q), base.0, base.1) == (x, y))
            {
                Some(q) => s.zoom = Some(q),
                None => s.range = Some((x, y)),
            }
        }
        _ => return Err("the layer zooms both axes at once".into()),
    }
    Ok(s)
}
