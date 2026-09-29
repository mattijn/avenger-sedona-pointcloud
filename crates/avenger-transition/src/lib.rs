//! Transitions of keyed items in the unit square of a plot, before any
//! coordinate system. From experiment 7's chart layer.
//!
//! Items are joined by key (D3's object constancy):
//! - an item in both frames moves, resizes and recolours;
//! - a new item whose parent was in the old frame starts as a slice of that
//!   parent (a class bar splits into its heatmap cells), and the reverse
//!   collapses cells back into their bar;
//! - any other new item fades in, and a vanished one fades out.
//!
//! A change between Cartesian and polar goes in two phases (`phases`): first
//! the items move to their new layout, then the plane bends through
//! `avenger_coords::Bend` (or the reverse). The caller draws the result
//! through `Bend { t: bend }`.
//!
//! `bins` lays a histogram out as bars or as one stacked bar, and carries a
//! value interval (a brush) into either layout as keyed items, so a brush
//! moves through a transition with the bars it covers.

pub mod bins;

use std::collections::HashMap;

/// Geometry in the unit square of the plot, before the coordinate system.
/// A rect is `[x0, x1, y0, y1]`.
#[derive(Clone, Debug, PartialEq)]
pub enum Geo {
    Rect([f64; 4]),
    Point([f64; 2]),
    Line(Vec<[f64; 2]>),
}

/// One item of a frame.
#[derive(Clone, Debug)]
pub struct Item {
    pub key: String,
    /// The item this one splits from, or collapses into, across a transition.
    pub parent: Option<String>,
    pub geo: Geo,
    pub fill: [f32; 4],
    pub size: f64,
    /// Height in unit space, for a tilted view.
    pub h: f64,
}

/// An item between two frames.
#[derive(Clone, Debug)]
pub struct Tweened {
    pub geo: Geo,
    pub fill: [f32; 4],
    pub size: f64,
    /// Drawn later (on top) when larger.
    pub z: i32,
    pub h: f64,
    /// The item's key, for hit-testing.
    pub key: String,
}

/// Where a transition is. `t` is the time, 0 to 1; `g` how far the geometry
/// has moved (from `phases`); `exit` and `enter` the opacity of items that
/// leave and arrive without a partner.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub t: f64,
    pub g: f64,
    pub exit: f64,
    pub enter: f64,
}

pub fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `ease` over the part of the time from `a` to `b`.
pub fn smooth(a: f64, b: f64, t: f64) -> f64 {
    ease((t - a) / (b - a))
}

pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

pub fn lerp_c(a: [f32; 4], b: [f32; 4], t: f64) -> [f32; 4] {
    let t = t as f32;
    [0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * t)
}

pub fn alpha(mut c: [f32; 4], a: f64) -> [f32; 4] {
    c[3] *= a.clamp(0.0, 1.0) as f32;
    c
}

/// The plane a frame is drawn on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    Cartesian,
    Polar,
}

/// The bend (0 Cartesian, 1 polar) and the geometry's progress `g` at time
/// `t`. Into polar, the items move in the first half and the plane bends in
/// the second; out of polar, the reverse.
pub fn phases(a: Plane, b: Plane, t: f64) -> (f64, f64) {
    let e = ease(t);
    match (a, b) {
        (Plane::Cartesian, Plane::Polar) => (smooth(0.5, 1.0, t), smooth(0.0, 0.5, t)),
        (Plane::Polar, Plane::Cartesian) => (1.0 - smooth(0.0, 0.5, t), smooth(0.5, 1.0, t)),
        (Plane::Polar, Plane::Polar) => (1.0, e),
        _ => (0.0, e),
    }
}

/// True when the frames share an item, or one is the parent of the other's:
/// then items move rather than crossfade.
pub fn related(a: &[Item], b: &[Item]) -> bool {
    a.iter().any(|i| b.iter().any(|j| j.key == i.key || j.parent.as_deref() == Some(&i.key) || i.parent.as_deref() == Some(&j.key)))
}

pub fn lerp_geo(a: &Geo, b: &Geo, t: f64) -> Option<Geo> {
    Some(match (a, b) {
        (Geo::Rect(a), Geo::Rect(b)) => Geo::Rect([0, 1, 2, 3].map(|i| lerp(a[i], b[i], t))),
        (Geo::Point(a), Geo::Point(b)) => Geo::Point([lerp(a[0], b[0], t), lerp(a[1], b[1], t)]),
        (Geo::Line(a), Geo::Line(b)) if a.len() == b.len() => {
            Geo::Line(a.iter().zip(b).map(|(p, q)| [lerp(p[0], q[0], t), lerp(p[1], q[1], t)]).collect())
        }
        _ => return None,
    })
}

