//! The `probe_guides` checks, built against Avenger `main` (b5e3bce) with its
//! prepared guides (a colorbar and legend place themselves in a slot). Writes
//! out/probe_guides_main.png.
//!
//! Usage: cargo run --release -p lidar-probes-main --bin probe_guides_main
use lidar_probes_main::make_symbol_legend;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float32Array};
use avenger_color::ColorOrGradient;
use avenger_common::canvas::CanvasDimensions;
use avenger_common::types::SymbolShape;
use lidar_probes_main::make_colorbar_marks;
use avenger_guides::legend::{LegendConfig, LegendSwatches, SymbolSwatches};
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
    let config = lidar_probes_main::colorbar_config();
    // `main` has no origin argument: a colorbar is placed in a slot. The slot
    // is the strip right of the plot, starting at [PLOT + 20, 10].
    let slot = [PLOT + 20.0, 10.0, 120.0, PLOT];
    // Up to f4890be both colour scales below drew a colorbar. Since d5f9738
    // the colorbar asks the scale for a numeric range (to place its axis), and
    // a range of more than two colours has none.
    let swapped = LinearScale::configured((46.0, 76.0), (0.0, 1.0))
        .with_range(Arc::new(arrow::array::StringArray::from(vec![
            "#2c7fb8", "#7fcdbb", "#edf8b1",
        ])) as ArrayRef);
    for (label, scale) in [
        ("configured + with_range, 3 colours", swapped),
        ("configured_color, 3 colours", LinearScale::configured_color((46.0, 76.0), vec!["#2c7fb8", "#7fcdbb", "#edf8b1"])),
        ("configured_color, 2 colours", LinearScale::configured_color((46.0, 76.0), vec!["#2c7fb8", "#edf8b1"])),
    ] {
        match make_colorbar_marks(&scale, "Elevation (m)", slot, PLOT, &config) {
            Ok(_) => println!("COLORBAR {label}: drawn"),
            Err(e) => println!("COLORBAR {label}: FAILS -> {e:?}"),
        }
    }
    let color_scale = LinearScale::configured_color((46.0, 76.0), vec!["#2c7fb8", "#edf8b1"]);
    let colorbar = make_colorbar_marks(&color_scale, "Elevation (m)", slot, PLOT, &config)?;
    println!(
        "\nPITFALL 1: colorbar group origin = {:?} (slot starts at [{}, 10.0])",
        colorbar.origin,
        PLOT + 20.0
    );
    describe("  ", &colorbar.marks);

    // Pitfall 2: symbol legend with a title - where does the title land?
    let legend = make_symbol_legend(&LegendConfig {
        title: Some("LiDAR class".into()),
        text: vec!["Ground".to_string(), "Building".to_string()].into(),
        swatches: LegendSwatches::Symbols(SymbolSwatches {
            shape: SymbolShape::Circle.into(),
            size: 60.0.into(),
            fill: vec![
                ColorOrGradient::Color([0.55, 0.43, 0.30, 1.0]),
                ColorOrGradient::Color([0.80, 0.25, 0.25, 1.0]),
            ]
            .into(),
            stroke: ColorOrGradient::Color([0.0; 4]).into(),
            ..Default::default()
        }),
        ..Default::default()
    }, [PLOT, 0.0, 120.0, PLOT], PLOT)?;
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
    canvas.set_scene(&scene, lidar_probes_main::text_engine())?;
    canvas.render().await?.save("out/probe_guides_main.png")?;
    println!("\nwrote out/probe_guides_main.png");
    Ok(())
}
