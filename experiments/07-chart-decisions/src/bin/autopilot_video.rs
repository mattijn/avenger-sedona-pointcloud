//! The autopilot, recorded: instructions are typed into the panel, Jev
//! decides after every word, and the one chart object transitions whenever a
//! decision passes the gates.
//!
//! Gates, as planned: the decision must be sure enough (confidence ≥ 0.5;
//! ≥ 0.9 for a change of chart kind before the instruction is complete),
//! complete (a zoom with a region, a colour with a colour), fit the chart,
//! and change something. Anything else is shown in the panel and left there.
//!
//! Decisions come from the response cache after the first run, so the video
//! rebuilds without a key, with the latencies that were measured.
//!
//!     set -a; source <env>/.env; set +a      # first run only
//!     cargo run --release -p lidar-decide --bin autopilot_video -- out/autopilot
//!     ffmpeg -framerate 30 -i out/autopilot/f%05d.png -c:v libx264 -pix_fmt yuv420p \
//!         -crf 26 experiments/07-chart-decisions/video/autopilot.mp4

use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_text::types::{TextAlign, TextBaseline};
use avenger_wgpu::canvas::{Canvas, PngCanvas};
use avenger_common::canvas::CanvasDimensions;
use lidar_common::{INK, MUTED};
use lidar_decide::deciders::{Decider, Decision, Jev};
use lidar_decide::layer::anim::{still, transition};
use lidar_decide::layer::model::{resolve, Dataset, Frame, State};
use lidar_decide::layer::{data, draw, pilot};
use lidar_decide::options::NotApplied;

type Error = Box<dyn std::error::Error>;

const SCRIPT: [&str; 12] = [
    "make the bars red",
    "which share does each class have?",
    "back to bars please",
    "emphasise the biggest classes",
    "how are the classes spread over height?",
    "how many points did each flight line record over time?",
    "zoom in on the start of the lines, the top left",
    "zoom back out",
    "where are the buildings?",
    "mark the tallest buildings",
    "zoom to the south-east",
    "show the whole tile again",
];
const FPS: f64 = 30.0;
const CHAR: f64 = 0.065;
const HOLD: f64 = 1.6;
const GATE: f64 = 0.5;
/// A new kind of chart is the costly change to undo, so before the
/// instruction is complete it needs a surer decision.
const GATE_MARK_EARLY: f64 = 0.9;
const W: f32 = 1400.0;
const H: f32 = 640.0;
const PX: f32 = 960.0;
const GREEN: [f32; 4] = [0.23, 0.55, 0.30, 1.0];
const AMBER: [f32; 4] = [0.80, 0.50, 0.10, 1.0];
const BAR: [f32; 4] = [0.30, 0.47, 0.66, 1.0];
const LIGHT: [f32; 4] = [0.93, 0.94, 0.96, 1.0];

/// A decision on a prefix of an instruction, and what the gates made of it.
struct Asked {
    step: usize,
    prefix: String,
    asked: f64,
    arrived: f64,
    d: Decision,
    gate: String,
    applied: bool,
}

struct Change {
    start: f64,
    dur: f64,
    from: usize,
    to: usize,
}

fn gate(s: &State, d: &Decision, complete: bool) -> (Option<State>, String) {
    let conf = d.confidence.unwrap_or(1.0);
    let mark = d.answers.get("action").and_then(|a| a.as_str()) == Some("mark");
    let need = if mark && !complete { GATE_MARK_EARLY } else { GATE };
    if conf < need {
        return (None, format!("waiting: confidence {conf:.2} < {need}"));
    }
    match pilot::apply(s, &d.answers) {
        Err(NotApplied::NeedsText) => (None, "needs free text".into()),
        Err(NotApplied::Unfit(m)) if m == "already so" => (None, "no change needed".into()),
        Err(NotApplied::Unfit(m)) => (None, format!("waiting: {m}")),
        Ok(n) if n == *s => (None, "no change".into()),
        Ok(n) => (Some(n), "applied".into()),
    }
}

fn t(s: &str, x: f32, y: f32, size: f32, color: [f32; 4], bold: bool) -> SceneMark {
    draw::text(s, x, y, size, color, TextAlign::Left, TextBaseline::Top, bold, 0.0)
}

