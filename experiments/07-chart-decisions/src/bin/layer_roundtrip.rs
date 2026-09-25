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
        ("a field that is not there", base.replace("--value h", "--value height")),
    ] {
        match editor::apply(&text, &d).await {
            Ok(a) => println!(
                "{what}: {:?} zoom {:?} threshold {:?} · {} cells · {} · {:.0} ms",
                a.state.mark, a.state.range, a.state.threshold, a.data.cells.len(), a.note, a.ms
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
