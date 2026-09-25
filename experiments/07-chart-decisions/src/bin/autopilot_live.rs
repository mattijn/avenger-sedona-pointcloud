//! The autopilot, live: type an instruction, Jev decides, and the one chart
//! object transitions.
//!
//! - Jev is asked 400 ms after you stop typing, and on Enter. Decisions run
//!   on the tokio runtime, so the window never waits on the network; a frame
//!   wake-up picks up their results.
//! - The gates are the video's: confidence ≥ 0.5 (≥ 0.9 for a change of chart
//!   kind before Enter), a complete intent that fits the chart, and a change.
//! - A decision that passes becomes experiment 6 pipeline lines. The pipeline
//!   validates and logs them, and the chart drawn is the fold of that log.
//! - Tab (or the button) toggles "stats for nerds": the pipeline, the last
//!   decision as the decider returned it, its translation, and timings.
//! - ⌘E (or the button) switches to editor mode: the same pipeline as text,
//!   written by hand. ⌘↵ applies it, Esc reverts to what is running.
//!
//! Decisions already in `cache/` replay without a key; new instructions call
//! OpenRouter:
//!
//!     set -a; source <env>/.env; set +a
//!     cargo run --release -p lidar-decide --bin autopilot_live

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use avenger_app::app::{AvengerApp, SceneBuild, SceneGraphBuilder};
use avenger_app::error::AvengerAppError;
use avenger_common::time::{Duration, Instant};
use avenger_eventstream::manager::EventStreamHandler;
use avenger_eventstream::runtime::{RuntimeHostCommand, RuntimeWakeKey};
use avenger_eventstream::scene::{SceneGraphEvent as Event, SceneGraphEventType as Type};
use avenger_eventstream::stream::{EventStreamConfig, UpdateStatus};
use avenger_eventstream::window::{Key, MouseButton, NamedKey};
use avenger_geometry::rtree::SceneGraphRTree;
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::text::SceneTextMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_text::types::{TextAlign, TextBaseline};
use avenger_wgpu::canvas::CanvasConfig;
use lidar_common::{INK, MUTED};
use lidar_decide::deciders::{Decider, Decision, Jev};
use lidar_decide::layer::anim::{still, transition};
use lidar_decide::layer::model::{resolve, Data, Dataset, Frame, State};
use lidar_decide::layer::{data, draw, editor, package, pilot};
use lidar_decide::layer_pipeline;
use lidar_decide::options::NotApplied;
use lidar_pipeline::pipeline::{Kind, Pipeline};
use serde_json::Value;

const W: f32 = 1400.0;
const H: f32 = 640.0;
const PX: f32 = 960.0;
const WAKE: &str = "autopilot";
/// The current pipeline, written on every change.
const PIPELINE_FILE: &str = "out/autopilot_live/pipeline.txt";
const FRAME: Duration = Duration::from_millis(16);
const DEBOUNCE: Duration = Duration::from_millis(400);
const GATE: f64 = 0.5;
const GATE_MARK_EARLY: f64 = 0.9;
const GREEN: [f32; 4] = [0.23, 0.55, 0.30, 1.0];
const AMBER: [f32; 4] = [0.80, 0.50, 0.10, 1.0];
const RED: [f32; 4] = [0.70, 0.10, 0.08, 1.0];
const BAR: [f32; 4] = [0.30, 0.47, 0.66, 1.0];
const LIGHT: [f32; 4] = [0.93, 0.94, 0.96, 1.0];
/// The buttons in the panel header.
const NERDS_BUTTON: [f32; 4] = [PX + 300.0, 24.0, 110.0, 26.0];
const AUTOPILOT_BUTTON: [f32; 4] = [PX, 24.0, 84.0, 26.0];
const EDITOR_BUTTON: [f32; 4] = [PX + 84.0, 24.0, 64.0, 26.0];
/// The editor's text area, and its monospace grid.
const EDIT_AREA: [f32; 4] = [PX, 96.0, 410.0, 392.0];
const COLS: usize = 60;
const CW: f32 = 6.62;
const LH: f32 = 14.0;

fn inside(p: [f32; 2], r: [f32; 4]) -> bool {
    p[0] >= r[0] && p[0] <= r[0] + r[2] && p[1] >= r[1] && p[1] <= r[1] + r[3]
}

/// A decision that came back from a background task.
struct Returned {
    prefix: String,
    complete: bool,
    wall_ms: f64,
    result: Result<Decision, String>,
}

/// The last decision, with what became of it.
#[derive(Clone)]
struct Shown {
    prefix: String,
    complete: bool,
    wall_ms: f64,
    decision: Decision,
    gate: String,
    colour: [f32; 4],
    lines: Vec<String>,
    fold: String,
}

