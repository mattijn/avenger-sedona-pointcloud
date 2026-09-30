//! The round trip of `intents.md`, run: for every case of the chart layer,
//! the question's intent, the chart the layer would draw, and whether the
//! chart leads back to the intent in the Financial Times' Visual Vocabulary.
//!
//! The intent comes from three places, side by side: the hand labels of
//! `cases_intent.json`, the offline keyword rules, and Jev asked one typed
//! question (when `OPENROUTER_API_KEY` is set; cached like every decision).
//! The chart is the one Jev's own decision draws (its cached pilot answer,
//! applied to the start state), beside the one the case expects.
//!
//!     cargo run --release -p lidar-decide --bin intent_roundtrip            # the cases: table and contact sheet
//!     cargo run --release -p lidar-decide --bin intent_roundtrip -- --ask   # type questions, one per line
//!     cargo run --release -p lidar-decide --bin intent_roundtrip -- --read  # the reader: `intent` and `facts` on each chart's pipeline

use std::io::BufRead;
use std::sync::Arc;

use avenger_scenegraph::marks::group::SceneGroup;
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_text::types::{TextAlign, TextBaseline};
use avenger_wgpu::canvas::{Canvas, PngCanvas};
use lidar_decide::deciders::{Decider, Jev};
use lidar_decide::layer::anim::still;
use lidar_decide::layer::intent;
use lidar_decide::layer::model::{resolve, Data, Dataset, Mark, State};
use lidar_decide::layer::{data, draw, pilot};
use serde_json::Value;

type Error = Box<dyn std::error::Error>;

const INK: [f32; 4] = [0.2, 0.22, 0.25, 1.0];
const MUTED: [f32; 4] = [0.45, 0.47, 0.5, 1.0];
const GREEN: [f32; 4] = [0.16, 0.5, 0.25, 1.0];
const RED: [f32; 4] = [0.72, 0.16, 0.12, 1.0];

fn mark_of(id: &str) -> Option<Mark> {
    pilot::MARKS.iter().map(|m| m.0).find(|m| m.id() == id)
}

fn state_for(mark: Mark) -> State {
    let dataset = Dataset::ALL.iter().map(|d| d.0).find(|d| mark.fits(*d)).unwrap();
    let mut s = State::new(dataset);
    s.mark = mark;
    s.title = s.default_title();
    s
}

fn start(v: &Value) -> State {
    state_for(v["mark"].as_str().and_then(mark_of).unwrap_or(Mark::Bars))
}

/// One question through everything: the intent by rules and by Jev, the
/// chart Jev's decision draws, and the round trip for it.
struct Trip {
    rules: &'static str,
    jev: Option<(String, f64)>,
    jev_error: Option<String>,
    chart: Mark,
    decided: String,
}

async fn trip(jev: &Jev, d: &Data, s: &State, text: &str) -> Trip {
    let obs = pilot::observation(s, d, text);
    let (jev_intent, jev_error) = match jev.decide(&obs, &intent::question()).await {
        Ok(dec) => {
            let i = dec.answers.get("intent").and_then(Value::as_str).unwrap_or("none").to_string();
            let c = dec.confidences.get("intent").and_then(Value::as_f64).or(dec.confidence).unwrap_or(0.0);
            (Some((i, c)), None)
        }
        Err(e) => (None, Some(e)),
    };
    // Jev's own decision, as the layer applies it: the chart it would draw.
    let (chart, decided) = match jev.decide(&obs, &pilot::questions()).await {
        Ok(dec) => match pilot::apply(s, &dec.answers) {
            Ok(n) => (n.mark, pilot::short(&dec.answers)),
            Err(_) => (s.mark, format!("{} (not applied)", pilot::short(&dec.answers))),
        },
        // Without a decision (a new question, no key), the chart the
        // vocabulary leads to from the intent, when the layer has one.
        Err(e) => {
            let i = jev_intent.as_ref().map_or(intent::rules(text), |(j, _)| j.as_str());
            let why = if e.contains("401") { "Jev refused the key (401)" } else { "no decision" };
            match intent::forward(i).first() {
                Some(m) => (*m, format!("{why}; the vocabulary's chart for {i}")),
                None => (s.mark, format!("{why}; the layer has no chart for {i}, so the chart stays")),
            }
        }
    };
    Trip { rules: intent::rules(text), jev: jev_intent, jev_error, chart, decided }
}

fn verdict(i: &str, m: Mark) -> String {
    if i == "none" {
        return "–".into();
    }
    let fwd: Vec<&str> = intent::forward(i).iter().map(|m| m.id()).collect();
    let back = intent::back(m).join(", ");
    if intent::closes(i, m) {
        format!("closes ({i} → {} → {i})", m.id())
    } else if fwd.is_empty() {
        format!("does not close: the layer has no chart for {i} (the FT would draw {}); {} is {back}",
            intent::missing(i)[..3].join(", "), m.id())
    } else {
        format!("does not close: {i} → {}, but {} is {back}", fwd.join(" or "), m.id())
    }
}

