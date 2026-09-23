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
        other => return Err(err(format!("`{other}` is not a chart command"))),
    }
    Ok(())
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
        .ok_or_else(|| err(format!("no {channel} field: use `chart` first")))?;
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
    let (lo, hi) = (get(0), get(1));
    Ok(if p.chart["mark"] == "bar" && channel == "y" {
        (lo.min(0.0), hi)
    } else {
        (lo, hi)
    })
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
                        ..Default::default()
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
    let chart = Chart::prepare(definition, ChartOptions::default())
        .await
        .map_err(|e| err(e.to_string()))?;
    let frame = chart
        .render(RenderOptions::default())
        .await
        .map_err(|e| err(e.to_string()))?;
    frame.to_png(2.0).await.map_err(|e| err(e.to_string()))
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
    let schema = Arc::new(df.schema().as_arrow().clone());
    let batches = df.collect().await?;
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
        ],
    }
}
