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

use super::model::{base_domains, emphasis, quarter_domains, Data, Dataset, Mark, Quarter, State, View};
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

/// `chart <mark> --x f[:T] --y f[:T] --color f[:T] …`: a mark with named
/// encoding channels and types, as in Vega-Lite. The layer draws five
/// combinations; each is run as the short command it stands for (`bar` with
/// a nominal x and a quantitative y is `bars <y> --by <x>`), so fields are
/// checked the same way, and the channels are kept in the chart state.
/// Aggregation stays in a `sql` stage before the mark.
struct ChartStep;

const CHANNELS: [&str; 10] = ["x", "y", "color", "theta", "detail", "size", "tooltip", "shape", "opacity", "text"];

/// A channel as written, `field` or `field:T`, with its type: given, or
/// from the field's Arrow type.
fn channel(p: &Pipeline, c: &Call, name: &str) -> Result<Option<(String, char)>> {
    let Some(v) = c.flag(name) else { return Ok(None) };
    let (f, t) = match v.rsplit_once(':') {
        Some((f, t)) if matches!(t, "N" | "O" | "Q" | "T" | "nominal" | "ordinal" | "quantitative" | "temporal") => (f, Some(t)),
        _ => (v, None),
    };
    let schema = p.plan()?.schema().clone();
    let dt = schema
        .fields()
        .iter()
        .find(|x| x.name() == f)
        .map(|x| x.data_type().clone())
        .ok_or_else(|| err(format!("chart: no field `{f}` for --{name} in the data (fields: {})", schema.fields().iter().map(|x| x.name().as_str()).collect::<Vec<_>>().join(", "))))?;
    let ty = match t {
        Some(t) => t.chars().next().unwrap().to_ascii_uppercase(),
        None if dt.is_numeric() => 'Q',
        None if matches!(dt, datafusion::arrow::datatypes::DataType::Timestamp(..) | datafusion::arrow::datatypes::DataType::Date32 | datafusion::arrow::datatypes::DataType::Date64) => 'T',
        None => 'N',
    };
    Ok(Some((f.to_string(), ty)))
}

fn type_name(t: char) -> &'static str {
    match t {
        'N' => "nominal",
        'O' => "ordinal",
        'Q' => "quantitative",
        _ => "temporal",
    }
}

