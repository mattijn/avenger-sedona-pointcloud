//! The decider's vocabulary for the chart layer: what it sees of the one
//! chart object, the questions it answers, and how chosen options change the
//! chart state.
//!
//! The kind of chart and the data go together: every mark names the data it
//! shows (a time series is always the flight lines), so one `mark` answer is
//! enough and no field has to be chosen. Options that do not fit the current
//! mark (a zoom on a pie) are refused here, not drawn.

use serde_json::{json, Map, Value};

use super::model::{Data, Dataset, Mark, Quarter, State};
use crate::options::{NotApplied, COLOURS};

pub const MARKS: [(Mark, &str); 5] = [
    (Mark::Bars, "a bar chart: points per LiDAR class"),
    (Mark::Pie, "a pie (donut) chart: the share of points per LiDAR class"),
    (Mark::Line, "a time series: points per half second along each of the four flight lines"),
    (Mark::Heatmap, "a heatmap: points per LiDAR class and 4 m height band, coloured by count"),
    (Mark::Map, "a map: buildings in 5 m cells at their coordinates"),
];

fn hex(h: &str) -> [f32; 4] {
    let v = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap() as f32 / 255.0;
    [v(1), v(3), v(5), 1.0]
}

pub fn colour_name(c: Option<[f32; 4]>) -> &'static str {
    let Some(c) = c else { return "default" };
    COLOURS.iter().find(|(_, h)| hex(h) == c).map_or("custom", |(n, _)| n)
}

fn quarter_id(q: Option<Quarter>) -> &'static str {
    match q {
        None => "all",
        Some(Quarter::NorthEast) => "north_east",
        Some(Quarter::NorthWest) => "north_west",
        Some(Quarter::SouthEast) => "south_east",
        Some(Quarter::SouthWest) => "south_west",
    }
}

/// What can be changed on a mark.
fn zoomable(m: Mark) -> bool {
    matches!(m, Mark::Line | Mark::Map)
}
/// A pie's slices are told apart by colour only, so it keeps its category
/// colours; so do the time series (one per line) and the heatmap (a scale).
fn recolourable(m: Mark) -> bool {
    matches!(m, Mark::Bars | Mark::Map)
}
fn highlightable(m: Mark) -> bool {
    matches!(m, Mark::Bars | Mark::Pie | Mark::Map)
}

/// The observation: the chart as it is now, what else can be shown, and the
/// instruction. Sizes of the tables, never their rows.
pub fn observation(s: &State, d: &Data, instruction: &str) -> Value {
    let rows = |ds: Dataset| match ds {
        Dataset::Classes => d.classes.len(),
        Dataset::Flight => d.flight.len(),
        Dataset::ClassHeight => d.class_height.len(),
        Dataset::Cells => d.cells.len(),
    };
    let about = MARKS.iter().find(|m| m.0 == s.mark).unwrap().1;
    json!({
        "policy": "Choose the chart that best shows the data; follow the instruction.",
        "chart": {
            "kind": s.mark.id(),
            "shows": about,
            "rows": rows(s.dataset),
            "title": s.title,
            "colour": colour_name(s.color),
            "zoom": quarter_id(s.zoom),
            "highlight": if s.highlight { "top 10 %" } else { "none" },
        },
        "other_charts": MARKS.iter().filter(|m| m.0 != s.mark).map(|m| m.1).collect::<Vec<_>>(),
        "instruction": instruction,
    })
}

fn choice(instructions: &str, criteria: &[(&str, &str)]) -> Value {
    let c: Map<String, Value> = criteria.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    json!({"type": "choice", "instructions": instructions, "criteria": c})
}

