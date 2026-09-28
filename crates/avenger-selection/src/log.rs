//! Selection updates written as JSON and read back, for a pipeline log.
//!
//! Not upstream: added in this repo (VENDORED.md). A log records what the
//! user did, not the chart's definitions: producer definitions hold
//! DataFusion expressions and captured scales, which the chart rebuilds from
//! its own specification. So an update is written as its producer's address
//! (selection, producer, view), the operation, and what was drawn; reading
//! it back looks the definition up in `Producers`.
//!
//! A log records what was drawn, not what it selected. A value with a
//! gesture is written as the gesture alone, and replay draws it again: the
//! crate redraws its own kinds (a lasso, `"polygon"`), and returns the
//! chart's kinds (a line brush, a timebox, CloudLasso), whose outcome depends
//! on the data, as `LogEntry::Drawn` for the chart to redraw. Replayed on the
//! same data, a log gives the same state; on data that has moved, the same
//! gestures select what they would select now. A value without a gesture,
//! such as keys clicked in a legend, is written as its tuples.
//!
//! Scalars are written as their Arrow type and their value cast to a string,
//! which DataFusion casts back: `{"type": "Int64", "value": "5"}`. Binary
//! values, which do not cast to a string, are refused.

use std::{collections::BTreeMap, ops::Bound, str::FromStr};

use datafusion::{
    arrow::{compute::cast, datatypes::DataType},
    common::ScalarValue,
};
use serde_json::{json, Map, Value};

use crate::{
    identity::ProducerAddress, state::Update, Error, Gesture, ProducerDefinition, ProducerId,
    ProjectionId, Result, SelectionId, SelectionUpdate, SelectionValue, ValueTest, ViewId,
};

/// The chart's producer definitions, by address, for reading a log back.
#[derive(Clone, Debug, Default)]
pub struct Producers(BTreeMap<ProducerAddress, ProducerDefinition>);
impl Producers {
    pub fn new(definitions: impl IntoIterator<Item = ProducerDefinition>) -> Self {
        Self(
            definitions
                .into_iter()
                .map(|d| (d.address().clone(), d))
                .collect(),
        )
    }
    fn get(&self, address: &ProducerAddress) -> Result<&ProducerDefinition> {
        self.0.get(address).ok_or_else(|| {
            bad(format!(
                "it names producer {} of view {} in selection {}, which the chart does not define",
                address.producer, address.origin, address.selection
            ))
        })
    }
}

impl SelectionUpdate {
    /// This update as one JSON object, for one line of a log.
    pub fn to_json(&self) -> Result<Value> {
        let address = |a: &ProducerAddress| json!({"selection": a.selection.as_str(), "producer": a.producer.as_str(), "view": a.origin.as_str()});
        let (op, at, value) = match &self.0 {
            Update::Set(p, v) => ("set", p.address(), Some(v)),
            Update::Toggle(p, v) => ("toggle", p.address(), Some(v)),
            Update::Clear(a) => ("clear", a, None),
            Update::ClearAll(id) => {
                return Ok(json!({"op": "clear_all", "selection": id.as_str()}))
            }
        };
        let mut o = address(at);
        o["op"] = json!(op);
        if let Some(v) = value {
            o["value"] = value_json(v)?;
        }
        Ok(o)
    }

    /// Read one logged update back against the chart's definitions, drawing
    /// a lasso again. A gesture of the chart's own kind is an error here; read
    /// such a log with `LogEntry::from_json` and redraw it.
    pub fn from_json(v: &Value, producers: &Producers) -> Result<Self> {
        match LogEntry::from_json(v, producers)? {
            LogEntry::Update(u) => Ok(u),
            LogEntry::Drawn(d) => Err(bad(format!(
                "a {} gesture is the chart's to draw again: read it with LogEntry::from_json",
                d.gesture.kind()
            ))),
        }
    }
}

/// One line of a log, read back.
#[derive(Clone, Debug)]
pub enum LogEntry {
    /// Ready to apply: a clear, keys, or a lasso drawn again by the crate.
    Update(SelectionUpdate),
    /// A gesture of the chart's own kind, to draw again against the data.
    Drawn(Drawn),
}

