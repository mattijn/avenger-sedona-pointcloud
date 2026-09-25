//! Phase H: Jev steers, Claude Haiku 4.5 writes the pipeline. Every case in
//! `cases_writer.json`, four ways:
//!
//! - `jev`: Jev's options only, applied as the autopilot does (pilot.rs);
//! - `haiku`: Haiku writes the pipeline from the instruction alone;
//! - `haiku+jev`: Haiku writes it with Jev's reading as direction;
//! - `routed`: Jev's options when they say it all (confidence >= 0.5 and no
//!   specifics), otherwise Haiku with Jev's direction.
//!
//! Whatever is written runs through `editor::apply`; a refusal goes back
//! with its reason, up to three tries. Scored on the folded state.
//!
//!     cargo run --release -p lidar-decide --bin writer_eval -- --check   # the references, no key
//!     set -a; source <env>/.env; set +a
//!     cargo run --release -p lidar-decide --bin writer_eval

use lidar_decide::deciders::{Decider, Decision, Jev, Writer};
use lidar_decide::layer::model::{base_domains, emphasis, quarter_domains, Data, Dataset, Mark, Quarter, State};
use lidar_decide::layer::{data, editor, package, pilot, writer};
use serde_json::{json, Value};

type Error = Box<dyn std::error::Error>;

const GATE: f64 = 0.5;
const TRIES: usize = 3;
const ARMS: [&str; 4] = ["jev", "haiku", "haiku+jev", "routed"];

fn start(v: &Value) -> State {
    let mark = pilot::MARKS.iter().map(|m| m.0).find(|m| Some(m.id()) == v["mark"].as_str()).unwrap_or(Mark::Bars);
    let dataset = Dataset::ALL.iter().map(|d| d.0).find(|d| mark.fits(*d)).unwrap();
    let mut s = State::new(dataset);
    s.mark = mark;
    s.title = s.default_title();
    s.highlight = v["highlight"].as_bool().unwrap_or(false);
    s
}

