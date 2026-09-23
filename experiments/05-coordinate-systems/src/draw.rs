//! Marks specified in input space, drawn through any coordinate system.
//! Nothing here knows which system it is drawing into; it only asks the
//! system to project points and lines, and whether it is rectilinear.

use avenger_color::ColorOrGradient;
use avenger_scenegraph::marks::group::SceneGroup;
use avenger_scenegraph::marks::line::SceneLineMark;
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::path::ScenePathMark;
use avenger_scenegraph::marks::rect::SceneRectMark;
use avenger_scenegraph::marks::symbol::SceneSymbolMark;
use avenger_scenegraph::marks::text::SceneTextMark;
use avenger_text::types::{FontWeight, FontWeightNameSpec, TextAlign, TextBaseline};
use lyon_path::math::point;
use lyon_path::Path;

use crate::coords::{CoordinateSystem, Screen};

/// Complete a mark's inputs with the system's defaults for the channels it
/// leaves out (z for a 2D mark in `cartesian3d`).
pub fn full(cs: &dyn CoordinateSystem, given: &[f64]) -> Vec<f64> {
    let mut p = given.to_vec();
    for c in cs.channels().iter().skip(given.len()) {
        p.push(c.default.unwrap_or(c.extent.0));
    }
    p
}

fn path_of(lines: &[Vec<Screen>], closed: bool) -> (Path, usize) {
    let mut b = Path::builder();
    let mut n = 0;
    for line in lines.iter().filter(|l| l.len() > 1) {
        b.begin(point(line[0][0] as f32, line[0][1] as f32));
        for p in &line[1..] {
            b.line_to(point(p[0] as f32, p[1] as f32));
        }
        b.end(closed);
        n += line.len();
    }
    (b.build(), n)
}

fn color(c: [f32; 4]) -> ColorOrGradient {
    ColorOrGradient::Color(c)
}

/// What drawing one mark cost, for the README tables.
#[derive(Default, Debug, Clone, Copy)]
pub struct Cost {
    pub instances: usize,
    pub vertices: usize,
    pub as_rects: bool,
}

/// Scatter. Points only move, so they stay symbol instances in every system;
/// a 3D system also imposes a back-to-front order.
pub fn points(
    cs: &dyn CoordinateSystem,
    pos: &[Vec<f64>],
    fill: &[[f32; 4]],
    size: f32,
) -> SceneMark {
    let full_pos: Vec<Vec<f64>> = pos.iter().map(|p| full(cs, p)).collect();
    let mut order: Vec<usize> = (0..pos.len()).collect();
    let depth: Vec<f64> = full_pos.iter().map(|p| cs.depth(p)).collect();
    order.sort_by(|a, b| depth[*b].total_cmp(&depth[*a]));
    let (mut x, mut y, mut f) = (vec![], vec![], vec![]);
    for i in order {
        if let Some(s) = cs.project(&full_pos[i]) {
            x.push(s[0] as f32);
            y.push(s[1] as f32);
            f.push(color(fill[i]));
        }
    }
    SceneSymbolMark {
        interactive: false,
        len: x.len() as u32,
        x: x.into(),
        y: y.into(),
        fill: f.into(),
        size: size.into(),
        ..Default::default()
    }
    .into()
}

