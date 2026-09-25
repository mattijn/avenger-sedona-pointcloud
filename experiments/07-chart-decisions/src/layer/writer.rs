//! Phase H: Jev steers, a general model writes the pipeline.
//!
//! Jev reads the instruction first and answers the pilot's questions, plus
//! one of its own: does the instruction carry specifics (a title, a number,
//! a range, a cell size) that none of the options can hold? Its reading goes
//! to the writer as direction and focus. The writer (Claude Haiku 4.5) gets
//! the pipeline behind the chart now, the grammar of the steps, and the
//! fields of each table, and writes the whole new pipeline.
//!
//! What it writes is not trusted: it runs through `editor::apply`, the same
//! path as a hand edit, which validates every stage and folds the result into
//! a state the layer can draw. A refusal goes back to the writer with its
//! reason, up to a fixed number of tries.

use serde_json::{json, Value};

use super::editor::{self, Applied};
use super::model::{base_domains, emphasis, quarter_domains, Data, Dataset, Mark, Quarter, State};
use super::{package, pilot};
use crate::deciders::{Decision, Writer, Written};
use crate::options::COLOURS;

/// The pilot's questions plus `specifics`. A separate set, so the pilot's
/// own questions, and the cache keys of every earlier decision, stay as
/// they are.
pub fn questions() -> Value {
    let mut q = pilot::questions();
    // A way out of the options. Without it, "filter ground" on a pie became
    // `mark/bars` (0.61): the nearest option, applied on the fast path.
    q["action"]["criteria"]["other"] = json!("something none of the options above do: filter the data, aggregate it differently, or any other change to the pipeline");
    // Going back is a change of its own, not "no change".
    q["action"]["criteria"]["undo"] = json!("undo the last change: go back one step to the chart as it was before it");
    q["action"]["criteria"]["reset"] = json!("start over: the whole chart back to how it was at the start (not only the zoom or the emphasis)");
    q["specifics"] = json!({
        "type": "choice",
        "instructions": "Does the instruction give specifics of its own that none of the options above can hold?",
        "criteria": {
            "none": "no: the chosen options say everything the instruction asks",
            "text": "yes: it gives its own text, number, threshold, range or cell size, or asks for more than one change",
        },
    });
    q
}

/// Jev's reading, as one line for the writer and the tables: the change it
/// chose, and whether it saw specifics. Its answers to the other questions
/// are left out; "filter ground" came with `mark: bars` beside `other`, and
/// the writer followed it.
pub fn direction(d: &Decision) -> String {
    let get = |k: &str| d.answers.get(k).and_then(Value::as_str).unwrap_or("keep");
    let mut change = format!("change: {}", pilot::short(&d.answers));
    if let Some(c) = d.confidence {
        change += &format!(" (confidence {c:.2})");
    }
    format!("{change}; specifics: {}", get("specifics"))
}

/// The pipeline behind `s`, as the editor shows it.
pub fn pipeline_text(s: &State, d: &Data) -> String {
    package::full(s, d).join("\n! ")
}

fn fields(ds: Dataset) -> &'static str {
    match ds {
        Dataset::Classes => "label (text), o (class order), n (points)",
        Dataset::Flight => "line (flight line id), t (seconds since the line entered the tile), n (points per 0.5 s)",
        Dataset::ClassHeight => "label (text), band (m above 42 m, 4 m bands), n (points)",
        Dataset::Cells => "cx, cy (cell corner, metres, Lambert-93), h (highest point in the cell, m)",
    }
}

fn chart_now(s: &State, d: &Data) -> String {
    let mut v = vec![
        format!("mark: {}", s.mark.id()),
        format!("table: {} ({})", s.dataset.id(), fields(s.dataset)),
        format!("title: {}", s.title),
        format!("colour: {}", pilot::colour_name(s.color)),
    ];
    if let Some((bx, by)) = base_domains(s.mark, d) {
        let ((x0, x1), (y0, y1)) = s.range.unwrap_or_else(|| quarter_domains(s.zoom, bx, by));
        v.push(format!("full extent: x {}..{}, y {}..{}", bx.0, bx.1, by.0, by.1));
        v.push(format!("shown now: x {x0}..{x1}, y {y0}..{y1}"));
        for (q, name) in [
            (Quarter::NorthWest, "top left (north-west)"),
            (Quarter::NorthEast, "top right (north-east)"),
            (Quarter::SouthWest, "bottom left (south-west)"),
            (Quarter::SouthEast, "bottom right (south-east)"),
        ] {
            let ((a, b), (c, e)) = quarter_domains(Some(q), bx, by);
            v.push(format!("quarter {name}: zoom {a}..{b} {c}..{e}"));
        }
    } else {
        v.push(format!("zoom: not available on a {}", s.mark.id()));
    }
    if let Some((f, top)) = emphasis(s.mark, d) {
        v.push(match (s.highlight, s.threshold) {
            (false, _) => format!("emphasis: none (the top 10 % would be datum.{f} >= {top})"),
            (true, t) => format!("emphasis: datum.{f} >= {}", t.unwrap_or(top)),
        });
    } else {
        v.push(format!("emphasis: not available on a {}", s.mark.id()));
    }
    if !matches!(s.mark, Mark::Bars | Mark::Map) {
        v.push(format!("colour: fixed on a {}, its colours encode the data", s.mark.id()));
    }
    v.join("\n")
}

