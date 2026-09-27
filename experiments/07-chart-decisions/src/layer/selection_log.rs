//! The layer's selections as lines of avenger-selection's log, in the
//! pipeline text: `selection "<json>"`.
//!
//! The log records what was drawn (FINDINGS.md 29): a brush, a line brush, a
//! timebox and a lasso are written as their gestures, which this chart
//! draws again when the line is replayed; keys clicked in a legend are
//! written as their values. The effect (`--effect filter`) is how the chart
//! shows a selection, not part of it, so it stays a `select` line of the
//! layer's own syntax, as does softness on keys, which have no gesture to
//! carry it.

use avenger_selection::{
    Gesture, LogEntry, ProducerDefinition, ProducerId, Producers, Projection, ProjectionId, Resolution, SelectionId,
    SelectionSet, SelectionUpdate, SelectionValue, ValueTest, ViewId,
};
use datafusion::{common::ScalarValue, logical_expr::col};
use serde_json::{json, Value};

use super::model::{Effect, Selection, State};

const SELECTION: &str = "chart";

fn id(s: &str) -> ProjectionId {
    ProjectionId::new(s).expect("a name")
}
fn keys_producer() -> ProducerDefinition {
    ProducerDefinition::new(SelectionId::new(SELECTION).unwrap(), ProducerId::new("keys").unwrap(), ViewId::new("chart").unwrap(),
        [Projection::new(id("key"), col("key")).unwrap()]).unwrap()
}
/// Drawn selections: the chart draws them itself, so the definition only
/// gives them an address.
fn drawn_producer() -> ProducerDefinition {
    ProducerDefinition::new(SelectionId::new(SELECTION).unwrap(), ProducerId::new("drawn").unwrap(), ViewId::new("chart").unwrap(),
        [Projection::new(id("x"), col("x")).unwrap(), Projection::new(id("y"), col("y")).unwrap()]).unwrap()
}
fn producers() -> Producers {
    Producers::new([keys_producer(), drawn_producer()])
}

/// The log entry for a state's selection.
pub fn entry(n: &State) -> Value {
    let soft = |g: Gesture| match n.soft {
        Some(w) => g.with_param("soft", w),
        None => g,
    };
    let drawn = |g: Gesture| SelectionUpdate::set(&drawn_producer(), SelectionValue::default().with_gesture(soft(g)));
    let update = match &n.selection {
        Selection::None => SelectionUpdate::clear_all(&SelectionId::new(SELECTION).unwrap()),
        Selection::Keys(k) => SelectionUpdate::set(
            &keys_producer(),
            SelectionValue::tuple([(id("key"), ValueTest::OneOf(k.iter().map(|k| ScalarValue::Utf8(Some(k.clone()))).collect()))]),
        ),
        Selection::Interval { x, y } => drawn(match y {
            Some(y) => Gesture::new("interval", [[x.0, y.0], [x.1, y.1]]),
            None => Gesture::new("interval", [[x.0, 0.0], [x.1, 0.0]]).with_param("x_only", 1.0),
        }),
        Selection::Segment { a, b } => drawn(Gesture::new("segment", [*a, *b])),
        Selection::Timebox { x, y } => drawn(Gesture::new("timebox", [[x.0, y.0], [x.1, y.1]])),
        Selection::Lasso { poly, tilt, structure } => {
            let mut g = Gesture::new("lasso", poly.iter().copied());
            if let Some((yaw, elevation)) = tilt {
                g = g.with_param("yaw", *yaw).with_param("elevation", *elevation);
            }
            if let Some(d) = structure {
                g = g.with_param("structure", *d);
            }
            drawn(g)
        }
    };
    update.to_json().expect("the layer's selections are finite")
}

/// The pipeline lines for a state's selection: the log entry, then the
/// layer's own `select` flags for what the entry does not carry.
pub fn log_lines(n: &State) -> Vec<String> {
    let json = serde_json::to_string(&entry(n)).expect("JSON");
    let mut lines = vec![format!("selection \"{}\"", json.replace('"', "\\\""))];
    let keys_soft = matches!(n.selection, Selection::Keys(_) | Selection::None).then_some(n.soft).flatten();
    let mut flags = String::new();
    if let Some(w) = keys_soft {
        flags += &format!(" --soft {w}");
    }
    if n.effect == Effect::Filter {
        flags += " --effect filter";
    }
    if !flags.is_empty() {
        lines.push(format!("select{flags}"));
    }
    lines
}

/// Apply one log entry to the chart, as `select` would: `chart["select"]`
/// in the layer's own form.
pub fn apply(entry: &Value, chart: &mut Value) -> Result<(), String> {
    let e = LogEntry::from_json(entry, &producers()).map_err(|e| e.to_string())?;
    let select = match e {
        LogEntry::Update(u) => {
            let s = SelectionSet::new([(SelectionId::new(SELECTION).unwrap(), Resolution::Global)]).unwrap().apply(u).map_err(|e| e.to_string())?;
            let c = s.contributions(&SelectionId::new(SELECTION).unwrap()).map_err(|e| e.to_string())?.next();
            match c {
                None => None,
                Some(c) => {
                    let keys: Vec<String> = c.value().as_tuples().iter().flatten().flat_map(|(_, t)| match t {
                        ValueTest::OneOf(vs) => vs.iter().map(|v| v.to_string()).collect(),
                        ValueTest::Equal(v) => vec![v.to_string()],
                        ValueTest::Range { .. } => vec![],
                    }).collect();
                    Some(json!({"kind": "point", "keys": keys}))
                }
            }
        }
        LogEntry::Drawn(d) => {
            let g = &d.gesture;
            let p = g.points();
            let two = || -> Result<([f64; 2], [f64; 2]), String> {
                match p {
                    [a, b] => Ok((*a, *b)),
                    _ => Err(format!("a {} gesture has two points", g.kind())),
                }
            };
            let mut s = match g.kind() {
                "interval" => {
                    let (a, b) = two()?;
                    let y = if g.param("x_only").is_some() { Value::Null } else { json!([a[1], b[1]]) };
                    json!({"kind": "interval", "x": [a[0], b[0]], "y": y})
                }
                "segment" => {
                    let (a, b) = two()?;
                    json!({"kind": "segment", "from": a, "to": b})
                }
                "timebox" => {
                    let (a, b) = two()?;
                    json!({"kind": "timebox", "x": [a[0], b[0]], "y": [a[1], b[1]]})
                }
                "lasso" => {
                    let tilt = match (g.param("yaw"), g.param("elevation")) {
                        (Some(y), Some(e)) => json!([y, e]),
                        _ => Value::Null,
                    };
                    json!({"kind": "lasso", "poly": p, "tilt": tilt, "structure": g.param("structure")})
                }
                other => return Err(format!("the layer does not draw a {other} gesture")),
            };
            if let Some(w) = g.param("soft") {
                s["soft"] = json!(w);
            }
            Some(s)
        }
    };
    match (select, chart.as_object_mut()) {
        (Some(s), _) => chart["select"] = s,
        (None, Some(o)) => {
            o.remove("select");
        }
        (None, None) => {}
    }
    Ok(())
}
