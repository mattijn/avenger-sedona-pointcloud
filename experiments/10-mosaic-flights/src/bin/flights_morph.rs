//! Does a brush follow a bar chart into a donut? The arrival-delay
//! histogram of the 10M flights, with a brush held in `avenger-selection`,
//! morphs into a donut through `avenger-transition` (the bars first stack
//! into one bar, then the plane bends) and `avenger-coords` (`Bend`, fitted
//! to the panel in every frame). The
//! brush is one more keyed item in the same layouts, so it bends with the
//! bars.
//!
//! What it measures, and prints:
//! 1. in every frame, whether the points of every bar that lie inside the
//!    drawn brush are exactly the points whose values the brush selects,
//!    against a brush left as a rectangle in pixels;
//! 2. a brush read back from the donut (`Bend::invert`, `Bins::value_at`)
//!    is the same range, and selects the same flights through the crate;
//! 3. with a second brush on distance, the delay counts change, and the
//!    brush's arc on the donut with them.
//!
//! Usage: cargo run --release -p lidar-flights --bin flights_morph -- <flights-10m.parquet> [out-dir] [--frames dir]

use std::time::Instant;

use avenger_coords::{draw, Bend, CoordinateSystem, Fitted, Screen};
use avenger_scenegraph::marks::{group::SceneGroup, mark::SceneMark};
use avenger_selection::{SelectionSet, SelectionValue, ValueTest};
use avenger_transition::bins::{Bins, Layout};
use avenger_transition::{ease, join, phases, Geo, Item, Plane, Timing, Tweened};
use datafusion::prelude::SessionContext;
use lidar_flights::*;

/// One square panel.
const P: f64 = 240.0;
/// The brush on arrival delay, in minutes. Neither end is a bin edge, so the
/// brush cuts two bars.
const BRUSH: [f64; 2] = [45.0, 125.0];
/// The second brush, on distance, for part 3.
const DISTANCE: [f64; 2] = [0.0, 800.0];
/// The donut's ring and the brush's reach across it, in unit y.
const RING: [f64; 2] = [0.5, 0.95];
const BRUSH_Y_STACK: [f64; 2] = [0.42, 1.0];
const BRUSH_FILL: [f32; 4] = [0.85, 0.2, 0.15, 0.10];
const BRUSH_EDGE: [f32; 4] = [0.85, 0.2, 0.15, 1.0];

fn delay() -> &'static Plot {
    &PLOTS[0]
}

/// The brushes as the window holds them: a range on each panel's column.
fn state(delay_range: Option<[f64; 2]>, distance: Option<[f64; 2]>) -> Result<SelectionSet, Error> {
    let mut s = empty();
    for (i, r) in [(0, delay_range), (2, distance)] {
        if let Some(r) = r {
            s = s.set(&brush_producer(&PLOTS[i]), SelectionValue::tuple([(pid("value"), ValueTest::range(r[0]..r[1]))]))?;
        }
    }
    Ok(s)
}

/// The delay panel's bins under the other panels' brushes: every display
/// bin of the domain, empty ones included, so the layouts line up.
async fn bins(ctx: &SessionContext, s: &SelectionSet) -> Result<Bins, Error> {
    let p = delay();
    let n = ((p.domain[1] - p.domain[0]) / p.step).round() as usize;
    let mut counts = vec![0.0; n];
    for (b, v) in histogram(ctx, p, cross(0).predicate(s)?, None).await? {
        if (0..n as i64).contains(&b) {
            counts[b as usize] = v;
        }
    }
    let edges = (0..=n).map(|i| p.domain[0] + i as f64 * p.step).collect();
    Ok(Bins::new(edges, counts))
}

/// A frame's items: the bars, the part of each bar the brush selects, and
/// the brush itself, all keyed, in one layout.
fn frame(b: &Bins, layout: Layout, brush: [f64; 2]) -> Vec<Item> {
    let mut items = b.items("bar", layout, |_| GREY);
    items.extend(b.clipped("sel", brush, layout, BLUE));
    let y = match layout {
        Layout::Bars { .. } => [0.0, 1.0],
        Layout::Stack { .. } => BRUSH_Y_STACK,
    };
    items.push(Item { key: "brush".into(), parent: None, geo: Geo::Rect(b.interval(brush, layout, y)), fill: BRUSH_FILL, size: 0.0, h: 0.0 });
    items
}

