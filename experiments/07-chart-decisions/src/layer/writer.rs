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
    // A way out of the options for the data itself. Without it, "filter
    // ground" on a pie became `mark/bars` (0.61): the nearest option.
    q["action"]["criteria"]["transform"] = json!("change the data behind the chart: filter rows, exclude or keep classes, aggregate differently, or compute a field");
    // Going back is a change of its own, not "no change".
    q["action"]["criteria"]["undo"] = json!("undo the last change: go back one step to the chart as it was before it");
    q["action"]["criteria"]["reset"] = json!("start over: the whole chart back to how it was at the start (not only the zoom or the emphasis)");
    // How the plot is seen, from experiment 5's coordinate systems.
    q["action"]["criteria"]["view"] = json!("change how the chart is seen: a fisheye or a magnifier over one area, a 3D tilt, or back to flat");
    // "Magnify" asks for a lens, not for the whole chart to zoom.
    q["action"]["criteria"]["zoom"] = json!("zoom the whole chart to a part of it, or back out to all of it: the axes change (not a lens or a magnifying glass)");
    q["view"] = json!({
        "type": "choice",
        "instructions": "If the view should change, to which?",
        "criteria": {
            "flat": "a plain flat view, without a lens or tilt",
            "fisheye": "a fisheye lens: one area enlarged, the rest kept around it",
            "magnifier": "magnify: a magnifying glass, an enlarged round inset over one area while the rest stays as it is",
            "tilt": "a tilted 3D view, with height as depth",
            "keep": "no view asked for",
        },
    });
    q["render"] = json!({
        "type": "choice",
        "instructions": "How should the result be shown?",
        "criteria": {
            "chart": "as a chart: the instruction asks for a chart, or a change to it",
            "table": "as a table of the rows, instead of a chart",
            "export": "written to a file (Parquet), to use elsewhere",
            "overview": "an overview of the data there is (tables, columns, types, ranges): the instruction asks what data there is, what it contains, or which columns or fields it has",
        },
    });
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

/// Jev's side answers are steadier than its action: "magnify the
/// north-east corner" came back as `zoom` (0.78) with `view magnifier`
/// (0.73). A view answered with confidence, beside an action that is only a
/// zoom or no change, becomes the action.
pub fn normalise(d: &mut Decision, current: &super::model::View) {
    let get = |k: &str| d.answers.get(k).and_then(Value::as_str).unwrap_or("keep").to_string();
    let (action, view) = (get("action"), get("view"));
    // Close to the action's confidence: "magnify" was zoom 0.78 against
    // magnifier 0.73; "zoom in" was zoom 1.00 against magnifier 0.59.
    let (cv, ca) = (d.confidence_of("view").unwrap_or(0.0), d.confidence.unwrap_or(1.0));
    if matches!(action.as_str(), "zoom" | "no_change") && view != "keep" && view != current.id() && cv >= 0.5 && cv >= ca - 0.15 {
        d.answers.insert("action".into(), json!("view"));
        d.confidence = d.confidence_of("view");
    }
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
    let render = match get("render") {
        "table" | "export" => format!("; render: {}", get("render")),
        _ => String::new(),
    };
    format!("{change}{render}; specifics: {}", get("specifics"))
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
    if s.view != super::model::View::Flat {
        v.push(format!("view: {}", package::view_line(&s.view)));
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

Chart commands: exactly one mark first, then any of the others. A mark names its encoding channels as \
`--channel field:type`, the types as in Vega-Lite (N nominal, O ordinal, Q quantitative, T temporal). \
Aggregate in a `sql` stage before the mark, not in a channel. The layer draws these five:
  chart bar --x <f>:N --y <f>:Q [--color <same f as x>]
  chart arc --theta <f>:Q --color <f>:N
  chart line --x <f>:Q --y <f>:Q --color <f>:N
  chart rect --x <f>:O --y <f>:N --color <f>:Q
  chart point --x <f>:Q --y <f>:Q --color <f>:Q
  title \"<text>\"
  set x.axis.title \"<text>\" · set y.axis.title \"<text>\"   axis titles (not on arc)
  set y.scale.type log                     bars only
  set x.scale.domain <a>,<b>               line and map; one axis, the other keeps its extent
  view fisheye --focus <x>,<y> [--radius r] [--distortion d]   a lens; focus in the plot's unit square, 0,0 bottom left
  view magnifier --focus <x>,<y> [--radius r] [--zoom k] [--offset auto|none]   a magnifier: a small source circle and a callout beside it (auto), or in place (none)
  view tilt [--yaw a] [--elevation e]                          3D, the map only, height as z
  view flat
  A pipeline has at most one `view` line, after the mark: a new view replaces the old line.
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
If the instruction asks what data there is, or which columns or fields it has, do not make a chart of it: \
reply with the single word `overview` instead, and the window shows an overview of the data. \
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
    /// The writer answered `overview`: a question about the data, not a chart.
    pub overview: bool,
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
        if text.trim_matches('`').trim().eq_ignore_ascii_case("overview") {
            attempts.push(Attempt { text, refused: None, written });
            return Ok(Outcome { attempts, applied: None, overview: true });
        }
        match editor::apply(&text, d).await {
            Ok(a) => {
                attempts.push(Attempt { text, refused: None, written });
                return Ok(Outcome { attempts, applied: Some(a), overview: false });
            }
            Err(e) => {
                refused.push((text.clone(), e.clone()));
                attempts.push(Attempt { text, refused: Some(e), written });
            }
        }
    }
    Ok(Outcome { attempts, applied: None, overview: false })
}
