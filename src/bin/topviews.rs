//! Top-view scatterplots of an IGN LiDAR HD tile, queried with
//! sedona-pointcloud + DataFusion and rendered with Avenger.
//!
//! Usage: cargo run --release --bin topviews -- <tile.copc.laz> <out_dir>

use std::sync::Arc;
use std::time::Instant;

use arrow::array::{Array, ArrayRef, AsArray, Float32Array, RecordBatch};
use arrow::compute::concat;
use arrow::datatypes::{Float32Type, UInt8Type};
use avenger_color::ColorOrGradient;
use avenger_common::canvas::CanvasDimensions;
use avenger_common::types::SymbolShape;
use avenger_common::value::ScalarOrArray;
use avenger_guides::axis::numeric::make_numeric_axis_marks;
use avenger_guides::axis::opts::{AxisConfig, AxisOrientation};
use avenger_guides::legend::symbol::{make_symbol_legend, SymbolLegendConfig};
use avenger_scales::scales::linear::LinearScale;
use avenger_scales::scales::ConfiguredScale;
use avenger_scenegraph::marks::group::{Clip, SceneGroup};
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::rect::SceneRectMark;
use avenger_scenegraph::marks::rule::SceneRuleMark;
use avenger_scenegraph::marks::symbol::SceneSymbolMark;
use avenger_scenegraph::marks::text::SceneTextMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_text::types::{FontWeight, FontWeightNameSpec, TextAlign, TextBaseline};
use avenger_wgpu::canvas::{Canvas, PngCanvas};
use datafusion::execution::SessionStateBuilder;
use datafusion::prelude::{SessionConfig, SessionContext};
use sedona_pointcloud::las::format::{Extension, LasFormatFactory};
use sedona_pointcloud::las::options::LasOptions;

const CLASSES: [(u8, &str, [f32; 4]); 5] = [
    (2, "Ground", [0.55, 0.43, 0.30, 1.0]),
    (3, "Low vegetation", [0.72, 0.84, 0.40, 1.0]),
    (4, "Medium vegetation", [0.35, 0.68, 0.33, 1.0]),
    (5, "High vegetation", [0.13, 0.43, 0.22, 1.0]),
    (6, "Building", [0.80, 0.25, 0.25, 1.0]),
];
const OTHER: [f32; 4] = [0.62, 0.64, 0.68, 1.0];
const INK: [f32; 4] = [0.1, 0.12, 0.15, 1.0];
const MUTED: [f32; 4] = [0.38, 0.42, 0.47, 1.0];
const PROFILE_Y: f32 = 400.0; // the cross-section from the first chart

fn context() -> SessionContext {
    let config = SessionConfig::new().with_option_extension(LasOptions::default());
    let mut state = SessionStateBuilder::new()
        .with_config(config)
        .with_default_features()
        .build();
    state
        .register_file_format(Arc::new(LasFormatFactory::new(Extension::Laz)), true)
        .unwrap();
    SessionContext::new_with_state(state).enable_url_table()
}

fn column(batches: &[RecordBatch], i: usize) -> ArrayRef {
    let parts: Vec<&dyn Array> = batches.iter().map(|b| b.column(i).as_ref()).collect();
    concat(&parts).unwrap()
}

fn class_colors(class: &ArrayRef) -> Vec<ColorOrGradient> {
    class
        .as_primitive::<UInt8Type>()
        .values()
        .iter()
        .map(|c| {
            let rgba = CLASSES
                .iter()
                .find(|(k, _, _)| k == c)
                .map(|(_, _, rgba)| *rgba);
            ColorOrGradient::Color(rgba.unwrap_or(OTHER))
        })
        .collect()
}

/// Linear colour scale from `domain`, with `colors` spread evenly across it.
fn ramp(domain: (f32, f32), colors: &[&str]) -> ConfiguredScale {
    LinearScale::configured_color(
        domain,
        colors.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
    )
    .with_option("clamp", true)
}

fn percentile(values: &ArrayRef, p: f32) -> f32 {
    let mut v: Vec<f32> = values.as_primitive::<Float32Type>().values().to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f32 * p) as usize]
}

