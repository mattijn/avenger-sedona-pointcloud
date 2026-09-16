//! Scatterplot of an IGN LiDAR HD tile: SedonaDB's LAS/LAZ reader on
//! DataFusion feeds Arrow arrays straight into Avenger scales, axes and the
//! offscreen wgpu renderer.
//!
//! Usage: cargo run --release --bin cross_section -- <tile.copc.laz> <out.png>

use std::sync::Arc;
use std::time::Instant;

use arrow::array::{ArrayRef, AsArray};
use arrow::compute::concat;
use arrow::datatypes::UInt8Type;
use avenger_color::ColorOrGradient;
use avenger_common::canvas::CanvasDimensions;
use avenger_common::types::SymbolShape;
use avenger_guides::axis::numeric::make_numeric_axis_marks;
use avenger_guides::axis::opts::{AxisConfig, AxisOrientation};
use avenger_guides::legend::symbol::{make_symbol_legend, SymbolLegendConfig};
use avenger_scales::scales::linear::LinearScale;
use avenger_scenegraph::marks::group::{Clip, SceneGroup};
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::symbol::SceneSymbolMark;
use avenger_scenegraph::marks::text::SceneTextMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_text::types::{FontWeight, FontWeightNameSpec, TextAlign, TextBaseline};
use avenger_wgpu::canvas::{Canvas, PngCanvas};
use datafusion::execution::SessionStateBuilder;
use datafusion::prelude::{SessionConfig, SessionContext};
use sedona_pointcloud::las::format::{Extension, LasFormatFactory};
use sedona_pointcloud::las::options::LasOptions;

/// ASPRS classes present in LiDAR HD, with display colours.
const CLASSES: [(u8, &str, [f32; 4]); 5] = [
    (2, "Ground", [0.55, 0.43, 0.30, 1.0]),
    (3, "Low vegetation", [0.72, 0.84, 0.40, 1.0]),
    (4, "Medium vegetation", [0.35, 0.68, 0.33, 1.0]),
    (5, "High vegetation", [0.13, 0.43, 0.22, 1.0]),
    (6, "Building", [0.80, 0.25, 0.25, 1.0]),
];
const OTHER: [f32; 4] = [0.62, 0.64, 0.68, 1.0];

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