fn rows(s: &State, d: &Data) -> usize {
    match s.dataset {
        Dataset::Classes => d.classes.len(),
        Dataset::Flight => d.flight.len(),
        Dataset::ClassHeight => d.class_height.len(),
        Dataset::Cells => d.cells.len(),
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn pair(v: &Value) -> (f64, f64) {
    (v[0].as_f64().unwrap(), v[1].as_f64().unwrap())
}

fn zoom_id(s: &State) -> &'static str {
    match (s.range, s.zoom) {
        (Some(_), _) => "custom",
        (None, None) => "all",
        (None, Some(Quarter::NorthEast)) => "north_east",
        (None, Some(Quarter::NorthWest)) => "north_west",
        (None, Some(Quarter::SouthEast)) => "south_east",
        (None, Some(Quarter::SouthWest)) => "south_west",
    }
}

/// Why the result does not meet the expectation, or nothing.
fn check(expect: &Value, start: &State, s: &State, n_rows: usize) -> Option<String> {
    let mut bad = vec![];
    for (k, v) in expect.as_object().unwrap() {
        let ok = match k.as_str() {
            "unchanged" => s == start,
            "mark" => v.as_str() == Some(s.mark.id()),
            "colour" => v.as_str() == Some(pilot::colour_name(s.color)),
            "title" => v.as_str().is_some_and(|t| t.trim().eq_ignore_ascii_case(s.title.trim())),
            "zoom" => v.as_str() == Some(zoom_id(s)),
            "highlight" => v.as_bool() == Some(s.highlight),
            "threshold" => match (v.as_f64(), s.threshold) {
                (None, None) => true,
                (Some(a), Some(b)) => close(a, b),
                _ => false,
            },
            "range" => s.range.is_some_and(|(x, y)| {
                let (ex, ey) = (pair(&v[0]), pair(&v[1]));
                close(x.0, ex.0) && close(x.1, ex.1) && close(y.0, ey.0) && close(y.1, ey.1)
            }),
            "range_x" => s.range.is_some_and(|(x, _)| {
                let e = pair(v);
                close(x.0, e.0) && close(x.1, e.1)
            }),
            "rows" => v.as_u64() == Some(n_rows as u64),
            "rows_gt" => v.as_u64().is_some_and(|m| n_rows as u64 > m),
            "view" => v.as_str() == Some(s.view.id()),
            other => panic!("unknown expectation {other}"),
        };
        if !ok {
            bad.push(k.clone());
        }
    }
    (!bad.is_empty()).then(|| bad.join(", "))
}

fn describe(start: &State, start_rows: usize, s: &State, n_rows: usize) -> String {
    if s == start && n_rows == start_rows {
        return "unchanged".into();
    }
    let mut v = vec![s.mark.id().to_string()];
    if s.title != s.default_title() {
        v.push(format!("\"{}\"", s.title));
    }
    if s.color.is_some() {
        v.push(pilot::colour_name(s.color).into());
    }
    if zoom_id(s) != "all" {
        v.push(format!("zoom {}", zoom_id(s)));
    }
    if s.highlight {
        v.push(s.threshold.map_or("top 10 %".into(), |t| format!(">= {t}")));
    }
    if n_rows != start_rows && s.dataset == start.dataset {
        v.push(format!("{n_rows} cells"));
    }
    v.join(" ")
}

fn fmt_range((a, b): (f64, f64)) -> String {
    format!("{a}..{b}")
}

/// The reference pipeline of a case, placeholders filled in.
fn reference(r: &Value, s: &State, d: &Data) -> String {
    let ds = Dataset::from_id(r["data"].as_str().unwrap()).unwrap();
    let mut text = package::data_stages(ds).join(" ! ");
    for p in r["replace"].as_array().into_iter().flatten() {
        text = text.replace(p[0].as_str().unwrap(), p[1].as_str().unwrap());
    }
    let mark = Mark::default_for(ds);
    let mark = if s.dataset == ds { s.mark } else { mark };
    let mark = if r["commands"][0].as_str().unwrap().starts_with("pie") { Mark::Pie } else { mark };
    for c in r["commands"].as_array().unwrap() {
        let mut c = c.as_str().unwrap().to_string();
        if let Some((bx, by)) = base_domains(mark, d) {
            let q = |q| {
                let (x, y) = quarter_domains(Some(q), bx, by);
                format!("{} {}", fmt_range(x), fmt_range(y))
            };
            c = c.replace("{nw}", &q(Quarter::NorthWest)).replace("{ne}", &q(Quarter::NorthEast)).replace("{y}", &fmt_range(by));
        }
        if let Some((_, top)) = emphasis(mark, d) {
            c = c.replace("{top}", &top.to_string());
        }
        text += &format!(" ! {c}");
    }
    text
}

struct Run {
    state: State,
    rows: usize,
    ms: f64,
    cost: f64,
    /// Writer tries, and whether the first was accepted.
    tries: usize,
    first: bool,
    texts: Vec<Value>,
    route: &'static str,
}

fn from_jev(s: &State, j: &Decision) -> State {
    if j.confidence.unwrap_or(0.0) < GATE {
        return s.clone();
    }
    pilot::apply(s, &j.answers).unwrap_or_else(|_| s.clone())
}

async fn from_writer(w: &Writer, s: &State, d: &Data, text: &str, dir: Option<&str>) -> Result<Run, Error> {
    let o = writer::write(w, s, d, &writer::pipeline_text(s, d), text, dir, TRIES).await?;
    let texts = o.attempts.iter().map(|a| json!({"pipeline": a.text, "refused": a.refused, "cached": a.written.cached})).collect();
    let (state, n) = match &o.applied {
        Some(a) => (a.state.clone(), rows(&a.state, &a.data)),
        None => (s.clone(), rows(s, d)),
    };
    Ok(Run {
        state,
        rows: n,
        ms: o.ms(),
        cost: o.cost(),
        tries: o.attempts.len(),
        first: o.attempts.first().is_some_and(|a| a.refused.is_none()),
        texts,
        route: "writer",
    })
}

fn p50(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let root = env!("CARGO_MANIFEST_DIR");
    std::env::set_current_dir(format!("{root}/../.."))?;
    let all: Value = serde_json::from_str(&std::fs::read_to_string(format!("{root}/cases_writer.json"))?)?;
    let cases = all["cases"].as_array().unwrap().clone();
    let back = all["back"].as_array().cloned().unwrap_or_default();
    let d = data::load().await?;

    if std::env::args().any(|a| a == "--check") {
        let mut failed = 0;
        for c in &cases {
            let s = start(&c["start"]);
            let text = reference(&c["reference"], &s, &d);
            let verdict = match editor::apply(&text, &d).await {
                Ok(a) => check(&c["expect"], &s, &a.state, rows(&a.state, &a.data)).map_or("ok".to_string(), |b| format!("MISMATCH: {b}")),
                Err(e) => format!("REFUSED: {e}"),
            };
            failed += usize::from(verdict != "ok");
            println!("{} {:<60} {verdict}", c["id"].as_str().unwrap(), c["text"].as_str().unwrap());
        }
        println!("\n{} of {} references reach their expectation", cases.len() - failed, cases.len());
        return Ok(());
    }

    let client = reqwest::Client::new();
    let jev = Jev { model: "typesafe/jev-1.13", client: client.clone() };
    let haiku = Writer { model: "anthropic/claude-haiku-4.5", client };
    let mut table = vec![];
    let mut log = vec![];
    let mut per_arm: Vec<Vec<(bool, Run)>> = (0..ARMS.len()).map(|_| vec![]).collect();

    for c in &cases {
        let id = c["id"].as_str().unwrap();
        let text = c["text"].as_str().unwrap();
        let s = start(&c["start"]);
        let mut j = jev.decide(&pilot::observation(&s, &d, text), &writer::questions()).await?;
        writer::normalise(&mut j, &s.view);
        let dir = writer::direction(&j);
        let jev_state = from_jev(&s, &j);
        let jev_run = Run { rows: rows(&jev_state, &d), state: jev_state, ms: j.ms, cost: j.cost, tries: 0, first: false, texts: vec![], route: "options" };
        let alone = from_writer(&haiku, &s, &d, text, None).await?;
        let mut steered = from_writer(&haiku, &s, &d, text, Some(&dir)).await?;
        steered.ms += j.ms;
        steered.cost += j.cost;
        let specifics = j.answers.get("specifics").and_then(Value::as_str) == Some("text");
        let fast = j.confidence.unwrap_or(0.0) >= GATE && !specifics && pilot::apply(&s, &j.answers).is_ok_and(|n| n != s || pilot::short(&j.answers) == "no_change");
        let routed = if fast {
            Run { state: jev_run.state.clone(), rows: jev_run.rows, ms: j.ms, cost: j.cost, tries: 0, first: false, texts: vec![], route: "options" }
        } else {
            let mut r = from_writer(&haiku, &s, &d, text, Some(&dir)).await?;
            r.ms += j.ms;
            r.cost += j.cost;
            r
        };
        let mut row = vec![format!("{id} {text}")];
        let mut entry = json!({"id": id, "text": text, "reading": {"answers": j.answers, "confidence": j.confidence, "direction": dir}});
        for (i, run) in [jev_run, alone, steered, routed].into_iter().enumerate() {
            let miss = check(&c["expect"], &s, &run.state, run.rows);
            let what = describe(&s, rows(&s, &d), &run.state, run.rows);
            let tries = if run.tries > 1 { format!(", {} tries", run.tries) } else { String::new() };
            row.push(match &miss {
                None => format!("{what}{tries}"),
                Some(b) => format!("**✗** {what} ({b}){tries}"),
            });
            entry[ARMS[i]] = json!({"right": miss.is_none(), "result": what, "route": run.route, "tries": run.tries, "ms": run.ms, "cost": run.cost, "attempts": run.texts});
            per_arm[i].push((miss.is_none(), run));
        }
        println!("{} done", id);
        table.push(row);
        log.push(entry);
    }

    let mut md = String::from("| Case | jev | haiku | haiku+jev | routed |\n|---|---|---|---|---|\n");
    for r in &table {
        md += &format!("| {} |\n", r.join(" | "));
    }
    md += "\n| Arm | Right, vocabulary (v) | Right, own text (w) | Writer used | Accepted on the first try | Tries | Latency p50 / max | Cost |\n|---|---|---|---|---|---|---|---|\n";
    for (i, arm) in ARMS.iter().enumerate() {
        let runs = &per_arm[i];
        let (v, w): (Vec<_>, Vec<_>) = runs.iter().zip(&cases).partition(|(_, c)| c["id"].as_str().unwrap().starts_with('v'));
        let right = |x: &[(&(bool, Run), &Value)]| format!("{}/{}", x.iter().filter(|(r, _)| r.0).count(), x.len());
        let written: Vec<_> = runs.iter().filter(|r| r.1.tries > 0).collect();
        let ms: Vec<f64> = runs.iter().map(|r| r.1.ms).collect();
        md += &format!(
            "| {arm} | {} | {} | {} | {} | {} | {:.0} / {:.0} ms | ${:.4} |\n",
            right(&v),
            right(&w),
            written.len(),
            if written.is_empty() { "–".into() } else { format!("{}/{}", written.iter().filter(|r| r.1.first).count(), written.len()) },
            written.iter().map(|r| r.1.tries).sum::<usize>(),
            p50(ms.clone()),
            ms.iter().cloned().fold(0.0, f64::max),
            runs.iter().map(|r| r.1.cost).sum::<f64>(),
        );
    }
    // Jev only: undo, reset, table and export.
    md += "\n| Case | Start | Expected | Jev | Render | Confidence |\n|---|---|---|---|---|---|\n";
    let mut right = 0;
    for c in &back {
        let s = start(&c["start"]);
        let text = c["text"].as_str().unwrap();
        let mut j = jev.decide(&pilot::observation(&s, &d, text), &writer::questions()).await?;
        writer::normalise(&mut j, &s.view);
        let got = j.answers.get("action").and_then(Value::as_str).unwrap_or("?");
        let render = j.answers.get("render").and_then(Value::as_str).unwrap_or("?");
        let view = j.answers.get("view").and_then(Value::as_str).unwrap_or("?");
        let ok = Some(got) == c["expect"].as_str() && c["expect_render"].as_str().is_none_or(|r| r == render) && c["expect_view"].as_str().is_none_or(|v| v == view);
        right += usize::from(ok);
        md += &format!(
            "| {} {text} | {} | {}{} | {}{} | {render} | {:.2} |\n",
            c["id"].as_str().unwrap(),
            s.mark.id(),
            c["expect"].as_str().unwrap(),
            c["expect_render"].as_str().map_or(String::new(), |r| format!(", {r}")),
            if ok { "" } else { "**✗** " },
            pilot::short(&j.answers),
            j.confidence.unwrap_or(0.0)
        );
    }
    md += &format!("\nJev only: {right}/{} right.\n", back.len());
    println!("\n{md}");
    std::fs::write(format!("{root}/results/writer.md"), &md)?;
    std::fs::write(format!("{root}/results/writer_decisions.json"), serde_json::to_string_pretty(&log)?)?;
    Ok(())
}