enum Legend {
    Classes,
    Colorbar(ConfiguredScale, &'static str, (f32, f32)),
}

struct Panel<'a> {
    title: String,
    subtitle: String,
    x: &'a ArrayRef,
    y: &'a ArrayRef,
    fill: ScalarOrArray<ColorOrGradient>,
    size: f32,
    /// Data window (relative metres) shown in the plot.
    window: [f32; 4],
    x_title: &'a str,
    y_title: &'a str,
    legend: Legend,
    profile_line: bool,
    line_color: [f32; 4],
}

fn text(lines: Vec<(String, f32, f32, f32, bool, [f32; 4])>) -> SceneTextMark {
    SceneTextMark {
        len: lines.len() as u32,
        text: lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().into(),
        x: lines.iter().map(|l| l.1).collect::<Vec<_>>().into(),
        y: lines.iter().map(|l| l.2).collect::<Vec<_>>().into(),
        font_size: lines.iter().map(|l| l.3).collect::<Vec<_>>().into(),
        font_weight: lines
            .iter()
            .map(|l| {
                FontWeight::Name(if l.4 {
                    FontWeightNameSpec::Bold
                } else {
                    FontWeightNameSpec::Normal
                })
            })
            .collect::<Vec<_>>()
            .into(),
        color: lines
            .iter()
            .map(|l| ColorOrGradient::Color(l.5))
            .collect::<Vec<_>>()
            .into(),
        align: TextAlign::Left.into(),
        baseline: TextBaseline::Alphabetic.into(),
        ..Default::default()
    }
}

