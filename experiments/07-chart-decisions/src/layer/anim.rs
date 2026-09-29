//! Transitions: one chart object moving from one frame to the next.
//!
//! The items move through `avenger-transition`: joined by key, split from
//! and collapsed into their parents, and, between Cartesian and polar, first
//! moved to their new layout and then bent. What stays here is this chart's
//! own: its axes (a shared linear axis tweens its domain, a zoom), titles,
//! legends, views and lenses.

use avenger_transition::{ease, join, lerp, phases, related, smooth, Plane, Timing};

use super::model::{Axis, Coords, Fit, Frame, Geo, Lens, View};

/// Geometry in the unit square of the plot, before the coordinate system.
pub use avenger_transition::Geo as UGeo;
/// An item between two frames.
pub use avenger_transition::Tweened as DItem;

/// An axis to draw, with its opacity during a crossfade.
#[derive(Clone, Debug)]
pub struct DAxis {
    pub axis: Axis,
    pub alpha: f32,
}

#[derive(Clone, Debug)]
pub struct Drawn {
    /// 0 is Cartesian, 1 is polar, in between is `Bend`.
    pub bend: f64,
    /// The view before and after, and how far between them.
    pub view_a: View,
    pub view_b: View,
    pub vt: f64,
    pub items: Vec<DItem>,
    pub x: Vec<DAxis>,
    pub y: Vec<DAxis>,
    pub titles: Vec<(String, f32)>,
    pub legends: Vec<(Vec<(String, [f32; 4])>, f32)>,
    pub colorbars: Vec<((f64, f64), f32)>,
    /// Lens rings and what they found, each with its opacity.
    pub lenses: Vec<(Lens, f32)>,
    pub fits: Vec<(Fit, f32)>,
    /// A lasso's outline (screen unit square), the view it was drawn in, and
    /// what it took.
    pub lasso: Option<(Vec<[f64; 2]>, Option<(f64, f64)>, String)>,
}

fn domain(a: &Axis) -> Option<(f64, f64)> {
    match a {
        Axis::Linear { lo, hi, .. } => Some((*lo, *hi)),
        _ => None,
    }
}

/// An item's geometry in unit space, with the given domains.
fn unit(g: &Geo, x: Option<(f64, f64)>, y: Option<(f64, f64)>) -> UGeo {
    let ux = |v: f64| x.map_or(v, |(lo, hi)| (v - lo) / (hi - lo));
    let uy = |v: f64| y.map_or(v, |(lo, hi)| (v - lo) / (hi - lo));
    match g {
        Geo::Rect { x0, x1, y0, y1 } => UGeo::Rect([*x0, *x1, *y0, *y1]),
        Geo::Point { x, y } => UGeo::Point([ux(*x), uy(*y)]),
        Geo::Line { pts } => UGeo::Line(pts.iter().map(|p| [ux(p[0]), uy(p[1])]).collect()),
    }
}

/// A frame at rest.
pub fn still(f: &Frame) -> Drawn {
    transition(f, f, 1.0)
}