#[derive(Clone)]
struct App {
    /// The four tables as cached, and the ones drawn now (an edited data
    /// stage replaces one).
    base: Arc<Data>,
    data: Arc<Data>,
    /// The data stages of the pipeline, as written.
    data_stages: Vec<String>,
    pipe: Arc<tokio::sync::Mutex<Pipeline>>,
    rt: tokio::runtime::Handle,
    jev: Arc<Jev>,
    questions: Arc<Value>,
    taken: Vec<(String, String)>,
    // The chart.
    state: State,
    from: Frame,
    to: Frame,
    started: Option<Instant>,
    dur: f64,
    // Input.
    input: String,
    typed_at: Option<Instant>,
    asked: String,
    // Decisions.
    returned: Arc<Mutex<Vec<Returned>>>,
    in_flight: usize,
    shown: Option<Shown>,
    changes: Vec<(String, String)>,
    decisions: usize,
    cached: usize,
    cost: f64,
    latencies: Vec<f64>,
    message: String,
    // Stats for nerds.
    nerds: bool,
    log: Vec<(String, f64)>,
    pipeline: Vec<String>,
    chart_json: String,
    frame_ms: f64,
    build_ms: f64,
    last_frame: Option<Instant>,
    items: usize,
    wake_generation: u64,
    started_at: std::time::Instant,
    // Editor mode.
    editing: bool,
    text: Vec<char>,
    caret: usize,
    scroll: usize,
    edit_status: (String, [f32; 4]),
    applying: bool,
    edited: Arc<Mutex<Option<Result<editor::Applied, String>>>>,
}

/// Visual rows of the editor text: (start, length) in chars, wrapped at
/// `COLS`.
fn rows(text: &[char]) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut start = 0;
    for (i, c) in text.iter().chain(std::iter::once(&'\n')).enumerate() {
        if *c == '\n' {
            let mut a = start;
            loop {
                let len = (i - a).min(COLS);
                out.push((a, len));
                a += len;
                if a >= i {
                    break;
                }
            }
            start = i + 1;
        }
    }
    out
}

/// The row and column of a caret.
fn caret_at(rows: &[(usize, usize)], caret: usize) -> (usize, usize) {
    for (r, (a, len)) in rows.iter().enumerate() {
        let next_continues = rows.get(r + 1).is_some_and(|n| n.0 == a + len);
        if caret >= *a && (caret < a + len || (caret == a + len && !next_continues)) {
            return (r, caret - a);
        }
    }
    let last = rows.len() - 1;
    (last, rows[last].1)
}

impl App {
    fn animating(&self, now: Instant) -> bool {
        self.started.is_some_and(|s| (now - s).as_secs_f64() < self.dur)
    }

    fn progress(&self, now: Instant) -> f64 {
        self.started.map_or(1.0, |s| ((now - s).as_secs_f64() / self.dur).min(1.0))
    }

    /// Ask Jev about the current input, in the background.
    fn ask(&mut self, complete: bool) {
        let prefix = self.input.trim().to_string();
        if prefix.is_empty() || (prefix == self.asked && !complete) {
            return;
        }
        self.asked = prefix.clone();
        self.in_flight += 1;
        let obs = pilot::observation(&self.state, &self.data, &prefix);
        let (jev, q, out) = (self.jev.clone(), self.questions.clone(), self.returned.clone());
        self.rt.spawn(async move {
            let t = std::time::Instant::now();
            let result = jev.decide(&obs, &q).await;
            let wall_ms = t.elapsed().as_secs_f64() * 1e3;
            out.lock().unwrap().push(Returned { prefix, complete, wall_ms, result });
        });
    }

    /// Gate the returned decisions and run the ones that pass through the
    /// pipeline.
    async fn collect(&mut self, now: Instant) {
        let returned: Vec<Returned> = std::mem::take(&mut *self.returned.lock().unwrap());
        for r in returned {
            self.in_flight -= 1;
            let d = match r.result {
                Ok(d) => d,
                Err(e) => {
                    self.message = format!("decider: {e}");
                    continue;
                }
            };
            self.decisions += 1;
            self.cached += d.cached as usize;
            self.cost += if d.cached { 0.0 } else { d.cost };
            self.latencies.push(r.wall_ms);
            let conf = d.confidence.unwrap_or(1.0);
            let mark = d.answers.get("action").and_then(Value::as_str) == Some("mark");
            let need = if mark && !r.complete { GATE_MARK_EARLY } else { GATE };
            let (next, gate) = if conf < need {
                (None, format!("waiting: confidence {conf:.2} < {need}"))
            } else {
                match pilot::apply(&self.state, &d.answers) {
                    Err(NotApplied::NeedsText) => (None, "needs free text".into()),
                    Err(NotApplied::Unfit(m)) if m == "already so" => (None, "no change needed".into()),
                    Err(NotApplied::Unfit(m)) => (None, format!("waiting: {m}")),
                    Ok(n) if n == self.state => (None, "no change".into()),
                    Ok(n) => (Some(n), "applied".into()),
                }
            };
            let mut shown = Shown {
                prefix: r.prefix.clone(),
                complete: r.complete,
                wall_ms: r.wall_ms,
                decision: d,
                colour: if next.is_some() { GREEN } else if gate.starts_with("waiting") { AMBER } else { MUTED },
                gate,
                lines: vec![],
                fold: String::new(),
            };
            if let Some(n) = next {
                // Another dataset is read from its cache, as it is.
                if n.dataset != self.state.dataset {
                    self.data = self.base.clone();
                    self.data_stages = package::data_stages(n.dataset);
                }
                let lines = package::lines(&self.state, &n, &self.data);
                let pipe = self.pipe.clone();
                let mut p = pipe.lock().await;
                let mut failed = None;
                for l in &lines {
                    if let Err(e) = p.run(l).await {
                        failed = Some(format!("rejected by the pipeline: {e}"));
                        break;
                    }
                }
                shown.lines = lines;
                match (failed, package::state(&p.chart, &self.data)) {
                    (Some(e), _) | (None, Err(e)) => {
                        shown.gate = e.clone();
                        shown.colour = RED;
                        shown.fold = e;
                    }
                    (None, Ok(folded)) => {
                        shown.fold = if folded == n { "folds to the decided state".into() } else { format!("folds to {folded:?}") };
                        self.changes.push((r.prefix.clone(), pilot::short(&shown.decision.answers)));
                        self.transition_to(folded, now);
                    }
                }
                self.snapshot(&p);
            }
            self.shown = Some(shown);
        }
    }