fn column(batches: &[arrow::record_batch::RecordBatch], i: usize) -> ArrayRef {
    let parts: Vec<&dyn arrow::array::Array> =
        batches.iter().map(|b| b.column(i).as_ref()).collect();
    concat(&parts).unwrap()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (tile, out) = (&args[1], &args[2]);

    // ---- data: a 2 m wide west–east strip through the tile ----------------
    let ctx = context();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?;
    let (y0, y1) = (6_867_400.0, 6_867_402.0);
    let sql = format!(
        "SELECT CAST(x - 657000 AS FLOAT) AS distance,
                CAST(z AS FLOAT)          AS elevation,
                classification
         FROM '{tile}'
         WHERE y >= {y0} AND y < {y1}
         ORDER BY classification"
    );
    let t = Instant::now();
    let batches = ctx.sql(&sql).await?.collect().await?;
    let n: usize = batches.iter().map(|b| b.num_rows()).sum();
    println!("query: {n} points in {:.2?}", t.elapsed());
    let distance = column(&batches, 0);
    let elevation = column(&batches, 1);
    let class = column(&batches, 2);

    // ---- chart --------------------------------------------------------------
    let (width, height) = (820.0f32, 300.0f32);
    let x_scale = LinearScale::configured((0.0, 1000.0), (0.0, width));
    let y_scale = LinearScale::configured((35.0, 100.0), (height, 0.0)).with_option("nice", true);

    // Categorical colours from the Arrow classification column.
    let fill: Vec<ColorOrGradient> = class
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
        .collect();

    let points = SceneSymbolMark {
        len: n as u32,
        x: x_scale.scale_to_numeric(&distance)?,
        y: y_scale.scale_to_numeric(&elevation)?,
        fill: fill.into(),
        size: 3.0.into(),
        shapes: vec![SymbolShape::Circle],
        ..Default::default()
    };
    let plot = SceneGroup {
        marks: vec![points.into()],
        clip: Clip::Rect {
            x: 0.5,
            y: 0.5,
            width: width - 1.0,
            height: height - 1.0,
        },
        ..Default::default()
    };

    let y_axis = make_numeric_axis_marks(
        &y_scale,
        "Elevation (m, IGN69)",
        [0.0, 0.0],
        &AxisConfig {
            dimensions: [width, height],
            orientation: AxisOrientation::Left,
            grid: true,
            ..Default::default()
        },
    )?;
    let x_axis = make_numeric_axis_marks(
        &x_scale,
        "Distance east of 657 000 m (Lambert-93)",
        [0.0, 0.0],
        &AxisConfig {
            dimensions: [width, height],
            orientation: AxisOrientation::Bottom,
            grid: false,
            ..Default::default()
        },
    )?;

    let mut labels: Vec<String> = CLASSES.iter().map(|(_, l, _)| l.to_string()).collect();
    labels.push("Other".into());
    let mut swatches: Vec<ColorOrGradient> = CLASSES
        .iter()
        .map(|(_, _, c)| ColorOrGradient::Color(*c))
        .collect();
    swatches.push(ColorOrGradient::Color(OTHER));
    let legend = make_symbol_legend(&SymbolLegendConfig {
        title: None, // stack places legend titles inside the plot; drawn below instead
        text: labels.into(),
        shape: SymbolShape::Circle.into(),
        size: 60.0.into(),
        fill: swatches.into(),
        stroke: ColorOrGradient::Color([0.0, 0.0, 0.0, 0.0]).into(),
        inner_width: width,
        inner_height: height,
        ..Default::default()
    })?;

    let title = SceneTextMark {
        len: 2,
        text: vec![
            "Paris LiDAR HD cross-section".to_string(),
            format!("{n} points with {y0:.0} ≤ y < {y1:.0} · tile LHD_FXX_0657_6868 · read with sedona-pointcloud + DataFusion"),
        ]
        .into(),
        x: 0.0.into(),
        y: vec![-34.0, -14.0].into(),
        font_size: vec![17.0, 11.0].into(),
        font_weight: vec![
            FontWeight::Name(FontWeightNameSpec::Bold),
            FontWeight::Name(FontWeightNameSpec::Normal),
        ]
        .into(),
        color: vec![
            ColorOrGradient::Color([0.1, 0.12, 0.15, 1.0]),
            ColorOrGradient::Color([0.38, 0.42, 0.47, 1.0]),
        ]
        .into(),
        align: TextAlign::Left.into(),
        baseline: TextBaseline::Alphabetic.into(),
        ..Default::default()
    };

    let legend_title = SceneTextMark {
        len: 1,
        text: "LiDAR class".into(),
        x: (width + 16.0).into(),
        y: (-6.0f32).into(),
        font_size: 12.0.into(),
        font_weight: FontWeight::Name(FontWeightNameSpec::Bold).into(),
        color: ColorOrGradient::Color([0.1, 0.12, 0.15, 1.0]).into(),
        baseline: TextBaseline::Alphabetic.into(),
        ..Default::default()
    };

    let chart = SceneGroup {
        origin: [70.0, 60.0],
        marks: vec![
            y_axis.into(),
            x_axis.into(),
            plot.into(),
            legend.into(),
            legend_title.into(),
            title.into(),
        ],
        ..Default::default()
    };
    let scene = SceneGraph {
        marks: vec![SceneMark::Group(chart)],
        width: 1060.0,
        height: 420.0,
        origin: [0.0; 2],
    };

    // ---- render -------------------------------------------------------------
    let t = Instant::now();
    let mut canvas = PngCanvas::new(
        CanvasDimensions {
            size: [scene.width, scene.height],
            scale: 2.0,
        },
        Default::default(),
    )
    .await?;
    canvas.set_scene(&scene)?;
    let img = canvas.render().await?;
    img.save(out)?;
    println!("render: {:.2?} -> {out}", t.elapsed());
    Ok(())
}
