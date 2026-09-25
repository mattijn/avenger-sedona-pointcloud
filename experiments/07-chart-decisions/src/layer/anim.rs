//! Transitions: one chart object moving from one frame to the next.
//!
//! Items are joined by key (D3's object constancy):
//! - an item in both frames moves, resizes and recolours;
//! - a new item whose parent was in the old frame starts as a slice of that
//!   parent (a class bar splits into its heatmap cells), and the reverse
//!   collapses cells back into their bar;
//! - any other new item fades in, and a vanished one fades out.
//!
//! When both frames share a linear axis, the domain itself tweens (a zoom).
//! A change between Cartesian and polar goes through `Bend` from experiment
//! 5 in two phases: first the items move to their new layout, then the plane
//! bends (or the reverse).

use std::collections::HashMap;

use super::model::{Axis, Coords, Frame, Geo, Item, View};

/// Geometry in the unit square of the plot, before the coordinate system.
#[derive(Clone, Debug)]
pub enum UGeo {
    Rect([f64; 4]),
    Point([f64; 2]),
    Line(Vec<[f64; 2]>),
}

#[derive(Clone, Debug)]
pub struct DItem {
    pub geo: UGeo,
    pub fill: [f32; 4],
    pub size: f64,
    /// Drawn later (on top) when larger.
    pub z: i32,
    /// Height in unit space, for a tilted view.
    pub h: f64,
}

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
}