/// The chart at time `t` from bars to donut: the items and the bend.
fn at(b: &Bins, t: f64, brush: [f64; 2]) -> (Vec<Tweened>, f64) {
    let max = b.counts.iter().cloned().fold(1.0, f64::max);
    let (from, to) = (frame(b, Layout::Bars { max }, brush), frame(b, Layout::Stack { y: RING }, brush));
    let (bend, g) = phases(Plane::Cartesian, Plane::Polar, t);
    let e = ease(t);
    (join(&from, &to, Timing { t, g, exit: 1.0 - e, enter: e }), bend)
}

fn rect_of(t: &Tweened) -> [f64; 4] {
    match t.geo {
        Geo::Rect(r) => r,
        _ => unreachable!("a histogram has only rects"),
    }
}

/// The outline of a unit rect through a system, as a closed polygon.
fn outline_of(cs: &dyn CoordinateSystem, r: [f64; 4]) -> Vec<Screen> {
    let ring = vec![vec![r[0], r[2]], vec![r[1], r[2]], vec![r[1], r[3]], vec![r[0], r[3]]];
    cs.project_line(&ring, true).into_iter().flatten().collect()
}

fn inside(poly: &[Screen], p: Screen) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

/// Part 1 for one frame: sample every bar at 20 × 5 points, and compare
/// "inside the drawn brush" with "the brush selects this value". Points
/// within 2 % of a bin's width of a brush edge are left out, as the outline
/// is sampled at 0.25 px. Returns (points, disagreements), for the brush as
/// drawn and for the brush left as its rectangle in pixels at t = 0.
fn check(b: &Bins, t: f64) -> ((usize, usize), (usize, usize)) {
    let (items, bend) = at(b, t, BRUSH);
    let bent = Bend { width: P, height: P, t: bend };
    let cs = Fitted::new(&bent, P, P);
    let find = |k: &str| items.iter().find(|i| i.key == k).map(rect_of);
    let brush = outline_of(&cs, find("brush").unwrap());
    let flat = outline_of(&Bend { width: P, height: P, t: 0.0 }, rect_of(&at(b, 0.0, BRUSH).0.into_iter().find(|i| i.key == "brush").unwrap()));
    let (mut n, mut wrong_bent, mut wrong_flat) = (0, 0, 0);
    for i in 0..b.counts.len() {
        let Some(r) = find(&format!("bar{i}")) else { continue };
        if b.counts[i] <= 0.0 {
            continue;
        }
        let w = b.edges[i + 1] - b.edges[i];
        for sx in 0..20 {
            let f = (sx as f64 + 0.5) / 20.0;
            let v = b.edges[i] + f * w;
            if (v - BRUSH[0]).abs() < 0.02 * w || (v - BRUSH[1]).abs() < 0.02 * w {
                continue;
            }
            let selected = v > BRUSH[0] && v < BRUSH[1];
            for sy in 0..5 {
                let u = [r[0] + f * (r[1] - r[0]), r[2] + (0.1 + 0.2 * sy as f64) * (r[3] - r[2])];
                let s = cs.project(&u).unwrap();
                n += 1;
                wrong_bent += (inside(&brush, s) != selected) as usize;
                wrong_flat += (inside(&flat, s) != selected) as usize;
            }
        }
    }
    ((n, wrong_bent), (n, wrong_flat))
}

/// The arc the brush spans on the donut, in degrees.
fn arc(b: &Bins, brush: [f64; 2]) -> f64 {
    let r = b.interval(brush, Layout::Stack { y: RING }, BRUSH_Y_STACK);
    (r[1] - r[0]) * 360.0
}