    fn open_editor(&mut self) {
        self.editing = true;
        self.text = self.pipeline.join("\n! ").chars().collect();
        self.caret = self.text.len();
        self.edit_status = (String::new(), MUTED);
    }

    fn apply_text(&mut self) {
        if self.applying {
            return;
        }
        self.applying = true;
        self.edit_status = ("applying…".into(), AMBER);
        let text: String = self.text.iter().collect();
        let (base, out) = (self.base.clone(), self.edited.clone());
        self.rt.spawn(async move {
            let r = editor::apply(&text, &base).await;
            *out.lock().unwrap() = Some(r);
        });
    }

    async fn collect_edit(&mut self, now: Instant) {
        let Some(r) = self.edited.lock().unwrap().take() else { return };
        self.applying = false;
        match r {
            Ok(a) => {
                self.edit_status = (format!("applied in {:.0} ms · {}", a.ms, a.note), GREEN);
                self.data = Arc::new(a.data);
                self.data_stages = a.data_stages;
                self.changes.push(("editor".into(), "pipeline".into()));
                // The chart changed by hand, not by a decision.
                self.shown = None;
                let pipe = self.pipe.clone();
                let mut p = pipe.lock().await;
                *p = a.pipeline;
                self.transition_to(a.state, now);
                self.snapshot(&p);
            }
            Err(e) => self.edit_status = (e, RED),
        }
    }

    fn edit_key(&mut self, key: &Key, text: Option<&str>, cmd: bool) -> bool {
        let r = rows(&self.text);
        let (row, col) = caret_at(&r, self.caret);
        match (key, text) {
            (Key::Named(NamedKey::Enter), _) if cmd => self.apply_text(),
            (Key::Named(NamedKey::Enter), _) => self.insert("\n"),
            (Key::Named(NamedKey::Escape), _) => self.open_editor(),
            (Key::Named(NamedKey::Backspace), _) if self.caret > 0 => {
                self.caret -= 1;
                self.text.remove(self.caret);
            }
            (Key::Named(NamedKey::Delete), _) if self.caret < self.text.len() => {
                self.text.remove(self.caret);
            }
            (Key::Named(NamedKey::ArrowLeft), _) => self.caret = self.caret.saturating_sub(1),
            (Key::Named(NamedKey::ArrowRight), _) => self.caret = (self.caret + 1).min(self.text.len()),
            (Key::Named(NamedKey::ArrowUp), _) if row > 0 => self.caret = r[row - 1].0 + col.min(r[row - 1].1),
            (Key::Named(NamedKey::ArrowDown), _) if row + 1 < r.len() => self.caret = r[row + 1].0 + col.min(r[row + 1].1),
            (Key::Named(NamedKey::Home), _) => self.caret = r[row].0,
            (Key::Named(NamedKey::End), _) => self.caret = r[row].0 + r[row].1,
            (Key::Named(NamedKey::Space), _) => self.insert(" "),
            (_, Some(t)) if !cmd && !t.chars().any(char::is_control) => self.insert(t),
            _ => return false,
        }
        true
    }

    fn insert(&mut self, t: &str) {
        for c in t.chars() {
            self.text.insert(self.caret, c);
            self.caret += 1;
        }
    }

    fn click_editor(&mut self, p: [f32; 2]) {
        let r = rows(&self.text);
        let row = (self.scroll + ((p[1] - EDIT_AREA[1] - 8.0) / LH).max(0.0) as usize).min(r.len() - 1);
        let col = (((p[0] - EDIT_AREA[0] - 8.0) / CW).round().max(0.0) as usize).min(r[row].1);
        self.caret = r[row].0 + col;
    }

    fn transition_to(&mut self, n: State, now: Instant) {
        let next = resolve(&n, &self.data);
        // A change during a transition starts from where that one was going.
        self.from = self.to.clone();
        self.dur = if self.from.coords != next.coords { 1.8 } else { 1.1 };
        self.to = next;
        self.state = n;
        self.started = Some(now);
    }

