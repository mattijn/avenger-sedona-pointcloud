//! Measures two things that matter for live charts, against whichever Avenger
//! revision this repo is pinned to:
//!
//! 1. whether a gradient fill survives rendering, at the top level and nested
//!    in an offset group;
//! 2. how the cost of installing and drawing a scene grows with the number of
//!    symbol marks.
//!
//! Usage: cargo run --release -p lidar-probes --bin probe_render

use std::time::Instant;

use avenger_color::{ColorOrGradient, Gradient, GradientStop, LinearGradient};
use avenger_common::canvas::CanvasDimensions;
use avenger_common::types::SymbolShape;
use avenger_geometry::rtree::SceneGraphRTree;
use avenger_scenegraph::marks::group::SceneGroup;
use avenger_scenegraph::marks::rect::SceneRectMark;
use avenger_scenegraph::marks::symbol::SceneSymbolMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_wgpu::canvas::{Canvas, PngCanvas};

const W: f32 = 400.0;
const H: f32 = 200.0;

fn gradient_x1(x1: f32) -> Gradient {
    Gradient::LinearGradient(LinearGradient {
        x0: 0.0,
        y0: 0.0,
        x1,
        y1: 0.0,
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: [0.17, 0.50, 0.72, 1.0],
            },
            GradientStop {
                offset: 1.0,
                color: [0.93, 0.97, 0.69, 1.0],
            },
        ],
    })
}

fn gradient() -> Gradient {
    Gradient::LinearGradient(LinearGradient {
        x0: 0.0,
        y0: 0.0,
        // Absolute pixel coordinates, the convention avenger-guides uses.
        x1: 360.0,
        y1: 0.0,
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: [0.17, 0.50, 0.72, 1.0],
            },
            GradientStop {
                offset: 1.0,
                color: [0.93, 0.97, 0.69, 1.0],
            },
        ],
    })
}

fn bar(fill_index: u32) -> SceneRectMark {
    SceneRectMark {
        len: 1,
        x: 20.0.into(),
        y: 20.0.into(),
        width: Some(360.0.into()),
        height: Some(160.0.into()),
        fill: ColorOrGradient::GradientIndex(fill_index).into(),
        ..Default::default()
    }
}

/// Reports whether the drawn bar actually changes colour across its width.
fn gradient_verdict(img: &image::RgbaImage, label: &str) {
    let y = img.height() / 2;
    let sample = |fx: f32| {
        let x = (img.width() as f32 * fx) as u32;
        let p = img.get_pixel(x.min(img.width() - 1), y);
        [p[0], p[1], p[2]]
    };
    let (left, mid, right) = (sample(0.12), sample(0.5), sample(0.88));
    let spread = (left[0] as i32 - right[0] as i32).abs()
        + (left[1] as i32 - right[1] as i32).abs()
        + (left[2] as i32 - right[2] as i32).abs();
    println!(
        "  {label:<28} left {left:?} mid {mid:?} right {right:?} -> {}",
        if spread > 30 {
            "gradient drawn"
        } else {
            "FLAT, gradient lost"
        }
    );
}

async fn render_at(scene: &SceneGraph, size: [f32; 2], scale: f32) -> (image::RgbaImage, f64, f64) {
    let mut canvas = PngCanvas::new(CanvasDimensions { size, scale }, Default::default())
        .await
        .unwrap();
    let t0 = Instant::now();
    canvas.set_scene(scene).unwrap();
    let set_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let t1 = Instant::now();
    let img = canvas.render().await.unwrap();
    let draw_ms = t1.elapsed().as_secs_f64() * 1000.0;
    (img, set_ms, draw_ms)
}

async fn render(scene: &SceneGraph, size: [f32; 2]) -> (image::RgbaImage, f64, f64) {
    render_at(scene, size, 1.0).await
}

/// Time one geometry-index build, best of three.
fn time_rtree(scene: &SceneGraph) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        let _rtree = SceneGraphRTree::from_scene_graph(scene);
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    best
}

