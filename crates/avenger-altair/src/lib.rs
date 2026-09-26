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

/// Validate, compile and render: the image's bytes (SVG as UTF-8).
pub fn render(spec: &str, format: Format, scale: f32, base_dir: Option<&str>) -> Result<Vec<u8>, Refusal> {
    render_timed(spec, format, scale, base_dir, &mut Stages::default())
}

pub fn render_timed(spec: &str, format: Format, scale: f32, base_dir: Option<&str>, st: &mut Stages) -> Result<Vec<u8>, Refusal> {
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
    let t = std::time::Instant::now();
    let unit = validate(spec)?;
    st.validate = ms(t);
    // Axis labels need a number formatter; without one, rendering fails
    // ("number formatting is not configured").
    let formatting = avenger_scales::formatter::ScaleFormatting::d3(Default::default(), Default::default());
    let options = VegaLiteOptions {
        base_dir: base_dir.map_or_else(|| ".".into(), Into::into),
        chart: avenger_chart::ChartOptions::default().with_formatting(formatting),
    };
    runtime().block_on(async move {
        let t = std::time::Instant::now();
        let chart = Chart::from_vegalite(&unit, &BTreeMap::new(), options)
            .await
            .map_err(|e| Refusal { path: e.path().to_string(), message: e.message(), stage: "compile" })?;
        st.compile = ms(t);
        let t = std::time::Instant::now();
        let frame = chart.render(RenderOptions::default()).await.map_err(|e| Refusal { path: "$".into(), message: e.to_string(), stage: "render" })?;
        st.render = ms(t);
        let t = std::time::Instant::now();
        let bytes = match format {
            Format::Svg => frame.to_svg().map(String::into_bytes),
            Format::Png => frame.to_png(scale).await,
        };
        st.export = ms(t);
        bytes.map_err(|e| Refusal { path: "$".into(), message: e.to_string(), stage: "render" })
    })
}

#[cfg(feature = "python")]
mod python;