/// A logged gesture the chart draws again, as it did when the user drew it.
#[derive(Clone, Debug)]
pub struct Drawn {
    pub toggle: bool,
    pub producer: ProducerDefinition,
    pub gesture: Gesture,
}
impl Drawn {
    /// The update, from the value the chart drew; the gesture goes with it.
    pub fn redraw(self, value: SelectionValue) -> SelectionUpdate {
        update(
            self.toggle,
            &self.producer,
            value.with_gesture(self.gesture),
        )
    }
}

impl LogEntry {
    /// Read one line of a log against the chart's definitions.
    pub fn from_json(v: &Value, producers: &Producers) -> Result<Self> {
        let s = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .ok_or_else(|| bad(format!("an update needs a string {k}")))
        };
        let address = || -> Result<ProducerAddress> {
            Ok(ProducerAddress {
                selection: SelectionId::new(s("selection")?)?,
                producer: ProducerId::new(s("producer")?)?,
                origin: ViewId::new(s("view")?)?,
            })
        };
        Ok(match s("op")? {
            op @ ("set" | "toggle") => {
                let producer = producers.get(&address()?)?;
                let toggle = op == "toggle";
                let value = v
                    .get("value")
                    .ok_or_else(|| bad("a set or toggle needs a value"))?;
                let value = match value.get("drawn") {
                    None => value_from(value)?,
                    Some(g) => {
                        let gesture = gesture_from(g)?;
                        match SelectionValue::from_gesture(producer, &gesture) {
                            Some(value) => value?,
                            None => {
                                return Ok(LogEntry::Drawn(Drawn {
                                    toggle,
                                    producer: producer.clone(),
                                    gesture,
                                }))
                            }
                        }
                    }
                };
                LogEntry::Update(update(toggle, producer, value))
            }
            "clear" => LogEntry::Update(SelectionUpdate::clear(producers.get(&address()?)?)),
            "clear_all" => LogEntry::Update(SelectionUpdate::clear_all(&SelectionId::new(s(
                "selection",
            )?)?)),
            other => return Err(bad(format!("unknown operation {other}"))),
        })
    }
}

fn update(toggle: bool, producer: &ProducerDefinition, value: SelectionValue) -> SelectionUpdate {
    if toggle {
        SelectionUpdate::toggle(producer, value)
    } else {
        SelectionUpdate::set(producer, value)
    }
}

fn bad(m: impl Into<String>) -> Error {
    Error::InvalidUpdate(format!("selection log: {}", m.into()))
}

fn value_json(v: &SelectionValue) -> Result<Value> {
    if let Some(g) = v.gesture() {
        return Ok(json!({"drawn": gesture_json(g)?}));
    }
    if v.partial().is_some() {
        return Err(bad(
            "partial degrees are redrawn from their gesture, and this value has none",
        ));
    }
    let tuples = v
        .as_tuples()
        .iter()
        .map(|t| {
            t.iter()
                .map(|(id, test)| term_json(id, test))
                .collect::<Result<Vec<_>>>()
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"tuples": tuples}))
}
fn value_from(v: &Value) -> Result<SelectionValue> {
    let tuples = v
        .get("tuples")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("a value needs tuples"))?
        .iter()
        .map(|t| {
            t.as_array()
                .ok_or_else(|| bad("a tuple is a list of terms"))?
                .iter()
                .map(term_from)
                .collect::<Result<Vec<_>>>()
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(SelectionValue::tuples(tuples))
}

fn term_json(id: &ProjectionId, test: &ValueTest) -> Result<Value> {
    let bound = |b: &Bound<ScalarValue>| -> Result<Value> {
        Ok(match b {
            Bound::Unbounded => Value::Null,
            Bound::Included(v) => json!({"included": scalar_json(v)?}),
            Bound::Excluded(v) => json!({"excluded": scalar_json(v)?}),
        })
    };
    Ok(match test {
        ValueTest::Equal(v) => {
            json!({"projection": id.as_str(), "test": "equal", "value": scalar_json(v)?})
        }
        ValueTest::OneOf(vs) => json!({"projection": id.as_str(), "test": "one_of",
            "values": vs.iter().map(scalar_json).collect::<Result<Vec<_>>>()?}),
        ValueTest::Range { lower, upper } => json!({"projection": id.as_str(), "test": "range",
            "lower": bound(lower)?, "upper": bound(upper)?}),
    })
}
fn term_from(v: &Value) -> Result<(ProjectionId, ValueTest)> {
    let id = ProjectionId::new(
        v.get("projection")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("a term needs a projection"))?,
    )?;
    let bound = |b: Option<&Value>| -> Result<Bound<ScalarValue>> {
        match b {
            None | Some(Value::Null) => Ok(Bound::Unbounded),
            Some(b) => match (b.get("included"), b.get("excluded")) {
                (Some(s), None) => Ok(Bound::Included(scalar_from(s)?)),
                (None, Some(s)) => Ok(Bound::Excluded(scalar_from(s)?)),
                _ => Err(bad("a bound is included or excluded")),
            },
        }
    };
    let test = match v.get("test").and_then(Value::as_str) {
        Some("equal") => ValueTest::Equal(scalar_from(
            v.get("value").ok_or_else(|| bad("equal needs a value"))?,
        )?),
        Some("one_of") => ValueTest::OneOf(
            v.get("values")
                .and_then(Value::as_array)
                .ok_or_else(|| bad("one_of needs values"))?
                .iter()
                .map(scalar_from)
                .collect::<Result<_>>()?,
        ),
        Some("range") => ValueTest::Range {
            lower: bound(v.get("lower"))?,
            upper: bound(v.get("upper"))?,
        },
        other => return Err(bad(format!("unknown test {other:?}"))),
    };
    Ok((id, test))
}

