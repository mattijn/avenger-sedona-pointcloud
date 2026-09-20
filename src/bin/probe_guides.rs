//! Re-tests the guide pitfalls listed in the README against whichever Avenger
//! revision this repo is pinned to. Prints where the colorbar and legend marks
//! land and writes out/probe_guides.png.
//!
//! Usage: cargo run --release --bin probe_guides
use std::sync::Arc;

use arrow::array::{ArrayRef, Float32Array};
use avenger_color::ColorOrGradient;
use avenger_common::canvas::CanvasDimensions;
use avenger_common::types::SymbolShape;
use avenger_guides::legend::colorbar::{make_colorbar_marks, ColorbarConfig, ColorbarOrientation};
use avenger_guides::legend::symbol::{make_symbol_legend, SymbolLegendConfig};
use avenger_scales::scales::linear::LinearScale;
use avenger_scenegraph::marks::group::SceneGroup;
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::rect::SceneRectMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_wgpu::canvas::{Canvas, PngCanvas};

const PLOT: f32 = 300.0;

fn describe(prefix: &str, marks: &[SceneMark]) {
    for m in marks {
        match m {
            SceneMark::Group(g) => {
                println!(
                    "{prefix}group origin {:?} ({} marks)",
                    g.origin,
                    g.marks.len()
                );
                describe(&format!("{prefix}  "), &g.marks);
            }
            SceneMark::Text(t) => {
                let xs = t.x.as_vec(t.len as usize, None);
                let ys = t.y.as_vec(t.len as usize, None);
                let txt = t.text.as_vec(t.len as usize, None);
                for i in 0..t.len as usize {
                    println!("{prefix}text {:?} at ({:.1}, {:.1})", txt[i], xs[i], ys[i]);
                }
            }
            SceneMark::Rect(r) => {
                let xs = r.x.as_vec(r.len as usize, None);
                let ys = r.y.as_vec(r.len as usize, None);
                let fills = r.fill.as_vec(r.len as usize, None);
                let kind = match &fills[0] {
                    ColorOrGradient::Color(c) => format!("solid {c:?}"),
                    ColorOrGradient::GradientIndex(i) => format!("gradient index {i}"),
                };
                println!(
                    "{prefix}rect len {} first at ({:.1}, {:.1}) fill {kind}",
                    r.len, xs[0], ys[0]
                );
            }
            other => println!("{prefix}{:?} mark", std::mem::discriminant(other)),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Pitfall 3: can a linear colour scale take more than two domain stops?
    let scale3 = LinearScale::configured((0.0, 1.0), (0.0, 1.0))
        .with_domain(Arc::new(Float32Array::from(vec![0.0f32, 0.5, 1.0])) as ArrayRef);
    let values: ArrayRef = Arc::new(Float32Array::from(vec![0.0f32, 0.25, 0.5, 0.75, 1.0]));
    match scale3.scale_to_color(&values) {
        Ok(_) => println!("PITFALL 3: three-stop linear domain ACCEPTED"),
        Err(e) => println!("PITFALL 3: three-stop linear domain rejected -> {e}"),
    }

    // Pitfall 1: colorbar honours its origin argument? gradient fill in PngCanvas?
    let color_scale = LinearScale::configured((46.0, 76.0), (0.0, 1.0))
        .with_range(Arc::new(arrow::array::StringArray::from(vec![
            "#2c7fb8", "#7fcdbb", "#edf8b1",
        ])) as ArrayRef);
    let colorbar = make_colorbar_marks(
        &color_scale,
        "Elevation (m)",
        [PLOT + 20.0, 10.0], // ask for a spot to the right of the plot
        &ColorbarConfig {
            orientation: ColorbarOrientation::Right,
            dimensions: [PLOT, PLOT],
            ..Default::default()
        },
    )?;
    println!(
        "\nPITFALL 1: colorbar group origin = {:?} (asked for [{}, 10.0])",
        colorbar.origin,
        PLOT + 20.0
    );
    describe("  ", &colorbar.marks);

    // Pitfall 2: symbol legend with a title - where does the title land?
    let legend = make_symbol_legend(&SymbolLegendConfig {
        title: Some("LiDAR class".into()),
        text: vec!["Ground".to_string(), "Building".to_string()].into(),
        shape: SymbolShape::Circle.into(),
        size: 60.0.into(),
        fill: vec![
            ColorOrGradient::Color([0.55, 0.43, 0.30, 1.0]),
            ColorOrGradient::Color([0.80, 0.25, 0.25, 1.0]),
        ]
        .into(),
        stroke: ColorOrGradient::Color([0.0; 4]).into(),
        inner_width: PLOT,
        inner_height: PLOT,
        ..Default::default()
    })?;
    println!(
        "\nPITFALL 2: legend group origin = {:?} (plot is {PLOT} wide)",
        legend.origin
    );
    describe("  ", &legend.marks);

    // Render both next to a plot-area rect so the PNG shows where they land.
    let frame = SceneRectMark {
        len: 1,
        x: 0.0.into(),
        y: 0.0.into(),
        width: Some(PLOT.into()),
        height: Some(PLOT.into()),
        fill: ColorOrGradient::Color([0.93, 0.94, 0.96, 1.0]).into(),
        ..Default::default()
    };
    let scene = SceneGraph {
        marks: vec![SceneGroup {
            origin: [40.0, 30.0],
            marks: vec![frame.into(), colorbar.into(), legend.into()],
            ..Default::default()
        }
        .into()],
        width: 640.0,
        height: 380.0,
        origin: [0.0, 0.0],
    };
    let mut canvas = PngCanvas::new(
        CanvasDimensions {
            size: [640.0, 380.0],
            scale: 2.0,
        },
        Default::default(),
    )
    .await?;
    canvas.set_scene(&scene)?;
    canvas.render().await?.save("out/probe_guides.png")?;
    println!("\nwrote out/probe_guides.png");
    Ok(())
}
