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
//!    brush's arc on the donut with them;
//! 4. a brush adjusted on the donut, by dragging its ends along the ring
//!    and reading each pointer position back, selects its flights through
//!    the crate and follows the donut back into bars.
//!
//! Usage: cargo run --release -p lidar-flights --bin flights_morph -- <flights-10m.parquet> [out-dir] [--frames dir]

use std::time::Instant;

use avenger_coords::{draw, Bend, CoordinateSystem, Fitted, Screen};
use avenger_color::ColorOrGradient;
use avenger_scenegraph::marks::{group::SceneGroup, mark::SceneMark, symbol::SceneSymbolMark};
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
/// Where part 4 drags the brush's ends to on the donut: around on time.
const ADJUSTED: [f64; 2] = [-20.0, 15.0];
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
fn check(b: &Bins, t: f64, brush_range: [f64; 2]) -> ((usize, usize), (usize, usize)) {
    let (items, bend) = at(b, t, brush_range);
    let bent = Bend { width: P, height: P, t: bend };
    let cs = Fitted::new(&bent, P, P);
    let find = |k: &str| items.iter().find(|i| i.key == k).map(rect_of);
    let brush = outline_of(&cs, find("brush").unwrap());
    let flat = outline_of(&Bend { width: P, height: P, t: 0.0 }, rect_of(&at(b, 0.0, brush_range).0.into_iter().find(|i| i.key == "brush").unwrap()));
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
            if (v - brush_range[0]).abs() < 0.02 * w || (v - brush_range[1]).abs() < 0.02 * w {
                continue;
            }
            let selected = v > brush_range[0] && v < brush_range[1];
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

/// A drag on the donut: the pointer moves along the middle of the ring from
/// the brush's edge `edge` (0 its start, 1 its end) at value `from` to the
/// place of value `to`, and every position is read back into a value
/// (`Fitted::invert`, then `Bins::value_at`). Returns, per frame, the
/// brush and the pointer in panel pixels.
fn drag(b: &Bins, brush: [f64; 2], edge: usize, to: f64, frames: usize) -> Vec<([f64; 2], Screen)> {
    let bend1 = Bend { width: P, height: P, t: 1.0 };
    let donut = Fitted::new(&bend1, P, P);
    let stack = Layout::Stack { y: RING };
    let mid = 0.5 * (RING[0] + RING[1]);
    let (u0, u1) = (b.position(brush[edge], stack), b.position(to, stack));
    (1..=frames)
        .map(|k| {
            let u = u0 + (u1 - u0) * ease(k as f64 / frames as f64);
            let pointer = donut.project(&[u, mid]).unwrap();
            let mut now = brush;
            now[edge] = b.value_at(donut.invert(pointer).unwrap()[0], stack);
            (now, pointer)
        })
        .collect()
}

/// The arc the brush spans on the donut, in degrees.
fn arc(b: &Bins, brush: [f64; 2]) -> f64 {
    let r = b.interval(brush, Layout::Stack { y: RING }, BRUSH_Y_STACK);
    (r[1] - r[0]) * 360.0
}

/// Draw one frame as a panel at `origin`, with a caption.
fn panel_at(b: &Bins, t: f64, brush: [f64; 2], origin: [f32; 2], caption: &str, pixel_brush: bool, pointer: Option<Screen>) -> SceneMark {
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
    if let Some(p) = pointer {
        marks.push(SceneSymbolMark { len: 1, x: vec![p[0] as f32].into(), y: vec![p[1] as f32].into(), size: 160.0.into(),
            fill: ColorOrGradient::Color([0.85, 0.2, 0.15, 0.55]).into(), stroke: ColorOrGradient::Color([1.0; 4]).into(),
            stroke_width: Some(2.0), interactive: false, ..Default::default() }.into());
    }
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
        let ((n, wb), (_, wf)) = check(&b1, t, BRUSH);
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
    let ((_, wb), _) = check(&b2, 1.0, BRUSH);
    println!("   points wrong on the cross-filtered donut: {wb}");

    // 4. The brush adjusted on the donut, then back to bars.
    println!("\n4. the brush adjusted on the donut: its start dragged to {} min, then its end to {} min", ADJUSTED[0], ADJUSTED[1]);
    let start = drag(&b1, BRUSH, 0, ADJUSTED[0], 45);
    let end = drag(&b1, start.last().unwrap().0, 1, ADJUSTED[1], 45);
    let (adjusted, last_pointer) = *end.last().unwrap();
    println!("   released at ({:.1}, {:.1}) px, read back as {:.9}..{:.9} min", last_pointer[0], last_pointer[1], adjusted[0], adjusted[1]);
    let c2 = count(state(Some(adjusted), None)?).await?;
    let clipped: f64 = (0..b1.counts.len()).filter_map(|i| {
        let (lo, hi) = (adjusted[0].max(b1.edges[i]), adjusted[1].min(b1.edges[i + 1]));
        (lo < hi).then(|| b1.counts[i] * (hi - lo) / (b1.edges[i + 1] - b1.edges[i]))
    }).sum();
    println!("   flights it selects through avenger-selection: {} ({:.2} %); its arc stands for {:.2} %", thousands(c2), c2 as f64 / n as f64 * 100.0, clipped / n as f64 * 100.0);
    let mut worst = 0;
    for k in 0..=20 {
        worst = worst.max(check(&b1, k as f64 / 20.0, adjusted).0 .1);
    }
    println!("   back to bars: most points wrong in one of 21 frames: {worst} of 2,500");

    // The figure.
    let gap = 36.0;
    let cols = 7.0;
    let size = [24.0 + cols * (P as f32 + gap), 90.0 + 2.0 * (P as f32 + 60.0)];
    let mut marks = heading("A brush through a bar-to-donut morph",
        &format!("{} flights · arrival delay, brushed {}–{} min in avenger-selection · the bars stack, then bend (avenger-transition, avenger-coords); the brush is one more item", thousands(n), BRUSH[0], BRUSH[1]));
    let row1 = [(0.0, "bars"), (0.25, "stacking"), (0.5, "one stacked bar"), (0.625, "bending"), (0.75, "half bent"), (0.875, "bending"), (1.0, "donut")];
    for (k, (t, cap)) in row1.iter().enumerate() {
        let x = 24.0 + k as f32 * (P as f32 + gap);
        marks.push(panel_at(&b1, *t, BRUSH, [x, 80.0], &format!("t = {t:.3} · {cap}"), false, None));
    }
    let y2 = 80.0 + P as f32 + 60.0;
    let range = |r: [f64; 2]| format!("{:.0}–{:.0} min", r[0], r[1]);
    let row2: [(f64, &Bins, [f64; 2], String, bool, Option<Screen>); 7] = [
        (0.75, &b1, BRUSH, "half bent, brush left in pixels".into(), true, None),
        (0.75, &b1, BRUSH, "half bent, brush bent with the bars".into(), false, None),
        (1.0, &b2, BRUSH, format!("distance {}–{} mi brushed: arc {:.0}°", DISTANCE[0], DISTANCE[1], arc(&b2, BRUSH)), false, None),
        (1.0, &b1, adjusted, format!("dragged on the donut: {}", range(adjusted)), false, Some(last_pointer)),
        (0.75, &b1, adjusted, "back: unbending".into(), false, None),
        (0.25, &b1, adjusted, "back: unstacking".into(), false, None),
        (0.0, &b1, adjusted, format!("back to bars: {}", range(adjusted)), false, None),
    ];
    for (k, (t, b, r, cap, px, ptr)) in row2.iter().enumerate() {
        let x = 24.0 + k as f32 * (P as f32 + gap);
        marks.push(panel_at(b, *t, *r, [x, y2], cap, *px, *ptr));
    }
    render(marks, size, &format!("{out}/morph_brush.png")).await?;

    // Frames for a video at 30 a second: into the donut (3 s), the brush's
    // two ends dragged along the ring (1.5 s each), and back into bars (3 s).
    if let Some(dir) = frames_dir {
        std::fs::create_dir_all(&dir)?;
        let size = [P as f32 + 48.0, P as f32 + 60.0];
        let mut shots: Vec<(f64, [f64; 2], Option<Screen>, String)> = vec![];
        let hold = |shots: &mut Vec<(f64, [f64; 2], Option<Screen>, String)>, n: usize| {
            let last = shots.last().unwrap().clone();
            shots.extend(std::iter::repeat(last).take(n));
        };
        shots.extend((0..=90).map(|k| (k as f64 / 90.0, BRUSH, None, format!("into a donut · brush {}", range(BRUSH)))));
        hold(&mut shots, 15);
        shots.extend(start.iter().chain(&end).map(|(r, p)| (1.0, *r, Some(*p), format!("dragging on the donut · brush {}", range(*r)))));
        hold(&mut shots, 15);
        shots.extend((0..=90).rev().map(|k| (k as f64 / 90.0, adjusted, None, format!("back to bars · brush {}", range(adjusted)))));
        hold(&mut shots, 30);
        for (k, (t, r, p, cap)) in shots.iter().enumerate() {
            render(vec![panel_at(&b1, *t, *r, [24.0, 20.0], cap, false, *p)], size, &format!("{dir}/frame_{k:05}.png")).await?;
        }
    }
    Ok(())
}