pub fn questions() -> Value {
    let mut marks: Vec<(&str, &str)> = MARKS.iter().map(|m| (m.0.id(), m.1)).collect();
    marks.push(("keep", "keep the current kind"));
    let mut colours: Vec<(&str, &str)> = COLOURS.iter().map(|(n, _)| (*n, *n)).collect();
    colours.push(("keep", "no colour asked for"));
    json!({
        "action": choice("Which single change should be made to the chart now?", &[
            ("no_change", "leave the chart as it is"),
            ("mark", "change the kind of chart (bars, pie, time series, heatmap, map)"),
            ("color", "change the colour of the marks"),
            ("zoom", "zoom to a part of the chart, or back out to all of it"),
            ("highlight", "emphasise the largest values, or remove the emphasis"),
            ("title", "change the chart's title"),
        ]),
        "mark": choice("If the kind of chart should change, which kind?", &marks),
        "colour": choice("If a colour is asked for, which one?", &colours),
        "region": choice("If the view should zoom, to which part?", &[
            ("north_east", "the top right (north-east) quarter"),
            ("north_west", "the top left (north-west) quarter"),
            ("south_east", "the bottom right (south-east) quarter"),
            ("south_west", "the bottom left (south-west) quarter"),
            ("all", "all of it, reset the zoom"),
            ("keep", "no zoom asked for"),
        ]),
        "subset": choice("If the emphasis should change, how?", &[
            ("top_10", "emphasise the highest 10 % (largest classes, tallest buildings)"),
            ("none", "remove the emphasis"),
            ("keep", "no emphasis asked for"),
        ]),
    })
}

/// The next state, or why there is none. A decision that would leave the
/// state as it is does not apply either: nothing is redrawn for it.
pub fn apply(s: &State, answers: &Map<String, Value>) -> Result<State, NotApplied> {
    let get = |k: &str| answers.get(k).and_then(Value::as_str).unwrap_or("keep");
    let unfit = |m: &str| Err(NotApplied::Unfit(m.into()));
    let mut n = s.clone();
    match get("action") {
        "no_change" => return Ok(n),
        "title" => return Err(NotApplied::NeedsText),
        "mark" => {
            let Some((m, _)) = MARKS.iter().find(|m| m.0.id() == get("mark")) else {
                return unfit("mark chosen but no kind");
            };
            n.mark = *m;
            n.dataset = Dataset::ALL.iter().map(|d| d.0).find(|d| m.fits(*d)).unwrap();
            if !zoomable(n.mark) {
                n.zoom = None;
            }
            if n.dataset != s.dataset {
                n.zoom = None;
            }
            n.highlight &= highlightable(n.mark);
            if !recolourable(n.mark) {
                n.color = None;
            }
            n.title = n.default_title();
        }
        "color" => {
            if !recolourable(s.mark) {
                return unfit("this chart's colours encode data");
            }
            match COLOURS.iter().find(|(c, _)| *c == get("colour")) {
                Some((_, h)) => n.color = Some(hex(h)),
                None => return unfit("colour chosen but no colour"),
            }
        }
        "zoom" => {
            if !zoomable(s.mark) {
                return unfit("this chart has no continuous axes to zoom");
            }
            n.zoom = match get("region") {
                "north_east" => Some(Quarter::NorthEast),
                "north_west" => Some(Quarter::NorthWest),
                "south_east" => Some(Quarter::SouthEast),
                "south_west" => Some(Quarter::SouthWest),
                "all" => None,
                _ => return unfit("zoom chosen but no region"),
            };
        }
        "highlight" => {
            if !highlightable(s.mark) {
                return unfit("no emphasis on this chart");
            }
            n.highlight = match get("subset") {
                "top_10" => true,
                "none" => false,
                _ => return unfit("emphasis chosen but no subset"),
            };
        }
        other => return unfit(&format!("unknown action {other}")),
    }
    if n == *s {
        return unfit("already so");
    }
    Ok(n)
}

/// A decision as `action/argument`, for tables and the panel.
pub fn short(answers: &Map<String, Value>) -> String {
    let get = |k: &str| answers.get(k).and_then(Value::as_str);
    let action = get("action").unwrap_or("?");
    let arg = match action {
        "mark" => get("mark"),
        "color" => get("colour"),
        "zoom" => get("region"),
        "highlight" => get("subset"),
        _ => None,
    };
    arg.map_or(action.to_string(), |a| format!("{action}/{a}"))
}
