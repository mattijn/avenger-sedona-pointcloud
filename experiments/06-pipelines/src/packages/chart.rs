//! The `chart` package: CQRS over an Avenger chart.
//!
//! Commands (`chart`, `set`, `undo`) change the chart state and nothing
//! else. The state is a fold of the command log, so replaying the log
//! rebuilds it, and undo is refolding without the last command. Queries
//! (`get`, `domain`, `history`) read the state or the data and change
//! nothing. `render` is the sink: only there is the data materialised, and
//! an `avenger-chart-definition` built and rendered by `avenger-chart`.

use std::sync::Arc;

use async_trait::async_trait;
use avenger_chart::{Chart, ChartOptions, RenderOptions};
use avenger_chart_definition::dataflow::{DataflowBuilder, TableSnapshot};
use avenger_chart_definition::{
    Axis, ChartDefinition, Domain, Range, RectEncoding, Scale, SymbolEncoding,
};
use datafusion::common::Column;
use datafusion::error::Result;
use datafusion::functions_aggregate::expr_fn::{max, min};
use datafusion::logical_expr::Expr;
use serde_json::{json, Value};

use crate::pipeline::{err, parse_pipeline, Call, Kind, Package, Pipeline, Step};

fn col(name: &str) -> Expr {
    Expr::Column(Column::from_name(name))
}

/// Apply one command to the chart state. Pure: the same log always gives
/// the same state.
pub fn apply(state: &mut Value, call: &Call) -> Result<()> {
    match call.name.as_str() {
        "chart" => {
            let mark = call.arg(0)?;
            if !matches!(mark, "symbol" | "bar") {
                return Err(err(format!("chart: unknown mark `{mark}` (symbol, bar)")));
            }
            let x = call.flag("x").ok_or_else(|| err("chart needs --x"))?;
            let y = call.flag("y").ok_or_else(|| err("chart needs --y"))?;
            state["mark"] = json!(mark);
            state["x"]["field"] = json!(x);
            state["y"]["field"] = json!(y);
            for k in ["fill", "size"] {
                if let Some(v) = call.flag(k) {
                    state[k] = json!(v);
                }
            }
        }
        "set" => {
            let (key, value) = (call.arg(0)?, call.arg(1)?);
            let mut target = &mut *state;
            for part in key.split('.') {
                target = &mut target[part];
            }
            *target = match value.parse::<f64>() {
                Ok(n) => json!(n),
                Err(_) => json!(value),
            };
        }
        // ---- option 2: task commands. Validated in `Task::run`; applying
        // an accepted task is pure, so the log replays without the data.
        "bars" | "points" => {
            let (x, y) = if call.name == "bars" {
                (call.flag("by").unwrap_or_default(), call.arg(0)?)
            } else {
                (
                    call.flag("x").unwrap_or_default(),
                    call.flag("y").unwrap_or_default(),
                )
            };
            state["mark"] = json!(if call.name == "bars" { "bar" } else { "symbol" });
            state["x"]["field"] = json!(x);
            state["y"]["field"] = json!(y);
            if let Some(v) = call.flag("size") {
                state["size"] = json!(v);
            }
        }
        "title" => state["title"] = json!(call.arg(0)?),
        "color" => state["fill"] = json!(call.arg(0)?),
        "rotate-labels" => {
            let angle: f64 = call
                .flag("angle")
                .and_then(|a| a.parse().ok())
                .unwrap_or(-45.0);
            state[call.arg(0)?]["labelAngle"] = json!(angle);
        }
        "zoom" => {
            for (i, channel) in ["x", "y"].iter().enumerate() {
                if let Some(r) = call.args.get(i) {
                    let (lo, hi) = range(r)?;
                    state[*channel]["domain"] = json!(format!("{lo},{hi}"));
                }
            }
        }
        "reset-zoom" => {
            for channel in ["x", "y"] {
                if let Some(o) = state[channel].as_object_mut() {
                    o.remove("domain");
                }
            }
        }
        "highlight" => {
            state["highlight"] = json!({
                "where": call.arg(0)?,
                "color": call.flag("color").unwrap_or("#f28e2b"),
            });
        }
        other => return Err(err(format!("`{other}` is not a chart command"))),
    }
    Ok(())
}