#[async_trait]
impl Step for ChartStep {
    fn kind(&self) -> Kind {
        Kind::Command
    }
    fn help(&self) -> &'static str {
        "chart bar --x f:N --y f:Q · chart arc --theta f:Q --color f:N · chart line --x f:Q --y f:Q --color f:N · chart rect --x f:O --y f:N --color f:Q · chart point --x f:Q --y f:Q --color f:Q"
    }
    async fn run(&self, p: &mut Pipeline, c: &Call) -> Result<Option<String>> {
        let mark = c.arg(0)?.to_string();
        let allowed: &[&str] = match mark.as_str() {
            "bar" => &["x", "y", "color"],
            "arc" => &["theta", "color"],
            "line" => &["x", "y", "color", "detail"],
            "rect" => &["x", "y", "color"],
            "point" | "square" | "circle" => &["x", "y", "color"],
            other => return Err(err(format!("chart: the layer draws bar, arc, line, rect and point, not `{other}`"))),
        };
        for k in c.flags.keys() {
            if !CHANNELS.contains(&k.as_str()) {
                return Err(err(format!("chart: `--{k}` is not an encoding channel")));
            }
            if !allowed.contains(&k.as_str()) {
                return Err(err(format!("chart {mark}: the layer has no `{k}` channel for this mark; it takes {}", allowed.join(", "))));
            }
        }
        // A constant colour, `--color #c44e52`, is a value, not a field.
        let constant = c.flag("color").filter(|v| v.starts_with('#')).map(String::from);
        let mut enc = serde_json::Map::new();
        let mut need = |name: &str, types: &str| -> Result<String> {
            let (f, t) = channel(p, c, name)?.ok_or_else(|| err(format!("chart {mark} needs --{name}")))?;
            if !types.contains(t) {
                let want: Vec<&str> = types.chars().map(type_name).collect();
                return Err(err(format!("chart {mark}: --{name} {f} is {}, the layer draws it as {}", type_name(t), want.join(" or "))));
            }
            enc.insert(name.into(), json!({"field": f, "type": type_name(t)}));
            Ok(f)
        };
        let (step, value, flags): (&str, Option<String>, Vec<(&str, String)>) = match mark.as_str() {
            "bar" => {
                let (x, y) = (need("x", "NO")?, need("y", "Q")?);
                if constant.is_none() && c.flag("color").is_some() {
                    let col = need("color", "NO")?;
                    if col != x {
                        return Err(err(format!("chart bar: bars are coloured by their own category (--color {x}) or one colour (--color #rrggbb), not by `{col}`")));
                    }
                }
                ("bars", Some(y), vec![("by", x)])
            }
            "arc" => {
                let (v, by) = (need("theta", "Q")?, need("color", "NO")?);
                ("pie", Some(v), vec![("by", by)])
            }
            "line" => {
                let (x, y) = (need("x", "QT")?, need("y", "Q")?);
                let series = if c.flag("detail").is_some() { need("detail", "NO")? } else { need("color", "NO")? };
                ("line", Some(y), vec![("x", x), ("series", series)])
            }
            "rect" => {
                let (x, y, v) = (need("x", "OQ")?, need("y", "NO")?, need("color", "Q")?);
                ("heatmap", Some(v), vec![("x", x), ("y", y)])
            }
            _ => {
                let (x, y, v) = (need("x", "Q")?, need("y", "Q")?, need("color", "Q")?);
                ("map", None, vec![("x", x), ("y", y), ("value", v)])
            }
        };
        let short = Call {
            name: step.into(),
            args: value.into_iter().collect(),
            flags: flags.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        };
        MarkStep.run(p, &short).await?;
        p.chart["encoding"] = Value::Object(enc);
        p.chart["vlmark"] = json!(mark);
        if let Some(h) = constant {
            p.chart["fill"] = json!(h);
        }
        Ok(None)
    }
}

/// `view flat | fisheye | magnifier | tilt [--focus x,y] [--radius r]
/// [--distortion d] [--zoom k] [--yaw a] [--elevation e]`: how the plot is
/// seen. The focus is in the plot's unit square (0,0 bottom left).
struct ViewStep;

#[async_trait]
impl Step for ViewStep {
    fn kind(&self) -> Kind {
        Kind::Command
    }
    fn help(&self) -> &'static str {
        "view flat · view fisheye --focus x,y [--radius r --distortion d] · view magnifier --focus x,y [--radius r --zoom k --offset auto|none] · view tilt [--yaw a --elevation e]"
    }
    async fn run(&self, p: &mut Pipeline, c: &Call) -> Result<Option<String>> {
        let kind = c.arg(0)?;
        let num = |k: &str, d: f64| -> Result<f64> {
            c.flag(k).map_or(Ok(d), |v| v.parse().map_err(|_| err(format!("view: --{k} {v} is not a number"))))
        };
        let focus = match c.flag("focus") {
            None => [0.5, 0.5],
            Some(v) => {
                let (a, b) = v.split_once(',').ok_or_else(|| err(format!("view: --focus {v} is not x,y")))?;
                let (a, b): (f64, f64) = (a.trim().parse().map_err(|_| err("view: --focus x is not a number"))?, b.trim().parse().map_err(|_| err("view: --focus y is not a number"))?);
                if !(0.0..=1.0).contains(&a) || !(0.0..=1.0).contains(&b) {
                    return Err(err(format!("view: --focus {v} is outside the plot (0..1, 0..1)")));
                }
                [a, b]
            }
        };
        let v = match kind {
            "flat" => json!({"kind": "flat"}),
            "fisheye" => json!({"kind": "fisheye", "focus": focus, "radius": num("radius", 0.35)?, "distortion": num("distortion", 3.0)?}),
            "magnifier" => {
                let offset = match c.flag("offset").unwrap_or("auto") {
                    "auto" => true,
                    "none" => false,
                    o => return Err(err(format!("view magnifier: --offset is auto or none, not {o}"))),
                };
                // Defaults from the lens studies: offset at 4×, in place at 2×.
                json!({"kind": "magnifier", "focus": focus, "radius": num("radius", if offset { 0.06 } else { 0.2 })?, "zoom": num("zoom", if offset { 4.0 } else { 2.0 })?, "offset": offset})
            }
            "tilt" => json!({"kind": "tilt", "yaw": num("yaw", 30.0)?, "elevation": num("elevation", 40.0)?}),
            other => return Err(err(format!("view: the layer has flat, fisheye, magnifier and tilt, not `{other}`"))),
        };
        p.chart["view"] = v;
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
    steps.push(("chart", Arc::new(ChartStep)));
    steps.push(("view", Arc::new(ViewStep)));
    steps.push(("clear-highlight", Arc::new(ClearHighlight)));
    Package { name: "layer", functions: vec![], steps }
}