/// The prompt for one try. `current` is the pipeline behind the chart now,
/// as the editor shows it; `refused` holds the earlier tries of this
/// instruction and why each was refused.
pub fn prompt(s: &State, d: &Data, current: &str, instruction: &str, direction: Option<&str>, refused: &[(String, String)]) -> String {
    let palette = COLOURS.iter().map(|(n, h)| format!("{h} ({n})")).collect::<Vec<_>>().join(", ");
    let examples = Dataset::ALL
        .iter()
        .map(|(ds, _, about)| {
            let m = Mark::default_for(*ds);
            let mut s = State::new(*ds);
            s.mark = m;
            format!("{about}:\n{}", pipeline_text(&s, d))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut out = format!(
        "You write the pipeline behind one chart. A pipeline is stages joined by `!`, one stage per line, \
data stages first, then chart commands.

Data stages:
  read <path> [--statistics]          the LiDAR tile; fields x, y, z (m), classification, gps_time, point_source_id, intensity
  filter --vega \"<Vega expression>\"   keep rows, for example \"datum.classification == 6\"
  sql \"<SELECT ... FROM input ...>\"   the previous stage is the table `input`

Chart commands: exactly one mark first, then any of the others.
  bars <n> --by <label> · pie <n> --by <label> · line <n> --x <t> --series <line> · heatmap <n> --x <band> --y <label> · map --x <cx> --y <cy> --value <h>
  title \"<text>\"
  color <hex>                            bars and map only; write the hex, not the name: {palette}
  highlight \"datum.<field> >= <number>\"  bars, pie and map; this is the only predicate form
  clear-highlight
  zoom <x0>..<x1> <y0>..<y1>             line and map only; always both axes, in data units

Each chart and the pipeline that builds it. Keep these data stages exactly as they are when the data does not \
need to change; they are then read from a cache.

{examples}

The chart now:
{}

The pipeline now:
{}

Instruction: {instruction}
",
        chart_now(s, d),
        current,
    );
    if let Some(dir) = direction {
        out += &format!(
            "\nA classifier read the instruction first. Take its reading as the direction and focus of the change; \
the instruction decides the details it cannot hold (texts, numbers, ranges).\nIts reading: {dir}\n"
        );
    }
    for (text, why) in refused {
        out += &format!("\nAn earlier answer was refused.\nIt was:\n{text}\nThe reason: {why}\n");
    }
    out += "\nReply with the whole new pipeline inside a ```pipeline block, and nothing else. \
If the instruction asks for something this pipeline cannot express, reply with the pipeline now, unchanged.";
    out
}

/// The pipeline in a reply: the first fenced block, or the whole reply.
pub fn extract(reply: &str) -> String {
    let body = reply.split("```").nth(1).map_or(reply, |b| b.strip_prefix("pipeline").unwrap_or(b));
    body.trim().to_string()
}

pub struct Attempt {
    pub text: String,
    pub refused: Option<String>,
    pub written: Written,
}

pub struct Outcome {
    pub attempts: Vec<Attempt>,
    pub applied: Option<Applied>,
}

impl Outcome {
    pub fn ms(&self) -> f64 {
        self.attempts.iter().map(|a| a.written.ms).sum()
    }
    pub fn cost(&self) -> f64 {
        self.attempts.iter().map(|a| a.written.cost).sum()
    }
}

/// Write, apply, and on a refusal try again with its reason.
pub async fn write(w: &Writer, s: &State, d: &Data, current: &str, instruction: &str, direction: Option<&str>, tries: usize) -> Result<Outcome, String> {
    let mut refused: Vec<(String, String)> = vec![];
    let mut attempts = vec![];
    for _ in 0..tries {
        let written = w.write(&prompt(s, d, current, instruction, direction, &refused)).await?;
        let text = extract(&written.text);
        match editor::apply(&text, d).await {
            Ok(a) => {
                attempts.push(Attempt { text, refused: None, written });
                return Ok(Outcome { attempts, applied: Some(a) });
            }
            Err(e) => {
                refused.push((text.clone(), e.clone()));
                attempts.push(Attempt { text, refused: Some(e), written });
            }
        }
    }
    Ok(Outcome { attempts, applied: None })
}