/// Rects given by two corners in the first two channels. Rectilinear systems
/// keep them as rect instances; any other system turns each into a closed
/// ring, samples it, and draws it as a path.
pub fn rects(
    cs: &dyn CoordinateSystem,
    lo: &[[f64; 2]],
    hi: &[[f64; 2]],
    fill: &[[f32; 4]],
    stroke: [f32; 4],
) -> (SceneMark, Cost) {
    if cs.is_rectilinear() {
        let (mut x, mut y, mut x2, mut y2) = (vec![], vec![], vec![], vec![]);
        for (a, b) in lo.iter().zip(hi) {
            let pa = cs.project(&full(cs, a)).unwrap();
            let pb = cs.project(&full(cs, b)).unwrap();
            x.push(pa[0].min(pb[0]) as f32);
            x2.push(pa[0].max(pb[0]) as f32);
            y.push(pa[1].min(pb[1]) as f32);
            y2.push(pa[1].max(pb[1]) as f32);
        }
        let mark = SceneRectMark {
            interactive: false,
            len: lo.len() as u32,
            x: x.into(),
            y: y.into(),
            x2: Some(x2.into()),
            y2: Some(y2.into()),
            fill: fill.iter().map(|c| color(*c)).collect::<Vec<_>>().into(),
            stroke: color(stroke).into(),
            stroke_width: 1.0.into(),
            ..Default::default()
        };
        let cost = Cost {
            instances: lo.len(),
            vertices: 4 * lo.len(),
            as_rects: true,
        };
        return (mark.into(), cost);
    }
    let mut paths = vec![];
    let mut vertices = 0;
    for (a, b) in lo.iter().zip(hi) {
        let ring: Vec<Vec<f64>> = [[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]]
            .iter()
            .map(|c| full(cs, c))
            .collect();
        let (path, n) = path_of(&cs.project_line(&ring, true), true);
        vertices += n;
        paths.push(path);
    }
    let mark = ScenePathMark {
        interactive: false,
        len: paths.len() as u32,
        path: paths.into(),
        fill: fill.iter().map(|c| color(*c)).collect::<Vec<_>>().into(),
        stroke: color(stroke).into(),
        stroke_width: Some(1.0),
        ..Default::default()
    };
    let cost = Cost {
        instances: lo.len(),
        vertices,
        as_rects: false,
    };
    (mark.into(), cost)
}

/// One filled path per ring, each with its own colour: pie slices, for
/// example. Rings are in the system's input space and are sampled by it.
pub fn rings(
    cs: &dyn CoordinateSystem,
    rings: &[Vec<Vec<f64>>],
    fill: &[[f32; 4]],
    stroke: [f32; 4],
) -> (SceneMark, Cost) {
    let mut paths = vec![];
    let mut vertices = 0;
    for r in rings {
        let (path, n) = path_of(&cs.project_line(r, true), true);
        vertices += n;
        paths.push(path);
    }
    let mark = ScenePathMark {
        interactive: false,
        len: paths.len() as u32,
        path: paths.into(),
        fill: fill.iter().map(|c| color(*c)).collect::<Vec<_>>().into(),
        stroke: color(stroke).into(),
        stroke_width: Some(0.75),
        ..Default::default()
    };
    let cost = Cost {
        instances: rings.len(),
        vertices,
        as_rects: false,
    };
    (mark.into(), cost)
}

/// A geoshape: closed rings in input space, filled as one path.
pub fn shape(
    cs: &dyn CoordinateSystem,
    rings: &[Vec<Vec<f64>>],
    fill: [f32; 4],
    stroke: [f32; 4],
    width: f32,
) -> SceneMark {
    let lines: Vec<Vec<Screen>> = rings
        .iter()
        .flat_map(|r| {
            let r: Vec<Vec<f64>> = r.iter().map(|p| full(cs, p)).collect();
            cs.project_line(&r, true)
        })
        .collect();
    ScenePathMark {
        interactive: false,
        len: 1,
        path: path_of(&lines, true).0.into(),
        fill: color(fill).into(),
        stroke: color(stroke).into(),
        stroke_width: Some(width),
        ..Default::default()
    }
    .into()
}

pub fn polylines(lines: &[Vec<Screen>], stroke: [f32; 4], width: f32) -> SceneMark {
    let (mut x, mut y, mut d) = (vec![], vec![], vec![]);
    for line in lines {
        for p in line {
            x.push(p[0] as f32);
            y.push(p[1] as f32);
            d.push(true);
        }
        x.push(0.0);
        y.push(0.0);
        d.push(false);
    }
    SceneLineMark {
        interactive: false,
        len: x.len() as u32,
        x: x.into(),
        y: y.into(),
        defined: d.into(),
        stroke: color(stroke),
        stroke_width: width,
        ..Default::default()
    }
    .into()
}

pub struct Tick {
    pub value: f64,
    pub label: String,
    /// False for a label without a grid line (a categorical band centre).
    pub line: bool,
}

