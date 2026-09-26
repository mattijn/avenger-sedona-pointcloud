//! Avenger as a backend for Altair. Altair builds a chart and writes its
//! Vega-Lite; this crate checks that spec with Avenger's own Vega-Lite types
//! (`avenger-vegalite-spec`), compiles it to a native chart definition
//! (`avenger-vegalite-compiler`), and renders it (SVG, PNG). PDF export needs
//! rustc 1.92 (krilla) and is left out on this machine's 1.89. Every
//! refusal names the property it is about, as a path into the spec.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use avenger_chart::{Chart, RenderOptions};
use avenger_vegalite_compiler::{spec::UnitSpec, FromVegaLite, VegaLiteOptions};

/// Why Avenger will not take a spec: where, and what.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    /// A path into the spec, `encoding.x.bin.step`; `$` is the root.
    pub path: String,
    pub message: String,
    /// `spec`: the spec itself is outside what Avenger reads; `compile`: it
    /// reads it, but cannot draw it yet; `render`: drawing failed.
    pub stage: &'static str,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} ({})", self.path, self.message, self.stage)
    }
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("a Tokio runtime"))
}

/// The budget for what active queries may materialise. The charge is
/// conservative and grows faster than the data: a histogram over 3 million
/// rows is charged 17.7 GB while the process peaks 121 MB above where it
/// started (FINDINGS.md 22). Avenger's default, 256 MB, refuses such charts;
/// this one is 64 GiB, and `AVENGER_MAX_MATERIALIZED_BYTES` overrides it.
pub fn budget() -> usize {
    std::env::var("AVENGER_MAX_MATERIALIZED_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(64 << 30)
}

/// A dataflow runtime as `Chart::prepare` builds one, with this budget.
fn dataflow(max_materialized_bytes: usize) -> Result<avenger_datafusion_dataflow::Runtime, Refusal> {
    use avenger_datafusion_dataflow::{datafusion::prelude::SessionContext, ExecutionConfig, Runtime, RuntimeConfig};
    Runtime::with_session_state_and_codec(
        SessionContext::new().state(),
        RuntimeConfig {
            execution: ExecutionConfig { max_materialized_bytes, ..Default::default() },
            function_versions: avenger_transform::function_versions(),
            ..Default::default()
        },
        std::sync::Arc::new(avenger_transform::TransformExtensionCodec::default()),
    )
    .map_err(|e| Refusal { path: "$".into(), message: e.to_string(), stage: "render" })
}

/// Parse and validate a Vega-Lite spec (JSON text) with Avenger's types.
pub fn validate(spec: &str) -> Result<UnitSpec, Refusal> {
    UnitSpec::from_json(spec).map_err(|e| Refusal { path: e.path().to_string(), message: e.message().to_string(), stage: "spec" })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Svg,
    Png,
}

/// Time per stage of the last `render`, in milliseconds.
#[derive(Clone, Debug, Default)]
pub struct Stages {
    pub validate: f64,
    pub compile: f64,
    pub render: f64,
    pub export: f64,
}

/// Tables bound by name (`{"data": {"name": ...}}`), as Arrow; they take
/// precedence over the spec's own `datasets`.
pub type Tables = BTreeMap<String, avenger_datafusion_dataflow::TableSnapshot>;

/// Validate, compile and render: the image's bytes (SVG as UTF-8).
pub fn render(spec: &str, format: Format, scale: f32, base_dir: Option<&str>) -> Result<Vec<u8>, Refusal> {
    render_timed(spec, format, scale, base_dir, &Tables::new(), &mut Stages::default())
}

pub fn render_timed(spec: &str, format: Format, scale: f32, base_dir: Option<&str>, tables: &Tables, st: &mut Stages) -> Result<Vec<u8>, Refusal> {
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
    let t = std::time::Instant::now();
    let unit = validate(spec)?;
    let camera = unit.camera.clone();
    st.validate = ms(t);
    // Axis labels need a number formatter; without one, rendering fails
    // ("number formatting is not configured").
    let formatting = avenger_scales::formatter::ScaleFormatting::d3(Default::default(), Default::default());
    let options = VegaLiteOptions {
        base_dir: base_dir.map_or_else(|| ".".into(), Into::into),
        chart: avenger_chart::ChartOptions { dataflow: Some(dataflow(budget())?), ..Default::default() }.with_formatting(formatting),
    };
    runtime().block_on(async move {
        let t = std::time::Instant::now();
        let chart = Chart::from_vegalite(&unit, tables, options)
            .await
            .map_err(|e| Refusal { path: e.path().to_string(), message: e.message(), stage: "compile" })?;
        st.compile = ms(t);
        let t = std::time::Instant::now();
        let frame = chart.render(RenderOptions::default()).await.map_err(|e| Refusal { path: "$".into(), message: e.to_string(), stage: "render" })?;
        st.render = ms(t);
        let t = std::time::Instant::now();
        let bytes = export(&frame, camera.as_ref(), format, scale).await;
        st.export = ms(t);
        bytes
    })
}

/// The frame's scene as an outline: groups (origin, clip) and marks (kind,
/// name, count), and the plot rectangle. For finding one's way in it.
pub fn scene_outline(spec: &str) -> Result<String, Refusal> {
    use avenger_scenegraph::marks::mark::SceneMark;
    let unit = validate(spec)?;
    let formatting = avenger_scales::formatter::ScaleFormatting::d3(Default::default(), Default::default());
    let options = VegaLiteOptions { chart: avenger_chart::ChartOptions { dataflow: Some(dataflow(budget())?), ..Default::default() }.with_formatting(formatting), ..Default::default() };
    runtime().block_on(async move {
        let chart = Chart::from_vegalite(&unit, &BTreeMap::new(), options).await.map_err(|e| Refusal { path: e.path().to_string(), message: e.message(), stage: "compile" })?;
        let frame = chart.render(RenderOptions::default()).await.map_err(|e| Refusal { path: "$".into(), message: e.to_string(), stage: "render" })?;
        let mut out = String::new();
        let sg = frame.scenegraph();
        out += &format!("scene {}x{}, origin {:?}\n", sg.width, sg.height, sg.origin);
        for p in frame.plots() {
            out += &format!("plot {:?} rect x {} y {} w {} h {}\n", p.path, p.rect.x, p.rect.y, p.rect.width, p.rect.height);
        }
        fn walk(m: &SceneMark, depth: usize, out: &mut String) {
            let pad = "  ".repeat(depth);
            match m {
                SceneMark::Group(g) => {
                    *out += &format!("{pad}group '{}' origin {:?} clip {:?} ({} marks)\n", g.name, g.origin, g.clip, g.marks.len());
                    for c in &g.marks {
                        walk(c, depth + 1, out);
                    }
                }
                SceneMark::Rect(r) => *out += &format!("{pad}rect '{}' len {} x {:?}\n", r.name, r.len, r.x.as_vec(r.len as usize, None).iter().take(3).collect::<Vec<_>>()),
                SceneMark::Rule(r) => *out += &format!("{pad}rule '{}' len {}\n", r.name, r.len),
                SceneMark::Text(t) => *out += &format!("{pad}text '{}' len {}\n", t.name, t.len),
                SceneMark::Path(p) => *out += &format!("{pad}path '{}' len {}\n", p.name, p.len),
                SceneMark::Line(l) => *out += &format!("{pad}line '{}' len {}\n", l.name, l.len),
                SceneMark::Symbol(s) => *out += &format!("{pad}symbol '{}' len {}\n", s.name, s.len),
                other => *out += &format!("{pad}{:?}\n", std::mem::discriminant(other)),
            }
        }
        for m in &sg.marks {
            walk(m, 0, &mut out);
        }
        Ok(out)
    })
}

/// A frame to image bytes, through the camera if there is one (PNG only).
async fn export(frame: &avenger_chart::RenderedChart, camera: Option<&avenger_vegalite_spec::Camera>, format: Format, scale: f32) -> Result<Vec<u8>, Refusal> {
    let refuse = |message: String| Refusal { path: "camera".into(), message, stage: "render" };
    let run = |e: String| Refusal { path: "$".into(), message: e, stage: "render" };
    let plot = frame.plots().first().map(|p| camera::Plot { x: p.rect.x, y: p.rect.y, w: p.rect.width, h: p.rect.height });
    let seen = match (camera, plot) {
        (Some(c), Some(plot)) => camera::apply(frame.scenegraph(), plot, c).map_err(refuse)?,
        _ => None,
    };
    match (seen, format) {
        (None, Format::Svg) => frame.to_svg().map(String::into_bytes).map_err(|e| run(e.to_string())),
        (None, Format::Png) => frame.to_png(scale).await.map_err(|e| run(e.to_string())),
        (Some(scene), Format::Png) => camera::png(&scene, scale).await.map_err(run),
        (Some(_), Format::Svg) => Err(refuse("a camera draws to PNG only".into())),
    }
}

/// A chart compiled once over a named table that grows: `append` adds rows,
/// `render` draws what is there now. After Avenger's `streaming_bars`: the
/// spec's `data.name` becomes a replaceable table input of the dataflow.
pub struct Live {
    chart: Chart,
    camera: Option<avenger_vegalite_spec::Camera>,
    input: avenger_datafusion_dataflow::TableInput,
    store: avenger_datafusion_dataflow::TableStore,
    inputs: avenger_datafusion_dataflow::Inputs,
}

impl Live {
    pub fn new(spec: &str, first: avenger_datafusion_dataflow::TableSnapshot) -> Result<Live, Refusal> {
        let unit = validate(spec)?;
        let name = match &unit.data {
            avenger_vegalite_spec::Data::Named { name, .. } => name.clone(),
            _ => return Err(Refusal { path: "data".into(), message: "a live chart needs {\"name\": ...} data".into(), stage: "spec" }),
        };
        let compile = |e: avenger_vegalite_compiler::CompileError| Refusal { path: e.path().to_string(), message: e.message(), stage: "compile" };
        let run = |e: String| Refusal { path: "$".into(), message: e, stage: "render" };
        let definition = avenger_vegalite_compiler::compile_vegalite_with_input(&unit, first.schema().clone()).map_err(compile)?;
        let input = definition.dataflow().interface().root().table_input(&name).map_err(|e| run(e.to_string()))?;
        let formatting = avenger_scales::formatter::ScaleFormatting::d3(Default::default(), Default::default());
        let options = avenger_chart::ChartOptions { dataflow: Some(dataflow(budget())?), ..Default::default() }.with_formatting(formatting);
        let store = avenger_datafusion_dataflow::TableStore::new(first);
        runtime().block_on(async {
            let chart = Chart::prepare(definition, options).await.map_err(|e| run(e.to_string()))?;
            let builder = chart.inputs().map_err(|e| run(e.to_string()))?;
            let inputs = builder.table(&input, store.snapshot()).and_then(|b| b.finish()).map_err(|e| run(e.to_string()))?;
            Ok(Live { chart, camera: unit.camera.clone(), input, store, inputs })
        })
    }

    /// Add rows; the next render includes them.
    pub fn append(&mut self, batches: Vec<arrow::record_batch::RecordBatch>) -> Result<usize, Refusal> {
        let run = |e: String| Refusal { path: "$".into(), message: e, stage: "render" };
        let snapshot = self.store.append_batches(batches).map_err(|e| run(e.to_string()))?;
        let rows = snapshot.num_rows();
        self.inputs = self.inputs.edit().table(&self.input, snapshot).and_then(|b| b.finish()).map_err(|e| run(e.to_string()))?;
        Ok(rows)
    }

    pub fn rows(&self) -> usize {
        self.store.snapshot().num_rows()
    }

    pub fn render(&self, format: Format, scale: f32) -> Result<Vec<u8>, Refusal> {
        let run = |e: String| Refusal { path: "$".into(), message: e, stage: "render" };
        runtime().block_on(async {
            let frame = self.chart.render(RenderOptions::default().inputs(self.inputs.clone())).await.map_err(|e| run(e.to_string()))?;
            export(&frame, self.camera.as_ref(), format, scale).await
        })
    }
}

pub mod camera;

#[cfg(feature = "python")]
mod python;
