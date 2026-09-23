//! The prototype. A coordinate system is a list of channels, a point
//! transform, a line transform and one flag; everything else (grids, labels,
//! morphs, rect-to-polygon fallback) is derived from those in `draw.rs`.

use avenger_geo::{MultiLine, PolylineSink, Projection, ProjectionKind, Projector};

/// Pixels inside the plot, origin top-left.
pub type Screen = [f64; 2];

#[derive(Clone, Copy, Debug)]
pub struct Channel {
    pub name: &'static str,
    /// Value used when a mark leaves the channel out. `None` means required.
    pub default: Option<f64>,
    /// Input range the grid spans: unit space for scaled channels, degrees
    /// for lon/lat.
    pub extent: (f64, f64),
}

pub trait CoordinateSystem {
    fn name(&self) -> String;
    fn channels(&self) -> Vec<Channel>;

    /// Channel inputs (all channels, defaults filled in) to plot pixels.
    /// `None` when the point cannot be drawn.
    fn project(&self, p: &[f64]) -> Option<Screen>;

    /// True when axis-aligned rectangles in input space stay axis-aligned
    /// rectangles on screen, so rect marks can stay rect instances.
    fn is_rectilinear(&self) -> bool {
        false
    }

    /// A polyline in input space to one or more screen polylines. The
    /// default samples adaptively in input space, which is right for any
    /// planar transform. Spherical systems override it.
    fn project_line(&self, pts: &[Vec<f64>], closed: bool) -> Vec<Vec<Screen>> {
        sample_line(self, pts, closed)
    }

    /// Painter's order: larger is further away. Only 3D systems need it.
    fn depth(&self, _p: &[f64]) -> f64 {
        0.0
    }

    /// Where tick labels for channel `i` sit: the value of the other
    /// channel, and the direction (−1 or +1 along it) that points outward.
    fn label_side(&self, i: usize) -> (f64, f64) {
        let other = self.channels()[1 - i];
        (other.extent.0, -1.0)
    }
}