/// A verdict in lines that fit under a chart: split after its clauses.
fn wrap(v: &str) -> Vec<String> {
    v.replace(": ", ":\n").replace("; ", ";\n").lines().map(String::from).collect()
}

/// A chart and its caption lines, at `origin`, shrunk by half.
fn panel(d: &Data, s: &State, origin: [f32; 2], lines: &[(String, [f32; 4], bool)]) -> SceneMark {
    let mut marks = draw::chart_marks(&still(&resolve(s, d)), draw::ORIGIN);
    for (k, (t, c, bold)) in lines.iter().enumerate() {
        marks.push(draw::text(t, 40.0, draw::SIZE[1] + 24.0 + k as f32 * 28.0, 20.0, *c, TextAlign::Left, TextBaseline::Middle, *bold, 0.0));
    }
    SceneGroup { origin, marks, ..Default::default() }.into()
}

async fn render(marks: Vec<SceneMark>, size: [f32; 2], scale: f32, path: &str) -> Result<(), Error> {
    use avenger_common::canvas::CanvasDimensions;
    let mut all: Vec<SceneMark> = vec![draw::text("", 0.0, 0.0, 1.0, INK, TextAlign::Left, TextBaseline::Top, false, 0.0)];
    all.extend(marks);
    let scene = SceneGraph { marks: vec![SceneGroup { marks: all, ..Default::default() }.into()], width: size[0], height: size[1], origin: [0.0; 2] };
    let mut canvas = PngCanvas::new(CanvasDimensions { size, scale }, Default::default()).await?;
    canvas.set_scene(&scene)?;
    canvas.render().await?.save(path)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let root = env!("CARGO_MANIFEST_DIR");
    let d = data::load().await?;
    let jev = Arc::new(Jev { model: "typesafe/jev-1.13", client: reqwest::Client::new() });
    let online = std::env::var("OPENROUTER_API_KEY").is_ok();
    println!("Jev: {}", if online { "asked (OPENROUTER_API_KEY is set); answers are cached" } else { "from the cache only (OPENROUTER_API_KEY is not set)" });

    if std::env::args().any(|a| a == "--ask") {
        return ask(&jev, &d).await;
    }
    // `--read`: the reader's side. Each of the layer's charts is built through
    // its pipeline from scratch, and asked `intent` and `facts`.
    if std::env::args().any(|a| a == "--read") {
        for (m, _) in pilot::MARKS {
            let (mut p, _) = lidar_decide::layer_pipeline().await?;
            for l in lidar_decide::layer::package::initial(&state_for(m), &d) {
                p.run(&l).await?;
            }
            let chart_line = p.log.iter().rev().find(|l| l.starts_with("chart ") || ["bars ", "pie ", "line ", "heatmap ", "map "].iter().any(|s| l.starts_with(s))).cloned().unwrap_or_default();
            let t = std::time::Instant::now();
            let out = p.run("intent ! facts").await?;
            println!("\n{} · {chart_line} ({:.0} ms)", m.id(), t.elapsed().as_secs_f64() * 1e3);
            for o in out {
                println!("  {o}");
            }
        }
        return Ok(());
    }

    let cases: Value = serde_json::from_str(&std::fs::read_to_string(format!("{root}/cases_layer.json"))?)?;
    let labels: Value = serde_json::from_str(&std::fs::read_to_string(format!("{root}/cases_intent.json"))?)?;
    let mut md = String::from(
        "# The intent round trip\n\nGenerated by `intent_roundtrip` from `cases_layer.json` and `cases_intent.json`. \
         The intent by hand, by the keyword rules and by Jev; the chart the case expects and the one Jev's decision draws; \
         and whether that chart leads back to the intent in the FT's Visual Vocabulary (`intents.md`).\n\n\
         | Case | Question | By hand | Rules | Jev | Case expects | Jev draws | Round trip, for what Jev draws |\n|---|---|---|---|---|---|---|---|\n",
    );
    let (mut agree_rules, mut agree_jev, mut asked, mut closed, mut questions) = (0, 0, 0, 0, 0);
    let mut sheet = vec![];
    for c in cases["cases"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        let text = c["text"].as_str().unwrap();
        let label = labels["intents"][id].as_str().unwrap_or("none");
        let s = start(&c["start"]);
        let t = trip(&jev, &d, &s, text).await;
        let expects = match &c["expect"]["mark"] {
            Value::String(m) => m.clone(),
            Value::Array(ms) => ms.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" or "),
            _ => format!("({})", c["expect"]["action"].as_str().unwrap_or("")),
        };
        agree_rules += (t.rules == label) as usize;
        if let Some((j, _)) = &t.jev {
            asked += 1;
            agree_jev += (j == label) as usize;
        }
        let jev_cell = match (&t.jev, &t.jev_error) {
            (Some((j, conf)), _) => format!("{j} ({conf:.2})"),
            (None, Some(e)) if e.contains("not set") || e.contains("cache") => "not asked".into(),
            (None, Some(e)) => format!("error: {}", e.lines().next().unwrap_or("").chars().take(60).collect::<String>()),
            _ => "–".into(),
        };
        let v = verdict(label, t.chart);
        if label != "none" {
            questions += 1;
            closed += intent::closes(label, t.chart) as usize;
        }
        md += &format!("| {id} | {text} | {label} | {} | {jev_cell} | {expects} | {} | {v} |\n", t.rules, t.chart.id());
        println!("{id:>4}  {:<52} hand {label:<16} rules {:<16} jev {jev_cell:<22} draws {:<8} {v}",
            text.chars().take(52).collect::<String>(), t.rules, t.chart.id());
        if label != "none" {
            sheet.push((id.to_string(), text.to_string(), label, t, expects));
        }
    }
    let jev_line = if asked > 0 { format!("Jev {agree_jev} of {asked}") } else { "Jev not asked".into() };
    let summary = format!("Intent agrees with the hand labels: rules {agree_rules} of 24, {jev_line}. \
        Of the {questions} questions, the chart Jev's decision draws closes the round trip for {closed}.");
    md += &format!("\n{summary}\n");
    println!("\n{summary}");
    let out = format!("{root}/results/intent_roundtrip.md");
    std::fs::write(&out, md)?;
    println!("-> {out}");

    // The contact sheet: each question, the chart drawn, the verdict.
    let (w, h) = (draw::SIZE[0], draw::SIZE[1] + 200.0);
    let mut marks = vec![];
    for (k, (id, text, label, t, expects)) in sheet.iter().enumerate() {
        let ok = intent::closes(label, t.chart);
        let jev = t.jev.as_ref().map_or("not asked".to_string(), |(j, c)| format!("{j} ({c:.2})"));
        let mut lines = vec![
            (format!("{id}  \u{201c}{text}\u{201d}"), INK, true),
            (format!("intent: by hand {label} · rules {} · Jev {jev}", t.rules), MUTED, false),
            (format!("the case expects {expects} · Jev draws {}", t.chart.id()), MUTED, false),
        ];
        lines.extend(wrap(&verdict(label, t.chart)).into_iter().map(|l| (l, if ok { GREEN } else { RED }, true)));
        let origin = [(k % 2) as f32 * w, (k / 2) as f32 * h];
        marks.push(panel(&d, &state_for(t.chart), origin, &lines));
    }
    let rows = sheet.len().div_ceil(2) as f32;
    let png = format!("{root}/images/intent_roundtrip.png");
    render(marks, [2.0 * w, rows * h], 0.6, &png).await?;
    println!("-> {png}");
    Ok(())
}