fn build(p: Panel) -> Result<SceneGraph, Box<dyn std::error::Error>> {
    let side = 620.0f32;
    let [x0, x1, y0, y1] = p.window;
    let x_scale = LinearScale::configured((x0, x1), (0.0, side));
    let y_scale = LinearScale::configured((y0, y1), (side, 0.0));

    let mut plot_marks: Vec<SceneMark> = vec![SceneSymbolMark {
        len: p.x.len() as u32,
        x: x_scale.scale_to_numeric(p.x)?,
        y: y_scale.scale_to_numeric(p.y)?,
        fill: p.fill,
        size: p.size.into(),
        shapes: vec![SymbolShape::Circle],
        ..Default::default()
    }
    .into()];
    if p.profile_line {
        let py = y_scale.scale_scalar_to_numeric(&PROFILE_Y.into())?;
        let py = *py.as_iter(1, None).next().unwrap();
        plot_marks.push(
            SceneRuleMark {
                len: 1,
                x: 0.0.into(),
                x2: side.into(),
                y: py.into(),
                y2: py.into(),
                stroke: ColorOrGradient::Color(p.line_color).into(),
                stroke_width: 1.5.into(),
                stroke_dash: Some(vec![6.0f32, 4.0].into()),
                ..Default::default()
            }
            .into(),
        );
        plot_marks.push(
            text(vec![(
                "cross-section".into(),
                6.0,
                py - 5.0,
                11.0,
                true,
                p.line_color,
            )])
            .into(),
        );
    }
    let plot = SceneGroup {
        marks: plot_marks,
        clip: Clip::Rect {
            x: 0.0,
            y: 0.0,
            width: side,
            height: side,
        },
        ..Default::default()
    };

    let axis = |scale: &ConfiguredScale, title: &str, o: AxisOrientation| {
        make_numeric_axis_marks(
            scale,
            title,
            [0.0, 0.0],
            &AxisConfig {
                dimensions: [side, side],
                orientation: o,
                grid: false,
                ..Default::default()
            },
        )
    };
    let y_axis = axis(&y_scale, p.y_title, AxisOrientation::Left)?;
    let x_axis = axis(&x_scale, p.x_title, AxisOrientation::Bottom)?;

    let mut marks: Vec<SceneMark> = vec![plot.into(), y_axis.into(), x_axis.into()];
    match p.legend {
        Legend::Classes => {
            let mut labels: Vec<String> = CLASSES.iter().map(|(_, l, _)| l.to_string()).collect();
            labels.push("Other".into());
            let mut swatches: Vec<ColorOrGradient> = CLASSES
                .iter()
                .map(|(_, _, c)| ColorOrGradient::Color(*c))
                .collect();
            swatches.push(ColorOrGradient::Color(OTHER));
            marks.push(
                make_symbol_legend(&SymbolLegendConfig {
                    title: None,
                    text: labels.into(),
                    shape: SymbolShape::Circle.into(),
                    size: 60.0.into(),
                    fill: swatches.into(),
                    stroke: ColorOrGradient::Color([0.0; 4]).into(),
                    inner_width: side,
                    inner_height: side,
                    ..Default::default()
                })?
                .into(),
            );
            marks.push(
                text(vec![(
                    "LiDAR class".into(),
                    side + 16.0,
                    -6.0,
                    12.0,
                    true,
                    INK,
                )])
                .into(),
            );
        }
        Legend::Colorbar(scale, title, (lo, hi)) => {
            // Stacked rects + an axis: the stack's colorbar is drawn at (0, 0)
            // inside the plot and filled with a single colour.
            let (bar_w, bar_h, steps) = (14.0f32, 260.0f32, 130usize);
            let values: ArrayRef = Arc::new(Float32Array::from(
                (0..steps)
                    .map(|i| lo + (hi - lo) * (i as f32 + 0.5) / steps as f32)
                    .collect::<Vec<_>>(),
            ));
            let step_h = bar_h / steps as f32;
            let bar = SceneRectMark {
                len: steps as u32,
                x: 0.0.into(),
                x2: Some(bar_w.into()),
                y: (0..steps)
                    .map(|i| bar_h - (i as f32 + 1.0) * step_h)
                    .collect::<Vec<_>>()
                    .into(),
                height: Some((step_h + 0.6).into()),
                fill: scale.scale_to_color(&values)?,
                ..Default::default()
            };
            let bar_scale = LinearScale::configured((lo, hi), (bar_h, 0.0));
            let bar_axis = make_numeric_axis_marks(
                &bar_scale,
                title,
                [0.0, 0.0],
                &AxisConfig {
                    dimensions: [bar_w, bar_h],
                    orientation: AxisOrientation::Right,
                    grid: false,
                    ..Default::default()
                },
            )?;
            marks.push(
                SceneGroup {
                    origin: [side + 20.0, 10.0],
                    marks: vec![bar.into(), bar_axis.into()],
                    ..Default::default()
                }
                .into(),
            );
        }
    }
    marks.push(
        text(vec![
            (p.title, 0.0, -34.0, 17.0, true, INK),
            (p.subtitle, 0.0, -14.0, 11.0, false, MUTED),
        ])
        .into(),
    );

    Ok(SceneGraph {
        marks: vec![SceneMark::Group(SceneGroup {
            origin: [78.0, 62.0],
            marks,
            ..Default::default()
        })],
        width: 900.0,
        height: 740.0,
        origin: [0.0; 2],
    })
}

