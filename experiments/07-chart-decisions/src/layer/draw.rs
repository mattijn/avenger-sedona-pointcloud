//! Drawing: a transition state becomes an `avenger-scenegraph` scene. Rects
//! stay rect instances while the plane is flat and become sampled paths while
//! it bends; lines are one mark per series; the map is symbol instances.

use avenger_color::ColorOrGradient;
use avenger_common::canvas::CanvasDimensions;
use avenger_scenegraph::marks::group::{Clip, SceneGroup};
use avenger_scenegraph::marks::line::SceneLineMark;
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::path::ScenePathMark;
use avenger_scenegraph::marks::rect::SceneRectMark;
use avenger_scenegraph::marks::symbol::SceneSymbolMark;
use avenger_scenegraph::marks::text::SceneTextMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_text::types::{FontWeight, FontWeightNameSpec, TextAlign, TextBaseline};
use lidar_common::{INK, MUTED};
use lyon_path::math::point;

use super::anim::{DAxis, Drawn, UGeo};
use super::model::{nice, viridis, Axis, View};

/// The plot square, in pixels.
pub const P: f64 = 400.0;
pub const ORIGIN: [f32; 2] = [100.0, 90.0];
pub const SIZE: [f32; 2] = [940.0, 640.0];
const GRID: [f32; 4] = [0.86, 0.88, 0.91, 1.0];

/// How the chart's guides and titles look. The default is the look of the
/// recordings; the live window may set another once, at startup.
#[derive(Clone, Debug)]
pub struct Style {
    pub font: String,
    pub ink: [f32; 4],
    pub muted: [f32; 4],
    pub grid: [f32; 4],
    pub title: [f32; 4],
    /// A grey line above the title, naming the subject.
    pub kicker: Option<(String, [f32; 4])>,
}

static STYLE: std::sync::OnceLock<Style> = std::sync::OnceLock::new();

pub fn set_style(s: Style) {
    let _ = STYLE.set(s);
}

fn style() -> &'static Style {
    STYLE.get_or_init(|| Style { font: "sans-serif".into(), ink: INK, muted: MUTED, grid: GRID, title: INK, kicker: None })
}

pub fn c(rgba: [f32; 4]) -> ColorOrGradient {
    ColorOrGradient::Color(rgba)
}

fn fade(mut rgba: [f32; 4], a: f32) -> [f32; 4] {
    rgba[3] *= a;
    rgba
}

/// Unit square → plot pixels, through `Bend(t)`: t = 0 is Cartesian (y up),
/// t = 1 is polar with θ from x (clockwise from 12 o'clock) and r from y.
/// The same family as experiment 5, rotated about the strip's midpoint so
/// the plot stays in place.
pub fn project(u: [f64; 2], t: f64) -> [f64; 2] {
    let (w, h) = (P, P);
    if t < 1e-6 {
        return [u[0] * w, (1.0 - u[1]) * h];
    }
    let r1 = 0.5 * w.min(h);
    let rm = 0.5 * r1;
    let phi = t * std::f64::consts::TAU;
    let s = w + (std::f64::consts::TAU * rm - w) * t;
    let rho = s / phi;
    let thick = h + (r1 - h) * t;
    let pm = [w / 2.0, h / 2.0 + rm * t];
    let theta = (u[0] - 0.5) * phi;
    let r = rho + (u[1] - 0.5) * thick;
    let v = [r * theta.sin(), rho - r * theta.cos()];
    let (sa, ca) = (t * std::f64::consts::PI).sin_cos();
    [pm[0] + v[0] * ca - v[1] * sa, pm[1] + v[0] * sa + v[1] * ca]
}

pub fn text(s: &str, x: f32, y: f32, size: f32, color: [f32; 4], align: TextAlign, baseline: TextBaseline, bold: bool, angle: f32) -> SceneMark {
    SceneTextMark {
        interactive: false,
        len: 1,
        text: s.to_string().into(),
        x: x.into(),
        y: y.into(),
        font: style().font.clone().into(),
        font_size: size.into(),
        color: c(color).into(),
        align: align.into(),
        baseline: baseline.into(),
        angle: angle.into(),
        font_weight: FontWeight::Name(if bold { FontWeightNameSpec::Bold } else { FontWeightNameSpec::Normal }).into(),
        ..Default::default()
    }
    .into()
}