// ---------------------------------------------------------------------------
// Decisions → pipeline lines.

fn hex_of(c: [f32; 4]) -> Option<&'static str> {
    COLOURS.iter().find(|(n, _)| super::pilot::colour_name(Some(c)) == *n).map(|(_, h)| *h)
}

/// The mark with its encoding, as Vega-Lite would name it.
fn mark_line(m: Mark) -> &'static str {
    match m {
        Mark::Bars => "chart bar --x label:N --y n:Q",
        Mark::Pie => "chart arc --theta n:Q --color label:N",
        Mark::Line => "chart line --x t:Q --y n:Q --color line:N",
        Mark::Heatmap => "chart rect --x band:O --y label:N --color n:Q",
        Mark::Map => "chart point --x cx:Q --y cy:Q --color h:Q",
    }
}

fn quoted(t: &str) -> String {
    format!("\"{}\"", t.replace('"', "\\\""))
}

/// The scale and axis properties that differ from the defaults, as `set`.
fn props(n: &State) -> Vec<String> {
    let mut v = vec![];
    if let Some(t) = &n.x_title {
        v.push(format!("set x.axis.title {}", quoted(t)));
    }
    if let Some(t) = &n.y_title {
        v.push(format!("set y.axis.title {}", quoted(t)));
    }
    if n.y_log {
        v.push("set y.scale.type log".into());
    }
    if n.view != View::Flat {
        v.push(view_line(&n.view));
    }
    v
}

fn short_num(x: f64) -> String {
    format!("{}", (x * 1000.0).round() / 1000.0)
}