/// The same scene with its groups marked non-interactive.
fn noninteractive(scene: &SceneGraph) -> SceneGraph {
    let marks = scene
        .marks
        .iter()
        .map(|m| match m {
            avenger_scenegraph::marks::mark::SceneMark::Group(g) => {
                let mut g = g.clone();
                g.interactive = false;
                avenger_scenegraph::marks::mark::SceneMark::Group(g)
            }
            other => other.clone(),
        })
        .collect();
    SceneGraph {
        marks,
        width: scene.width,
        height: scene.height,
        origin: scene.origin,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // What the scale hands the colorbar for the same ramp the charts use.
    {
        use avenger_scales::scales::linear::LinearScale;
        let scale =
            LinearScale::configured((46.0, 76.0), (0.0, 1.0))
                .with_range(std::sync::Arc::new(arrow::array::StringArray::from(vec![
                    "#2c7fb8", "#7fcdbb", "#edf8b1",
                ])) as arrow::array::ArrayRef);
        match scale.color_range_as_gradient_stops(5) {
            Ok(stops) => {
                println!("0. scale.color_range_as_gradient_stops(5)");
                for st in stops {
                    println!("  offset {:.2} -> {:?}", st.offset, st.color);
                }
            }
            Err(e) => println!("0. color_range_as_gradient_stops failed: {e}"),
        }
        println!();
    }

    println!("1. gradient fills: how is x1 interpreted? (bar spans x 20..380)");
    for x1 in [0.25f32, 1.0, 180.0, 360.0] {
        let scene = SceneGraph {
            marks: vec![SceneGroup {
                origin: [0.0, 0.0],
                gradients: vec![gradient_x1(x1)],
                marks: vec![bar(0).into()],
                ..Default::default()
            }
            .into()],
            width: W,
            height: H,
            origin: [0.0, 0.0],
        };
        for scale in [1.0f32, 2.0] {
            let (img, _, _) = render_at(&scene, [W, H], scale).await;
            gradient_verdict(&img, &format!("x1 = {x1}, canvas scale {scale}"));
        }
    }
    println!();
    let flat = SceneGraph {
        marks: vec![bar(0).into()],
        width: W,
        height: H,
        origin: [0.0, 0.0],
    };
    // A top-level mark carries no group gradients, so wrap it in a group that does.
    let top = SceneGraph {
        marks: vec![SceneGroup {
            origin: [0.0, 0.0],
            gradients: vec![gradient()],
            marks: vec![bar(0).into()],
            ..Default::default()
        }
        .into()],
        width: W,
        height: H,
        origin: [0.0, 0.0],
    };
    let nested = SceneGraph {
        marks: vec![SceneGroup {
            origin: [10.0, 10.0],
            gradients: vec![gradient()],
            marks: vec![SceneGroup {
                origin: [5.0, 5.0],
                marks: vec![bar(0).into()],
                ..Default::default()
            }
            .into()],
            ..Default::default()
        }
        .into()],
        width: W,
        height: H,
        origin: [0.0, 0.0],
    };
    let solid = SceneGraph {
        marks: vec![SceneGroup {
            origin: [0.0, 0.0],
            marks: vec![SceneRectMark {
                fill: ColorOrGradient::Color([0.17, 0.50, 0.72, 1.0]).into(),
                ..bar(0)
            }
            .into()],
            ..Default::default()
        }
        .into()],
        width: W,
        height: H,
        origin: [0.0, 0.0],
    };
    let _ = flat;
    for (label, scene) in [
        ("solid fill (control)", &solid),
        ("group with gradient", &top),
        ("nested group", &nested),
    ] {
        let (img, _, _) = render(scene, [W, H]).await;
        gradient_verdict(&img, label);
    }

    let mut rtree_ms: Vec<(usize, f64, f64)> = Vec::new();
    println!("\n2. cost of a scene of symbols (scale 1.0, {W} x {H} canvas -> 900 x 900)");
    println!(
        "  {:>9}  {:>10}  {:>10}  {:>12}",
        "symbols", "set_scene", "render", "per symbol"
    );
    for n in [10_000usize, 50_000, 150_000, 300_000] {
        let xs: Vec<f32> = (0..n).map(|i| (i % 900) as f32).collect();
        let ys: Vec<f32> = (0..n).map(|i| ((i / 900) % 900) as f32).collect();
        let scene = SceneGraph {
            marks: vec![SceneGroup {
                origin: [0.0, 0.0],
                marks: vec![SceneSymbolMark {
                    len: n as u32,
                    x: xs.into(),
                    y: ys.into(),
                    size: 6.0.into(),
                    shapes: vec![SymbolShape::Circle],
                    fill: ColorOrGradient::Color([0.2, 0.5, 0.8, 1.0]).into(),
                    stroke_width: None,
                    ..Default::default()
                }
                .into()],
                ..Default::default()
            }
            .into()],
            width: 900.0,
            height: 900.0,
            origin: [0.0, 0.0],
        };
        // Three passes, report the best: the first includes warm-up.
        let (mut best_set, mut best_draw) = (f64::MAX, f64::MAX);
        for _ in 0..3 {
            let (_, set_ms, draw_ms) = render(&scene, [900.0, 900.0]).await;
            best_set = best_set.min(set_ms);
            best_draw = best_draw.min(draw_ms);
        }
        println!(
            "  {n:>9}  {best_set:>8.1} ms  {best_draw:>8.1} ms  {:>10.2} us",
            (best_set + best_draw) * 1000.0 / n as f64
        );
        rtree_ms.push((n, time_rtree(&scene), time_rtree(&noninteractive(&scene))));
    }

    println!("\n3. rebuilding the geometry index for the same scenes");
    println!(
        "  {:>9}  {:>14}  {:>20}",
        "symbols", "interactive", "interactive: false"
    );
    for (n, yes, no) in rtree_ms {
        println!("  {n:>9}  {yes:>11.1} ms  {no:>17.1} ms");
    }
    Ok(())
}