pub fn rule(x: f32, y: f32, x2: f32, y2: f32, color: [f32; 4]) -> SceneMark {
    SceneLineMark {
        interactive: false,
        len: 2,
        x: vec![x, x2].into(),
        y: vec![y, y2].into(),
        stroke: c(color),
        stroke_width: 1.0,
        ..Default::default()
    }
    .into()
}

/// Numbers as axis labels: 1.2M, 450k, or 657,400.
pub fn fmt(v: f64, step: f64) -> String {
    if v.abs() >= 1e6 && step >= 1e5 {
        let m = v / 1e6;
        if (m - m.round()).abs() < 1e-9 { format!("{m:.0}M") } else { format!("{m:.1}M") }
    } else if v.abs() >= 1e4 && step >= 1e3 && v.abs() < 1e6 {
        format!("{:.0}k", v / 1e3)
    } else if v.abs() >= 1e5 {
        let s = format!("{v:.0}");
        let mut out = String::new();
        for (i, ch) in s.chars().enumerate() {
            if i > 0 && (s.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(ch);
        }
        out
    } else if step < 1.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

fn ticks(lo: f64, hi: f64) -> (Vec<f64>, f64) {
    let (a, b) = nice(lo, hi);
    let span = (b - a).max(1e-9);
    let step = [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * 10f64.powf((span / 5.0).log10().floor()))
        .find(|s| span / s <= 6.0)
        .unwrap();
    let mut v = (lo / step).ceil() * step;
    let mut out = vec![];
    while v <= hi + 1e-9 {
        out.push(v);
        v += step;
    }
    (out, step)
}

/// Grid lines and labels, through the chart's projection: straight rules
/// while the view is flat, sampled lines when a lens or a tilt bends them.
fn axis_marks(ax: &DAxis, horizontal: bool, proj: &dyn Fn([f64; 2]) -> [f64; 2], flat: bool, marks: &mut Vec<SceneMark>) {
    let (p, a) = (P as f32, ax.alpha);
    if a < 0.01 {
        return;
    }
    let st = style();
    let (ink, muted, grid) = (fade(st.ink, a), fade(st.muted, a), fade(st.grid, a));
    // One grid line at unit position u, across the plot.
    let line = |u: f64, marks: &mut Vec<SceneMark>| {
        let at = |s: f64| if horizontal { proj([u, s]) } else { proj([s, u]) };
        if flat {
            let (q0, q1) = (at(0.0), at(1.0));
            marks.push(rule(q0[0] as f32, q0[1] as f32, q1[0] as f32, q1[1] as f32, grid));
        } else {
            let pts: Vec<[f64; 2]> = (0..=32).map(|k| at(k as f64 / 32.0)).collect();
            marks.push(polyline(&pts, grid, 1.0));
        }
    };
    // Where a label for unit position u sits: at the plot's edge.
    let label_at = |u: f64| if horizontal { proj([u, 0.0]) } else { proj([0.0, u]) };
    let tick = |u: f64, s: &str, marks: &mut Vec<SceneMark>| {
        let q = label_at(u);
        if horizontal {
            marks.push(text(s, q[0] as f32, q[1] as f32 + 8.0, 11.0, muted, TextAlign::Center, TextBaseline::Top, false, 0.0));
        } else {
            marks.push(text(s, q[0] as f32 - 8.0, q[1] as f32, 11.0, muted, TextAlign::Right, TextBaseline::Middle, false, 0.0));
        }
    };
    let title = |field: &str, marks: &mut Vec<SceneMark>, below: f32| {
        if horizontal {
            marks.push(text(field, p / 2.0, p + below, 12.0, ink, TextAlign::Center, TextBaseline::Top, false, 0.0));
        } else {
            marks.push(text(field, -62.0, p / 2.0, 12.0, ink, TextAlign::Center, TextBaseline::Bottom, false, -90.0));
        }
    };
    match &ax.axis {
        Axis::None => {}
        Axis::Linear { field, lo, hi } => {
            let (ts, step) = ticks(*lo, *hi);
            for v in ts {
                let u = (v - lo) / (hi - lo);
                line(u, marks);
                tick(u, &fmt(v, step), marks);
            }
            title(field, marks, 30.0);
        }
        Axis::Log { field, lo, hi } => {
            let (l0, l1) = (lo.log10(), hi.log10());
            let mut e = l0.round();
            while e <= l1 + 1e-9 {
                let u = (e - l0) / (l1 - l0);
                let v = 10f64.powf(e);
                line(u, marks);
                tick(u, &fmt(v, v), marks);
                e += 1.0;
            }
            title(field, marks, 30.0);
        }
        Axis::Band { field, labels } => {
            let n = labels.len() as f64;
            for (i, l) in labels.iter().enumerate() {
                let u = (i as f64 + 0.5) / n;
                let q = label_at(u);
                if horizontal {
                    // Two-word labels wrap, so neighbouring bands do not overlap.
                    let (a, b) = if l.len() > 9 { l.split_once(' ').unwrap_or((l, "")) } else { (l.as_str(), "") };
                    marks.push(text(a, q[0] as f32, q[1] as f32 + 8.0, 11.0, muted, TextAlign::Center, TextBaseline::Top, false, 0.0));
                    if !b.is_empty() {
                        marks.push(text(b, q[0] as f32, q[1] as f32 + 21.0, 11.0, muted, TextAlign::Center, TextBaseline::Top, false, 0.0));
                    }
                } else {
                    marks.push(text(l, q[0] as f32 - 8.0, q[1] as f32, 11.0, muted, TextAlign::Right, TextBaseline::Middle, false, 0.0));
                }
            }
            if horizontal {
                title(field, marks, 38.0);
            }
        }
    }
}

fn polyline(pts: &[[f64; 2]], stroke: [f32; 4], width: f32) -> SceneMark {
    SceneLineMark {
        interactive: false,
        len: pts.len() as u32,
        x: pts.iter().map(|p| p[0] as f32).collect::<Vec<_>>().into(),
        y: pts.iter().map(|p| p[1] as f32).collect::<Vec<_>>().into(),
        stroke: c(stroke),
        stroke_width: width,
        ..Default::default()
    }
    .into()
}

// ---------------------------------------------------------------------------
// Views: after the bend, a lens or a tilt, as in experiment 5's
// coordinate systems (`Fisheye`, `Cartesian3d`). Each is a point transform,
// and a change of view blends the two transforms' outputs.

/// Sarkar–Brown fisheye in unit space: points within `radius` of `focus`
/// move outward, everything else stays.
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

/// Experiment 5's `Cartesian3d` over the plot square: x, y and height z,
/// seen from `yaw` around and `elevation` above the horizon. Returns the
/// screen point and its depth (larger is further away).
pub fn tilt(u: [f64; 2], h: f64, yaw: f64, elevation: f64) -> ([f64; 2], f64) {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (se, ce) = elevation.to_radians().sin_cos();
    let (x, y, z) = (u[0] - 0.5, u[1] - 0.5, h * 0.35);
    let (xr, yr) = (x * cy - y * sy, x * sy + y * cy);
    let (sx, sy2, depth) = (xr, yr * se + z * ce, yr * ce - z * se);
    let s = P / std::f64::consts::SQRT_2 * 0.98;
    ([P / 2.0 + s * sx, P * 0.62 - s * sy2], depth)
}

fn view_point(v: &View, u: [f64; 2], h: f64, bend: f64) -> ([f64; 2], f64) {
    match v {
        View::Flat | View::Magnifier { .. } => (project(u, bend), 0.0),
        View::Fisheye { focus, radius, distortion } => (project(fisheye(u, *focus, *radius, *distortion), bend), 0.0),
        View::Tilt { yaw, elevation } => tilt(u, h, *yaw, *elevation),
    }
}

/// The drawn frame's projection: unit point and height to plot pixels, and
/// a depth for painter's order.
fn projector(d: &Drawn) -> impl Fn([f64; 2], f64) -> ([f64; 2], f64) + '_ {
    move |u, h| {
        let (b, db) = view_point(&d.view_b, u, h, d.bend);
        if d.vt >= 1.0 || d.view_a == d.view_b {
            return (b, db);
        }
        let (a, da) = view_point(&d.view_a, u, h, d.bend);
        let t = d.vt;
        ([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], da + (db - da) * t)
    }
}

fn is_identity(v: &View) -> bool {
    matches!(v, View::Flat | View::Magnifier { .. })
}

fn ring_with(r: [f64; 4], proj: &dyn Fn([f64; 2]) -> [f64; 2]) -> lyon_path::Path {
    let [x0, x1, y0, y1] = r;
    let n = 24;
    let edge = |a: [f64; 2], b: [f64; 2]| (0..n).map(move |k| {
        let s = k as f64 / n as f64;
        [a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s]
    });
    let pts: Vec<[f64; 2]> = edge([x0, y0], [x1, y0]).chain(edge([x1, y0], [x1, y1])).chain(edge([x1, y1], [x0, y1])).chain(edge([x0, y1], [x0, y0])).map(proj).collect();
    let mut b = lyon_path::Path::builder();
    b.begin(point(pts[0][0] as f32, pts[0][1] as f32));
    for p in &pts[1..] {
        b.line_to(point(p[0] as f32, p[1] as f32));
    }
    b.end(true);
    b.build()
}

fn circle(c: [f64; 2], r: f64) -> lyon_path::Path {
    let mut b = lyon_path::Path::builder();
    for k in 0..=72 {
        let a = k as f64 / 72.0 * std::f64::consts::TAU;
        let p = point((c[0] + r * a.cos()) as f32, (c[1] + r * a.sin()) as f32);
        if k == 0 { b.begin(p); } else { b.line_to(p); }
    }
    b.end(true);
    b.build()
}

/// The items as marks, through `proj`. Rects stay rect instances when
/// `flat`, and become sampled rings otherwise.
fn item_marks(d: &Drawn, proj: &dyn Fn([f64; 2], f64) -> ([f64; 2], f64), flat: bool, size_k: f64) -> Vec<SceneMark> {
    // A tilted view has no clip rectangle: what lies outside the plot's
    // square (a zoom) is left out instead.
    let tilted = matches!(d.view_a, View::Tilt { .. }) || matches!(d.view_b, View::Tilt { .. });
    let inside = |u: [f64; 2]| !tilted || ((0.0..=1.0).contains(&u[0]) && (0.0..=1.0).contains(&u[1]));
    let mut plot: Vec<SceneMark> = vec![];
    let rects: Vec<_> = d.items.iter().filter_map(|i| match i.geo { UGeo::Rect(r) => Some((r, i.fill)), _ => None }).collect();
    if !rects.is_empty() {
        if flat {
            let (mut x, mut y, mut x2, mut y2, mut f) = (vec![], vec![], vec![], vec![], vec![]);
            for (r, fill) in &rects {
                let (a, b) = (proj([r[0], r[3]], 0.0).0, proj([r[1], r[2]], 0.0).0);
                x.push(a[0] as f32);
                y.push(a[1] as f32);
                x2.push(b[0] as f32);
                y2.push(b[1] as f32);
                f.push(c(*fill));
            }
            plot.push(SceneRectMark { interactive: false, len: rects.len() as u32, x: x.into(), y: y.into(), x2: Some(x2.into()), y2: Some(y2.into()), fill: f.into(), stroke: c([1.0, 1.0, 1.0, 0.8]).into(), stroke_width: 0.75.into(), ..Default::default() }.into());
        } else {
            let p2 = |u: [f64; 2]| proj(u, 0.0).0;
            plot.push(
                ScenePathMark {
                    interactive: false,
                    len: rects.len() as u32,
                    path: rects.iter().map(|(r, _)| ring_with(*r, &p2)).collect::<Vec<_>>().into(),
                    fill: rects.iter().map(|(_, f)| c(*f)).collect::<Vec<_>>().into(),
                    stroke: c([1.0, 1.0, 1.0, 0.8]).into(),
                    stroke_width: Some(0.75),
                    ..Default::default()
                }
                .into(),
            );
        }
    }
    for i in &d.items {
        if let UGeo::Line(pts) = &i.geo {
            let px: Vec<[f64; 2]> = pts.iter().map(|u| proj(*u, 0.0).0).collect();
            plot.push(polyline(&px, i.fill, i.size as f32));
        }
    }
    // Points in painter's order: the furthest first, when tilted. Their
    // area follows the projection's local magnification, so cells that
    // touch on a flat map still touch under a lens.
    let e = 1e-3;
    let mut points: Vec<_> = d
        .items
        .iter()
        .filter_map(|i| match i.geo {
            UGeo::Point(u) if inside(u) => {
                let (q, depth) = proj(u, i.h);
                let m = if flat { 1.0 } else {
                    let (qx, qy) = (proj([u[0] + e, u[1]], i.h).0, proj([u[0], u[1] + e], i.h).0);
                    let mx = (qx[0] - q[0]).hypot(qx[1] - q[1]) / (e * P);
                    let my = (qy[0] - q[0]).hypot(qy[1] - q[1]) / (e * P);
                    (mx * my).clamp(0.05, 25.0)
                };
                Some((q, i.fill, i.size * size_k * m, depth))
            }
            _ => None,
        })
        .collect();
    points.sort_by(|a, b| b.3.total_cmp(&a.3));
    if !points.is_empty() {
        plot.push(
            SceneSymbolMark {
                interactive: false,
                len: points.len() as u32,
                x: points.iter().map(|p| p.0[0] as f32).collect::<Vec<_>>().into(),
                y: points.iter().map(|p| p.0[1] as f32).collect::<Vec<_>>().into(),
                fill: points.iter().map(|p| c(p.1)).collect::<Vec<_>>().into(),
                size: points.iter().map(|p| p.2 as f32).collect::<Vec<_>>().into(),
                ..Default::default()
            }
            .into(),
        );
    }
    plot
}

/// The chart as scene marks, placed at `origin` (top left of the plot).
pub fn chart_marks(d: &Drawn, origin: [f32; 2]) -> Vec<SceneMark> {
    let proj = projector(d);
    let flat = d.bend < 1e-6 && is_identity(&d.view_a) && is_identity(&d.view_b);
    // A view that keeps the plot inside its square: flat or a fisheye.
    let inside = d.bend < 1e-6 && !matches!(d.view_a, View::Tilt { .. }) && !matches!(d.view_b, View::Tilt { .. });
    let mut guides: Vec<SceneMark> = vec![];
    if d.bend < 0.5 {
        let a = (1.0 - 2.0 * d.bend) as f32;
        let p2 = |u: [f64; 2]| proj(u, 0.0).0;
        for ax in &d.x {
            axis_marks(&DAxis { axis: ax.axis.clone(), alpha: ax.alpha * a }, true, &p2, flat, &mut guides);
        }
        for ax in &d.y {
            axis_marks(&DAxis { axis: ax.axis.clone(), alpha: ax.alpha * a }, false, &p2, flat, &mut guides);
        }
    }
    let plot = item_marks(d, &proj, flat, 1.0);
    let mut marks = guides;
    // Clip while the plot stays in its square; a bend or a tilt reaches past it.
    let clip = if inside { Clip::Rect { x: 0.0, y: 0.0, width: P as f32, height: P as f32 } } else { Clip::None };
    marks.push(SceneMark::Group(SceneGroup { marks: plot, clip, ..Default::default() }));
    // A magnifier: the same items again, flat and scaled about the focus,
    // inside a circle over the plot. It grows in and out with the view.
    for (v, w) in [(d.view_a, 1.0 - d.vt), (d.view_b, d.vt)] {
        let View::Magnifier { focus, radius, zoom } = v else { continue };
        let w = if d.view_a == d.view_b { 1.0 } else { w };
        if w < 0.02 || d.bend > 1e-6 {
            continue;
        }
        let cen = project(focus, 0.0);
        let r = radius * P * w;
        let lens = move |u: [f64; 2], _h: f64| {
            let q = project(u, 0.0);
            ([cen[0] + zoom * (q[0] - cen[0]), cen[1] + zoom * (q[1] - cen[1])], 0.0)
        };
        let mut inner = vec![ScenePathMark { interactive: false, len: 1, path: vec![circle(cen, r)].into(), fill: c([1.0; 4]).into(), ..Default::default() }.into()];
        inner.extend(item_marks(d, &lens, true, zoom * zoom));
        marks.push(SceneMark::Group(SceneGroup { marks: inner, clip: Clip::Path { path: circle(cen, r), fill_rule: Default::default() }, ..Default::default() }));
        marks.push(ScenePathMark { interactive: false, len: 1, path: vec![circle(cen, r)].into(), fill: c([0.0; 4]).into(), stroke: c(fade(style().ink, 0.7)).into(), stroke_width: Some(1.5), ..Default::default() }.into());
    }
    // Title, legend, colour bar.
    for (t, a) in &d.titles {
        if *a > 0.01 {
            marks.push(text(t, 0.0, -30.0, 17.0, fade(style().title, *a), TextAlign::Left, TextBaseline::Alphabetic, true, 0.0));
            if let Some((k, c)) = &style().kicker {
                marks.push(text(k, 0.0, -52.0, 17.0, fade(*c, *a), TextAlign::Left, TextBaseline::Alphabetic, true, 0.0));
            }
        }
    }
    let lx = P as f32 + 36.0;
    for (legend, a) in &d.legends {
        if *a < 0.01 {
            continue;
        }
        for (k, (label, col)) in legend.iter().enumerate() {
            let y = 6.0 + k as f32 * 22.0;
            marks.push(
                SceneRectMark {
                    interactive: false,
                    len: 1,
                    x: lx.into(),
                    y: y.into(),
                    width: Some(12.0.into()),
                    height: Some(12.0.into()),
                    fill: c(fade(*col, *a)).into(),
                    ..Default::default()
                }
                .into(),
            );
            marks.push(text(label, lx + 18.0, y + 6.0, 12.0, fade(style().ink, *a), TextAlign::Left, TextBaseline::Middle, false, 0.0));
        }
    }
    for ((_, vmax), a) in &d.colorbars {
        if *a < 0.01 {
            continue;
        }
        let (h, n) = (220.0f32, 44);
        let (mut y, mut y2, mut f) = (vec![], vec![], vec![]);
        for k in 0..n {
            let t0 = k as f32 / n as f32;
            y.push(h * (1.0 - t0 - 1.0 / n as f32));
            y2.push(h * (1.0 - t0));
            f.push(c(fade(viridis(t0 as f64 + 0.5 / n as f64), *a)));
        }
        marks.push(
            SceneRectMark {
                interactive: false,
                len: n as u32,
                x: lx.into(),
                width: Some(16.0.into()),
                y: y.into(),
                y2: Some(y2.into()),
                fill: f.into(),
                ..Default::default()
            }
            .into(),
        );
        let mut v = 1.0f64;
        while v <= *vmax {
            let t = (v.ln_1p() / vmax.ln_1p()) as f32;
            let label = if v >= 1e6 { format!("{:.0}M", v / 1e6) } else if v >= 1e3 { format!("{:.0}k", v / 1e3) } else { format!("{v:.0}") };
            marks.push(text(&label, lx + 22.0, h * (1.0 - t), 11.0, fade(style().muted, *a), TextAlign::Left, TextBaseline::Middle, false, 0.0));
            v *= 100.0;
        }
        marks.push(text("points (log scale)", lx, h + 20.0, 11.0, fade(style().ink, *a), TextAlign::Left, TextBaseline::Top, false, 0.0));
    }
    vec![SceneMark::Group(SceneGroup { origin, marks, ..Default::default() })]
}

/// A filled rectangle, optionally rounded and outlined.
pub fn rect(x: f32, y: f32, w: f32, h: f32, fill: [f32; 4], stroke: Option<[f32; 4]>, radius: f32) -> SceneMark {
    SceneRectMark {
        interactive: false,
        len: 1,
        x: x.into(),
        y: y.into(),
        width: Some(w.into()),
        height: Some(h.into()),
        fill: c(fill).into(),
        stroke: c(stroke.unwrap_or([0.0; 4])).into(),
        stroke_width: stroke.map_or(0.0, |_| 1.0).into(),
        corner_radius: radius.into(),
        ..Default::default()
    }
    .into()
}

pub fn scene(d: &Drawn) -> SceneGraph {
    SceneGraph { marks: chart_marks(d, ORIGIN), width: SIZE[0], height: SIZE[1], origin: [0.0; 2] }
}

pub fn dims() -> CanvasDimensions {
    CanvasDimensions { size: SIZE, scale: 1.0 }
}
