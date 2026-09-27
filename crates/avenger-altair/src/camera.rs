//! How the plot is seen (`camera` in the spec), applied to the scene Avenger
//! renders: a fisheye warps everything in the plot's unit square, a tilt
//! stands the bars up as blocks on a ground plane. The rest of the scene
//! (titles, legends) is left as it is. After experiment 7's views.

use std::sync::Arc;

use avenger_scenegraph::marks::group::{Clip, SceneGroup};
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::path::ScenePathMark;
use avenger_scenegraph::marks::text::SceneTextMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_vegalite_spec::Camera;
use lyon_path::math::point;

/// The plot rectangle, in scene coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Plot {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Sarkar-Brown fisheye in the unit square: points within `radius` of
/// `focus` move outward, the rest stay.
pub fn fisheye(u: [f64; 2], focus: [f64; 2], radius: f64, distortion: f64) -> [f64; 2] {
    let d = [u[0] - focus[0], u[1] - focus[1]];
    let r = d[0].hypot(d[1]);
    if r <= 0.0 || r >= radius {
        return u;
    }
    let x = r / radius;
    let g = (distortion + 1.0) * x / (distortion * x + 1.0);
    let k = g * radius / r;
    [focus[0] + d[0] * k, focus[1] + d[1] * k]
}

fn same_origin(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 0.5 && (a[1] - b[1]).abs() < 0.5
}

/// The scene with the camera applied, or `None` for a flat camera.
pub fn apply(scene: &SceneGraph, plot: Plot, camera: &Camera) -> Result<Option<SceneGraph>, String> {
    match camera {
        Camera::Flat => Ok(None),
        Camera::Fisheye { focus, radius, distortion } => {
            let (f, r, d) = (*focus, *radius, *distortion);
            let warp = move |x: f32, y: f32| -> [f32; 2] {
                // Plot-local pixels to the unit square (y up); points outside
                // it (ticks, labels) move with their edge.
                let (cx, cy) = (x.clamp(0.0, plot.w), y.clamp(0.0, plot.h));
                let u = [(cx / plot.w) as f64, 1.0 - (cy / plot.h) as f64];
                let v = fisheye(u, f, r, d);
                [v[0] as f32 * plot.w + (x - cx), (1.0 - v[1] as f32) * plot.h + (y - cy)]
            };
            Ok(Some(map_plot(scene, plot, &|m| warp_mark(m, &warp))))
        }
        Camera::Tilt { yaw, elevation } => tilt(scene, plot, *yaw, *elevation).map(Some),
    }
}

/// Apply `f` to every mark drawn in plot coordinates: the marks of groups
/// whose origin is the plot's, and of their child groups.
fn map_plot(scene: &SceneGraph, plot: Plot, f: &dyn Fn(&SceneMark) -> SceneMark) -> SceneGraph {
    fn walk(m: &SceneMark, inside: bool, plot: Plot, f: &dyn Fn(&SceneMark) -> SceneMark) -> SceneMark {
        match m {
            SceneMark::Group(g) => {
                let inside = inside || same_origin(g.origin, [plot.x, plot.y]);
                let mut g = g.clone();
                // A warped plot reaches past the clip of an axis group.
                if inside {
                    g.clip = Clip::None;
                }
                g.marks = g.marks.iter().map(|c| walk(c, inside, plot, f)).collect();
                SceneMark::Group(g)
            }
            other if inside => f(other),
            other => other.clone(),
        }
    }
    let mut out = scene.clone();
    out.marks = scene.marks.iter().map(|m| walk(m, false, plot, f)).collect();
    out
}

fn polyline(points: &[[f32; 2]], close: bool) -> lyon_path::Path {
    let mut b = lyon_path::Path::builder();
    b.begin(point(points[0][0], points[0][1]));
    for p in &points[1..] {
        b.line_to(point(p[0], p[1]));
    }
    b.end(close);
    b.build()
}