/// The k-th of m vertical slices of a parent rect.
fn slice(parent: &Geo, k: usize, m: usize) -> Option<Geo> {
    let Geo::Rect([x0, x1, y0, y1]) = parent else { return None };
    let (a, b) = (k as f64 / m as f64, (k + 1) as f64 / m as f64);
    Some(Geo::Rect([*x0, *x1, lerp(*y0, *y1, a), lerp(*y0, *y1, b)]))
}

/// Children of each parent in a frame, ordered left to right.
fn children(items: &[Item], units: &HashMap<&str, &Geo>) -> HashMap<String, Vec<String>> {
    let mut c: HashMap<String, Vec<(f64, String)>> = HashMap::new();
    for it in items {
        if let (Some(p), Some(Geo::Rect(r))) = (&it.parent, units.get(it.key.as_str())) {
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

/// The items between frame `a` and frame `b`, back to front.
pub fn join(a: &[Item], b: &[Item], timing: Timing) -> Vec<Tweened> {
    let Timing { t, g, exit: exit_a, enter: enter_b } = timing;
    let e = ease(t);
    let ua: HashMap<&str, &Geo> = a.iter().map(|i| (i.key.as_str(), &i.geo)).collect();
    let ub: HashMap<&str, &Geo> = b.iter().map(|i| (i.key.as_str(), &i.geo)).collect();
    let ia: HashMap<&str, &Item> = a.iter().map(|i| (i.key.as_str(), i)).collect();
    let ib: HashMap<&str, &Item> = b.iter().map(|i| (i.key.as_str(), i)).collect();
    let (ca, cb) = (children(a, &ua), children(b, &ub));

    let mut items = vec![];
    for it in a {
        let k = it.key.as_str();
        match ib.get(k) {
            Some(jt) => match lerp_geo(ua[k], ub[k], g) {
                Some(geo) => items.push(Tweened { geo, fill: lerp_c(it.fill, jt.fill, e), size: lerp(it.size, jt.size, e), z: 0, h: lerp(it.h, jt.h, e), key: it.key.clone() }),
                None => {
                    items.push(Tweened { geo: ua[k].clone(), fill: alpha(it.fill, 1.0 - e), size: it.size, z: 0, h: it.h, key: it.key.clone() });
                    items.push(Tweened { geo: ub[k].clone(), fill: alpha(jt.fill, e), size: jt.size, z: 1, h: jt.h, key: jt.key.clone() });
                }
            },
            None => {
                // Collapse into the parent's slice when the parent returns.
                let target = it.parent.as_ref().and_then(|p| {
                    let sibs = ca.get(p)?;
                    slice(ub.get(p.as_str())?, sibs.iter().position(|s| s == k)?, sibs.len())
                });
                match target.and_then(|t| lerp_geo(ua[k], &t, g)) {
                    Some(geo) => items.push(Tweened { geo, fill: alpha(it.fill, 1.0 - smooth(0.75, 1.0, t)), size: it.size, z: 1, h: it.h, key: it.key.clone() }),
                    None => {
                        // A parent whose children take over leaves quickly.
                        let fade = if cb.contains_key(k) { 1.0 - smooth(0.0, 0.25, t) } else { exit_a };
                        items.push(Tweened { geo: ua[k].clone(), fill: alpha(it.fill, fade), size: it.size, z: 0, h: it.h, key: it.key.clone() });
                    }
                }
            }
        }
    }
    for jt in b {
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
        match start.and_then(|(s, pf)| lerp_geo(&s, ub[k], g).map(|geo| (geo, pf))) {
            Some((geo, pf)) => items.push(Tweened { geo, fill: lerp_c(pf, jt.fill, e), size: jt.size, z: 1, h: jt.h, key: jt.key.clone() }),
            None => {
                let fade = if ca.contains_key(k) { smooth(0.75, 1.0, t) } else { enter_b };
                items.push(Tweened { geo: ub[k].clone(), fill: alpha(jt.fill, fade), size: jt.size, z: 1, h: jt.h, key: jt.key.clone() });
            }
        }
    }
    items.sort_by(|p, q| (p.z, p.size as i64).cmp(&(q.z, q.size as i64)));
    items
}