fn scalar_json(v: &ScalarValue) -> Result<Value> {
    let t = v.data_type();
    if matches!(
        t,
        DataType::Binary
            | DataType::LargeBinary
            | DataType::BinaryView
            | DataType::FixedSizeBinary(_)
    ) {
        return Err(bad(format!(
            "{t} values do not cast to a string, so the log refuses them"
        )));
    }
    let value = if v.is_null() {
        Value::Null
    } else {
        let s = cast(&v.to_array()?, &DataType::Utf8)
            .map_err(datafusion::common::DataFusionError::from)?;
        Value::String(ScalarValue::try_from_array(&s, 0)?.to_string())
    };
    Ok(json!({"type": t.to_string(), "value": value}))
}
fn scalar_from(v: &Value) -> Result<ScalarValue> {
    let t = v
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("a scalar needs a type"))?;
    let t = DataType::from_str(t).map_err(|e| bad(format!("type {t}: {e}")))?;
    Ok(match v.get("value") {
        None | Some(Value::Null) => ScalarValue::try_from(&t)?,
        Some(Value::String(s)) => {
            let value = ScalarValue::try_from_string(s.clone(), &t)?;
            if value.is_null() {
                return Err(bad(format!("{s} does not read as {t}")));
            }
            value
        }
        Some(_) => return Err(bad("a scalar's value is a string or null")),
    })
}

fn gesture_json(g: &Gesture) -> Result<Value> {
    let finite = g
        .points()
        .iter()
        .flatten()
        .chain(g.params().iter().map(|(_, v)| v))
        .all(|v| v.is_finite());
    if !finite {
        return Err(bad("a gesture's numbers must be finite"));
    }
    let params: Map<String, Value> = g
        .params()
        .iter()
        .map(|(n, v)| (n.clone(), json!(v)))
        .collect();
    let on: Vec<&str> = g.projections().iter().map(|p| p.as_str()).collect();
    Ok(json!({"kind": g.kind(), "points": g.points(), "params": params, "on": on}))
}
fn gesture_from(v: &Value) -> Result<Gesture> {
    let kind = v
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("a gesture needs a kind"))?;
    let num = |x: &Value| {
        x.as_f64()
            .ok_or_else(|| bad("a gesture's numbers are numbers"))
    };
    let points = v
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("a gesture needs points"))?
        .iter()
        .map(|p| match p.as_array().map(Vec::as_slice) {
            Some([x, y]) => Ok([num(x)?, num(y)?]),
            _ => Err(bad("a point is [x, y]")),
        })
        .collect::<Result<Vec<_>>>()?;
    let on = match v.get("on").and_then(Value::as_array) {
        None => Vec::new(),
        Some(a) => a
            .iter()
            .map(|p| {
                ProjectionId::new(
                    p.as_str()
                        .ok_or_else(|| bad("a gesture's projections are names"))?,
                )
            })
            .collect::<Result<Vec<_>>>()?,
    };
    let mut g = Gesture::new(kind, points).on(on);
    if let Some(ps) = v.get("params").and_then(Value::as_object) {
        for (n, x) in ps {
            g = g.with_param(n.clone(), num(x)?);
        }
    }
    Ok(g)
}