/// `a..b` as two numbers.
fn range(s: &str) -> Result<(f64, f64)> {
    let parts: Vec<&str> = s.split("..").collect();
    match parts.as_slice() {
        [a, b] => match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
            (Ok(a), Ok(b)) => Ok((a, b)),
            _ => Err(err(format!("`{s}` is not a range like 657500..658000"))),
        },
        _ => Err(err(format!("`{s}` is not a range like 657500..658000"))),
    }
}

/// Rebuild the state from a command log.
pub fn fold(log: &[String]) -> Result<Value> {
    let mut state = json!({});
    for line in log {
        for call in parse_pipeline(line)? {
            apply(&mut state, &call)?;
        }
    }
    Ok(state)
}

struct Command;

#[async_trait]
impl Step for Command {
    fn kind(&self) -> Kind {
        Kind::Command
    }
    fn help(&self) -> &'static str {
        "chart symbol|bar --x f --y f [--fill css] [--size n] · set <key> <value>: change the chart"
    }
    async fn run(&self, p: &mut Pipeline, c: &Call) -> Result<Option<String>> {
        apply(&mut p.chart, c)?;
        Ok(None)
    }
}

struct Undo;

#[async_trait]
impl Step for Undo {
    fn kind(&self) -> Kind {
        Kind::Command
    }
    fn help(&self) -> &'static str {
        "undo: drop the last command and refold the log"
    }
    async fn run(&self, p: &mut Pipeline, _c: &Call) -> Result<Option<String>> {
        let dropped = p.log.pop().ok_or_else(|| err("undo: nothing to undo"))?;
        p.chart = fold(&p.log)?;
        Ok(Some(format!("undid: {dropped}")))
    }
}

struct Get;

#[async_trait]
impl Step for Get {
    fn kind(&self) -> Kind {
        Kind::Query
    }
    fn help(&self) -> &'static str {
        "get [key]: read the chart state"
    }
    async fn run(&self, p: &mut Pipeline, c: &Call) -> Result<Option<String>> {
        let mut v = &p.chart;
        if let Some(key) = c.args.first() {
            for part in key.split('.') {
                v = &v[part];
            }
        }
        Ok(Some(v.to_string()))
    }
}

struct History;