fn fit(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("…{}", s.chars().skip(s.chars().count() - n + 1).collect::<String>()) }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let out = std::env::args().nth(1).unwrap_or("out/autopilot".into());
    std::fs::create_dir_all(&out)?;
    let d = data::load().await?;
    let jev = Jev { model: "typesafe/jev-1.13", client: reqwest::Client::new() };
    let q = pilot::questions();

    // Pass 1: the timeline. Decisions after every word, against the state
    // as it is at that moment.
    let mut states = vec![State::new(Dataset::Classes)];
    let mut asked: Vec<Asked> = vec![];
    let mut changes: Vec<Change> = vec![];
    let mut starts = vec![];
    let mut now = 2.0;
    for (step, text) in SCRIPT.iter().enumerate() {
        starts.push(now);
        let words: Vec<&str> = text.split(' ').collect();
        let mut end = now;
        for k in 1..=words.len() {
            let prefix = words[..k].join(" ");
            let at = now + prefix.chars().count() as f64 * CHAR;
            let cur = states.last().unwrap().clone();
            let obs = pilot::observation(&cur, &d, &prefix);
            let dec = jev.decide(&obs, &q).await.map_err(|e| format!("{prefix}: {e}"))?;
            let arrived = at + (dec.ms / 1000.0).clamp(0.15, 1.5);
            let (next, g) = gate(&cur, &dec, k == words.len());
            let applied = next.is_some();
            if let Some(n) = next {
                let bend = resolve(&cur, &d).coords != resolve(&n, &d).coords;
                let dur = if bend { 1.8 } else { 1.1 };
                let start = changes.last().map_or(arrived, |c| arrived.max(c.start + c.dur));
                states.push(n);
                changes.push(Change { start, dur, from: states.len() - 2, to: states.len() - 1 });
                end = end.max(start + dur);
            }
            println!("{:>2} {:<58} {:<18} {:.2} {}", step + 1, format!("\"{prefix}\""), pilot::short(&dec.answers), dec.confidence.unwrap_or(1.0), g);
            end = end.max(arrived);
            asked.push(Asked { step, prefix, asked: at, arrived, d: dec, gate: g, applied });
        }
        now = end + HOLD;
    }
    let total = now + 1.0;
    let cost: f64 = asked.iter().map(|a| a.d.cost).sum();
    println!("{} decisions, {} changes, {:.0} s, ${cost:.4}", asked.len(), changes.len(), total);
    let frames: Vec<Frame> = states.iter().map(|s| resolve(s, &d)).collect();

    // Pass 2: render.
    let mut canvas = PngCanvas::new(CanvasDimensions { size: [W, H], scale: 1.0 }, Default::default()).await?;
    let n = (total * FPS).round() as usize;
    for i in 0..n {
        let tau = i as f64 / FPS;
        // The chart.
        let drawn = match changes.iter().rev().find(|c| c.start <= tau) {
            None => still(&frames[0]),
            Some(c) if tau < c.start + c.dur => transition(&frames[c.from], &frames[c.to], (tau - c.start) / c.dur),
            Some(c) => still(&frames[c.to]),
        };
        let mut marks = vec![draw::rect(0.0, 0.0, W, H, [1.0; 4], None, 0.0)];
        marks.extend(draw::chart_marks(&drawn, draw::ORIGIN));

        // The panel.
        marks.push(draw::rect(PX - 20.0, 0.0, W - PX + 20.0, H, [0.975, 0.978, 0.985, 1.0], None, 0.0));
        marks.push(t("Autopilot", PX, 28.0, 20.0, INK, true));
        marks.push(t("Jev 1.13 via OpenRouter, deciding after every word", PX, 56.0, 12.0, MUTED, false));
        marks.push(t("sees the chart state and the instruction, no data rows", PX, 72.0, 12.0, MUTED, false));
        let step = starts.iter().rposition(|s| *s <= tau);
        let typed = step.map_or(String::new(), |k| {
            let c = ((tau - starts[k]) / CHAR).floor().max(0.0) as usize;
            SCRIPT[k].chars().take(c).collect()
        });
        marks.push(draw::rect(PX, 102.0, 410.0, 36.0, [1.0; 4], Some([0.78, 0.80, 0.84, 1.0]), 6.0));
        let caret = if (tau * 2.0).fract() < 0.5 { "|" } else { " " };
        marks.push(t(&format!("{}{caret}", fit(&typed, 44)), PX + 12.0, 112.0, 15.0, INK, false));

        // The latest decision, or the one in flight.
        let pending = asked.iter().rev().find(|a| a.asked <= tau && a.arrived > tau);
        let last = asked.iter().rev().find(|a| a.arrived <= tau && Some(a.step) == step);
        let mut y = 160.0;
        if let Some(a) = last {
            marks.push(t(&format!("decision on \"{}\"", fit(&a.prefix, 36)), PX, y, 12.0, MUTED, false));
            marks.push(t(&pilot::short(&a.d.answers).replace('/', "  ·  ").replace('_', " "), PX, y + 18.0, 20.0, INK, true));
            marks.push(t(&format!("{:.0} ms", a.d.ms), PX + 350.0, y + 22.0, 12.0, MUTED, false));
            y += 56.0;
            for (k, (opt, p)) in a.d.probs.iter().take(6).enumerate() {
                let yy = y + k as f32 * 24.0;
                let chosen = a.d.answers.get("action").and_then(|v| v.as_str()) == Some(opt);
                marks.push(t(&opt.replace('_', " "), PX, yy + 2.0, 13.0, if chosen { INK } else { MUTED }, chosen));
                marks.push(draw::rect(PX + 100.0, yy + 2.0, 240.0, 14.0, LIGHT, None, 3.0));
                marks.push(draw::rect(PX + 100.0, yy + 2.0, (240.0 * *p as f32).max(1.0), 14.0, if chosen { BAR } else { [0.70, 0.74, 0.80, 1.0] }, None, 3.0));
                marks.push(t(&format!("{p:.2}"), PX + 350.0, yy + 2.0, 12.0, MUTED, false));
            }
            y += 6.0 * 24.0 + 8.0;
            let colour = if a.applied { GREEN } else if a.gate.starts_with("waiting") { AMBER } else { MUTED };
            marks.push(t(&a.gate, PX, y, 14.0, colour, true));
        } else {
            y += 56.0 + 6.0 * 24.0 + 8.0;
        }
        if pending.is_some() {
            marks.push(t("deciding…", PX + 300.0, 160.0, 12.0, AMBER, false));
        }

        // What changed so far.
        y += 40.0;
        marks.push(draw::rule(PX, y - 12.0, PX + 410.0, y - 12.0, [0.85, 0.87, 0.90, 1.0]));
        marks.push(t("changes", PX, y, 12.0, MUTED, false));
        let done: Vec<&Asked> = asked.iter().filter(|a| a.applied && a.arrived <= tau).collect();
        for (k, a) in done.iter().rev().take(5).enumerate() {
            let alpha = 1.0 - 0.15 * k as f32;
            let c = [INK[0], INK[1], INK[2], alpha];
            marks.push(t(&fit(&a.prefix, 34), PX, y + 20.0 + k as f32 * 20.0, 12.0, c, false));
            marks.push(t(&pilot::short(&a.d.answers), PX + 280.0, y + 20.0 + k as f32 * 20.0, 12.0, c, true));
        }
        let seen: Vec<&Asked> = asked.iter().filter(|a| a.arrived <= tau).collect();
        let spent: f64 = seen.iter().map(|a| a.d.cost).sum();
        marks.push(t(&format!("{} decisions · {} changes · ${spent:.4}", seen.len(), done.len()), PX, H - 32.0, 12.0, MUTED, false));

        canvas.set_scene(&SceneGraph { marks, width: W, height: H, origin: [0.0; 2] })?;
        canvas.render().await?.save(format!("{out}/f{i:05}.png"))?;
    }
    std::fs::write(
        concat!(env!("CARGO_MANIFEST_DIR"), "/results/cache_used_autopilot_video.txt"),
        lidar_decide::deciders::used_cache().join("\n"),
    )?;
    println!("wrote {n} frames to {out}");
    Ok(())
}