/// The chart between frame `a` (t = 0) and frame `b` (t = 1).
pub fn transition(a: &Frame, b: &Frame, t: f64) -> Drawn {
    let e = ease(t);
    // Two phases when the coordinate system changes.
    let plane = |c: Coords| match c {
        Coords::Cartesian => Plane::Cartesian,
        Coords::Polar => Plane::Polar,
    };
    let (bend, g) = phases(plane(a.coords), plane(b.coords), t);
    // A shared linear axis tweens its domain; otherwise the axes crossfade.
    let shared = |p: &Axis, q: &Axis| match (p, q) {
        (Axis::Linear { field: f1, .. }, Axis::Linear { field: f2, .. }) if f1 == f2 => true,
        _ => false,
    };
    let tween = |p: &Axis, q: &Axis| -> Option<(f64, f64)> {
        if shared(p, q) {
            let (pa, qa) = (domain(p)?, domain(q)?);
            Some((lerp(pa.0, qa.0, e), lerp(pa.1, qa.1, e)))
        } else {
            None
        }
    };
    let (dx, dy) = (tween(&a.x, &b.x), tween(&a.y, &b.y));
    // Guides that change do so in sequence, never on top of each other: the
    // old ones fade out in the first half, the new ones in during the second.
    let out_a = (1.0 - smooth(0.0, 0.45, t)) as f32;
    let in_b = smooth(0.55, 1.0, t) as f32;
    // Frames with no item in common (different data) do the same with their
    // items; related frames move their items instead.
    // Unit geometry per frame, under the tweened domains where they exist.
    let units = |f: &Frame, x: Option<(f64, f64)>, y: Option<(f64, f64)>| -> Vec<avenger_transition::Item> {
        f.items.iter().map(|i| avenger_transition::Item {
            key: i.key.clone(), parent: i.parent.clone(), geo: unit(&i.geo, x, y), fill: i.fill, size: i.size, h: i.h,
        }).collect()
    };
    let ua = units(a, dx.or(domain(&a.x)), dy.or(domain(&a.y)));
    let ub = units(b, dx.or(domain(&b.x)), dy.or(domain(&b.y)));
    let (exit_a, enter_b) = if related(&ua, &ub) { (1.0 - e, e) } else { (out_a as f64, in_b as f64) };
    let axes = |p: &Axis, q: &Axis, d: Option<(f64, f64)>| -> Vec<DAxis> {
        match (d, q) {
            (Some((lo, hi)), Axis::Linear { field, .. }) => {
                vec![DAxis { axis: Axis::Linear { field: field.clone(), lo, hi }, alpha: 1.0 }]
            }
            _ if p == q => vec![DAxis { axis: q.clone(), alpha: 1.0 }],
            _ => vec![DAxis { axis: p.clone(), alpha: out_a }, DAxis { axis: q.clone(), alpha: in_b }],
        }
    };

    let items = join(&ua, &ub, Timing { t, g, exit: exit_a, enter: enter_b });

    let crossfade = |same: bool| if same { (0.0f32, 1.0f32) } else { (out_a, in_b) };
    let (ta, tb) = crossfade(a.state.title == b.state.title);
    let titles = vec![(a.state.title.clone(), ta), (b.state.title.clone(), tb)];
    let (la, lb) = crossfade(a.legend == b.legend);
    let legends = vec![(a.legend.clone(), la), (b.legend.clone(), lb)];
    let (ba, bb) = crossfade(a.colorbar == b.colorbar);
    let colorbars = [(a.colorbar, ba), (b.colorbar, bb)]
        .into_iter()
        .filter_map(|(c, w)| c.map(|c| (c, w)))
        .collect();
    let vt = if a.view == b.view { 1.0 } else { e };
    // A lens of the same kind slides to its new focus; otherwise the rings
    // crossfade. What it found crossfades either way.
    let lenses = match (&a.lens, &b.lens) {
        (Some((la, _)), Some((lb, _))) if std::mem::discriminant(la) == std::mem::discriminant(lb) => {
            let (fa, fb) = (la.focus(), lb.focus());
            vec![(lb.with_focus([lerp(fa[0], fb[0], e), lerp(fa[1], fb[1], e)]), 1.0)]
        }
        _ => [(&a.lens, 1.0 - e as f32), (&b.lens, e as f32)].into_iter().filter_map(|(l, w)| l.as_ref().map(|l| (l.0, w))).collect(),
    };
    let same_fits = a.lens.as_ref().map(|l| &l.1) == b.lens.as_ref().map(|l| &l.1);
    let fits = [(&a.lens, if same_fits { 0.0 } else { 1.0 - e as f32 }), (&b.lens, if same_fits { 1.0 } else { e as f32 })]
        .into_iter()
        .flat_map(|(l, w)| l.iter().flat_map(move |l| l.1.iter().map(move |f| (f.clone(), w))))
        .filter(|(_, w)| *w > 0.0)
        .collect();
    Drawn { bend, view_a: a.view, view_b: b.view, vt, items, x: axes(&a.x, &b.x, dx), y: axes(&a.y, &b.y, dy), titles, legends, colorbars, lenses, fits, lasso: b.lasso.clone() }
}