/// `n` points from `a` to `b` (inclusive of `a`, not of `b`).
fn edge(a: [f32; 2], b: [f32; 2], n: usize) -> impl Iterator<Item = [f32; 2]> {
    (0..n).map(move |k| {
        let t = k as f32 / n as f32;
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    })
}

const SEGMENTS: usize = 16;

fn warp_mark(m: &SceneMark, warp: &dyn Fn(f32, f32) -> [f32; 2]) -> SceneMark {
    match m {
        SceneMark::Rect(r) => {
            let n = r.len as usize;
            let (x, y) = (r.x.as_vec(n, None), r.y.as_vec(n, None));
            let x2 = match (&r.x2, &r.width) {
                (Some(x2), _) => x2.as_vec(n, None),
                (None, Some(w)) => w.as_vec(n, None).iter().zip(&x).map(|(w, x)| x + w).collect(),
                _ => x.clone(),
            };
            let y2 = match (&r.y2, &r.height) {
                (Some(y2), _) => y2.as_vec(n, None),
                (None, Some(h)) => h.as_vec(n, None).iter().zip(&y).map(|(h, y)| y + h).collect(),
                _ => y.clone(),
            };
            let paths: Vec<lyon_path::Path> = (0..n)
                .map(|i| {
                    let (a, b, c, d) = ([x[i], y[i]], [x2[i], y[i]], [x2[i], y2[i]], [x[i], y2[i]]);
                    let pts: Vec<[f32; 2]> = edge(a, b, SEGMENTS).chain(edge(b, c, SEGMENTS)).chain(edge(c, d, SEGMENTS)).chain(edge(d, a, SEGMENTS)).map(|p| warp(p[0], p[1])).collect();
                    polyline(&pts, true)
                })
                .collect();
            SceneMark::Path(ScenePathMark {
                name: r.name.clone(),
                len: r.len,
                path: paths.into(),
                fill: r.fill.clone(),
                stroke: r.stroke.clone(),
                stroke_width: r.stroke_width.as_vec(1, None).first().copied(),
                ..Default::default()
            })
        }
        SceneMark::Rule(r) => {
            let n = r.len as usize;
            let (x, y, x2, y2) = (r.x.as_vec(n, None), r.y.as_vec(n, None), r.x2.as_vec(n, None), r.y2.as_vec(n, None));
            let paths: Vec<lyon_path::Path> = (0..n)
                .map(|i| {
                    let mut pts: Vec<[f32; 2]> = edge([x[i], y[i]], [x2[i], y2[i]], SEGMENTS * 2).map(|p| warp(p[0], p[1])).collect();
                    pts.push(warp(x2[i], y2[i]));
                    polyline(&pts, false)
                })
                .collect();
            SceneMark::Path(ScenePathMark {
                name: r.name.clone(),
                len: r.len,
                path: paths.into(),
                fill: avenger_color::ColorOrGradient::Color([0.0; 4]).into(),
                stroke: r.stroke.clone(),
                stroke_width: r.stroke_width.as_vec(1, None).first().copied(),
                ..Default::default()
            })
        }
        SceneMark::Text(t) => {
            let n = t.len as usize;
            let (x, y) = (t.x.as_vec(n, None), t.y.as_vec(n, None));
            let (nx, ny): (Vec<f32>, Vec<f32>) = x.iter().zip(&y).map(|(x, y)| warp(*x, *y)).map(|p| (p[0], p[1])).unzip();
            let mut t: SceneTextMark = (**t).clone();
            t.x = nx.into();
            t.y = ny.into();
            SceneMark::Text(Arc::new(t))
        }
        other => other.clone(),
    }
}

/// A 3D point on the ground (u, v in the unit square) at height z (0..1 of
/// the plot height), projected into plot pixels; with its depth.
fn project(u: f64, v: f64, z: f64, yaw: f64, elevation: f64, plot: Plot) -> ([f32; 2], f64) {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (se, ce) = elevation.to_radians().sin_cos();
    let (x, y) = (u - 0.5, v - 0.5);
    let (xr, yr) = (x * cy - y * sy, x * sy + y * cy);
    let (sx, sz, depth) = (xr, yr * se + z * ce, yr * ce - z * se);
    let s = plot.w.min(plot.h * 1.4) as f64 * 0.9;
    ([(plot.w as f64 / 2.0 + s * sx) as f32, (plot.h as f64 * 0.68 - s * sz) as f32], depth)
}