impl Tick {
    pub fn new(value: f64, label: impl Into<String>) -> Self {
        Tick {
            value,
            label: label.into(),
            line: true,
        }
    }
    pub fn line(value: f64) -> Self {
        Tick {
            value,
            label: String::new(),
            line: true,
        }
    }
    pub fn label(value: f64, label: impl Into<String>) -> Self {
        Tick {
            value,
            label: label.into(),
            line: false,
        }
    }
}

/// The grid of any system: for every tick on channel i, the line where
/// channel i holds that value and the other channel runs over its extent.
/// Channels beyond the first two stay at their default. Lines are densified
/// in input space first, because a line that is straight in input space (a
/// parallel) is not in general a great circle.
pub fn grid(
    cs: &dyn CoordinateSystem,
    ticks: [&[Tick]; 2],
    stroke: [f32; 4],
    ink: [f32; 4],
) -> Vec<SceneMark> {
    let channels = cs.channels();
    let mut lines = vec![];
    let mut labels = vec![];
    for i in 0..2 {
        let j = 1 - i;
        let (lo, hi) = channels[j].extent;
        for Tick {
            value: t,
            label,
            line,
        } in ticks[i]
        {
            let at = |v: f64| {
                let mut p = [0.0; 2];
                p[i] = *t;
                p[j] = v;
                full(cs, &p)
            };
            if *line {
                let pts: Vec<Vec<f64>> = (0..=64)
                    .map(|k| at(lo + (hi - lo) * k as f64 / 64.0))
                    .collect();
                lines.extend(cs.project_line(&pts, false));
            }

            if label.is_empty() {
                continue;
            }
            let (v, dir) = cs.label_side(i);
            let eps = 1e-3 * (hi - lo) * dir;
            if let (Some(a), Some(b)) = (cs.project(&at(v)), cs.project(&at(v + eps))) {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let len = dx.hypot(dy).max(1e-9);
                let (ux, uy) = (dx / len, dy / len);
                labels.push(text_at(
                    label,
                    [a[0] + 7.0 * ux, a[1] + 7.0 * uy],
                    [ux, uy],
                    ink,
                ));
            }
        }
    }
    let mut marks = vec![polylines(&lines, stroke, 0.7)];
    marks.extend(labels);
    marks
}

fn text_at(s: &str, p: Screen, dir: [f64; 2], ink: [f32; 4]) -> SceneMark {
    let align = if dir[0] > 0.4 {
        TextAlign::Left
    } else if dir[0] < -0.4 {
        TextAlign::Right
    } else {
        TextAlign::Center
    };
    let baseline = if dir[1] > 0.4 {
        TextBaseline::Top
    } else if dir[1] < -0.4 {
        TextBaseline::Bottom
    } else {
        TextBaseline::Middle
    };
    SceneTextMark {
        interactive: false,
        len: 1,
        text: s.to_string().into(),
        x: (p[0] as f32).into(),
        y: (p[1] as f32).into(),
        font_size: 11.0.into(),
        color: color(ink).into(),
        align: align.into(),
        baseline: baseline.into(),
        ..Default::default()
    }
    .into()
}

pub fn title(lines: &[(&str, f32, bool, [f32; 4])], origin: [f32; 2]) -> SceneMark {
    let mut y = 0.0;
    let marks = lines
        .iter()
        .map(|(s, size, bold, c)| {
            y += size * 1.35;
            SceneTextMark {
                interactive: false,
                len: 1,
                text: s.to_string().into(),
                x: 0.0.into(),
                y: y.into(),
                font_size: (*size).into(),
                font_weight: FontWeight::Name(if *bold {
                    FontWeightNameSpec::Bold
                } else {
                    FontWeightNameSpec::Normal
                })
                .into(),
                color: color(*c).into(),
                align: TextAlign::Left.into(),
                baseline: TextBaseline::Alphabetic.into(),
                ..Default::default()
            }
            .into()
        })
        .collect();
    group(origin, marks)
}

pub fn group(origin: [f32; 2], marks: Vec<SceneMark>) -> SceneMark {
    SceneMark::Group(SceneGroup {
        origin,
        marks,
        ..Default::default()
    })
}
