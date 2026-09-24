//! Phase A: the task vocabulary of experiment 6 as option lists, and the
//! translation of chosen options back into task commands.
//!
//! A classifier returns choices, not values. So the questions only ask for
//! intent (which action, which colour, which region, which named subset), and
//! every argument a classifier cannot produce is derived from the data: fields
//! from their roles, zoom ranges and highlight thresholds from statistics.
//! Free text (a title) cannot be expressed at all.

use serde_json::{json, Map, Value};

use crate::observe::{Role, Stats};

pub const COLOURS: [(&str, &str); 5] = [
    ("red", "#c44e52"),
    ("blue", "#4c78a8"),
    ("orange", "#f28e2b"),
    ("green", "#59a14f"),
    ("grey", "#9aa0a6"),
];

/// Narrow: the kind of chart is fixed, only styling and view change.
/// Free: the decider may also change the kind of chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Narrow,
    Free,
}

impl Policy {
    pub fn text(self) -> &'static str {
        match self {
            Policy::Narrow => "Keep the current kind of chart whatever happens; only styling and the view may change.",
            Policy::Free => "Choose the chart that best shows the data and its latest change.",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Policy::Narrow => "narrow",
            Policy::Free => "free",
        }
    }
}

fn choice(instructions: &str, criteria: &[(&str, String)]) -> Value {
    let mut c = Map::new();
    for (k, v) in criteria {
        c.insert(k.to_string(), json!(v));
    }
    json!({"type": "choice", "instructions": instructions, "criteria": c})
}

fn field_for_highlight(chart: &Value, s: &Stats) -> Option<String> {
    let used = [chart["x"]["field"].as_str(), chart["y"]["field"].as_str()];
    // The measure on the y axis, or the first measure not on an axis (the
    // height of a map's points).
    if let Some(y) = used[1] {
        if s.column(y).is_some_and(|c| c.role == Role::Measure) {
            return Some(y.to_string());
        }
    }
    s.with_role(Role::Measure)
        .find(|c| !used.contains(&Some(c.name.as_str())))
        .map(|c| c.name.clone())
}

/// The questions for one decision, all asked in one call.
pub fn questions(policy: Policy, chart: &Value, s: &Stats) -> Value {
    let mut actions: Vec<(&str, String)> = vec![
        ("no_change", "leave the chart as it is".into()),
        ("color", "change the colour of the marks".into()),
        (
            "zoom",
            "zoom to a part of the data, or back out to all of it".into(),
        ),
        ("rotate_labels", "rotate the axis labels".into()),
        ("highlight", "emphasise a subset of the data".into()),
        ("title", "change the chart's title".into()),
    ];
    if policy == Policy::Free {
        actions.insert(
            1,
            (
                "mark",
                "change the kind of chart (bars, a scatter plot, a map)".into(),
            ),
        );
    }
    let mut q = Map::new();
    q.insert(
        "action".into(),
        choice(
            "Which single change should be made to the chart now?",
            &actions,
        ),
    );
    if policy == Policy::Free {
        q.insert(
            "mark".into(),
            choice(
                "If the kind of chart should change, which kind?",
                &[
                    ("bars", "bars of a count or measure per category".into()),
                    ("points", "a scatter plot of two measures".into()),
                    ("map", "points placed at their map coordinates".into()),
                    ("keep", "keep the current kind".into()),
                ],
            ),
        );
    }
    let mut colours: Vec<(&str, String)> =
        COLOURS.iter().map(|(n, _)| (*n, n.to_string())).collect();
    colours.push(("keep", "no colour asked for".into()));
    q.insert(
        "colour".into(),
        choice("If a colour is asked for, which one?", &colours),
    );
    q.insert(
        "region".into(),
        choice(
            "If the view should zoom, to which part of the data?",
            &[
                ("north_east", "the top right (north-east) quarter".into()),
                ("north_west", "the top left (north-west) quarter".into()),
                ("south_east", "the bottom right (south-east) quarter".into()),
                ("south_west", "the bottom left (south-west) quarter".into()),
                ("all", "all of the data, reset the zoom".into()),
                ("keep", "no zoom asked for".into()),
            ],
        ),
    );
    q.insert(
        "angle".into(),
        choice(
            "If labels should rotate, how far?",
            &[
                ("tilt", "tilted, 45 degrees".into()),
                ("vertical", "vertical, 90 degrees".into()),
                ("keep", "no rotation asked for".into()),
            ],
        ),
    );
    let f = field_for_highlight(chart, s).unwrap_or_else(|| "the measure".into());
    q.insert(
        "subset".into(),
        choice(
            "If a subset should be emphasised, which one?",
            &[
                (
                    "top_10",
                    format!("the highest 10 % of {f} (tall, large, high values)"),
                ),
                (
                    "outliers",
                    format!("extreme values of {f}, far above all the others"),
                ),
                ("keep", "no subset asked for".into()),
            ],
        ),
    );
    Value::Object(q)
}