#[async_trait]
impl Step for History {
    fn kind(&self) -> Kind {
        Kind::Query
    }
    fn help(&self) -> &'static str {
        "history: the command log"
    }
    async fn run(&self, p: &mut Pipeline, _c: &Call) -> Result<Option<String>> {
        Ok(Some(
            p.log
                .iter()
                .enumerate()
                .map(|(i, l)| format!("{i}: {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ))
    }
}

/// The domain a channel will have: set explicitly, or the data's extent.
/// Vega's `domain('x')`, answered by a query.
async fn domain(p: &Pipeline, channel: &str) -> Result<(f64, f64)> {
    if let Some(d) = p.chart[channel]["domain"].as_str() {
        let v: Vec<f64> = d.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if v.len() == 2 {
            return Ok((v[0], v[1]));
        }
    }
    let field = p.chart[channel]["field"]
        .as_str()
        .ok_or_else(|| err(format!("no {channel} field: use `chart` first")))?
        .to_string();
    let (lo, hi) = data_extent(p, &field).await?;
    Ok(if p.chart["mark"] == "bar" && channel == "y" {
        (lo.min(0.0), hi)
    } else {
        (lo, hi)
    })
}

/// The data's extent for a field, whatever the chart state says.
async fn data_extent(p: &Pipeline, field: &str) -> Result<(f64, f64)> {
    let batches = p
        .dataframe()?
        .aggregate(
            vec![],
            vec![min(col(field)).alias("lo"), max(col(field)).alias("hi")],
        )?
        .collect()
        .await?;
    let get = |i: usize| {
        let a = arrow::compute::cast(batches[0].column(i), &arrow::datatypes::DataType::Float64)
            .unwrap();
        arrow::array::AsArray::as_primitive::<arrow::datatypes::Float64Type>(&a).value(0)
    };
    Ok((get(0), get(1)))
}

fn field_names(p: &Pipeline) -> Result<Vec<String>> {
    Ok(p.plan()?
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().to_string())
        .collect())
}

/// Option 2: a task command. Each task checks its intent against the data
/// before it is accepted; a rejected task is not logged.
struct Task;

#[async_trait]
impl Step for Task {
    fn kind(&self) -> Kind {
        Kind::Command
    }
    fn help(&self) -> &'static str {
        "bars <y> --by <x> · points --x f --y f · title <text> · color <css> · rotate-labels x|y [--angle] · zoom <x0..x1> [<y0..y1>] · reset-zoom · highlight <vega predicate>"
    }
    async fn run(&self, p: &mut Pipeline, c: &Call) -> Result<Option<String>> {
        let fields = field_names(p)?;
        let known = |f: &str| {
            if fields.iter().any(|x| x == f) {
                Ok(())
            } else {
                Err(err(format!(
                    "{}: no field `{f}` in the data (fields: {})",
                    c.name,
                    fields.join(", ")
                )))
            }
        };
        match c.name.as_str() {
            "bars" => {
                known(c.arg(0)?)?;
                known(
                    c.flag("by")
                        .ok_or_else(|| err("bars needs --by <category field>"))?,
                )?;
            }
            "points" => {
                known(c.flag("x").ok_or_else(|| err("points needs --x"))?)?;
                known(c.flag("y").ok_or_else(|| err("points needs --y"))?)?;
            }
            "title" if c.arg(0)?.trim().is_empty() => return Err(err("title: empty")),
            "color" => {
                let v = c.arg(0)?;
                let hex = v.len() == 7
                    && v.starts_with('#')
                    && v[1..].chars().all(|h| h.is_ascii_hexdigit());
                if !hex {
                    return Err(err(format!("color: `{v}` is not a #rrggbb colour")));
                }
            }
            "rotate-labels" => {
                if !matches!(c.arg(0)?, "x" | "y") {
                    return Err(err("rotate-labels: axis is x or y"));
                }
                let a: f64 = c
                    .flag("angle")
                    .and_then(|a| a.parse().ok())
                    .unwrap_or(-45.0);
                if !(-90.0..=90.0).contains(&a) {
                    return Err(err("rotate-labels: angle between -90 and 90"));
                }
            }
            "zoom" => {
                // A zoom must be ordered and must show some data.
                for (i, channel) in ["x", "y"].iter().enumerate() {
                    let Some(r) = c.args.get(i) else { continue };
                    let (lo, hi) = range(r)?;
                    if lo >= hi {
                        return Err(err(format!("zoom: {channel} range {r} is empty")));
                    }
                    let field = p.chart[*channel]["field"]
                        .as_str()
                        .ok_or_else(|| err("zoom: draw something first (bars or points)"))?
                        .to_string();
                    let (dlo, dhi) = data_extent(p, &field).await?;
                    if hi < dlo || lo > dhi {
                        return Err(err(format!(
                            "zoom: {channel} range {r} holds no data ({field} runs {dlo}..{dhi})"
                        )));
                    }
                }
            }
            "highlight" => {
                // The predicate must compile against the data (Vega stays Vega).
                p.vega(c.arg(0)?)?;
            }
            _ => {}
        }
        apply(&mut p.chart, c)?;
        Ok(None)
    }
}

/// Option 2's read model: what the chart shows, answered as one document
/// (a DTO), rather than the internal state by key.
struct Describe;

#[async_trait]
impl Step for Describe {
    fn kind(&self) -> Kind {
        Kind::Query
    }
    fn help(&self) -> &'static str {
        "describe: what the chart shows (mark, fields, domains, rows, highlight)"
    }
    async fn run(&self, p: &mut Pipeline, _c: &Call) -> Result<Option<String>> {
        let s = &p.chart;
        let mark = s["mark"]
            .as_str()
            .ok_or_else(|| err("describe: no chart yet"))?;
        let mut out = json!({
            "mark": if mark == "bar" { "bars" } else { "points" },
            "title": s["title"],
            "rows": p.dataframe()?.count().await?,
        });
        for channel in ["x", "y"] {
            let field = s[channel]["field"].as_str().unwrap_or_default();
            let numeric = !(mark == "bar" && channel == "x");
            out[channel] = json!({"field": field});
            if numeric {
                let (lo, hi) = domain(p, channel).await?;
                out[channel]["domain"] = json!([lo, hi]);
                out[channel]["zoomed"] = json!(s[channel]["domain"].is_string());
            }
            if let Some(a) = s[channel]["labelAngle"].as_f64() {
                out[channel]["label_angle"] = json!(a);
            }
        }
        if let Some(pred) = s["highlight"]["where"].as_str() {
            let n = p.dataframe()?.filter(p.vega(pred)?)?.count().await?;
            out["highlight"] = json!({"where": pred, "rows": n});
        }
        Ok(Some(out.to_string()))
    }
}

