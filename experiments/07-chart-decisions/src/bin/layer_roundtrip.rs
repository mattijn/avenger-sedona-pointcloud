//! Does the pipeline say what the decision meant? For every pair of chart
//! states the layer can be in: build the first through the pipeline from
//! scratch, run the lines that take it to the second, fold the pipeline's
//! chart state back, and require it to equal the second exactly.
//!
//!     cargo run --release -p lidar-decide --bin layer_roundtrip

use lidar_decide::layer::model::{Dataset, Mark, Quarter, State};
use lidar_decide::layer::{data, editor, package, pilot};
use lidar_decide::layer_pipeline;

type Error = Box<dyn std::error::Error>;

/// Every state the autopilot can reach.
fn states() -> Vec<State> {
    let quarters = [None, Some(Quarter::NorthEast), Some(Quarter::NorthWest), Some(Quarter::SouthEast), Some(Quarter::SouthWest)];
    let mut v = vec![];
    for (m, _) in pilot::MARKS {
        let ds = Dataset::ALL.iter().map(|d| d.0).find(|d| m.fits(*d)).unwrap();
        let colours: Vec<Option<[f32; 4]>> = if matches!(m, Mark::Bars | Mark::Map) {
            vec![None, pilot::colour_of("red"), pilot::colour_of("green")]
        } else {
            vec![None]
        };
        let highlights: &[bool] = if matches!(m, Mark::Bars | Mark::Pie | Mark::Map) { &[false, true] } else { &[false] };
        let zooms: &[Option<Quarter>] = if matches!(m, Mark::Line | Mark::Map) { &quarters } else { &[None] };
        for c in &colours {
            for h in highlights {
                for z in zooms {
                    let mut s = State::new(ds);
                    s.mark = m;
                    s.title = s.default_title();
                    s.color = *c;
                    s.highlight = *h;
                    s.zoom = *z;
                    v.push(s);
                }
            }
        }
    }
    v
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let d = data::load().await?;
    let all = states();
    let (mut ok, mut lines_run) = (0, 0);
    let t = std::time::Instant::now();
    for s in &all {
        for n in &all {
            let (mut p, _) = layer_pipeline().await?;
            for l in package::initial(s, &d) {
                p.run(&l).await.map_err(|e| format!("initial {s:?}: {l}: {e}"))?;
            }
            let from = package::state(&p.chart, &d).map_err(|e| format!("fold of {s:?}: {e}"))?;
            if from != *s {
                return Err(format!("initial fold differs:\n{s:?}\n{from:?}").into());
            }
            let lines = package::lines(s, n, &d);
            for l in &lines {
                p.run(l).await.map_err(|e| format!("{s:?} → {n:?}: {l}: {e}"))?;
                lines_run += 1;
            }
            let got = package::state(&p.chart, &d).map_err(|e| format!("fold {s:?} → {n:?}: {e}"))?;
            if got != *n {
                return Err(format!("{s:?} → {n:?} via {lines:?} folds to {got:?}").into());
            }
            ok += 1;
        }
    }
    println!(
        "{} states, {ok} transitions, {lines_run} pipeline lines: every fold equals its target ({:.1?})",
        all.len(),
        t.elapsed()
    );
    // The full pipeline behind each state, run from the tile as one text,
    // must fold to that state too.
    let t = std::time::Instant::now();
    for s in &all {
        let text = package::full(s, &d).join(" ! ");
        let (mut p, _) = layer_pipeline().await?;
        p.run(&text).await.map_err(|e| format!("full pipeline of {s:?}: {e}"))?;
        let got = package::state(&p.chart, &d).map_err(|e| format!("fold of the full pipeline of {s:?}: {e}"))?;
        if got != *s {
            return Err(format!("full pipeline of {s:?} folds to {got:?}").into());
        }
    }
    println!("{} full pipelines from the tile: every fold equals its state ({:.1?})", all.len(), t.elapsed());
    // Editor mode: each full pipeline, as text with one stage per line,
    // applies to its own state, reading the cached table.
    let t = std::time::Instant::now();
    for s in &all {
        let text = package::full(s, &d).join("\n! ");
        let a = editor::apply(&text, &d).await.map_err(|e| format!("editor on {s:?}: {e}"))?;
        if a.state != *s || !a.note.contains("cache") {
            return Err(format!("editor on {s:?}: {:?} ({})", a.state, a.note).into());
        }
    }
    println!("{} pipeline texts through the editor: every one applies to its state, from the cache ({:.1?})", all.len(), t.elapsed());
    // Edits the autopilot cannot make.
    let map = all.iter().find(|s| s.mark == Mark::Map && s.zoom.is_none() && !s.highlight && s.color.is_none()).unwrap();
    let base = package::full(map, &d).join("\n! ");
    for (what, text) in [
        ("custom zoom and threshold", format!("{base}\n! highlight \"datum.h >= 70\"\n! zoom 657200..657600 6867300..6867700")),
        ("10 m cells", base.replace("floor(x/5)*5", "floor(x/10)*10").replace("floor(y/5)*5", "floor(y/10)*10")),
        ("a predicate the layer cannot draw", format!("{base}\n! highlight \"datum.h < 50\"")),
        ("a field that is not there", base.replace("--color h:Q", "--color height:Q")),
    ] {
        match editor::apply(&text, &d).await {
            Ok(a) => println!(
                "{what}: {:?} zoom {:?} threshold {:?} · {} cells · {} · {:.0} ms",
                a.state.mark, a.state.range, a.state.threshold, a.data.cells.len(), a.note, a.ms
            ),
            Err(e) => println!("{what}: refused: {e}"),
        }
    }
    // Channels, types and `set`: what the layer draws, and what it refuses.
    let bars = all.iter().find(|s| s.mark == Mark::Bars && !s.highlight && s.color.is_none()).unwrap();
    let line = all.iter().find(|s| s.mark == Mark::Line && s.zoom.is_none()).unwrap();
    let (bars, line) = (package::full(bars, &d).join("\n! "), package::full(line, &d).join("\n! "));
    for (what, text) in [
        ("axis titles and a log scale", format!("{bars}\n! set x.axis.title \"LiDAR class\"\n! set y.axis.title \"points\"\n! set y.scale.type log")),
        ("the short form of experiment 6", format!("{bars}\n! set x.title \"LiDAR class\"")),
        ("one axis zoomed", format!("{line}\n! set x.scale.domain 0,5")),
        ("a constant colour as a value", bars.replace("--y n:Q", "--y n:Q --color #c44e52")),
        ("a channel the mark does not have", base.replace("--color h:Q", "--color h:Q --size h:Q")),
        ("a type the layer does not draw", bars.replace("label:N", "label:Q")),
        ("a mark the layer does not draw", bars.replace("chart bar", "chart area")),
        ("a property the layer does not draw", format!("{bars}\n! set x.axis.labelAngle -45")),
        ("a log scale on a map", format!("{base}\n! set y.scale.type log")),
        ("two classes selected", format!("{bars}\n! select point --keys \"Ground;Building\"")),
        ("only the selection, axes kept", format!("{bars}\n! select point --keys \"Ground;Building\" --effect filter")),
        ("a brush on the map", format!("{base}\n! select interval --x 657200..657600 --y 6867300..6867700")),
        ("a brush on bars", format!("{bars}\n! select interval --x 0..3")),
        ("a soft brush on the map", format!("{base}\n! select interval --x 657600..657800 --y 6867350..6867550 --soft 0.15")),
        ("softness added afterwards", format!("{base}\n! select interval --x 657600..657800 --y 6867350..6867550\n! select --soft 0.15")),
        ("a soft line brush", format!("{line}\n! select segment --from 18.9,185000 --to 21.6,145000 --soft 0.2")),
        ("a soft timebox", format!("{line}\n! select timebox --x 14..18 --y 120000..200000 --soft 0.1")),
        ("softness wider than the plot", format!("{base}\n! select interval --x 657600..657800 --soft 2")),
        ("a regression lens on the flight lines", format!("{line}\n! lens regression --focus 0.3,0.6 --radius 0.2")),
        ("a sampling lens on the map", format!("{base}\n! lens sample --focus 0.7,0.45 --radius 0.15 --keep 0.2")),
        ("a mole lens on the map in 3D", format!("{base}\n! view tilt\n! lens mole --focus 0.7,0.45 --radius 0.15 --above 0.25")),
        ("a mole lens at 0.1", format!("{base}\n! lens mole --focus 0.5,0.5 --radius 0.2 --above 0.1")),
        ("a mole lens at 0.3", format!("{base}\n! lens mole --focus 0.5,0.5 --radius 0.2 --above 0.3")),
        ("a mole lens at 0.15", format!("{base}\n! lens mole --focus 0.5,0.5 --radius 0.2 --above 0.15")),
        ("a mole lens at 0.2", format!("{base}\n! lens mole --focus 0.5,0.5 --radius 0.2 --above 0.2")),
        ("a mole lens at 0.5", format!("{base}\n! lens mole --focus 0.5,0.5 --radius 0.2 --above 0.5")),
        ("a mole lens at 0.9", format!("{base}\n! lens mole --focus 0.5,0.5 --radius 0.2 --above 0.9")),
        ("a lens cleared", format!("{base}\n! lens mole --focus 0.7,0.45\n! lens clear")),
        ("a regression lens on the map", format!("{base}\n! lens regression --focus 0.5,0.5")),
        ("a sampling lens on bars", format!("{bars}\n! lens sample --focus 0.5,0.5")),
        ("a lens under a fisheye", format!("{base}\n! view fisheye --focus 0.5,0.5\n! lens mole --focus 0.5,0.5")),
        ("a lasso on the flat map", format!("{base}\n! select lasso --poly \"0.6,0.35;0.8,0.35;0.8,0.55;0.6,0.55\"")),
        ("a lasso in 3D", format!("{base}\n! zoom 657500..658000 6867250..6867750\n! view tilt --yaw 30 --elevation 35\n! select lasso --poly \"0.42,0.52;0.75,0.55;0.78,0.32;0.45,0.28\" --yaw 30 --elevation 35")),
        ("CloudLasso in 3D", format!("{base}\n! zoom 657500..658000 6867250..6867750\n! view tilt --yaw 30 --elevation 35\n! select lasso --poly \"0.42,0.52;0.75,0.55;0.78,0.32;0.45,0.28\" --yaw 30 --elevation 35 --structure 0.3")),
        ("a lasso of two points", format!("{base}\n! select lasso --poly \"0.1,0.1;0.2,0.2\"")),
        ("a lasso on bars", format!("{bars}\n! select lasso --poly \"0.1,0.1;0.9,0.1;0.5,0.9\"")),
    ] {
        match editor::apply(&text, &d).await {
            Ok(a) => println!(
                "{what}: {:?} · x title {:?} · y title {:?} · log {} · zoom {:?} · colour {} · selection {:?} {:?} soft {:?} · lens {:?} · {} items, {:?} selected, found {:?}",
                a.state.mark, a.state.x_title, a.state.y_title, a.state.y_log, a.state.range, a.state.color.is_some(), a.state.selection, a.state.effect, a.state.soft, a.state.lens,
                lidar_decide::layer::model::resolve(&a.state, &a.data).items.len(), lidar_decide::layer::model::resolve(&a.state, &a.data).selected,
                {
                    let f = lidar_decide::layer::model::resolve(&a.state, &a.data);
                    f.lens.map(|l| l.1.into_iter().map(|f| f.label).collect::<Vec<_>>()).or(f.lasso.map(|l| vec![l.2]))
                }
            ),
            Err(e) => println!("{what}: refused: {e}"),
        }
    }
    // Examples, for the README.
    let a = &all[0];
    for n in [&all[3], &all[all.len() - 1]] {
        println!("{:?}/{:?} → {:?}/{:?}/{:?}:\n  {}", a.mark, a.color.is_some(), n.mark, n.zoom, n.highlight, package::lines(a, n, &d).join("\n  "));
    }
    Ok(())
}