/// Check the channels a mark sets against a system: unknown names and
/// missing required channels are errors, and everything left out is filled
/// with its default. Returns, per system channel, the index into the mark's
/// channels or the default value.
pub fn resolve(
    cs: &dyn CoordinateSystem,
    given: &[&str],
) -> Result<Vec<Result<usize, f64>>, String> {
    let channels = cs.channels();
    for g in given {
        if !channels.iter().any(|c| c.name == *g) {
            let names: Vec<_> = channels.iter().map(|c| c.name).collect();
            return Err(format!(
                "`{g}` is not a channel of {}; it has {}",
                cs.name(),
                names.join(", ")
            ));
        }
    }
    channels
        .iter()
        .map(|c| match given.iter().position(|g| *g == c.name) {
            Some(i) => Ok(Ok(i)),
            None => c
                .default
                .map(Err)
                .ok_or_else(|| format!("{} requires `{}`", cs.name(), c.name)),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Adaptive sampling: subdivide a segment in input space until its projected
// midpoint lies within TOL pixels of the projected chord.

const TOL: f64 = 0.25;
const MAX_DEPTH: u32 = 14;
const MAX_CHORD: f64 = 48.0;

pub fn sample_line<C: CoordinateSystem + ?Sized>(
    cs: &C,
    pts: &[Vec<f64>],
    closed: bool,
) -> Vec<Vec<Screen>> {
    let mut out = vec![];
    let mut cur: Vec<Screen> = vec![];
    if pts.is_empty() {
        return out;
    }
    let segs = if closed { pts.len() } else { pts.len() - 1 };
    let mut pa = cs.project(&pts[0]);
    if let Some(p) = pa {
        cur.push(p);
    }
    for i in 0..segs {
        let (a, b) = (&pts[i], &pts[(i + 1) % pts.len()]);
        let pb = cs.project(b);
        subdivide(cs, a, pa, b, pb, 0, &mut cur, &mut out);
        pa = pb;
    }
    if cur.len() > 1 {
        out.push(cur);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn subdivide<C: CoordinateSystem + ?Sized>(
    cs: &C,
    a: &[f64],
    pa: Option<Screen>,
    b: &[f64],
    pb: Option<Screen>,
    depth: u32,
    cur: &mut Vec<Screen>,
    out: &mut Vec<Vec<Screen>>,
) {
    let m: Vec<f64> = a.iter().zip(b).map(|(x, y)| 0.5 * (x + y)).collect();
    let pm = cs.project(&m);
    let done = match (pa, pb, pm) {
        (Some(pa), Some(pb), Some(pm)) => {
            let chord = [0.5 * (pa[0] + pb[0]), 0.5 * (pa[1] + pb[1])];
            let dev = (pm[0] - chord[0]).hypot(pm[1] - chord[1]);
            let len = (pb[0] - pa[0]).hypot(pb[1] - pa[1]);
            depth >= MAX_DEPTH || (dev < TOL && (cs.is_rectilinear() || len < MAX_CHORD))
        }
        _ => depth >= MAX_DEPTH,
    };
    if !done {
        subdivide(cs, a, pa, &m, pm, depth + 1, cur, out);
        subdivide(cs, &m, pm, b, pb, depth + 1, cur, out);
        return;
    }
    match pb {
        Some(p) => cur.push(p),
        None => {
            if cur.len() > 1 {
                out.push(std::mem::take(cur));
            }
            cur.clear();
        }
    }
}

// ---------------------------------------------------------------------------

/// The default. Unit inputs, y up.
pub struct Cartesian {
    pub width: f64,
    pub height: f64,
}

impl CoordinateSystem for Cartesian {
    fn name(&self) -> String {
        "cartesian".into()
    }
    fn channels(&self) -> Vec<Channel> {
        vec![
            Channel {
                name: "x",
                default: None,
                extent: (0.0, 1.0),
            },
            Channel {
                name: "y",
                default: None,
                extent: (0.0, 1.0),
            },
        ]
    }
    fn project(&self, p: &[f64]) -> Option<Screen> {
        Some([p[0] * self.width, (1.0 - p[1]) * self.height])
    }
    fn is_rectilinear(&self) -> bool {
        true
    }
    fn project_line(&self, pts: &[Vec<f64>], closed: bool) -> Vec<Vec<Screen>> {
        let mut line: Vec<Screen> = pts.iter().filter_map(|p| self.project(p)).collect();
        if closed && !line.is_empty() {
            line.push(line[0]);
        }
        vec![line]
    }
}

/// θ from the first channel (clockwise from 12 o'clock), r from the second.
/// The channels keep the names x and y, so a Cartesian spec needs no
/// renaming to be drawn in polar.
pub struct Polar {
    pub width: f64,
    pub height: f64,
    /// Inner radius as a fraction of the outer one: 0 for a pie or rose,
    /// > 0 for a donut.
    pub inner: f64,
}

impl Polar {
    fn centre_radius(&self) -> ([f64; 2], f64) {
        (
            [self.width / 2.0, self.height / 2.0],
            0.5 * self.width.min(self.height),
        )
    }
}

impl CoordinateSystem for Polar {
    fn name(&self) -> String {
        "polar".into()
    }
    fn channels(&self) -> Vec<Channel> {
        vec![
            Channel {
                name: "x",
                default: None,
                extent: (0.0, 1.0),
            },
            Channel {
                name: "y",
                default: None,
                extent: (0.0, 1.0),
            },
        ]
    }
    fn project(&self, p: &[f64]) -> Option<Screen> {
        let (c, r_out) = self.centre_radius();
        let theta = p[0] * std::f64::consts::TAU;
        let r = r_out * (self.inner + (1.0 - self.inner) * p[1]);
        Some([c[0] + r * theta.sin(), c[1] - r * theta.cos()])
    }
    fn label_side(&self, i: usize) -> (f64, f64) {
        if i == 0 {
            (1.0, 1.0) // angle labels outside the outer circle
        } else {
            (0.0, -1.0) // radius labels left of the 12 o'clock spoke
        }
    }
}

/// Longitude and latitude in degrees through an `avenger-geo` projection.
/// Inputs are not scaled: the projection is the scale.
pub struct Spatial {
    pub label: String,
    pub projector: Projector,
    pub extent: [[f64; 2]; 2],
}

impl Spatial {
    /// Fit `kind` so the lon/lat `extent` fills a `width` × `height` plot.
    pub fn fit(
        label: &str,
        kind: ProjectionKind,
        rotate: [f64; 3],
        extent: [[f64; 2]; 2],
        width: f64,
        height: f64,
    ) -> Self {
        let [[x0, y0], [x1, y1]] = extent;
        let n = 32;
        let ring: Vec<[f64; 2]> = (0..=n)
            .map(|i| [x0 + (x1 - x0) * i as f64 / n as f64, y0])
            .chain((0..=n).map(|i| [x1, y0 + (y1 - y0) * i as f64 / n as f64]))
            .chain((0..=n).map(|i| [x1 - (x1 - x0) * i as f64 / n as f64, y1]))
            .chain((0..=n).map(|i| [x0, y1 - (y1 - y0) * i as f64 / n as f64]))
            .collect();
        let mut projection = Projection::new(kind).with_rotate(rotate);
        projection
            .fit_extent([[0.0, 0.0], [width, height]], &MultiLine(vec![ring]))
            .expect("fit the extent");
        Spatial {
            label: label.into(),
            projector: projection.build(),
            extent,
        }
    }
}

impl CoordinateSystem for Spatial {
    fn name(&self) -> String {
        format!("spatial ({})", self.label)
    }
    fn channels(&self) -> Vec<Channel> {
        let [[x0, y0], [x1, y1]] = self.extent;
        vec![
            Channel {
                name: "lon",
                default: None,
                extent: (x0, x1),
            },
            Channel {
                name: "lat",
                default: None,
                extent: (y0, y1),
            },
        ]
    }
    fn project(&self, p: &[f64]) -> Option<Screen> {
        self.projector.project(p[0], p[1]).map(|(x, y)| [x, y])
    }
    /// Great-circle resampling and clipping come from `avenger-geo`.
    fn project_line(&self, pts: &[Vec<f64>], closed: bool) -> Vec<Vec<Screen>> {
        let mut line: Vec<[f64; 2]> = pts.iter().map(|p| [p[0], p[1]]).collect();
        if closed && !line.is_empty() {
            line.push(line[0]);
        }
        let mut sink = PolylineSink::default();
        self.projector.stream(&MultiLine(vec![line]), &mut sink);
        let mut out = vec![];
        let mut cur = vec![];
        for ((x, y), d) in sink.x.iter().zip(&sink.y).zip(&sink.defined) {
            if *d {
                cur.push([*x as f64, *y as f64]);
            } else if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }
}

/// x, y and z in unit space, seen from `yaw` degrees around the vertical and
/// `elevation` degrees above the horizon (90 is straight down). z defaults to
/// 0, so a 2D chart lies on the floor.
pub struct Cartesian3d {
    pub width: f64,
    pub height: f64,
    pub yaw: f64,
    pub elevation: f64,
    /// Height of the unit z range relative to the unit x/y range.
    pub z_scale: f64,
}

impl Cartesian3d {
    fn view(&self, p: &[f64]) -> (f64, f64, f64) {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (se, ce) = self.elevation.to_radians().sin_cos();
        let (x, y, z) = (p[0] - 0.5, p[1] - 0.5, p[2] * self.z_scale);
        let xr = x * cy - y * sy;
        let yr = x * sy + y * cy;
        (xr, yr * se + z * ce, yr * ce - z * se)
    }
}

impl CoordinateSystem for Cartesian3d {
    fn name(&self) -> String {
        "cartesian3d".into()
    }
    fn channels(&self) -> Vec<Channel> {
        vec![
            Channel {
                name: "x",
                default: None,
                extent: (0.0, 1.0),
            },
            Channel {
                name: "y",
                default: None,
                extent: (0.0, 1.0),
            },
            Channel {
                name: "z",
                default: Some(0.0),
                extent: (0.0, 1.0),
            },
        ]
    }
    fn project(&self, p: &[f64]) -> Option<Screen> {
        let (sx, sy, _) = self.view(p);
        // The unit square's diagonal fits the width; the floor sits a little
        // below centre to leave room for z.
        let s = self.width.min(self.height * 1.4) / std::f64::consts::SQRT_2 * 0.92;
        Some([self.width / 2.0 + s * sx, self.height * 0.6 - s * sy])
    }
    fn depth(&self, p: &[f64]) -> f64 {
        self.view(p).2
    }
}

/// `a` at t = 0, `b` at t = 1, and the pointwise interpolation in between.
/// Both must take the same channels.
pub struct Blend<'a> {
    pub a: &'a dyn CoordinateSystem,
    pub b: &'a dyn CoordinateSystem,
    pub t: f64,
}

impl CoordinateSystem for Blend<'_> {
    fn name(&self) -> String {
        format!("blend({}, {}, {:.2})", self.a.name(), self.b.name(), self.t)
    }
    /// The longer channel list, so blending into `cartesian3d` supplies z.
    fn channels(&self) -> Vec<Channel> {
        let (a, b) = (self.a.channels(), self.b.channels());
        if b.len() > a.len() {
            b
        } else {
            a
        }
    }
    fn project(&self, p: &[f64]) -> Option<Screen> {
        let (pa, pb) = (self.a.project(p)?, self.b.project(p)?);
        Some([
            pa[0] + (pb[0] - pa[0]) * self.t,
            pa[1] + (pb[1] - pa[1]) * self.t,
        ])
    }
    fn is_rectilinear(&self) -> bool {
        self.t == 0.0 && self.a.is_rectilinear()
    }
    fn label_side(&self, i: usize) -> (f64, f64) {
        if self.t < 0.5 {
            self.a.label_side(i)
        } else {
            self.b.label_side(i)
        }
    }
}

/// Cartesian at t = 0, `Polar { inner: 0 }` at t = 1, and in between the
/// strip bent around a circle whose angle grows with t. This interpolates
/// the transform's parameters (curvature, radius, rotation) rather than its
/// output, so every intermediate frame is itself a valid polar-like system.
pub struct Bend {
    pub width: f64,
    pub height: f64,
    pub t: f64,
}

impl CoordinateSystem for Bend {
    fn name(&self) -> String {
        format!("bend(cartesian → polar, {:.2})", self.t)
    }
    fn channels(&self) -> Vec<Channel> {
        vec![
            Channel {
                name: "x",
                default: None,
                extent: (0.0, 1.0),
            },
            Channel {
                name: "y",
                default: None,
                extent: (0.0, 1.0),
            },
        ]
    }
    fn project(&self, p: &[f64]) -> Option<Screen> {
        let (w, h, t) = (self.width, self.height, self.t);
        if t < 1e-9 {
            return Cartesian {
                width: w,
                height: h,
            }
            .project(p);
        }
        let r1 = 0.5 * w.min(h); // outer radius of the final polar system
        let rm = 0.5 * r1; // its midline (y = 0.5) radius
        let phi = t * std::f64::consts::TAU; // angle the x range spans
        let s = w + (std::f64::consts::TAU * rm - w) * t; // midline length
        let rho = s / phi; // midline radius
        let thick = h + (r1 - h) * t; // radial extent of the unit y range
                                      // Bend with the strip's midpoint P on top of the circle, then turn
                                      // the whole picture about P, so the frame stays in place while x = 0
                                      // ends at 12 o'clock as in `Polar`.
        let pm = [w / 2.0, h / 2.0 + rm * t];
        let theta = (p[0] - 0.5) * phi;
        let r = rho + (p[1] - 0.5) * thick;
        let v = [r * theta.sin(), rho - r * theta.cos()];
        let (sa, ca) = (t * std::f64::consts::PI).sin_cos();
        Some([pm[0] + v[0] * ca - v[1] * sa, pm[1] + v[0] * sa + v[1] * ca])
    }
    fn label_side(&self, i: usize) -> (f64, f64) {
        if self.t < 0.5 {
            (0.0, -1.0)
        } else {
            Polar {
                width: self.width,
                height: self.height,
                inner: 0.0,
            }
            .label_side(i)
        }
    }
}

/// A transition between systems that read *different* inputs, such as
/// `cartesian` (unit metres) and `spatial` (lon/lat). Each position carries
/// both encodings, `a`'s first `split` values and then `b`'s, so the data
/// must know how to express itself in both. That is the job of a CRS on the
/// data, not of either coordinate system.
pub struct Paired<'a> {
    pub a: &'a dyn CoordinateSystem,
    pub b: &'a dyn CoordinateSystem,
    pub split: usize,
    pub t: f64,
}

impl CoordinateSystem for Paired<'_> {
    fn name(&self) -> String {
        format!(
            "paired({}, {}, {:.2})",
            self.a.name(),
            self.b.name(),
            self.t
        )
    }
    fn channels(&self) -> Vec<Channel> {
        let mut c = self.a.channels();
        c.extend(self.b.channels());
        c
    }
    fn project(&self, p: &[f64]) -> Option<Screen> {
        let (pa, pb) = (
            self.a.project(&p[..self.split])?,
            self.b.project(&p[self.split..])?,
        );
        Some([
            pa[0] + (pb[0] - pa[0]) * self.t,
            pa[1] + (pb[1] - pa[1]) * self.t,
        ])
    }
}

// ---------------------------------------------------------------------------
// Distortion lenses. Each is just another point transform over the same x/y
// channels, so bars, donuts and maps all pass through them unchanged.

fn unit_to_screen(p: [f64; 2], width: f64, height: f64) -> Screen {
    [p[0] * width, (1.0 - p[1]) * height]
}

/// Sarkar–Brown graphical fisheye: magnifies around `focus` within `radius`
/// (unit space), leaving everything outside untouched.
pub struct Fisheye {
    pub width: f64,
    pub height: f64,
    pub focus: [f64; 2],
    pub radius: f64,
    pub distortion: f64,
}

impl CoordinateSystem for Fisheye {
    fn name(&self) -> String {
        "fisheye".into()
    }
    fn channels(&self) -> Vec<Channel> {
        Cartesian {
            width: self.width,
            height: self.height,
        }
        .channels()
    }
    fn project(&self, p: &[f64]) -> Option<Screen> {
        let d = [p[0] - self.focus[0], p[1] - self.focus[1]];
        let r = d[0].hypot(d[1]);
        let q = if r > 0.0 && r < self.radius {
            let x = r / self.radius;
            let g = (self.distortion + 1.0) * x / (self.distortion * x + 1.0);
            let k = g * self.radius / r;
            [self.focus[0] + d[0] * k, self.focus[1] + d[1] * k]
        } else {
            [p[0], p[1]]
        };
        Some(unit_to_screen(q, self.width, self.height))
    }
}

/// A hyperbolic (Poincaré-disk-like) view: distance from `focus` is
/// compressed by tanh, so the whole plane fits in a disc and the focus is
/// magnified.
pub struct Hyperbolic {
    pub width: f64,
    pub height: f64,
    pub focus: [f64; 2],
    pub k: f64,
}

impl CoordinateSystem for Hyperbolic {
    fn name(&self) -> String {
        "hyperbolic".into()
    }
    fn channels(&self) -> Vec<Channel> {
        Cartesian {
            width: self.width,
            height: self.height,
        }
        .channels()
    }
    fn project(&self, p: &[f64]) -> Option<Screen> {
        let d = [p[0] - self.focus[0], p[1] - self.focus[1]];
        let r = d[0].hypot(d[1]);
        let big_r = 0.5 * self.width.min(self.height);
        let s = if r > 0.0 {
            (self.k * r).tanh() / r
        } else {
            self.k
        };
        Some([
            self.width / 2.0 + big_r * d[0] * s,
            self.height / 2.0 - big_r * d[1] * s,
        ])
    }
}

/// Rotates each point about the centre by an angle that fades from
/// `angle` at the centre to 0 at the corners.
pub struct Twirl {
    pub width: f64,
    pub height: f64,
    pub angle: f64,
}

impl CoordinateSystem for Twirl {
    fn name(&self) -> String {
        "twirl".into()
    }
    fn channels(&self) -> Vec<Channel> {
        Cartesian {
            width: self.width,
            height: self.height,
        }
        .channels()
    }
    fn project(&self, p: &[f64]) -> Option<Screen> {
        let d = [p[0] - 0.5, p[1] - 0.5];
        let r = d[0].hypot(d[1]);
        let fade = (1.0 - (r / std::f64::consts::FRAC_1_SQRT_2).min(1.0)).powi(2);
        let (s, c) = (self.angle.to_radians() * fade).sin_cos();
        let q = [0.5 + d[0] * c - d[1] * s, 0.5 + d[0] * s + d[1] * c];
        Some(unit_to_screen(q, self.width, self.height))
    }
}

/// Composition: a polar glyph at every position of an outer system
/// (vega-lite#7848). Inputs are the outer system's channels, then θ (unit
/// turn) and r (unit radius). The glyph lives in screen space, `radius`
/// pixels at r = 1, so pies stay round under any outer system, including a
/// map projection; only their centres go through the outer transform.
pub struct Nested<'a> {
    pub outer: &'a dyn CoordinateSystem,
    /// Number of outer input values at the start of each position.
    pub split: usize,
    pub radius: f64,
}

impl CoordinateSystem for Nested<'_> {
    fn name(&self) -> String {
        format!("{} × polar glyph", self.outer.name())
    }
    fn channels(&self) -> Vec<Channel> {
        let mut c = self.outer.channels();
        c.truncate(self.split);
        c.push(Channel {
            name: "theta",
            default: Some(0.0),
            extent: (0.0, 1.0),
        });
        c.push(Channel {
            name: "r",
            default: Some(1.0),
            extent: (0.0, 1.0),
        });
        c
    }
    fn project(&self, p: &[f64]) -> Option<Screen> {
        let o = self.outer.project(&p[..self.split])?;
        let theta = p[self.split] * std::f64::consts::TAU;
        let r = p[self.split + 1] * self.radius;
        Some([o[0] + r * theta.sin(), o[1] - r * theta.cos()])
    }
}