struct DomainQuery;

#[async_trait]
impl Step for DomainQuery {
    fn kind(&self) -> Kind {
        Kind::Query
    }
    fn help(&self) -> &'static str {
        "domain x|y: the domain the chart will use (explicit, or the data's extent)"
    }
    async fn run(&self, p: &mut Pipeline, c: &Call) -> Result<Option<String>> {
        let (lo, hi) = domain(p, c.arg(0)?).await?;
        Ok(Some(format!("[{lo}, {hi}]")))
    }
}

struct Render;

#[async_trait]
impl Step for Render {
    fn kind(&self) -> Kind {
        Kind::Sink
    }
    fn help(&self) -> &'static str {
        "render <path.png> [--definition <path.avc>]: materialise, build the chart definition, render"
    }
    async fn run(&self, p: &mut Pipeline, c: &Call) -> Result<Option<String>> {
        let path = c.arg(0)?.to_string();
        let (definition, rows) = build_definition(p).await?;
        if let Some(out) = c.flag("definition") {
            let bytes = definition.to_bytes().map_err(|e| err(e.to_string()))?;
            std::fs::write(out, &bytes)?;
        }
        let bytes = render_blocking(definition)?;
        std::fs::write(&path, &bytes)?;
        Ok(Some(format!(
            "rendered {path} from {rows} rows ({} bytes)",
            bytes.len()
        )))
    }
}

/// Render on a thread of its own: `avenger-chart`'s PNG export holds GPU
/// state that is not `Send`, so its future cannot cross an async step.
pub fn render_blocking(definition: ChartDefinition) -> Result<Vec<u8>> {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(render_definition(definition))
    })
    .join()
    .map_err(|_| err("render thread panicked"))?
}

/// Load a serialised chart definition (native plans and data snapshots)
/// and render it, with no pipeline and no source file.
pub fn render_avc(bytes: Vec<u8>) -> Result<Vec<u8>> {
    use avenger_chart_definition::dataflow::{Runtime, RuntimeConfig};
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async move {
                let runtime =
                    Runtime::new(RuntimeConfig::default()).map_err(|e| err(e.to_string()))?;
                let definition = ChartDefinition::from_bytes(&bytes, &runtime)
                    .map_err(|e| err(e.to_string()))?;
                let chart = Chart::prepare(
                    definition,
                    ChartOptions {
                        dataflow: Some(runtime),
                        // Numbers on axes need a formatter since avenger#139.
                        text_engine: Some(lidar_common::text_engine().clone()),
                    },
                )
                .await
                .map_err(|e| err(e.to_string()))?;
                let frame = chart
                    .render(RenderOptions::default())
                    .await
                    .map_err(|e| err(e.to_string()))?;
                frame.to_png(2.0).await.map_err(|e| err(e.to_string()))
            })
    })
    .join()
    .map_err(|_| err("render thread panicked"))?
}

/// Render a chart definition to PNG bytes.
pub async fn render_definition(definition: ChartDefinition) -> Result<Vec<u8>> {
    let chart = Chart::prepare(definition, ChartOptions { text_engine: Some(lidar_common::text_engine().clone()), ..Default::default() })
        .await
        .map_err(|e| err(e.to_string()))?;
    let frame = chart
        .render(RenderOptions::default())
        .await
        .map_err(|e| err(e.to_string()))?;
    frame.to_png(2.0).await.map_err(|e| err(e.to_string()))
}