/// The `view` command for a view.
pub fn view_line(v: &View) -> String {
    let f = |p: [f64; 2]| format!("{},{}", short_num(p[0]), short_num(p[1]));
    match v {
        View::Flat => "view flat".into(),
        View::Fisheye { focus, radius, distortion } => format!("view fisheye --focus {} --radius {} --distortion {}", f(*focus), short_num(*radius), short_num(*distortion)),
        View::Magnifier { focus, radius, zoom, offset, .. } => format!("view magnifier --focus {} --radius {} --zoom {}{}", f(*focus), short_num(*radius), short_num(*zoom), if *offset { "" } else { " --offset none" }),
        View::Tilt { yaw, elevation } => format!("view tilt --yaw {} --elevation {}", short_num(*yaw), short_num(*elevation)),
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

fn title_line(t: &str) -> String {
    format!("title \"{}\"", t.replace('"', "\\\""))
}

/// A fresh mark (after an optional `read`), then what it carries.
fn fresh(read: bool, n: &State, d: &Data) -> Vec<String> {
    let read = if read { format!("read {} ! ", super::data::source(n.dataset)) } else { String::new() };
    let mut out = vec![format!("{read}{}", mark_line(n.mark))];
    if n.title != n.default_title() {
        out.push(title_line(&n.title));
    }
    out.extend(props(n));
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
    if n.title != s.title {
        out.push(title_line(&n.title));
    }
    if n.x_title != s.x_title {
        out.push(format!("set x.axis.title {}", quoted(n.x_title.as_deref().unwrap_or(""))));
    }
    if n.y_title != s.y_title {
        out.push(format!("set y.axis.title {}", quoted(n.y_title.as_deref().unwrap_or(""))));
    }
    if n.y_log != s.y_log {
        out.push(format!("set y.scale.type {}", if n.y_log { "log" } else { "linear" }));
    }
    if n.view != s.view {
        out.push(view_line(&n.view));
    }
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

/// A domain from `zoom` (`x.domain`) or `set x.scale.domain a,b`.
fn domain(v: &Value) -> Option<(f64, f64)> {
    let s = v["domain"].as_str().or(v["scale"]["domain"].as_str())?;
    let (a, b) = s.split_once(',').or_else(|| s.split_once(".."))?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// The properties `set` may give a channel; anything else is refused, since
/// the layer would not draw it.
const PROPS: [&str; 6] = ["axis.title", "title", "scale.type", "scale.domain", "domain", "scale.scheme"];

fn check_props(chart: &Value) -> std::result::Result<(), String> {
    fn walk(prefix: &str, v: &Value, out: &mut Vec<String>) {
        match v.as_object() {
            Some(o) => o.iter().for_each(|(k, x)| walk(&if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") }, x, out)),
            None => out.push(prefix.to_string()),
        }
    }
    for ch in ["x", "y", "color"] {
        let mut keys = vec![];
        walk("", &chart[ch], &mut keys);
        for k in keys.iter().filter(|k| !k.is_empty() && *k != "field" && *k != "type") {
            if !PROPS.contains(&k.as_str()) {
                return Err(format!("the layer draws {ch}.axis.title, {ch}.scale.domain and y.scale.type (bars), not {ch}.{k}"));
            }
        }
    }
    if let Some(s) = chart["color"]["scale"]["scheme"].as_str().filter(|s| *s != "viridis") {
        return Err(format!("the heatmap draws viridis only, not {s}"));
    }
    Ok(())
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
    check_props(chart)?;
    let mut s = State::new(dataset);
    s.mark = mark;
    s.title = chart["title"].as_str().map_or_else(|| s.default_title(), String::from);
    // `set x.axis.title` (and experiment 6's `set x.title`); empty is the default.
    let title_of = |ch: &str| chart[ch]["axis"]["title"].as_str().or(chart[ch]["title"].as_str()).filter(|t| !t.is_empty()).map(String::from);
    s.x_title = title_of("x");
    s.y_title = title_of("y");
    if mark == Mark::Pie && (s.x_title.is_some() || s.y_title.is_some()) {
        return Err("a pie has no axes to title".into());
    }
    s.view = match chart["view"]["kind"].as_str() {
        None | Some("flat") => View::Flat,
        Some(k) => {
            let n = |f: &str| chart["view"][f].as_f64().unwrap_or(0.0);
            let focus = [chart["view"]["focus"][0].as_f64().unwrap_or(0.5), chart["view"]["focus"][1].as_f64().unwrap_or(0.5)];
            match k {
                "fisheye" => View::Fisheye { focus, radius: n("radius"), distortion: n("distortion") },
                "magnifier" => View::Magnifier { focus, radius: n("radius"), zoom: n("zoom"), offset: chart["view"]["offset"].as_bool().unwrap_or(true), side: 0, anchor: [f64::NAN; 2] },
                _ if mark != Mark::Map => return Err("the layer tilts the map only, with height as z".into()),
                _ => View::Tilt { yaw: n("yaw"), elevation: n("elevation") },
            }
        }
    };
    if mark == Mark::Pie && matches!(s.view, View::Magnifier { .. }) {
        return Err("the magnifier works on flat charts, not on a pie (a fisheye does)".into());
    }
    match chart["y"]["scale"]["type"].as_str() {
        None | Some("linear") => {}
        Some("log") if mark == Mark::Bars => s.y_log = true,
        Some("log") => return Err("the layer draws a log scale on bars only".into()),
        Some(t) => return Err(format!("the layer draws linear and log scales, not {t}")),
    }
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
    // One axis alone keeps the other's full extent.
    let base = base_domains(mark, d);
    let (dx, dy) = match (domain(&chart["x"]), domain(&chart["y"]), base) {
        (Some(x), None, Some((_, by))) => (Some(x), Some(by)),
        (None, Some(y), Some((bx, _))) => (Some(bx), Some(y)),
        (x, y, _) => (x, y),
    };
    match (dx, dy) {
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