async fn render(scene: &SceneGraph, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let t = Instant::now();
    let mut canvas = PngCanvas::new(
        CanvasDimensions {
            size: [scene.width, scene.height],
            scale: 2.0,
        },
        Default::default(),
    )
    .await?;
    canvas.set_scene(scene)?;
    canvas.render().await?.save(path)?;
    println!("  render {:.2?} -> {path}", t.elapsed());
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (tile, out) = (&args[1], &args[2]);
    let ctx = context();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?;
    let tile_label =
        "tile LHD_FXX_0657_6868 (Greater Paris) · sedona-pointcloud + DataFusion → Avenger";
    let east = "Easting − 657 000 (m, Lambert-93)";
    let north = "Northing − 6 867 000 (m)";

    // ---- 1 m cells: highest point, its class, mean intensity ---------------
    let t = Instant::now();
    let sql = format!(
        "SELECT CAST(floor(x - 657000) + 0.5 AS FLOAT)            AS cx,
                CAST(floor(y - 6867000) + 0.5 AS FLOAT)           AS cy,
                CAST(max(z) AS FLOAT)                             AS zmax,
                first_value(classification ORDER BY z DESC)       AS top_class,
                CAST(avg(intensity) AS FLOAT)                     AS intensity,
                count(*)                                          AS n
         FROM '{tile}'
         GROUP BY 1, 2"
    );
    let cells = ctx.sql(&sql).await?.collect().await?;
    let (cx, cy, zmax, top_class, intensity) = (
        column(&cells, 0),
        column(&cells, 1),
        column(&cells, 2),
        column(&cells, 3),
        column(&cells, 4),
    );
    let n_cells = cx.len();
    println!(
        "cells: {n_cells} 1 m cells from 17.3M points in {:.2?}",
        t.elapsed()
    );
    let full = [0.0, 1000.0, 0.0, 1000.0];
    let cell_size = 1.1; // symbol area in px² for 0.62 px cells, slightly overlapping

    let height_scale = ramp(
        (46.0, 76.0),
        &[
            "#0d0887", "#6a00a8", "#b12a90", "#e16462", "#fca636", "#f0f921",
        ],
    );
    render(
        &build(Panel {
            title: "Surface height, top view".into(),
            subtitle: format!("highest point in each of {n_cells} 1 m cells · {tile_label}"),
            x: &cx,
            y: &cy,
            fill: height_scale.scale_to_color(&zmax)?,
            size: cell_size,
            window: full,
            x_title: east,
            y_title: north,
            legend: Legend::Colorbar(height_scale, "Elevation (m)", (46.0, 76.0)),
            profile_line: true,
            line_color: [1.0, 1.0, 1.0, 1.0],
        })?,
        &format!("{out}/top_height.png"),
    )
    .await?;

    render(
        &build(Panel {
            title: "What the scanner sees from above".into(),
            subtitle: format!("class of the highest point per 1 m cell · {tile_label}"),
            x: &cx,
            y: &cy,
            fill: class_colors(&top_class).into(),
            size: cell_size,
            window: full,
            x_title: east,
            y_title: north,
            legend: Legend::Classes,
            profile_line: false,
            line_color: INK,
        })?,
        &format!("{out}/top_class.png"),
    )
    .await?;

    let (lo, hi) = (percentile(&intensity, 0.02), percentile(&intensity, 0.98));
    let intensity_scale = ramp((lo, hi), &["#111111", "#f5f5f5"]);
    render(
        &build(Panel {
            title: "Return intensity, top view".into(),
            subtitle: format!("mean intensity per 1 m cell, stretched between 2nd and 98th percentile · {tile_label}"),
            x: &cx,
            y: &cy,
            fill: intensity_scale.scale_to_color(&intensity)?,
            size: cell_size,
            window: full,
            x_title: east,
            y_title: north,
            legend: Legend::Colorbar(intensity_scale, "Intensity", (lo, hi)),
            profile_line: false,
            line_color: INK,
        })?,
        &format!("{out}/top_intensity.png"),
    )
    .await?;

    // ---- raw points in a 120 × 120 m window ---------------------------------
    let win = [700.0f32, 820.0, 340.0, 460.0];
    let t = Instant::now();
    let sql = format!(
        "SELECT CAST(x - 657000 AS FLOAT)  AS rx,
                CAST(y - 6867000 AS FLOAT) AS ry,
                classification
         FROM '{tile}'
         WHERE x >= {} AND x < {} AND y >= {} AND y < {}
         ORDER BY z",
        657000.0 + win[0] as f64,
        657000.0 + win[1] as f64,
        6867000.0 + win[2] as f64,
        6867000.0 + win[3] as f64,
    );
    let raw = ctx.sql(&sql).await?.collect().await?;
    let (rx, ry, rc) = (column(&raw, 0), column(&raw, 1), column(&raw, 2));
    println!("zoom: {} raw points in {:.2?}", rx.len(), t.elapsed());
    render(
        &build(Panel {
            title: "Every point in a 120 × 120 m window".into(),
            subtitle: format!(
                "{} raw points ({:.0} pts/m²), drawn low to high · {tile_label}",
                rx.len(),
                rx.len() as f32 / 14_400.0
            ),
            x: &rx,
            y: &ry,
            fill: class_colors(&rc).into(),
            size: 1.4,
            window: win,
            x_title: east,
            y_title: north,
            legend: Legend::Classes,
            profile_line: true,
            line_color: INK,
        })?,
        &format!("{out}/zoom_points.png"),
    )
    .await?;
    Ok(())
}
