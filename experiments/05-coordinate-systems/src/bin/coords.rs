//! Experiment 5: the same mark specifications drawn through five coordinate
//! systems. Writes bars.png, morph.png and maps.png, and prints what each
//! system cost.
//!
//!     cargo run --release -p lidar-coords --bin coords -- <tile.copc.laz> <out-dir>

use std::time::Instant;

use avenger_common::canvas::CanvasDimensions;
use avenger_geo::ProjectionKind;
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_wgpu::canvas::{Canvas, PngCanvas};
use lidar_common::{las_context, INK, MUTED};
use lidar_coords::coords::{
    resolve, Bend, Blend, Cartesian, Cartesian3d, CoordinateSystem, Polar, Screen, Spatial,
};
use lidar_coords::draw::{self, Cost};
use lidar_coords::specs::{bars, load, map, stack, MapSetup};

const W: f64 = 300.0;
const H: f64 = 300.0;

type Error = Box<dyn std::error::Error>;

/// A chart specification: draws itself through whichever system it is given.
type Spec<'a> = dyn Fn(&dyn CoordinateSystem) -> (Vec<SceneMark>, Cost) + 'a;

struct Panel {
    title: String,
    subtitle: String,
    marks: Vec<SceneMark>,
}

struct Row {
    figure: &'static str,
    system: String,
    mark: &'static str,
    cost: Cost,
    ms: f64,
}

fn figure(panels: Vec<Panel>, cols: usize) -> SceneGraph {
    let (cell_w, cell_h) = (W as f32 + 120.0, H as f32 + 150.0);
    let marks = panels
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let origin = [
                (i % cols) as f32 * cell_w + 60.0,
                (i / cols) as f32 * cell_h + 90.0,
            ];
            let mut marks = p.marks;
            marks.push(draw::title(
                &[
                    (&p.title, 14.0, true, INK),
                    (&p.subtitle, 10.5, false, MUTED),
                ],
                [-40.0, -82.0],
            ));
            draw::group(origin, marks)
        })
        .collect();
    SceneGraph {
        marks,
        width: cols as f32 * cell_w,
        height: 0.0,
        origin: [0.0; 2],
    }
}

async fn render(mut scene: SceneGraph, rows: usize, path: &str) -> Result<(), Error> {
    scene.height = rows as f32 * (H as f32 + 150.0) + 10.0;
    let t = Instant::now();
    let mut canvas = PngCanvas::new(
        CanvasDimensions {
            size: [scene.width, scene.height],
            scale: 2.0,
        },
        Default::default(),
    )
    .await?;
    canvas.set_scene(&scene)?;
    canvas.render().await?.save(path)?;
    println!("  render {:.0?} -> {path}", t.elapsed());
    Ok(())
}