/// Draw one frame as a panel at `origin`, with a caption.
fn panel_at(b: &Bins, t: f64, brush: [f64; 2], origin: [f32; 2], caption: &str, pixel_brush: bool) -> SceneMark {
    let (items, bend) = at(b, t, brush);
    let bent = Bend { width: P, height: P, t: bend };
    let cs = Fitted::new(&bent, P, P);
    let mut marks = vec![];
    let draw_rects = |sel: &dyn Fn(&Tweened) -> bool, stroke: [f32; 4]| -> SceneMark {
        let chosen: Vec<&Tweened> = items.iter().filter(|i| sel(i)).collect();
        let lo: Vec<[f64; 2]> = chosen.iter().map(|i| { let r = rect_of(i); [r[0], r[2]] }).collect();
        let hi: Vec<[f64; 2]> = chosen.iter().map(|i| { let r = rect_of(i); [r[1], r[3]] }).collect();
        let fill: Vec<[f32; 4]> = chosen.iter().map(|i| i.fill).collect();
        draw::rects(&cs, &lo, &hi, &fill, stroke).0
    };
    if pixel_brush {
        // The brush as a rectangle in pixels, where it was drawn on the bars.
        let (flat, _) = at(b, 0.0, brush);
        let r = rect_of(flat.iter().find(|i| i.key == "brush").unwrap());
        let flat_cs = Bend { width: P, height: P, t: 0.0 };
        marks.push(draw::rects(&flat_cs, &[[r[0], r[2]]], &[[r[1], r[3]]], &[BRUSH_FILL], BRUSH_EDGE).0);
    } else {
        marks.push(draw_rects(&|i| i.key == "brush", BRUSH_EDGE));
    }
    marks.push(draw_rects(&|i| i.key.starts_with("bar"), [1.0, 1.0, 1.0, 1.0]));
    marks.push(draw_rects(&|i| i.key.starts_with("sel"), [0.0; 4]));
    marks.push(text(caption, 0.0, P as f32 + 22.0, 11.0, INK));
    SceneGroup { origin, marks, ..Default::default() }.into()
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().collect();
    let data = args.get(1).cloned().unwrap_or("data/flights-10m.parquet".into());
    let out = args.get(2).filter(|a| !a.starts_with("--")).cloned().unwrap_or("experiments/10-mosaic-flights/images".into());
    let frames_dir = args.iter().position(|a| a == "--frames").and_then(|i| args.get(i + 1).cloned());
    std::fs::create_dir_all(&out)?;

    let ctx = SessionContext::new();
    let t0 = Instant::now();
    let n = load(&ctx, &data).await?;
    println!("{} flights loaded in {:.0} ms", thousands(n), ms(t0));

    // The brush on delay alone: the delay panel is not filtered by its own brush.
    let s1 = state(Some(BRUSH), None)?;
    let t0 = Instant::now();
    let b1 = bins(&ctx, &s1).await?;
    println!("delay histogram, {} bins: {:.0} ms", b1.counts.len(), ms(t0));

    // 1. Does the brush follow?
    println!("\n1. points of the bars inside the drawn brush, against the points the brush selects");
    println!("   {:>5}  {:>6}  {:>22}  {:>22}", "t", "bend", "brush bent with bars", "brush left in pixels");
    let (mut worst_bent, mut worst_flat) = (0, 0);
    for k in 0..=20 {
        let t = k as f64 / 20.0;
        let ((n, wb), (_, wf)) = check(&b1, t);
        let (bend, _) = phases(Plane::Cartesian, Plane::Polar, t);
        worst_bent = worst_bent.max(wb);
        worst_flat = worst_flat.max(wf);
        println!("   {t:>5.2}  {bend:>6.2}  {:>14} of {n:>5}  {:>14} of {n:>5}", format!("{wb} wrong"), format!("{wf} wrong"));
    }
    println!("   most wrong in one frame: bent {worst_bent}, in pixels {worst_flat}");

    // 2. A brush read back from the donut.
    println!("\n2. the brush read back from the donut");
    let bend1 = Bend { width: P, height: P, t: 1.0 };
    let donut = Fitted::new(&bend1, P, P);
    let r = b1.interval(BRUSH, Layout::Stack { y: RING }, BRUSH_Y_STACK);
    let mid_y = 0.5 * (RING[0] + RING[1]);
    // Where the pointer would press and release: the arc's two ends, mid-ring.
    let (press, release) = (donut.project(&[r[0], mid_y]).unwrap(), donut.project(&[r[1], mid_y]).unwrap());
    let ua = donut.invert(press).unwrap();
    let ub = donut.invert(release).unwrap();
    let stack = Layout::Stack { y: RING };
    let back = [b1.value_at(ua[0], stack), b1.value_at(ub[0], stack)];
    println!("   pressed at ({:.1}, {:.1}) px, released at ({:.1}, {:.1}) px", press[0], press[1], release[0], release[1]);
    println!("   read back as {:.9}..{:.9} min (brush {}..{})", back[0], back[1], BRUSH[0], BRUSH[1]);
    let count = |s: SelectionSet| {
        let ctx = &ctx;
        async move {
            let f = cross(1).predicate(&s)?;
            Ok::<usize, Error>(ctx.table("flights").await?.filter(f)?.count().await?)
        }
    };
    let (c0, c1) = (count(state(Some(BRUSH), None)?).await?, count(state(Some(back), None)?).await?);
    println!("   flights the brush selects through avenger-selection: {} drawn on the bars, {} read back from the donut", thousands(c0), thousands(c1));

    // 3. Cross-filtered by a brush on distance.
    println!("\n3. a brush on distance ({}–{} miles) changes the delay counts, and the arc with them", DISTANCE[0], DISTANCE[1]);
    let b2 = bins(&ctx, &state(Some(BRUSH), Some(DISTANCE))?).await?;
    let share = |b: &Bins| {
        let total: f64 = b.counts.iter().sum();
        let r = b.interval(BRUSH, Layout::Stack { y: RING }, BRUSH_Y_STACK);
        (r[1] - r[0], total)
    };
    let ((s_a, n_a), (s_b, n_b)) = (share(&b1), share(&b2));
    println!("   without it: the brush's arc is {:.1}°, {:.2} % of {} flights", arc(&b1, BRUSH), s_a * 100.0, thousands(n_a as usize));
    println!("   with it:    the brush's arc is {:.1}°, {:.2} % of {} flights", arc(&b2, BRUSH), s_b * 100.0, thousands(n_b as usize));
    let ((_, wb), _) = check(&b2, 1.0);
    println!("   points wrong on the cross-filtered donut: {wb}");

    // The figure.
    let gap = 36.0;
    let cols = 7.0;
    let size = [24.0 + cols * (P as f32 + gap), 90.0 + 2.0 * (P as f32 + 60.0)];
    let mut marks = heading("A brush through a bar-to-donut morph",
        &format!("{} flights · arrival delay, brushed {}–{} min in avenger-selection · the bars stack, then bend (avenger-transition, avenger-coords); the brush is one more item", thousands(n), BRUSH[0], BRUSH[1]));
    let row1 = [(0.0, "bars"), (0.25, "stacking"), (0.5, "one stacked bar"), (0.625, "bending"), (0.75, "half bent"), (0.875, "bending"), (1.0, "donut")];
    for (k, (t, cap)) in row1.iter().enumerate() {
        let x = 24.0 + k as f32 * (P as f32 + gap);
        marks.push(panel_at(&b1, *t, BRUSH, [x, 80.0], &format!("t = {t:.3} · {cap}"), false));
    }
    let y2 = 80.0 + P as f32 + 60.0;
    let row2: [(f64, &Bins, &str, bool); 4] = [
        (0.75, &b1, "half bent, brush left in pixels", true),
        (0.75, &b1, "half bent, brush bent with the bars", false),
        (1.0, &b1, &format!("donut: brush arc {:.0}°", arc(&b1, BRUSH)), false),
        (1.0, &b2, &format!("distance {}–{} mi brushed: arc {:.0}°", DISTANCE[0], DISTANCE[1], arc(&b2, BRUSH)), false),
    ];
    for (k, (t, b, cap, px)) in row2.iter().enumerate() {
        let x = 24.0 + k as f32 * (P as f32 + gap);
        marks.push(panel_at(b, *t, BRUSH, [x, y2], cap, *px));
    }
    render(marks, size, &format!("{out}/morph_brush.png")).await?;

    // Frames for a video: there and back, 3 s each way at 30 frames a second.
    if let Some(dir) = frames_dir {
        std::fs::create_dir_all(&dir)?;
        let size = [P as f32 + 48.0, P as f32 + 60.0];
        let ts: Vec<f64> = (0..=90).map(|k| k as f64 / 90.0).chain(std::iter::repeat(1.0).take(20)).chain((0..=90).rev().map(|k| k as f64 / 90.0)).collect();
        for (k, t) in ts.iter().enumerate() {
            render(vec![panel_at(&b1, *t, BRUSH, [24.0, 20.0], &format!("t = {t:.2}"), false)], size, &format!("{dir}/frame_{k:05}.png")).await?;
        }
    }
    Ok(())
}