    /// What stats for nerds shows of the pipeline.
    fn snapshot(&mut self, p: &Pipeline) {
        self.log = p
            .trace
            .iter()
            .filter(|(_, k, _)| matches!(k, Kind::Source | Kind::Transform | Kind::Command))
            .map(|(c, _, ms)| (c.clone(), *ms))
            .collect();
        self.chart_json = p.chart.to_string();
        self.pipeline = self.data_stages.iter().cloned().chain(package::commands(&self.state, &self.data)).collect();
        let _ = std::fs::create_dir_all("out/autopilot_live");
        let _ = std::fs::write(PIPELINE_FILE, self.pipeline.join("\n! ") + "\n");
    }
}

// ---------------------------------------------------------------------------

fn t(s: &str, x: f32, y: f32, size: f32, color: [f32; 4], bold: bool) -> SceneMark {
    draw::text(s, x, y, size, color, TextAlign::Left, TextBaseline::Top, bold, 0.0)
}

fn mono(s: &str, x: f32, y: f32, color: [f32; 4]) -> SceneMark {
    SceneTextMark {
        len: 1,
        text: s.to_string().into(),
        x: x.into(),
        y: y.into(),
        font: "monospace".to_string().into(),
        font_size: 11.0.into(),
        color: draw::c(color).into(),
        baseline: TextBaseline::Top.into(),
        ..Default::default()
    }
    .into()
}