/// Why a decision did not become a command.
#[derive(Debug, Clone, PartialEq)]
pub enum NotApplied {
    /// The option needs free text a classifier cannot give (a title).
    NeedsText,
    /// The decision is incomplete or does not fit the chart.
    Unfit(String),
}

/// Turn chosen options into experiment 6 task commands.
pub fn commands(
    answers: &Map<String, Value>,
    chart: &Value,
    s: &Stats,
) -> Result<Vec<String>, NotApplied> {
    let get = |k: &str| answers.get(k).and_then(Value::as_str).unwrap_or("keep");
    let unfit = |m: &str| Err(NotApplied::Unfit(m.into()));
    match get("action") {
        "no_change" => Ok(vec![]),
        "title" => Err(NotApplied::NeedsText),
        "color" => match COLOURS.iter().find(|(n, _)| *n == get("colour")) {
            Some((_, hex)) => Ok(vec![format!("color {hex}")]),
            None => unfit("color chosen but no colour"),
        },
        "rotate_labels" => match get("angle") {
            "vertical" => Ok(vec!["rotate-labels x --angle -90".into()]),
            _ => Ok(vec!["rotate-labels x --angle -45".into()]),
        },
        "zoom" => {
            let region = get("region");
            if region == "all" {
                return Ok(vec!["reset-zoom".into()]);
            }
            let (Some(xf), Some(yf)) = (chart["x"]["field"].as_str(), chart["y"]["field"].as_str())
            else {
                return unfit("nothing drawn to zoom");
            };
            let (Some(x), Some(y)) = (s.column(xf), s.column(yf)) else {
                return unfit("zoom fields missing");
            };
            let (Some(x0), Some(x1), Some(y0), Some(y1)) = (x.min, x.max, y.min, y.max) else {
                return unfit("zoom needs numeric x and y");
            };
            let (xm, ym) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            let (xr, yr) = match region {
                "north_east" => ((xm, x1), (ym, y1)),
                "north_west" => ((x0, xm), (ym, y1)),
                "south_east" => ((xm, x1), (y0, ym)),
                "south_west" => ((x0, xm), (y0, ym)),
                _ => return unfit("zoom chosen but no region"),
            };
            Ok(vec![format!("zoom {}..{} {}..{}", xr.0, xr.1, yr.0, yr.1)])
        }
        "highlight" => {
            let Some(f) = field_for_highlight(chart, s) else {
                return unfit("no measure to highlight");
            };
            let c = s.column(&f).unwrap();
            let t = match get("subset") {
                "outliers" => c.p999,
                _ => c.p90,
            };
            match t {
                Some(t) => Ok(vec![format!("highlight \"datum.{f} > {t}\"")]),
                None => unfit("no threshold"),
            }
        }
        "mark" => {
            let measures: Vec<&str> = s
                .with_role(Role::Measure)
                .map(|c| c.name.as_str())
                .collect();
            let coords: Vec<&str> = s
                .with_role(Role::Coordinate)
                .map(|c| c.name.as_str())
                .collect();
            let category = s.with_role(Role::Category).next().map(|c| c.name.as_str());
            match get("mark") {
                "bars" => match (category, measures.first()) {
                    (Some(c), Some(m)) => Ok(vec![format!("bars {m} --by {c}")]),
                    _ => unfit("bars need a category and a measure"),
                },
                "points" => match measures.as_slice() {
                    [a, b, ..] => Ok(vec![format!("points --x {a} --y {b}")]),
                    _ => unfit("a scatter needs two measures"),
                },
                "map" => match coords.as_slice() {
                    [a, b, ..] => Ok(vec![format!("points --x {a} --y {b} --size 4")]),
                    _ => unfit("a map needs coordinates"),
                },
                _ => unfit("mark chosen but no kind"),
            }
        }
        other => unfit(&format!("unknown action {other}")),
    }
}