/// Best similarity transform from `a` to `b` (least squares), as scale,
/// rotation in degrees, and the largest residual in pixels.
fn similarity(a: &[Screen], b: &[Screen]) -> (f64, f64, f64) {
    let n = a.len() as f64;
    let ma = a
        .iter()
        .fold([0.0, 0.0], |s, p| [s[0] + p[0] / n, s[1] + p[1] / n]);
    let mb = b
        .iter()
        .fold([0.0, 0.0], |s, p| [s[0] + p[0] / n, s[1] + p[1] / n]);
    let (mut re, mut im, mut den) = (0.0, 0.0, 0.0);
    for (p, q) in a.iter().zip(b) {
        let (x, y) = (p[0] - ma[0], p[1] - ma[1]);
        let (u, v) = (q[0] - mb[0], q[1] - mb[1]);
        re += u * x + v * y;
        im += v * x - u * y;
        den += x * x + y * y;
    }
    let (sr, si) = (re / den, im / den);
    let max = a
        .iter()
        .zip(b)
        .map(|(p, q)| {
            let (x, y) = (p[0] - ma[0], p[1] - ma[1]);
            let (u, v) = (sr * x - si * y + mb[0], si * x + sr * y + mb[1]);
            (u - q[0]).hypot(v - q[1])
        })
        .fold(0.0, f64::max);
    (sr.hypot(si), si.atan2(sr).to_degrees(), max)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().collect();
    let (tile, out) = (&args[1], &args[2]);
    let ctx = las_context();
    let t = Instant::now();
    let (classes, cells, (z_lo, z_hi)) = load(&ctx, tile, 5.0).await?;
    println!("queries {:.2?}", t.elapsed());
    println!("points per class {:?}", classes.counts);

    // ---- channel validation: the system decides which channels exist ------
    let cart = Cartesian {
        width: W,
        height: H,
    };
    let polar = Polar {
        width: W,
        height: H,
        inner: 0.0,
    };
    let c3d = Cartesian3d {
        width: W,
        height: H,
        yaw: -30.0,
        elevation: 35.0,
        z_scale: 0.35,
    };
    println!("\nchannel check");
    for (cs, given) in [
        (&cart as &dyn CoordinateSystem, &["x", "y"][..]),
        (&c3d, &["x", "y"]),
        (&cart, &["x", "y", "z"]),
        (&c3d, &["x", "z"]),
    ] {
        let r = match resolve(cs, given) {
            Ok(v) => format!("ok {v:?}  (Ok(i) = mark channel i, Err(v) = default v)"),
            Err(e) => format!("error: {e}"),
        };
        println!("  {:<12} {:<15} {r}", cs.name(), format!("{given:?}"));
    }

    let mut rows: Vec<Row> = vec![];
    let mut panel = |figure: &'static str,
                     mark: &'static str,
                     cs: &dyn CoordinateSystem,
                     title: String,
                     subtitle: &str,
                     f: &Spec| {
        let t = Instant::now();
        let (marks, cost) = f(cs);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        rows.push(Row {
            figure,
            system: cs.name(),
            mark,
            cost,
            ms,
        });
        Panel {
            title,
            subtitle: subtitle.into(),
            marks,
        }
    };

    // ---- figure 1: bars ----------------------------------------------------
    let systems: [(&dyn CoordinateSystem, &str); 3] = [
        (&cart, "cartesian (the default)"),
        (&polar, "polar: θ ← x, r ← y"),
        (&c3d, "cartesian3d: z left out, so z = 0"),
    ];
    let mut panels = vec![];
    for (cs, sub) in systems {
        panels.push(panel(
            "bars",
            "rect ×6",
            cs,
            "Points per class".into(),
            sub,
            &|cs| bars(cs, &classes),
        ));
    }
    for (cs, sub) in systems {
        panels.push(panel(
            "bars",
            "rect ×6 stacked",
            cs,
            "Share per class".into(),
            sub,
            &|cs| stack(cs, &classes),
        ));
    }
    render(figure(panels, 3), 2, &format!("{out}/bars.png")).await?;

    // ---- figure 2: morph ---------------------------------------------------
    let mut panels = vec![];
    for k in 0..5 {
        let t = k as f64 / 4.0;
        let blend = Blend {
            a: &cart,
            b: &polar,
            t,
        };
        panels.push(panel(
            "morph",
            "rect ×6 stacked",
            &blend,
            format!("t = {t:.2}"),
            "blend(cartesian, polar, t)",
            &|cs| stack(cs, &classes),
        ));
    }
    for k in 0..5 {
        let t = k as f64 / 4.0;
        let bend = Bend {
            width: W,
            height: H,
            t,
        };
        panels.push(panel(
            "morph",
            "rect ×6 stacked",
            &bend,
            format!("t = {t:.2}"),
            "bend: interpolate curvature, not pixels",
            &|cs| stack(cs, &classes),
        ));
    }
    render(figure(panels, 5), 2, &format!("{out}/morph.png")).await?;

    // ---- figure 3: maps ----------------------------------------------------
    println!(
        "\n{} cells of 5 m, height {z_lo:.1}–{z_hi:.1} m",
        cells.len()
    );
    let t = Instant::now();
    let setup = MapSetup::new(&cells);
    println!(
        "cell inputs for all systems (incl. Lambert-93 → lon/lat) {:.1?}",
        t.elapsed()
    );
    let MapSetup {
        in_unit,
        in_unit_2d,
        in_lonlat,
        metres,
        lon_ticks,
        lat_ticks,
        extent,
    } = &setup;
    let extent = *extent;

    // Lambert-93's own parameters on a sphere: parallels 44° and 49°,
    // central meridian 3°E.
    let conic = Spatial::fit(
        "conic conformal",
        ProjectionKind::ConicConformal {
            parallels: (44.0, 49.0),
        },
        [-3.0, 0.0, 0.0],
        extent,
        W,
        H,
    );
    let plate = Spatial::fit(
        "equirectangular",
        ProjectionKind::Equirectangular,
        [0.0; 3],
        extent,
        W,
        H,
    );
    let c3d_map = Cartesian3d {
        width: W,
        height: H,
        yaw: -25.0,
        elevation: 38.0,
        z_scale: 0.22,
    };

    let sub_l93 = "Lambert-93 metres as they are: no projection";
    let panels = vec![
        panel(
            "maps",
            "symbol + geoshape",
            &cart,
            "cartesian".into(),
            sub_l93,
            &|cs| map(cs, in_unit_2d, [metres, metres], 2.0),
        ),
        panel(
            "maps",
            "symbol + geoshape",
            &conic,
            "spatial · conic conformal".into(),
            "lon/lat; the grid is the graticule",
            &|cs| map(cs, in_lonlat, [lon_ticks, lat_ticks], 2.0),
        ),
        panel(
            "maps",
            "symbol + geoshape",
            &plate,
            "spatial · equirectangular".into(),
            "same spec; stretched east–west at 49°N",
            &|cs| map(cs, in_lonlat, [lon_ticks, lat_ticks], 2.0),
        ),
        panel(
            "maps",
            "symbol + geoshape",
            &c3d_map,
            "cartesian3d".into(),
            "z ← surface height; footprint and grid at z = 0",
            &|cs| map(cs, in_unit, [metres, metres], 2.0),
        ),
    ];
    render(figure(panels, 4), 1, &format!("{out}/maps.png")).await?;

    // ---- does spatial agree with cartesian on the same tile? ---------------
    let a: Vec<Screen> = in_unit_2d
        .pos
        .iter()
        .filter_map(|p| cart.project(p))
        .collect();
    let b: Vec<Screen> = in_lonlat
        .pos
        .iter()
        .filter_map(|p| conic.project(p))
        .collect();
    let (scale, rot, resid) = similarity(&a, &b);
    println!(
        "\ncartesian (Lambert-93) vs spatial (spherical conic), {} cells:\n  \
         similarity scale {scale:.5}, rotation {rot:.4}°, largest residual {resid:.4} px",
        a.len()
    );

    println!("\n| figure | system | mark | instances | vertices | drawn as | build |");
    println!("|---|---|---|---|---|---|---|");
    for r in &rows {
        println!(
            "| {} | {} | {} | {} | {} | {} | {:.2} ms |",
            r.figure,
            r.system,
            r.mark,
            r.cost.instances,
            r.cost.vertices,
            if r.figure == "maps" {
                "symbols"
            } else if r.cost.as_rects {
                "rects"
            } else {
                "paths"
            },
            r.ms
        );
    }
    Ok(())
}