fn fit(s: &str, n: usize) -> String {
    let c = s.chars().count();
    if c <= n { s.to_string() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

/// Break a stage into rows of at most `n` characters, at spaces.
fn wrap(s: &str, n: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    for word in s.split(' ') {
        let row = rows.last_mut().unwrap();
        if !row.is_empty() && row.chars().count() + 1 + word.chars().count() > n {
            rows.push(word.to_string());
        } else {
            if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(word);
        }
    }
    rows
}

fn tail(s: &str, n: usize) -> String {
    let c = s.chars().count();
    if c <= n { s.to_string() } else { format!("…{}", s.chars().skip(c - n + 1).collect::<String>()) }
}

fn panel(s: &App, marks: &mut Vec<SceneMark>) {
    marks.push(draw::rect(PX - 20.0, 0.0, W - PX + 20.0, H, [0.975, 0.978, 0.985, 1.0], None, 0.0));
    for (label, r, on) in [
        ("autopilot", AUTOPILOT_BUTTON, !s.editing),
        ("editor", EDITOR_BUTTON, s.editing),
        ("stats for nerds", NERDS_BUTTON, s.nerds),
    ] {
        let [bx, by, bw, bh] = r;
        marks.push(draw::rect(bx, by, bw, bh, if on { INK } else { [1.0; 4] }, Some([0.78, 0.80, 0.84, 1.0]), 5.0));
        marks.push(t(label, bx + 10.0, by + 6.0, 12.0, if on { [1.0; 4] } else { INK }, on && r != NERDS_BUTTON));
    }
    if s.editing {
        editor_panel(s, marks);
        return;
    }
    marks.push(t("Jev 1.13 via OpenRouter · asks 400 ms after typing, and on Enter", PX, 58.0, 12.0, MUTED, false));
    marks.push(t("Tab: stats for nerds · ⌘E: editor · Esc: clear", PX, 74.0, 12.0, MUTED, false));

    marks.push(draw::rect(PX, 102.0, 410.0, 36.0, [1.0; 4], Some([0.55, 0.62, 0.72, 1.0]), 6.0));
    let caret = if (s.started_at.elapsed().as_secs_f64() * 2.0).fract() < 0.5 { "|" } else { " " };
    if s.input.is_empty() {
        marks.push(t(&format!("{caret}try: show this as a pie"), PX + 12.0, 112.0, 15.0, [0.7, 0.72, 0.76, 1.0], false));
    } else {
        marks.push(t(&format!("{}{caret}", tail(&s.input, 44)), PX + 12.0, 112.0, 15.0, INK, false));
    }
    if s.in_flight > 0 {
        marks.push(t("deciding…", PX + 330.0, 144.0, 12.0, AMBER, false));
    }

    let mut y = 164.0;
    if let Some(a) = &s.shown {
        marks.push(t(&format!("decision on \"{}\"{}", fit(&a.prefix, 34), if a.complete { " ⏎" } else { "" }), PX, y, 12.0, MUTED, false));
        marks.push(t(&pilot::short(&a.decision.answers).replace('/', "  ·  ").replace('_', " "), PX, y + 18.0, 20.0, INK, true));
        let latency = if a.decision.cached { format!("cache · {:.0} ms live", a.decision.ms) } else { format!("{:.0} ms", a.wall_ms) };
        marks.push(draw::text(&latency, PX + 410.0, y + 22.0, 12.0, MUTED, TextAlign::Right, TextBaseline::Top, false, 0.0));
        y += 56.0;
        for (k, (opt, p)) in a.decision.probs.iter().take(6).enumerate() {
            let yy = y + k as f32 * 24.0;
            let chosen = a.decision.answers.get("action").and_then(Value::as_str) == Some(opt);
            marks.push(t(&opt.replace('_', " "), PX, yy + 2.0, 13.0, if chosen { INK } else { MUTED }, chosen));
            marks.push(draw::rect(PX + 100.0, yy + 2.0, 240.0, 14.0, LIGHT, None, 3.0));
            marks.push(draw::rect(PX + 100.0, yy + 2.0, (240.0 * *p as f32).max(1.0), 14.0, if chosen { BAR } else { [0.70, 0.74, 0.80, 1.0] }, None, 3.0));
            marks.push(t(&format!("{p:.2}"), PX + 350.0, yy + 2.0, 12.0, MUTED, false));
        }
        y += 6.0 * 24.0 + 8.0;
        marks.push(t(&fit(&a.gate, 60), PX, y, 14.0, a.colour, true));
    } else {
        y += 56.0 + 6.0 * 24.0 + 8.0;
        marks.push(t("type an instruction", PX, y, 14.0, MUTED, false));
    }
    y += 40.0;
    marks.push(draw::rule(PX, y - 12.0, PX + 410.0, y - 12.0, [0.85, 0.87, 0.90, 1.0]));
    marks.push(t("changes", PX, y, 12.0, MUTED, false));
    for (k, (prefix, short)) in s.changes.iter().rev().take(5).enumerate() {
        let c = [INK[0], INK[1], INK[2], 1.0 - 0.15 * k as f32];
        marks.push(t(&fit(prefix, 34), PX, y + 20.0 + k as f32 * 20.0, 12.0, c, false));
        marks.push(t(short, PX + 280.0, y + 20.0 + k as f32 * 20.0, 12.0, c, true));
    }
    let footer = if s.message.is_empty() {
        format!("{} decisions ({} cached) · {} changes · ${:.4} spent", s.decisions, s.cached, s.changes.len(), s.cost)
    } else {
        s.message.clone()
    };
    marks.push(t(&fit(&footer, 64), PX, H - 32.0, 12.0, if s.message.is_empty() { MUTED } else { RED }, false));
}

/// Editor mode: the pipeline as text.
fn editor_panel(s: &App, marks: &mut Vec<SceneMark>) {
    marks.push(t("the chart as a pipeline · ⌘↵ apply · Esc revert · ⌘E autopilot", PX, 64.0, 12.0, MUTED, false));
    let [ex, ey, ew, eh] = EDIT_AREA;
    marks.push(draw::rect(ex, ey, ew, eh, [1.0; 4], Some([0.55, 0.62, 0.72, 1.0]), 6.0));
    let r = rows(&s.text);
    let (row, col) = caret_at(&r, s.caret);
    let visible = ((eh - 16.0) / LH) as usize;
    for (k, (a, len)) in r.iter().enumerate().skip(s.scroll).take(visible) {
        let line: String = s.text[*a..a + len].iter().collect();
        // Stage names stand out; continuation rows are plain.
        let starts_stage = *a == 0 || s.text[a - 1] == '\n';
        let colour = if starts_stage { INK } else { [0.25, 0.28, 0.33, 1.0] };
        marks.push(mono(&line, ex + 8.0, ey + 8.0 + (k - s.scroll) as f32 * LH, colour));
    }
    if row >= s.scroll && row < s.scroll + visible && (s.started_at.elapsed().as_secs_f64() * 2.0).fract() < 0.6 {
        let (cx, cy) = (ex + 8.0 + col as f32 * CW, ey + 8.0 + (row - s.scroll) as f32 * LH);
        marks.push(draw::rect(cx - 0.5, cy - 1.0, 1.4, 14.0, INK, None, 0.0));
    }
    let mut y = ey + eh + 10.0;
    let (status, colour) = &s.edit_status;
    for line in wrap(status, 62).iter().take(3) {
        marks.push(t(line, PX, y, 12.0, *colour, false));
        y += 16.0;
    }
    let help = [
        "bars n --by label · pie n --by label · line n --x t --series line",
        "heatmap n --x band --y label · map --x cx --y cy --value h",
        "color #rrggbb · highlight \"datum.<f> >= <n>\" · clear-highlight",
        "zoom x0..x1 y0..y1 · reset-zoom · data: read, filter, calc, sql",
    ];
    for (k, h) in help.iter().enumerate() {
        marks.push(t(h, PX, H - 84.0 + k as f32 * 16.0, 11.0, MUTED, false));
    }
}

/// Stats for nerds: an overlay on the chart.
fn nerds(s: &App, now: Instant, marks: &mut Vec<SceneMark>) {
    let (x0, y0, w, h) = (16.0, 16.0, PX - 52.0, H - 32.0);
    marks.push(draw::rect(x0, y0, w, h, [0.08, 0.09, 0.11, 0.93], None, 8.0));
    let (head, text, dim) = ([0.55, 0.78, 1.0, 1.0], [0.92, 0.93, 0.95, 1.0], [0.62, 0.65, 0.70, 1.0]);
    let x = x0 + 16.0;
    let mut y = y0 + 14.0;
    let line = |marks: &mut Vec<SceneMark>, label: &str, value: &str, colour: [f32; 4], y: &mut f32| {
        marks.push(mono(label, x, *y, dim));
        marks.push(mono(&fit(value, 108), x + 76.0, *y, colour));
        *y += 15.0;
    };
    marks.push(mono("stats for nerds", x, y, head));
    marks.push(mono(&format!("the whole pipeline behind this chart, runnable as it stands · also in {PIPELINE_FILE}"), x + 130.0, y, dim));
    y += 20.0;
    // One stage per line, wrapped, never cut. Stages the last decision
    // added are green.
    let added = |stage: &str| s.shown.as_ref().is_some_and(|a| a.lines.iter().any(|l| l.split(" ! ").any(|p| p == stage)));
    for (i, stage) in s.pipeline.iter().enumerate() {
        let colour = if i > 0 && added(stage) { [0.62, 0.90, 0.62, 1.0] } else { text };
        for (k, row) in wrap(stage, 126).iter().enumerate() {
            let lead = match (i, k) {
                (0, 0) => "  ",
                (_, 0) => "! ",
                _ => "    ",
            };
            marks.push(mono(&format!("{lead}{row}"), x, y, colour));
            y += 14.0;
        }
    }
    y += 8.0;
    let packages = "vega_format · vega_compat · sedona · chart · layer";
    let taken: Vec<String> = s.taken.iter().map(|(n, prev)| format!("layer's {} replaces {prev}'s", n.trim_start_matches("step "))).collect();
    line(marks, "packages", &format!("{packages}   ({})", taken.join(", ")), dim, &mut y);
    // The session's log: every line run so far, newest last.
    let start = s.log.len().saturating_sub(4);
    for (i, (c, ms)) in s.log.iter().enumerate().skip(start) {
        let label = if i == start { "history" } else { "" };
        line(marks, label, &format!("{:>3}  {:<92}{:>7.2} ms", i + 1, fit(c, 92), ms), dim, &mut y);
    }
    y += 6.0;
    let json = &s.chart_json;
    line(marks, "chart", &json.chars().take(108).collect::<String>(), text, &mut y);
    if json.chars().count() > 108 {
        line(marks, "", &json.chars().skip(108).collect::<String>(), text, &mut y);
    }
    y += 10.0;
    if let Some(a) = &s.shown {
        let d = &a.decision;
        line(marks, "asked", &format!("\"{}\"  ({})", a.prefix, if a.complete { "Enter" } else { "400 ms after typing" }), text, &mut y);
        line(marks, "answers", &Value::Object(d.answers.clone()).to_string(), text, &mut y);
        let probs: Vec<String> = d.probs.iter().map(|(k, p)| format!("{k} {p:.2}")).collect();
        line(marks, "action p", &probs.join(" · "), text, &mut y);
        line(
            marks,
            "jev",
            &format!(
                "confidence {:.2} · {:.0} ms here · {} ({:.0} ms at the first call) · {} input tokens · ${:.6}",
                d.confidence.unwrap_or(1.0),
                a.wall_ms,
                if d.cached { "from cache" } else { "live" },
                d.ms,
                d.input_tokens,
                d.cost
            ),
            text,
            &mut y,
        );
        line(marks, "gate", &a.gate, a.colour, &mut y);
        if a.lines.is_empty() {
            line(marks, "lines", "none", dim, &mut y);
        }
        for (i, l) in a.lines.iter().enumerate() {
            line(marks, if i == 0 { "lines" } else { "" }, l, [0.62, 0.90, 0.62, 1.0], &mut y);
        }
        if !a.fold.is_empty() {
            line(marks, "fold", &a.fold, text, &mut y);
        }
    } else if s.changes.last().is_some_and(|c| c.0 == "editor") {
        line(marks, "edited", "by hand in the editor, not by a decision", text, &mut y);
        line(marks, "status", &s.edit_status.0, s.edit_status.1, &mut y);
    } else {
        line(marks, "asked", "nothing yet", dim, &mut y);
    }
    y += 10.0;
    let mut l = s.latencies.clone();
    l.sort_by(f64::total_cmp);
    let p50 = l.get(l.len() / 2).map_or("–".into(), |v| format!("{v:.0} ms"));
    line(
        marks,
        "render",
        &format!(
            "{} items · {:?} · transition {:.2} · build {:.1} ms · frame {:.0} ms",
            s.items,
            s.to.coords,
            s.progress(now),
            s.build_ms,
            s.frame_ms
        ),
        text,
        &mut y,
    );
    line(
        marks,
        "session",
        &format!("{} decisions · {} from cache · {} changes · p50 {p50} · ${:.4} spent", s.decisions, s.cached, s.changes.len(), s.cost),
        text,
        &mut y,
    );
}

fn build(s: &mut App) -> SceneBuild {
    let now = Instant::now();
    let t0 = std::time::Instant::now();
    if let Some(last) = s.last_frame {
        s.frame_ms = 0.8 * s.frame_ms + 0.2 * (now - last).as_secs_f64() * 1e3;
    }
    s.last_frame = Some(now);
    if s.editing {
        let (row, _) = caret_at(&rows(&s.text), s.caret);
        let visible = ((EDIT_AREA[3] - 16.0) / LH) as usize;
        if row < s.scroll {
            s.scroll = row;
        } else if row >= s.scroll + visible {
            s.scroll = row + 1 - visible;
        }
    }
    let drawn = match s.started {
        Some(_) if s.animating(now) => transition(&s.from, &s.to, s.progress(now)),
        _ => still(&s.to),
    };
    s.items = drawn.items.len();
    let mut marks = vec![draw::rect(0.0, 0.0, W, H, [1.0; 4], None, 0.0)];
    marks.extend(draw::chart_marks(&drawn, draw::ORIGIN));
    if s.nerds {
        nerds(s, now, &mut marks);
    }
    panel(s, &mut marks);
    s.build_ms = t0.elapsed().as_secs_f64() * 1e3;

    // Keep waking while something moves: a transition, a decision in
    // flight, the caret, or a pending debounce.
    let mut commands = vec![];
    let deadline = match s.typed_at {
        Some(at) if s.input.trim() != s.asked => (at + DEBOUNCE).min(now + FRAME * 30),
        _ if s.animating(now) || s.in_flight > 0 || s.applying => now + FRAME,
        _ => now + Duration::from_millis(500),
    };
    s.wake_generation += 1;
    commands.push(RuntimeHostCommand::RequestWakeup {
        key: RuntimeWakeKey::new(WAKE, 0, "frame"),
        deadline,
        generation: s.wake_generation,
    });
    SceneBuild {
        scene_graph: SceneGraph { marks, width: W, height: H, origin: [0.0; 2] },
        commands,
        rebuild_geometry: false,
    }
}

struct Builder;
#[async_trait]
impl SceneGraphBuilder<App> for Builder {
    async fn build(&self, s: &mut App) -> Result<SceneGraph, AvengerAppError> {
        Ok(build(s).scene_graph)
    }
    async fn build_with_effects(&self, s: &mut App) -> Result<SceneBuild, AvengerAppError> {
        Ok(build(s))
    }
}

struct Input;
#[async_trait]
impl EventStreamHandler<App> for Input {
    async fn handle(&self, event: &Event, s: &mut App, _rtree: &SceneGraphRTree) -> UpdateStatus {
        let now = Instant::now();
        let rerender = UpdateStatus { rerender: true, ..Default::default() };
        match event {
            Event::RuntimeWake(w) if w.key.namespace == WAKE => {
                if s.typed_at.is_some_and(|at| now - at >= DEBOUNCE) {
                    s.typed_at = None;
                    s.ask(false);
                }
                s.collect(now).await;
                s.collect_edit(now).await;
                rerender
            }
            Event::KeyPress(e) => {
                let cmd = e.modifiers.meta || e.modifiers.control;
                if matches!(e.key, Key::Character('e') | Key::Character('E')) && cmd {
                    if s.editing { s.editing = false } else { s.open_editor() }
                    return rerender;
                }
                if matches!(e.key, Key::Named(NamedKey::Tab)) {
                    s.nerds = !s.nerds;
                    return rerender;
                }
                if s.editing {
                    return if s.edit_key(&e.key, e.text.as_deref(), cmd) { rerender } else { UpdateStatus::default() };
                }
                match (&e.key, e.text.as_deref()) {
                    (Key::Named(NamedKey::Tab), _) => s.nerds = !s.nerds,
                    (Key::Named(NamedKey::Escape), _) => {
                        s.input.clear();
                        s.typed_at = None;
                    }
                    (Key::Named(NamedKey::Enter), _) => {
                        s.typed_at = None;
                        s.ask(true);
                        s.input.clear();
                    }
                    (Key::Named(NamedKey::Backspace), _) => {
                        s.input.pop();
                        s.typed_at = Some(now);
                    }
                    (Key::Named(NamedKey::Space), _) => {
                        s.input.push(' ');
                        s.typed_at = Some(now);
                    }
                    (_, Some(text)) if !text.chars().any(char::is_control) && !e.modifiers.meta && !e.modifiers.control => {
                        s.input.push_str(text);
                        s.message.clear();
                        s.typed_at = Some(now);
                    }
                    _ => return UpdateStatus::default(),
                }
                rerender
            }
            Event::MouseDown(e) if e.button == MouseButton::Left => {
                let p = e.position;
                if inside(p, NERDS_BUTTON) {
                    s.nerds = !s.nerds;
                } else if inside(p, AUTOPILOT_BUTTON) {
                    s.editing = false;
                } else if inside(p, EDITOR_BUTTON) && !s.editing {
                    s.open_editor();
                } else if s.editing && inside(p, EDIT_AREA) {
                    s.click_editor(p);
                } else {
                    return UpdateStatus::default();
                }
                rerender
            }
            _ => UpdateStatus::default(),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let data = Arc::new(runtime.block_on(data::load())?);
    let (mut pipe, taken) = runtime.block_on(layer_pipeline())?;
    let state = State::new(Dataset::Classes);
    for l in package::initial(&state, &data) {
        runtime.block_on(pipe.run(&l))?;
    }
    let state = package::state(&pipe.chart, &data)?;
    let frame = resolve(&state, &data);
    let mut app = App {
        base: data.clone(),
        data: data.clone(),
        data_stages: package::data_stages(state.dataset),
        pipe: Arc::new(tokio::sync::Mutex::new(Pipeline::new(pipe.ctx.clone()))),
        rt: runtime.handle().clone(),
        jev: Arc::new(Jev { model: "typesafe/jev-1.13", client: reqwest::Client::new() }),
        questions: Arc::new(pilot::questions()),
        taken,
        state,
        from: frame.clone(),
        to: frame,
        started: None,
        dur: 1.0,
        input: String::new(),
        typed_at: None,
        asked: String::new(),
        returned: Arc::new(Mutex::new(vec![])),
        in_flight: 0,
        shown: None,
        changes: vec![],
        decisions: 0,
        cached: 0,
        cost: 0.0,
        latencies: vec![],
        message: if std::env::var("OPENROUTER_API_KEY").is_ok() {
            String::new()
        } else {
            "no OPENROUTER_API_KEY: only cached decisions work".into()
        },
        nerds: false,
        log: vec![],
        pipeline: vec![],
        chart_json: String::new(),
        frame_ms: 16.0,
        build_ms: 0.0,
        last_frame: None,
        items: 0,
        wake_generation: 0,
        started_at: std::time::Instant::now(),
        editing: false,
        text: vec![],
        caret: 0,
        scroll: 0,
        edit_status: (String::new(), MUTED),
        applying: false,
        edited: Arc::new(Mutex::new(None)),
    };
    app.snapshot(&pipe);
    app.pipe = Arc::new(tokio::sync::Mutex::new(pipe));

    // `--snapshot <dir> "instruction" ...`: the same path as the window
    // (Enter, gates, pipeline, fold), rendered to PNG with and without stats
    // for nerds, for checking without a display.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--snapshot") {
        let dir = args.get(2).ok_or("--snapshot <dir> instructions...")?.clone();
        std::fs::create_dir_all(&dir)?;
        return runtime.block_on(async move {
            use avenger_wgpu::canvas::{Canvas, PngCanvas};
            let mut canvas = PngCanvas::new(avenger_common::canvas::CanvasDimensions { size: [W, H], scale: 1.0 }, Default::default()).await?;
            for (i, text) in args[3..].iter().enumerate() {
                if let Some(path) = text.strip_prefix('@') {
                    app.open_editor();
                    app.text = std::fs::read_to_string(path)?.trim_end().chars().collect();
                    app.caret = app.text.len();
                    app.apply_text();
                    while app.edited.lock().unwrap().is_none() {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    app.collect_edit(Instant::now()).await;
                    app.started = None;
                    for nerds in [false, true] {
                        app.nerds = nerds;
                        canvas.set_scene(&build(&mut app).scene_graph)?;
                        canvas.render().await?.save(format!("{dir}/{i:02}_editor{}.png", if nerds { "_nerds" } else { "" }))?;
                    }
                    println!("editor {path}: {}", app.edit_status.0);
                    app.editing = false;
                    continue;
                }
                app.input = text.clone();
                app.ask(true);
                app.input.clear();
                while app.returned.lock().unwrap().is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                app.shown = None;
                app.message.clear();
                app.collect(Instant::now()).await;
                app.started = None;
                if !app.message.is_empty() {
                    println!("{text:<45} error: {}", app.message);
                    continue;
                }
                for nerds in [false, true] {
                    app.nerds = nerds;
                    canvas.set_scene(&build(&mut app).scene_graph)?;
                    canvas.render().await?.save(format!("{dir}/{i:02}{}.png", if nerds { "_nerds" } else { "" }))?;
                }
                println!("{text:<45} {:<20} {}", app.shown.as_ref().map_or(String::new(), |a| pilot::short(&a.decision.answers)), app.shown.as_ref().map_or(String::new(), |a| format!("{} | {}", a.gate, a.lines.join(" ; "))));
            }
            Ok(())
        });
    }

    let engine = avenger_text::default_text_engine();
    let avenger = runtime.block_on(AvengerApp::try_new_with_text_engine(
        app,
        Arc::new(Builder),
        vec![(
            EventStreamConfig {
                types: vec![Type::KeyPress, Type::MouseDown, Type::RuntimeWake],
                ..Default::default()
            },
            Arc::new(Input) as Arc<dyn EventStreamHandler<App>>,
        )],
        engine.clone(),
    ))?;
    let options = avenger_winit_wgpu::WinitWgpuAvengerAppOptions::new(2.0)
        .window_attributes(
            winit::window::WindowAttributes::default()
                .with_title("Experiment 7 · autopilot")
                .with_resizable(false)
                .with_inner_size(winit::dpi::LogicalSize::new(W as f64, H as f64)),
        )
        .canvas_config(CanvasConfig { text_engine: Some(engine), ..Default::default() });
    let (mut host, event_loop) =
        avenger_winit_wgpu::WinitWgpuAvengerApp::try_new_and_event_loop_with_options(avenger, options, runtime)?;
    event_loop.run_app(&mut host)?;
    if let Some(error) = host.take_fatal_error() {
        return Err(error.into());
    }
    Ok(())
}