/// Executed batches can disagree with the plan, and with each other, on
/// nullability (a UNION ALL of an aggregate and a literal row, for example).
/// A snapshot needs one schema: all fields nullable, every batch rebuilt on it.
fn uniform(
    planned: arrow::datatypes::SchemaRef,
    batches: Vec<arrow::array::RecordBatch>,
) -> Result<(arrow::datatypes::SchemaRef, Vec<arrow::array::RecordBatch>)> {
    let fields: Vec<arrow::datatypes::Field> = planned
        .fields()
        .iter()
        .map(|f| f.as_ref().clone().with_nullable(true))
        .collect();
    let schema = Arc::new(arrow::datatypes::Schema::new(fields));
    let batches = batches
        .into_iter()
        .map(|b| arrow::array::RecordBatch::try_new(schema.clone(), b.columns().to_vec()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok((schema, batches))
}

/// The sink's work: the data becomes one snapshot, the chart state becomes
/// scales, a mark and axes.
pub async fn build_definition(p: &Pipeline) -> Result<(ChartDefinition, usize)> {
    let s = &p.chart;
    let mark = s["mark"]
        .as_str()
        .ok_or_else(|| err("render: no chart yet, use `chart` first"))?;
    let (xf, yf) = (
        s["x"]["field"].as_str().unwrap().to_string(),
        s["y"]["field"].as_str().unwrap().to_string(),
    );
    // Materialise once; the domains come from this snapshot, not from
    // re-running the lazy plan.
    let df = p.dataframe()?;
    let planned = Arc::new(df.schema().as_arrow().clone());
    let (schema, batches) = uniform(planned, df.collect().await?)?;
    let rows = batches.iter().map(|b| b.num_rows()).sum();
    let extent = |channel: &str, field: &str| -> (f64, f64) {
        if let Some(d) = s[channel]["domain"].as_str() {
            let v: Vec<f64> = d.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if v.len() == 2 {
                return (v[0], v[1]);
            }
        }
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for b in &batches {
            if let Some(a) = b.column_by_name(field) {
                if let Ok(a) = arrow::compute::cast(a, &arrow::datatypes::DataType::Float64) {
                    let a =
                        arrow::array::AsArray::as_primitive::<arrow::datatypes::Float64Type>(&a);
                    for v in a.iter().flatten().filter(|v| v.is_finite()) {
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                }
            }
        }
        if mark == "bar" && channel == "y" {
            (lo.min(0.0), hi)
        } else {
            (lo, hi)
        }
    };
    let (xd, yd) = (extent("x", &xf), extent("y", &yf));
    let e = |x: avenger_chart_definition::dataflow::Error| err(x.to_string());
    let mut flow = DataflowBuilder::new();
    let source = flow
        .table_snapshot(
            "data",
            TableSnapshot::from_batches(schema, batches).map_err(e)?,
        )
        .map_err(e)?;
    let table = flow.table_output("rows", &source).map_err(e)?;
    // `highlight`: the rows matching a Vega predicate, drawn again on top.
    let highlight = match s["highlight"]["where"].as_str() {
        Some(pred) => {
            let hl = p.dataframe()?.filter(p.vega(pred)?)?;
            let hl_planned = Arc::new(hl.schema().as_arrow().clone());
            let (hl_schema, hl_batches) = uniform(hl_planned, hl.collect().await?)?;
            let node = flow
                .table_snapshot(
                    "highlight",
                    TableSnapshot::from_batches(hl_schema, hl_batches).map_err(e)?,
                )
                .map_err(e)?;
            let color = s["highlight"]["color"]
                .as_str()
                .unwrap_or("#f28e2b")
                .to_string();
            Some((flow.table_output("highlighted", &node).map_err(e)?, color))
        }
        None => None,
    };
    let mut chart = ChartDefinition::builder(flow.finish().map_err(e)?);
    if let Some(t) = s["title"].as_str() {
        chart.title(t);
    }
    let (w, h) = (
        s["width"].as_f64().unwrap_or(420.0) as f32,
        s["height"].as_f64().unwrap_or(320.0) as f32,
    );
    let text = |k: &str, default: &str| s[k]["title"].as_str().unwrap_or(default).to_string();
    let (x_title, y_title) = (text("x", &xf), text("y", &yf));
    let angle = |k: &str| s[k]["labelAngle"].as_f64().map(|a| a as f32);
    let axis = |a: Axis, k: &str, title: &str| {
        let a = a.title(title.to_string());
        match angle(k) {
            Some(deg) => a.label_angle(deg),
            None => a,
        }
    };
    let fill = s["fill"].as_str().unwrap_or("#4c78a8").to_string();
    let size = s["size"]
        .as_f64()
        .or_else(|| s["size"].as_str().and_then(|v| v.parse().ok()))
        .unwrap_or(9.0);
    chart
        .plot("main", |plot| {
            plot.content_size(w, h);
            if mark == "bar" {
                let x = plot.scale(
                    "x",
                    Scale::band(Domain::column(&table, &xf), Range::PlotWidth).padding_inner(0.15),
                )?;
                let y = plot.scale(
                    "y",
                    Scale::linear(Domain::numeric(yd.0, yd.1), Range::PlotHeightReversed),
                )?;
                plot.rect(
                    "bars",
                    &table,
                    RectEncoding::new()
                        .x(x.field(&xf))
                        .width(x.bandwidth())
                        .y(y.field(&yf))
                        .y2(y.constant(0.0))
                        .fill(fill.clone()),
                )?;
                if let Some((hl, color)) = &highlight {
                    plot.rect(
                        "highlight",
                        hl,
                        RectEncoding::new()
                            .x(x.field(&xf))
                            .width(x.bandwidth())
                            .y(y.field(&yf))
                            .y2(y.constant(0.0))
                            .fill(color.clone()),
                    )?;
                }
                plot.axis(axis(Axis::bottom(&x), "x", &x_title))?;
                plot.axis(axis(Axis::left(&y), "y", &y_title))?;
            } else {
                let x = plot.scale(
                    "x",
                    Scale::linear(Domain::numeric(xd.0, xd.1), Range::PlotWidth),
                )?;
                let y = plot.scale(
                    "y",
                    Scale::linear(Domain::numeric(yd.0, yd.1), Range::PlotHeightReversed),
                )?;
                plot.symbol(
                    "points",
                    &table,
                    SymbolEncoding::new()
                        .x(x.field(&xf))
                        .y(y.field(&yf))
                        .size(size)
                        .fill(fill.clone()),
                )?;
                if let Some((hl, color)) = &highlight {
                    plot.symbol(
                        "highlight",
                        hl,
                        SymbolEncoding::new()
                            .x(x.field(&xf))
                            .y(y.field(&yf))
                            .size(size)
                            .fill(color.clone()),
                    )?;
                }
                plot.axis(axis(Axis::bottom(&x), "x", &x_title))?;
                plot.axis(axis(Axis::left(&y), "y", &y_title))?;
            }
            Ok(())
        })
        .map_err(|x| err(x.to_string()))?;
    Ok((chart.finish().map_err(|x| err(x.to_string()))?, rows))
}

pub fn package() -> Package {
    Package {
        name: "chart",
        functions: vec![],
        steps: vec![
            ("chart", Arc::new(Command)),
            ("set", Arc::new(Command)),
            ("undo", Arc::new(Undo)),
            ("get", Arc::new(Get)),
            ("history", Arc::new(History)),
            ("domain", Arc::new(DomainQuery)),
            ("render", Arc::new(Render)),
            // option 2: task commands and a read model
            ("bars", Arc::new(Task)),
            ("points", Arc::new(Task)),
            ("title", Arc::new(Task)),
            ("color", Arc::new(Task)),
            ("rotate-labels", Arc::new(Task)),
            ("zoom", Arc::new(Task)),
            ("reset-zoom", Arc::new(Task)),
            ("highlight", Arc::new(Task)),
            ("describe", Arc::new(Describe)),
        ],
    }
}
