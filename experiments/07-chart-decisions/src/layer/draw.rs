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
use super::model::{nice, viridis, Axis};

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

fn ring(r: [f64; 4], t: f64) -> lyon_path::Path {
    let [x0, x1, y0, y1] = r;
    let n = 48;
    let edge = |a: [f64; 2], b: [f64; 2]| (0..n).map(move |k| {
        let s = k as f64 / n as f64;
        [a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s]
    });
    let pts: Vec<[f64; 2]> = edge([x0, y0], [x1, y0])
        .chain(edge([x1, y0], [x1, y1]))
        .chain(edge([x1, y1], [x0, y1]))
        .chain(edge([x0, y1], [x0, y0]))
        .map(|u| project(u, t))
        .collect();
    let mut b = lyon_path::Path::builder();
    b.begin(point(pts[0][0] as f32, pts[0][1] as f32));
    for p in &pts[1..] {
        b.line_to(point(p[0] as f32, p[1] as f32));
    }
    b.end(true);
    b.build()
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

fn axis_marks(ax: &DAxis, horizontal: bool, marks: &mut Vec<SceneMark>) {
    let (p, a) = (P as f32, ax.alpha);
    if a < 0.01 {
        return;
    }
    let st = style();
    let (ink, muted, grid) = (fade(st.ink, a), fade(st.muted, a), fade(st.grid, a));
    match &ax.axis {
        Axis::None => {}
        Axis::Linear { field, lo, hi } => {
            let (ts, step) = ticks(*lo, *hi);
            for v in ts {
                let u = ((v - lo) / (hi - lo)) as f32;
                if horizontal {
                    marks.push(rule(u * p, 0.0, u * p, p, grid));
                    marks.push(text(&fmt(v, step), u * p, p + 8.0, 11.0, muted, TextAlign::Center, TextBaseline::Top, false, 0.0));
                } else {
                    let y = (1.0 - u) * p;
                    marks.push(rule(0.0, y, p, y, grid));
                    marks.push(text(&fmt(v, step), -8.0, y, 11.0, muted, TextAlign::Right, TextBaseline::Middle, false, 0.0));
                }
            }
            if horizontal {
                marks.push(text(field, p / 2.0, p + 30.0, 12.0, ink, TextAlign::Center, TextBaseline::Top, false, 0.0));
            } else {
                marks.push(text(field, -62.0, p / 2.0, 12.0, ink, TextAlign::Center, TextBaseline::Bottom, false, -90.0));
            }
        }
        Axis::Log { field, lo, hi } => {
            let (l0, l1) = (lo.log10(), hi.log10());
            let mut e = l0.round();
            while e <= l1 + 1e-9 {
                let v = 10f64.powf(e);
                let u = ((e - l0) / (l1 - l0)) as f32;
                let y = (1.0 - u) * p;
                marks.push(rule(0.0, y, p, y, grid));
                marks.push(text(&fmt(v, v), -8.0, y, 11.0, muted, TextAlign::Right, TextBaseline::Middle, false, 0.0));
                e += 1.0;
            }
            if horizontal {
                marks.push(text(field, p / 2.0, p + 30.0, 12.0, ink, TextAlign::Center, TextBaseline::Top, false, 0.0));
            } else {
                marks.push(text(field, -62.0, p / 2.0, 12.0, ink, TextAlign::Center, TextBaseline::Bottom, false, -90.0));
            }
        }
        Axis::Band { field, labels } => {
            let n = labels.len() as f32;
            for (i, l) in labels.iter().enumerate() {
                let u = (i as f32 + 0.5) / n;
                if horizontal {
                    // Two-word labels wrap, so neighbouring bands do not overlap.
                    let (a, b) = if l.len() > 9 { l.split_once(' ').unwrap_or((l, "")) } else { (l.as_str(), "") };
                    marks.push(text(a, u * p, p + 8.0, 11.0, muted, TextAlign::Center, TextBaseline::Top, false, 0.0));
                    if !b.is_empty() {
                        marks.push(text(b, u * p, p + 21.0, 11.0, muted, TextAlign::Center, TextBaseline::Top, false, 0.0));
                    }
                } else {
                    marks.push(text(l, -8.0, (1.0 - u) * p, 11.0, muted, TextAlign::Right, TextBaseline::Middle, false, 0.0));
                }
            }
            if horizontal {
                marks.push(text(field, p / 2.0, p + 38.0, 12.0, ink, TextAlign::Center, TextBaseline::Top, false, 0.0));
            }
        }
    }
}

/// The chart as scene marks, placed at `origin` (top left of the plot).
pub fn chart_marks(d: &Drawn, origin: [f32; 2]) -> Vec<SceneMark> {
    let mut plot: Vec<SceneMark> = vec![];
    let mut guides: Vec<SceneMark> = vec![];
    if d.bend < 0.5 {
        let a = (1.0 - 2.0 * d.bend) as f32;
        for ax in &d.x {
            axis_marks(&DAxis { axis: ax.axis.clone(), alpha: ax.alpha * a }, true, &mut guides);
        }
        for ax in &d.y {
            axis_marks(&DAxis { axis: ax.axis.clone(), alpha: ax.alpha * a }, false, &mut guides);
        }
    }
    // Rects: instances while flat, sampled rings while bending.
    let rects: Vec<_> = d.items.iter().filter_map(|i| match i.geo { UGeo::Rect(r) => Some((r, i.fill)), _ => None }).collect();
    if !rects.is_empty() {
        if d.bend < 1e-6 {
            let px = |r: [f64; 4]| (project([r[0], r[3]], 0.0), project([r[1], r[2]], 0.0));
            let (mut x, mut y, mut x2, mut y2, mut f) = (vec![], vec![], vec![], vec![], vec![]);
            for (r, fill) in &rects {
                let (a, b) = px(*r);
                x.push(a[0] as f32);
                y.push(a[1] as f32);
                x2.push(b[0] as f32);
                y2.push(b[1] as f32);
                f.push(c(*fill));
            }
            plot.push(
                SceneRectMark {
                    interactive: false,
                    len: rects.len() as u32,
                    x: x.into(),
                    y: y.into(),
                    x2: Some(x2.into()),
                    y2: Some(y2.into()),
                    fill: f.into(),
                    stroke: c([1.0, 1.0, 1.0, 0.8]).into(),
                    stroke_width: 0.75.into(),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            plot.push(
                ScenePathMark {
                    interactive: false,
                    len: rects.len() as u32,
                    path: rects.iter().map(|(r, _)| ring(*r, d.bend)).collect::<Vec<_>>().into(),
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
            let px: Vec<[f64; 2]> = pts.iter().map(|u| project(*u, d.bend)).collect();
            plot.push(
                SceneLineMark {
                    interactive: false,
                    len: px.len() as u32,
                    x: px.iter().map(|p| p[0] as f32).collect::<Vec<_>>().into(),
                    y: px.iter().map(|p| p[1] as f32).collect::<Vec<_>>().into(),
                    stroke: c(i.fill),
                    stroke_width: i.size as f32,
                    ..Default::default()
                }
                .into(),
            );
        }
    }
    let points: Vec<_> = d.items.iter().filter_map(|i| match i.geo { UGeo::Point(u) => Some((project(u, d.bend), i.fill, i.size)), _ => None }).collect();
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
    let mut marks = guides;
    // Clip only while flat: a bending chart may reach past the square.
    let clip = if d.bend < 1e-6 { Clip::Rect { x: 0.0, y: 0.0, width: P as f32, height: P as f32 } } else { Clip::None };
    marks.push(SceneMark::Group(SceneGroup { marks: plot, clip, ..Default::default() }));
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