fn shade(c: [f32; 4], k: f32) -> [f32; 4] {
    [c[0] * k, c[1] * k, c[2] * k, c[3]]
}

/// Bars as blocks on the ground, the value lines on a back wall, the x
/// labels along the front edge. Only rect marks are stood up.
fn tilt(scene: &SceneGraph, plot: Plot, yaw: f64, elevation: f64) -> Result<SceneGraph, String> {
    use avenger_color::ColorOrGradient as C;
    let mut bars: Vec<([f64; 2], f64, [f32; 4])> = vec![];
    let (mut x_labels, mut y_ticks): (Vec<(String, f64)>, Vec<(String, f64)>) = (vec![], vec![]);
    let (mut x_title, mut y_title) = (None, None);
    fn collect(m: &SceneMark, inside: bool, axis: Option<bool>, plot: Plot, bars: &mut Vec<([f64; 2], f64, [f32; 4])>, xl: &mut Vec<(String, f64)>, yl: &mut Vec<(String, f64)>, xt: &mut Option<String>, yt: &mut Option<String>) -> Result<(), String> {
        match m {
            SceneMark::Group(g) => {
                let inside = inside || same_origin(g.origin, [plot.x, plot.y]);
                // Axis groups clip to a band below (x) or left of (y) the plot.
                let axis = match (&g.clip, axis) {
                    (Clip::Rect { y, .. }, None) if *y > plot.h * 0.5 => Some(true),
                    (Clip::Rect { x, .. }, None) if *x < 0.0 => Some(false),
                    (_, a) => a,
                };
                for c in &g.marks {
                    collect(c, inside, axis, plot, bars, xl, yl, xt, yt)?;
                }
            }
            SceneMark::Rect(r) if inside && axis.is_none() && r.name != "rule_mark" => {
                let n = r.len as usize;
                let (x, y) = (r.x.as_vec(n, None), r.y.as_vec(n, None));
                let w = r.width.as_ref().map(|w| w.as_vec(n, None)).unwrap_or_else(|| r.x2.as_ref().map(|x2| x2.as_vec(n, None).iter().zip(&x).map(|(a, b)| a - b).collect()).unwrap_or(vec![0.0; n]));
                let h = r.height.as_ref().map(|h| h.as_vec(n, None)).unwrap_or_else(|| r.y2.as_ref().map(|y2| y2.as_vec(n, None).iter().zip(&y).map(|(a, b)| a - b).collect()).unwrap_or(vec![0.0; n]));
                let fill = r.fill.as_vec(n, None);
                for i in 0..n {
                    let (x0, x1) = (x[i].min(x[i] + w[i]), x[i].max(x[i] + w[i]));
                    let top = y[i].min(y[i] + h[i]);
                    let bottom = y[i].max(y[i] + h[i]);
                    let z = ((bottom - top) / plot.h) as f64;
                    let color = match &fill[i] {
                        C::Color(c) => *c,
                        _ => [0.3, 0.47, 0.66, 1.0],
                    };
                    bars.push(([(x0 / plot.w) as f64, (x1 / plot.w) as f64], z, color));
                }
            }
            SceneMark::Text(t) if inside => {
                let n = t.len as usize;
                let (text, x, y) = (t.text.as_vec(n, None), t.x.as_vec(n, None), t.y.as_vec(n, None));
                match (axis, n) {
                    (Some(true), 1) => *xt = Some(text[0].clone()),
                    (Some(false), 1) => *yt = Some(text[0].clone()),
                    (Some(true), _) => xl.extend(text.iter().zip(&x).map(|(s, x)| (s.clone(), (*x / plot.w) as f64))),
                    (Some(false), _) => yl.extend(text.iter().zip(&y).map(|(s, y)| (s.clone(), 1.0 - (*y / plot.h) as f64))),
                    _ => {}
                }
            }
            SceneMark::Rect(_) | SceneMark::Rule(_) | SceneMark::Text(_) => {}
            other if inside && axis.is_none() => return Err(format!("the tilt stands up bars; this chart draws {:?}", std::mem::discriminant(other))),
            _ => {}
        }
        Ok(())
    }
    for m in &scene.marks {
        collect(m, false, None, plot, &mut bars, &mut x_labels, &mut y_ticks, &mut x_title, &mut y_title)?;
    }
    if bars.is_empty() {
        return Err("the tilt stands up bars; this chart has none".into());
    }
    let p = |u: f64, v: f64, z: f64| project(u, v, z, yaw, elevation, plot);
    let mut marks: Vec<SceneMark> = vec![];
    let ink = [0.2, 0.2, 0.2, 1.0];
    let faint = [0.75, 0.75, 0.75, 1.0];
    // Ground and back wall.
    let ground: Vec<[f32; 2]> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].iter().map(|(u, v)| p(*u, *v, 0.0).0).collect();
    marks.push(SceneMark::Path(ScenePathMark { len: 1, path: vec![polyline(&ground, true)].into(), fill: C::Color([0.97, 0.97, 0.97, 1.0]).into(), stroke: C::Color(faint).into(), stroke_width: Some(1.0), ..Default::default() }));
    let lines: Vec<lyon_path::Path> = y_ticks.iter().map(|(_, z)| polyline(&[p(0.0, 1.0, *z).0, p(1.0, 1.0, *z).0], false)).collect();
    if !lines.is_empty() {
        marks.push(SceneMark::Path(ScenePathMark { len: lines.len() as u32, path: lines.into(), fill: C::Color([0.0; 4]).into(), stroke: C::Color(faint).into(), stroke_width: Some(0.75), ..Default::default() }));
    }
    // Faces of every block, back to front.
    let depth = bars.iter().map(|b| b.0[1] - b.0[0]).fold(f64::INFINITY, f64::min).clamp(0.02, 0.2);
    let (v0, v1) = (0.5 - depth / 2.0, 0.5 + depth / 2.0);
    let mut faces: Vec<(f64, Vec<[f32; 2]>, [f32; 4])> = vec![];
    for (u, z, color) in &bars {
        let (a, b) = (u[0], u[1]);
        let quad = |pts: [(f64, f64, f64); 4], k: f32| {
            let proj: Vec<([f32; 2], f64)> = pts.iter().map(|(u, v, z)| p(*u, *v, *z)).collect();
            let d = proj.iter().map(|q| q.1).sum::<f64>() / 4.0;
            (d, proj.iter().map(|q| q.0).collect::<Vec<_>>(), shade(*color, k))
        };
        faces.push(quad([(a, v0, *z), (b, v0, *z), (b, v1, *z), (a, v1, *z)], 1.15));
        faces.push(quad([(a, v0, 0.0), (b, v0, 0.0), (b, v0, *z), (a, v0, *z)], 1.0));
        faces.push(quad([(a, v1, 0.0), (b, v1, 0.0), (b, v1, *z), (a, v1, *z)], 1.0));
        faces.push(quad([(a, v0, 0.0), (a, v1, 0.0), (a, v1, *z), (a, v0, *z)], 0.78));
        faces.push(quad([(b, v0, 0.0), (b, v1, 0.0), (b, v1, *z), (b, v0, *z)], 0.78));
    }
    faces.sort_by(|x, y| y.0.total_cmp(&x.0));
    let fills: Vec<C> = faces.iter().map(|f| C::Color([f.2[0].min(1.0), f.2[1].min(1.0), f.2[2].min(1.0), f.2[3]])).collect();
    let paths: Vec<lyon_path::Path> = faces.iter().map(|f| polyline(&f.1, true)).collect();
    marks.push(SceneMark::Path(ScenePathMark { name: "bars".into(), len: paths.len() as u32, path: paths.into(), fill: fills.into(), stroke: C::Color([1.0, 1.0, 1.0, 0.6]).into(), stroke_width: Some(0.5), ..Default::default() }));
    // Labels: x along the front edge, values up the back-left corner.
    let label = |text: Vec<String>, at: Vec<[f32; 2]>, align: avenger_text::types::TextAlign| -> SceneMark {
        let (x, y): (Vec<f32>, Vec<f32>) = at.iter().map(|a| (a[0], a[1])).unzip();
        SceneMark::Text(Arc::new(SceneTextMark {
            len: text.len() as u32,
            text: text.into(),
            x: x.into(),
            y: y.into(),
            align: align.into(),
            baseline: avenger_text::types::TextBaseline::Middle.into(),
            color: C::Color(ink).into(),
            font_size: 10.0.into(),
            ..Default::default()
        }))
    };
    // Each label in front of its block, a little ahead of the row.
    let front = (v0 - 0.08).max(0.0);
    let xs: Vec<[f32; 2]> = x_labels.iter().map(|(_, u)| { let q = p(*u, front, 0.0).0; [q[0], q[1] + 8.0] }).collect();
    if !x_labels.is_empty() {
        marks.push(label(x_labels.iter().map(|l| l.0.clone()).collect(), xs, avenger_text::types::TextAlign::Center));
    }
    if !y_ticks.is_empty() {
        let ys: Vec<[f32; 2]> = y_ticks.iter().map(|(_, z)| { let q = p(0.0, 1.0, *z).0; [q[0] - 6.0, q[1]] }).collect();
        marks.push(label(y_ticks.iter().map(|l| l.0.clone()).collect(), ys, avenger_text::types::TextAlign::Right));
    }
    if let Some(t) = x_title {
        let q = p(0.5, 0.0, 0.0).0;
        marks.push(label(vec![t], vec![[q[0], q[1] + 22.0]], avenger_text::types::TextAlign::Center));
    }
    if let Some(t) = y_title {
        let q = p(0.0, 1.0, 1.0).0;
        marks.push(label(vec![t], vec![[q[0], q[1] - 14.0]], avenger_text::types::TextAlign::Center));
    }
    // Replace the plot's own groups (grid, bars, axes) by the 3D drawing.
    let mut out = scene.clone();
    out.marks = scene
        .marks
        .iter()
        .map(|m| match m {
            SceneMark::Group(g) => {
                let mut g = g.clone();
                g.marks.retain(|c| !matches!(c, SceneMark::Group(c) if same_origin(c.origin, [plot.x, plot.y])));
                g.marks.push(SceneMark::Group(SceneGroup { origin: [plot.x, plot.y], marks: marks.clone(), ..Default::default() }));
                SceneMark::Group(g)
            }
            other => other.clone(),
        })
        .collect();
    Ok(out)
}

/// A scene to PNG bytes, as `RenderedChart::to_png` does.
pub async fn png(scene: &SceneGraph, scale: f32) -> Result<Vec<u8>, String> {
    use avenger_wgpu::canvas::{Canvas, CanvasConfig, PngCanvas};
    let e = |x: &dyn std::fmt::Display| x.to_string();
    let engine = avenger_scales::formatter::ScaleFormatting::d3(Default::default(), Default::default()).configure_text_engine(avenger_text::default_text_engine());
    let mut canvas = PngCanvas::new(
        avenger_common::canvas::CanvasDimensions { size: [scene.width, scene.height], scale },
        CanvasConfig { text_engine: Some(engine), ..Default::default() },
    )
    .await
    .map_err(|x| e(&x))?;
    canvas.set_scene(scene).map_err(|x| e(&x))?;
    let image = canvas.render().await.map_err(|x| e(&x))?;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).map_err(|x| e(&x))?;
    Ok(bytes.into_inner())
}