/// Questions typed one per line: each through the round trip, its chart
/// written as a PNG. The chart starts as bars, as the layer does.
async fn ask(jev: &Jev, d: &Data) -> Result<(), Error> {
    let dir = format!("{}/../../out/intent_ask", env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&dir)?;
    let s = state_for(Mark::Bars);
    println!("Type a question about the tile's data (an empty line ends):");
    let stdin = std::io::stdin();
    for (k, line) in stdin.lock().lines().enumerate() {
        let text = line?;
        if text.trim().is_empty() {
            break;
        }
        let t = trip(jev, d, &s, &text).await;
        let intent = t.jev.as_ref().map_or(t.rules.to_string(), |(j, _)| j.clone());
        let by = t.jev.as_ref().map_or("rules".to_string(), |(_, c)| format!("Jev, {c:.2}"));
        println!("  intent: {intent} ({by}; rules say {})", t.rules);
        println!("  Jev's decision: {} → draws {}", t.decided, t.chart.id());
        println!("  {}", verdict(&intent, t.chart));
        if intent != "none" {
            println!("  the FT would draw: {}", intent::missing(&intent).join(", "));
        }
        let path = format!("{dir}/{k:02}.png");
        let ok = intent::closes(&intent, t.chart);
        let mut lines = vec![(format!("\u{201c}{text}\u{201d}"), INK, true), (format!("intent {intent} ({by}) · {}", t.decided), MUTED, false)];
        lines.extend(wrap(&verdict(&intent, t.chart)).into_iter().map(|l| (l, if ok { GREEN } else { RED }, true)));
        render(vec![panel(d, &state_for(t.chart), [0.0, 0.0], &lines)], [draw::SIZE[0], draw::SIZE[1] + 200.0], 1.0, &path).await?;
        println!("  -> {}", std::fs::canonicalize(&path)?.display());
    }
    Ok(())
}