pub fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn smooth(a: f64, b: f64, t: f64) -> f64 {
    ease((t - a) / (b - a))
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn lerp_c(a: [f32; 4], b: [f32; 4], t: f64) -> [f32; 4] {
    let t = t as f32;
    [0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn alpha(mut c: [f32; 4], a: f64) -> [f32; 4] {
    c[3] *= a.clamp(0.0, 1.0) as f32;
    c
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

fn lerp_geo(a: &UGeo, b: &UGeo, t: f64) -> Option<UGeo> {
    Some(match (a, b) {
        (UGeo::Rect(a), UGeo::Rect(b)) => UGeo::Rect([0, 1, 2, 3].map(|i| lerp(a[i], b[i], t))),
        (UGeo::Point(a), UGeo::Point(b)) => UGeo::Point([lerp(a[0], b[0], t), lerp(a[1], b[1], t)]),
        (UGeo::Line(a), UGeo::Line(b)) if a.len() == b.len() => {
            UGeo::Line(a.iter().zip(b).map(|(p, q)| [lerp(p[0], q[0], t), lerp(p[1], q[1], t)]).collect())
        }
        _ => return None,
    })
}

/// The k-th of m vertical slices of a parent rect.
fn slice(parent: &UGeo, k: usize, m: usize) -> Option<UGeo> {
    let UGeo::Rect([x0, x1, y0, y1]) = parent else { return None };
    let (a, b) = (k as f64 / m as f64, (k + 1) as f64 / m as f64);
    Some(UGeo::Rect([*x0, *x1, lerp(*y0, *y1, a), lerp(*y0, *y1, b)]))
}

/// Children of each parent in a frame, ordered left to right (by band).
fn children(f: &Frame, units: &HashMap<&str, UGeo>) -> HashMap<String, Vec<String>> {
    let mut c: HashMap<String, Vec<(f64, String)>> = HashMap::new();
    for it in &f.items {
        if let (Some(p), Some(UGeo::Rect(r))) = (&it.parent, units.get(it.key.as_str())) {
            c.entry(p.clone()).or_default().push((r[0], it.key.clone()));
        }
    }
    c.into_iter()
        .map(|(p, mut v)| {
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            (p, v.into_iter().map(|x| x.1).collect())
        })
        .collect()
}

/// A frame at rest.
pub fn still(f: &Frame) -> Drawn {
    transition(f, f, 1.0)
}

/// The chart between frame `a` (t = 0) and frame `b` (t = 1).
pub fn transition(a: &Frame, b: &Frame, t: f64) -> Drawn {
    let e = ease(t);
    // Two phases when the coordinate system changes.
    let (bend, g) = match (a.coords, b.coords) {
        (Coords::Cartesian, Coords::Polar) => (smooth(0.5, 1.0, t), smooth(0.0, 0.5, t)),
        (Coords::Polar, Coords::Cartesian) => (1.0 - smooth(0.0, 0.5, t), smooth(0.5, 1.0, t)),
        (Coords::Polar, Coords::Polar) => (1.0, e),
        _ => (0.0, e),
    };
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
    let related = a.items.iter().any(|i| b.items.iter().any(|j| j.key == i.key || j.parent.as_deref() == Some(&i.key) || i.parent.as_deref() == Some(&j.key)));
    let (exit_a, enter_b) = if related { (1.0 - e, e) } else { (out_a as f64, in_b as f64) };
    let axes = |p: &Axis, q: &Axis, d: Option<(f64, f64)>| -> Vec<DAxis> {
        match (d, q) {
            (Some((lo, hi)), Axis::Linear { field, .. }) => {
                vec![DAxis { axis: Axis::Linear { field: field.clone(), lo, hi }, alpha: 1.0 }]
            }
            _ if p == q => vec![DAxis { axis: q.clone(), alpha: 1.0 }],
            _ => vec![DAxis { axis: p.clone(), alpha: out_a }, DAxis { axis: q.clone(), alpha: in_b }],
        }
    };

    // Unit geometry per frame, under the tweened domains where they exist.
    let ua: HashMap<&str, UGeo> =
        a.items.iter().map(|i| (i.key.as_str(), unit(&i.geo, dx.or(domain(&a.x)), dy.or(domain(&a.y))))).collect();
    let ub: HashMap<&str, UGeo> =
        b.items.iter().map(|i| (i.key.as_str(), unit(&i.geo, dx.or(domain(&b.x)), dy.or(domain(&b.y))))).collect();
    let ia: HashMap<&str, &Item> = a.items.iter().map(|i| (i.key.as_str(), i)).collect();
    let ib: HashMap<&str, &Item> = b.items.iter().map(|i| (i.key.as_str(), i)).collect();
    let (ca, cb) = (children(a, &ua), children(b, &ub));

    let mut items = vec![];
    for it in &a.items {
        let k = it.key.as_str();
        match ib.get(k) {
            Some(jt) => match lerp_geo(&ua[k], &ub[k], g) {
                Some(geo) => items.push(DItem { geo, fill: lerp_c(it.fill, jt.fill, e), size: lerp(it.size, jt.size, e), z: 0, h: lerp(it.h, jt.h, e) }),
                None => {
                    items.push(DItem { geo: ua[k].clone(), fill: alpha(it.fill, 1.0 - e), size: it.size, z: 0, h: it.h });
                    items.push(DItem { geo: ub[k].clone(), fill: alpha(jt.fill, e), size: jt.size, z: 1, h: jt.h });
                }
            },
            None => {
                // Collapse into the parent's slice when the parent returns.
                let target = it.parent.as_ref().and_then(|p| {
                    let sibs = ca.get(p)?;
                    slice(ub.get(p.as_str())?, sibs.iter().position(|s| s == k)?, sibs.len())
                });
                match target.and_then(|t| lerp_geo(&ua[k], &t, g)) {
                    Some(geo) => items.push(DItem { geo, fill: alpha(it.fill, 1.0 - smooth(0.75, 1.0, t)), size: it.size, z: 1, h: it.h }),
                    None => {
                        // A parent whose children take over leaves quickly.
                        let fade = if cb.contains_key(k) { 1.0 - smooth(0.0, 0.25, t) } else { exit_a };
                        items.push(DItem { geo: ua[k].clone(), fill: alpha(it.fill, fade), size: it.size, z: 0, h: it.h });
                    }
                }
            }
        }
    }
    for jt in &b.items {
        let k = jt.key.as_str();
        if ia.contains_key(k) {
            continue;
        }
        // Grow out of the parent's slice when the parent was there.
        let start = jt.parent.as_ref().and_then(|p| {
            let sibs = cb.get(p)?;
            let parent = ia.get(p.as_str())?;
            Some((slice(ua.get(p.as_str())?, sibs.iter().position(|s| s == k)?, sibs.len())?, parent.fill))
        });
        match start.and_then(|(s, pf)| lerp_geo(&s, &ub[k], g).map(|geo| (geo, pf))) {
            Some((geo, pf)) => items.push(DItem { geo, fill: lerp_c(pf, jt.fill, e), size: jt.size, z: 1, h: jt.h }),
            None => {
                let fade = if ca.contains_key(k) { smooth(0.75, 1.0, t) } else { enter_b };
                items.push(DItem { geo: ub[k].clone(), fill: alpha(jt.fill, fade), size: jt.size, z: 1, h: jt.h });
            }
        }
    }
    items.sort_by(|p, q| (p.z, p.size as i64).cmp(&(q.z, q.size as i64)));

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
    Drawn { bend, view_a: a.view, view_b: b.view, vt, items, x: axes(&a.x, &b.x, dx), y: axes(&a.y, &b.y, dy), titles, legends, colorbars }
}
