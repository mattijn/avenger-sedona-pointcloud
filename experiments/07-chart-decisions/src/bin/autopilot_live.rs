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
//! - On Enter, Jev also says whether the instruction carries specifics of its
//!   own (a title, a threshold, a range, a cell size). If it does, or its
//!   options do not fit, Claude Haiku 4.5 writes the whole new pipeline with
//!   Jev's reading as direction (phase H, `writer.rs`). What it writes runs
//!   through the editor's path; stats for nerds shows the diff and the tries.
//!   `--record` leaves the writer out, so the video rebuilds from the cache.
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
use avenger_eventstream::window::{ClipboardEvent, Key, MouseButton, NamedKey};
use avenger_geometry::rtree::SceneGraphRTree;
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::text::SceneTextMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_text::measurement::TextMeasurementConfig;
use avenger_text::types::{FontStyle, FontWeight, FontWeightNameSpec, TextAlign, TextBaseline, TextSyntaxMode};
use avenger_wgpu::canvas::CanvasConfig;
use lidar_decide::deciders::{Decider, Decision, Jev, Writer};
use lidar_decide::layer::anim::{still, transition};
use lidar_decide::layer::model::{resolve, Data, Dataset, Frame, State};
use lidar_decide::layer::theme::Theme;
use lidar_decide::layer::{data, draw, editor, package, pilot, writer};
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
/// The buttons in the panel header.
const NERDS_BUTTON: [f32; 4] = [PX + 300.0, 24.0, 110.0, 26.0];
const COPY_BUTTON: [f32; 4] = [PX + 196.0, 24.0, 96.0, 26.0];
const DATA_BUTTON: [f32; 4] = [PX + 152.0, 24.0, 40.0, 26.0];
/// The session as text: what was typed, what became of it, the pipeline.
const SESSION_FILE: &str = "out/autopilot_live/session.txt";
/// The view as it was when the session was copied.
const VIEW_FILE: &str = "out/autopilot_live/session-view.png";
const AUTOPILOT_BUTTON: [f32; 4] = [PX, 24.0, 84.0, 26.0];
const EDITOR_BUTTON: [f32; 4] = [PX + 84.0, 24.0, 64.0, 26.0];
/// The editor's text area, and its monospace grid.
const EDIT_AREA: [f32; 4] = [PX, 96.0, 410.0, 392.0];
/// Pipeline text size and row height in the editor.
const CODE: f32 = 12.0;
/// The autopilot box, and its monospace grid.
const INPUT_BOX: [f32; 4] = [PX, 102.0, 410.0, 36.0];
const INPUT: f32 = 14.0;
const LH: f32 = 16.0;
/// Two clicks within this time, and this close, are a double click.
const MULTI_CLICK: f64 = 0.4;

/// The app's clock: real time, or the recorder's virtual time.
static VIRTUAL: std::sync::OnceLock<Mutex<Instant>> = std::sync::OnceLock::new();

fn clock() -> Instant {
    VIRTUAL.get().map_or_else(Instant::now, |m| *m.lock().unwrap())
}

fn recording() -> bool {
    VIRTUAL.get().is_some()
}

/// The theme, its typefaces, and text widths, set once at startup.
struct Ui {
    th: Theme,
    font: String,
    code: String,
    engine: avenger_text::TextEngine,
    widths: Mutex<std::collections::HashMap<(char, u32, bool), f32>>,
}

static UI: std::sync::OnceLock<Ui> = std::sync::OnceLock::new();

fn ui() -> &'static Ui {
    UI.get().expect("the theme is set at startup")
}

fn measure(engine: &avenger_text::TextEngine, s: &str, font: &str, size: f32) -> Option<f32> {
    let params = avenger_text::LabelParams::default();
    engine
        .measure_bounds(&TextMeasurementConfig {
            text: s,
            font,
            font_size: size,
            font_weight: FontWeight::Name(FontWeightNameSpec::Normal),
            font_style: FontStyle::Normal,
            syntax_mode: TextSyntaxMode::Plain,
            params: &params,
            number_locale: None,
            number_locale_specs: None,
            datetime_locale: None,
            datetime_timezone: None,
            datetime_locale_specs: None,
        })
        .ok()
        .map(|b| b.width)
}

impl Ui {
    fn new(th: Theme) -> Self {
        let engine = avenger_text::default_text_engine();
        // A family that is not installed cannot be measured.
        let font = th
            .fonts
            .iter()
            .find(|f| measure(&engine, "Hamburgefonstiv", f, 20.0).is_some())
            .unwrap_or(&"sans-serif")
            .to_string();
        let code = th.code_font.map_or(font.clone(), String::from);
        Ui { th, font, code, engine, widths: Mutex::new(Default::default()) }
    }

    /// The advance of one character, measured once.
    fn cw(&self, c: char, size: f32, code: bool) -> f32 {
        let key = (c, (size * 10.0) as u32, code);
        if let Some(w) = self.widths.lock().unwrap().get(&key) {
            return *w;
        }
        let f = if code { &self.code } else { &self.font };
        let w = match (measure(&self.engine, &format!("|{c}|"), f, size), measure(&self.engine, "||", f, size)) {
            (Some(a), Some(b)) => (a - b).max(0.0),
            _ => size * 0.55,
        };
        self.widths.lock().unwrap().insert(key, w);
        w
    }

    fn width(&self, s: &[char], size: f32, code: bool) -> f32 {
        s.iter().map(|c| self.cw(*c, size, code)).sum()
    }

    /// The character boundary nearest to `x` from the start of `s`.
    fn index_at(&self, s: &[char], x: f32, size: f32, code: bool) -> usize {
        let mut acc = 0.0;
        for (i, c) in s.iter().enumerate() {
            let w = self.cw(*c, size, code);
            if x < acc + w / 2.0 {
                return i;
            }
            acc += w;
        }
        s.len()
    }

    fn radius(&self, r: f32) -> f32 {
        if self.th.radius == 0.0 { 0.0 } else { r }
    }
}

fn ink() -> [f32; 4] {
    ui().th.text
}
fn muted() -> [f32; 4] {
    ui().th.muted
}
fn accent() -> [f32; 4] {
    ui().th.accent
}

fn inside(p: [f32; 2], r: [f32; 4]) -> bool {
    p[0] >= r[0] && p[0] <= r[0] + r[2] && p[1] >= r[1] && p[1] <= r[1] + r[3]
}

/// A decision that came back from a background task.
struct Returned {
    asked_at: Instant,
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
    /// When Haiku wrote the pipeline: its tries, and what it removed.
    written: Option<WrittenInfo>,
    /// `table` or `export`: the data is shown or written, not drawn.
    render: Option<String>,
    /// The chart when Jev was asked, for the session.
    before: String,
    removed: Vec<String>,
}

#[derive(Clone, Default)]
struct WrittenInfo {
    direction: String,
    /// (ms, cost, from cache, why it was refused).
    tries: Vec<(f64, f64, bool, Option<String>, String)>,
    /// The pipeline text of every try, for the session.
    texts: Vec<String>,
    removed: Vec<String>,
    wall_ms: f64,
    done: bool,
}

/// A pipeline Haiku wrote, back from its background task.
struct WrittenBack {
    shown: Shown,
    result: Result<writer::Outcome, String>,
    wall_ms: f64,
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
    /// The writer, and the questions Jev answers on Enter when it is on.
    writer: Option<Arc<Writer>>,
    enter_questions: Arc<Value>,
    written_back: Arc<Mutex<Vec<WrittenBack>>>,
    writing: usize,
    /// The pipeline behind every earlier chart, newest last, and the first
    /// one: undo and reset apply them again.
    history: Vec<String>,
    first: String,
    /// The instructions sent with Enter, and a line per outcome, for the
    /// copy button and `--snapshot @@session.txt`.
    sent: Vec<String>,
    session: Vec<String>,
    notice: String,
    exports: usize,
    t0: Instant,
    copied_at: Option<Instant>,
    logged: usize,
    /// Render the next frame to `VIEW_FILE` as well.
    save_view: bool,
    started_at: String,
    commit: String,
    taken: Vec<(String, String)>,
    // The chart.
    state: State,
    from: Frame,
    to: Frame,
    started: Option<Instant>,
    dur: f64,
    // Input.
    input: Field,
    /// Whether the text field (the autopilot box, or the editor) has
    /// keyboard focus: a blue border and a blinking caret.
    focused: bool,
    last_key: Instant,
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
    /// The last click in a text field, and how many came in a row.
    last_click: Option<(Instant, [f32; 2])>,
    /// A drag that selects text: in the editor (true) or the autopilot box.
    dragging: Option<bool>,
    clicks: u32,
    // Editor mode.
    editing: bool,
    code: Field,
    scroll: usize,
    edit_status: (String, [f32; 4]),
    applying: bool,
    apply_started: Instant,
    edited: Arc<Mutex<Option<Result<editor::Applied, String>>>>,
    /// A text without a chart command runs as a query; its rows are shown
    /// over the chart until the chart changes or the editor is left.
    queried: Arc<Mutex<Option<Result<editor::Table, String>>>>,
    table: Option<Arc<editor::Table>>,
}

/// What a key did to a text field.
#[derive(PartialEq)]
enum Edit {
    Changed,
    Moved,
    Unhandled,
}

/// A text field: its characters, the caret, and where a selection
/// started (the selection runs from there to the caret).
#[derive(Clone, Default)]
struct Field {
    text: Vec<char>,
    caret: usize,
    anchor: Option<usize>,
}

impl Field {
    fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.caret).then(|| (a.min(self.caret), a.max(self.caret)))
    }

    /// Move the caret; with Shift the selection grows, without it collapses.
    fn move_to(&mut self, to: usize, shift: bool) {
        if shift {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = to.min(self.text.len());
    }

    fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((a, b)) => {
                self.text.drain(a..b);
                self.caret = a;
                self.anchor = None;
                true
            }
            None => {
                self.anchor = None;
                false
            }
        }
    }

    fn insert(&mut self, s: &str) {
        self.delete_selection();
        for c in s.chars() {
            self.text.insert(self.caret, c);
            self.caret += 1;
        }
    }

    fn set(&mut self, s: &str) {
        self.text = s.chars().collect();
        self.caret = self.text.len();
        self.anchor = None;
    }

    fn string(&self) -> String {
        self.text.iter().collect()
    }

    /// Select the word (or run of spaces, or of punctuation) at the caret.
    fn select_word(&mut self) {
        let t = &self.text;
        let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c == ' ' { 1 } else { 2 };
        let at = if self.caret < t.len() && t[self.caret] != '\n' {
            self.caret
        } else if self.caret > 0 {
            self.caret - 1
        } else {
            return;
        };
        if t.get(at) == Some(&'\n') {
            return;
        }
        let k = class(t[at]);
        let (mut a, mut b) = (at, at + 1);
        while a > 0 && t[a - 1] != '\n' && class(t[a - 1]) == k {
            a -= 1;
        }
        while b < t.len() && t[b] != '\n' && class(t[b]) == k {
            b += 1;
        }
        self.anchor = Some(a);
        self.caret = b;
    }

    /// Select the line at the caret.
    fn select_line(&mut self) {
        let a = self.text[..self.caret].iter().rposition(|c| *c == '\n').map_or(0, |i| i + 1);
        let b = self.text[self.caret..].iter().position(|c| *c == '\n').map_or(self.text.len(), |i| self.caret + i);
        self.anchor = Some(a);
        self.caret = b;
    }

    /// The start of the word before the caret, or the end of the one after.
    fn word(&self, forward: bool) -> usize {
        let t = &self.text;
        let mut i = self.caret;
        if forward {
            while i < t.len() && !t[i].is_alphanumeric() {
                i += 1;
            }
            while i < t.len() && t[i].is_alphanumeric() {
                i += 1;
            }
        } else {
            while i > 0 && !t[i - 1].is_alphanumeric() {
                i -= 1;
            }
            while i > 0 && t[i - 1].is_alphanumeric() {
                i -= 1;
            }
        }
        i
    }
}

/// The modifier keys held with a key.
#[derive(Clone, Copy)]
struct Mods {
    cmd: bool,
    shift: bool,
    alt: bool,
}

/// Editing shared by the autopilot box and the editor: typing, Backspace
/// and Delete (a selection first), ⌘Backspace (to the start of the line),
/// ⌥Backspace (a word), ⌘A, ← → (⌥ by word, ⌘ to the line's ends), with
/// Shift to select. What is typed replaces a selection. macOS can send
/// Backspace as a control character; that counts as Backspace too.
fn edit(f: &mut Field, key: &Key, t: Option<&str>, m: Mods) -> Edit {
    let backspace = matches!(key, Key::Named(NamedKey::Backspace) | Key::Character('\u{8}') | Key::Character('\u{7f}'))
        || matches!(t, Some("\u{8}") | Some("\u{7f}"));
    let delete = matches!(key, Key::Named(NamedKey::Delete));
    let line_start = f.text[..f.caret].iter().rposition(|c| *c == '\n').map_or(0, |i| i + 1);
    let line_end = f.text[f.caret..].iter().position(|c| *c == '\n').map_or(f.text.len(), |i| f.caret + i);
    if m.cmd && matches!(key, Key::Character('a') | Key::Character('A')) {
        f.anchor = Some(0);
        f.caret = f.text.len();
        return Edit::Moved;
    }
    if backspace || delete {
        if f.delete_selection() {
            return Edit::Changed;
        }
        let (a, b) = match (backspace, m.cmd, m.alt) {
            (true, true, _) => (line_start, f.caret),
            (true, _, true) => (f.word(false), f.caret),
            (true, _, _) => (f.caret.saturating_sub(1), f.caret),
            (false, _, true) => (f.caret, f.word(true)),
            (false, _, _) => (f.caret, (f.caret + 1).min(f.text.len())),
        };
        if a == b {
            return Edit::Moved;
        }
        f.text.drain(a..b);
        f.caret = a;
        return Edit::Changed;
    }
    match key {
        Key::Named(NamedKey::ArrowLeft) => {
            let to = match (f.selection(), m.shift, m.cmd, m.alt) {
                (Some((a, _)), false, false, false) => a,
                (_, _, true, _) => line_start,
                (_, _, _, true) => f.word(false),
                _ => f.caret.saturating_sub(1),
            };
            f.move_to(to, m.shift);
            Edit::Moved
        }
        Key::Named(NamedKey::ArrowRight) => {
            let to = match (f.selection(), m.shift, m.cmd, m.alt) {
                (Some((_, b)), false, false, false) => b,
                (_, _, true, _) => line_end,
                (_, _, _, true) => f.word(true),
                _ => f.caret + 1,
            };
            f.move_to(to, m.shift);
            Edit::Moved
        }
        Key::Named(NamedKey::Home) => {
            f.move_to(line_start, m.shift);
            Edit::Moved
        }
        Key::Named(NamedKey::End) => {
            f.move_to(line_end, m.shift);
            Edit::Moved
        }
        Key::Named(NamedKey::Space) => {
            f.insert(" ");
            Edit::Changed
        }
        _ => match t {
            Some(t) if !m.cmd && !t.is_empty() && !t.chars().any(char::is_control) => {
                f.insert(t);
                Edit::Changed
            }
            _ => Edit::Unhandled,
        },
    }
}

/// Visual rows of the editor text: (start, length) in chars, wrapped at
/// spaces to the width of the text area.
fn rows(text: &[char]) -> Vec<(usize, usize)> {
    let max = EDIT_AREA[2] - 16.0;
    let mut out = vec![];
    let mut start = 0;
    for (end, c) in text.iter().chain(std::iter::once(&'\n')).enumerate() {
        if *c != '\n' {
            continue;
        }
        let mut a = start;
        loop {
            let (mut w, mut i, mut space) = (0.0, a, None);
            while i < end {
                let cw = ui().cw(text[i], CODE, true);
                if w + cw > max && i > a {
                    break;
                }
                w += cw;
                if text[i] == ' ' {
                    space = Some(i);
                }
                i += 1;
            }
            if i >= end {
                out.push((a, end - a));
                break;
            }
            let b = space.map_or(i, |sp| sp + 1);
            out.push((a, b - a));
            a = b;
        }
        start = end + 1;
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

/// The first character shown in the autopilot box, so the caret is in view.
fn input_first(f: &Field) -> usize {
    let max = INPUT_BOX[2] - 24.0;
    let mut first = f.caret;
    let mut w = 0.0;
    while first > 0 {
        let cw = ui().cw(f.text[first - 1], INPUT, true);
        if w + cw > max {
            break;
        }
        w += cw;
        first -= 1;
    }
    first
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
        let prefix = self.input.string().trim().to_string();
        if prefix.is_empty() || (prefix == self.asked && !complete) {
            return;
        }
        self.asked = prefix.clone();
        self.in_flight += 1;
        let obs = pilot::observation(&self.state, &self.data, &prefix);
        // With the writer on, typing and Enter ask the same questions, so
        // what Enter will do shows while typing.
        let q = if self.writer.is_some() { self.enter_questions.clone() } else { self.questions.clone() };
        let (jev, out) = (self.jev.clone(), self.returned.clone());
        let asked_at = clock();
        self.rt.spawn(async move {
            let t = std::time::Instant::now();
            let result = jev.decide(&obs, &q).await;
            let wall_ms = t.elapsed().as_secs_f64() * 1e3;
            out.lock().unwrap().push(Returned { asked_at, prefix, complete, wall_ms, result });
        });
    }

    /// Gate the returned decisions and run the ones that pass through the
    /// pipeline.
    async fn collect(&mut self, now: Instant) {
        // A recording lets each decision arrive after the latency measured
        // when it was first made.
        let all: Vec<Returned> = std::mem::take(&mut *self.returned.lock().unwrap());
        let (returned, later): (Vec<Returned>, Vec<Returned>) = all.into_iter().partition(|r| {
            let ms = r.result.as_ref().map_or(300.0, |d| d.ms).clamp(150.0, 1500.0);
            !recording() || r.asked_at + Duration::from_secs_f64(ms / 1e3) <= now
        });
        self.returned.lock().unwrap().extend(later);
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
            let action = d.answers.get("action").and_then(Value::as_str).unwrap_or("").to_string();
            // Table and export go by how sure Jev is of `render` itself, not
            // of the action: "view as table head 2" was mark/keep at 0.26–0.56,
            // with table at 0.77.
            let render = d.answers.get("render").and_then(Value::as_str).unwrap_or("chart").to_string();
            let render_conf = d.confidence_of("render").unwrap_or(conf);
            let specifics = d.answers.get("specifics").and_then(Value::as_str) == Some("text");
            // While typing, what only Enter can do is held, and said.
            let held = if r.complete || self.writer.is_none() {
                None
            } else if render != "chart" && render_conf >= GATE {
                Some(match render.as_str() {
                    "table" => "Enter: the data as a table".to_string(),
                    "overview" => "Enter: an overview of the data there is".to_string(),
                    _ => "Enter: the data as a Parquet file".to_string(),
                })
            } else if conf < GATE {
                None
            } else {
                match action.as_str() {
                    "transform" => Some("Enter: Haiku writes the change to the data".to_string()),
                    "undo" => Some("Enter: undo the last change".into()),
                    "reset" => Some("Enter: back to the first chart".into()),
                    "title" => Some("Enter: Haiku writes the title".into()),
                    _ if specifics => Some("Enter: Haiku writes it, with the details".into()),
                    _ => None,
                }
            };
            let (next, gate) = match held {
                Some(h) => (None, h),
                None => (next, gate),
            };
            // On Enter, the routing measured in phase H: Jev's options when
            // they say it all, otherwise the writer.
            let fast = conf >= GATE
                && d.answers.get("specifics").and_then(Value::as_str) != Some("text")
                && pilot::apply(&self.state, &d.answers).is_ok_and(|n| n != self.state || pilot::short(&d.answers) == "no_change");
            let to_writer = r.complete && self.writer.is_some() && !fast;
            let back = d.answers.get("action").and_then(Value::as_str).filter(|a| matches!(*a, "undo" | "reset")).map(String::from);
            let mut shown = Shown {
                prefix: r.prefix.clone(),
                complete: r.complete,
                wall_ms: r.wall_ms,
                decision: d,
                colour: if next.is_some() { ui().th.ok } else if gate.starts_with("waiting") { accent() } else { muted() },
                gate,
                lines: vec![],
                fold: String::new(),
                written: None,
                render: None,
                before: state_line(&self.state),
                removed: vec![],
            };
            if shown.gate.starts_with("Enter:") {
                shown.colour = accent();
            }
            if let Some(a) = back.filter(|_| r.complete && conf >= GATE) {
                self.go_back(a == "reset", shown, now).await;
                continue;
            }
            if r.complete && self.writer.is_some() && render_conf >= GATE && render != "chart" {
                shown.render = Some(render.clone());
                if render == "overview" {
                    shown.gate = "an overview of the data there is, the chart is unchanged".into();
                    shown.colour = ui().th.ok;
                    self.log(&shown);
                    self.shown = Some(shown);
                    self.show_overview();
                    continue;
                }
                // Unless Jev is sure nothing else changes, Haiku writes the
                // data stages; they are shown or written, not drawn. Jev's
                // action was wrong for "show the ground points as a table"
                // (mark/bars), its render answer was not.
                if action != "no_change" || specifics {
                    self.write(shown);
                } else {
                    let stages = self.data_stages.clone();
                    self.render_data(&stages, shown);
                }
                continue;
            }
            if to_writer {
                self.write(shown);
                continue;
            }
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
                        shown.colour = ui().th.error;
                        shown.fold = e;
                    }
                    (None, Ok(folded)) => {
                        shown.fold = if folded == n { "folds to the decided state".into() } else { format!("folds to {folded:?}") };
                        self.changes.push((r.prefix.clone(), pilot::short(&shown.decision.answers)));
                        self.remember();
                        self.transition_to(folded, now);
                    }
                }
                self.snapshot(&p);
            }
            self.log(&shown);
            self.shown = Some(shown);
        }
    }

    /// One line of the session per decision, while typing (…) or on Enter
    /// (⏎): the text, Jev's answer, and what the window did with it; then
    /// the pipeline lines it added.
    fn log(&mut self, a: &Shown) {
        let d = &a.decision;
        let t = (clock() - self.t0).as_secs_f64();
        let how = if a.complete { "on Enter" } else { "while typing" };
        self.logged += 1;
        let mut v = vec![format!("{t:.1} s, {how}: \"{}\"", a.prefix)];
        v.push(format!("  chart    {}", a.before));
        // Every answer with its confidence, the chosen action first.
        let mut answers: Vec<String> = d
            .answers
            .iter()
            .map(|(k, x)| format!("{k} {}{}", x.as_str().unwrap_or("?"), d.confidence_of(k).map_or(String::new(), |c| format!(" {c:.2}"))))
            .collect();
        answers.sort_by_key(|x| !x.starts_with("action "));
        v.push(format!("  jev      {}", answers.join(" · ")));
        let probs: Vec<String> = d.probs.iter().take(5).map(|(k, p)| format!("{k} {p:.2}")).collect();
        if !probs.is_empty() {
            v.push(format!("  action p {}", probs.join(" · ")));
        }
        let asked = if d.cached { format!("from cache ({:.0} ms when first asked)", d.ms) } else { format!("live, {:.0} ms", a.wall_ms) };
        v.push(format!("  asked    {asked} · {}", d.cache));
        if let Some(w) = &a.written {
            v.push(format!("  writer   direction: {}", w.direction));
            for (i, ((ms, cost, cached, refused, cache), text)) in w.tries.iter().zip(&w.texts).enumerate() {
                let what = refused.as_ref().map_or("accepted".to_string(), |r| format!("refused: {}", fit(r, 140)));
                v.push(format!("  try {}    {what} · {ms:.0} ms · ${cost:.4} · {} · {cache}", i + 1, if *cached { "from cache" } else { "live" }));
                if refused.is_some() {
                    v.extend(text.lines().map(|l| format!("           | {l}")));
                }
            }
        }
        v.push(format!("  window   {}", a.gate));
        v.extend(a.lines.iter().map(|l| format!("           + {l}")));
        v.extend(a.removed.iter().map(|l| format!("           - {l}")));
        // A table or an overview says what it showed when it arrives.
        if a.render.is_none() && a.complete {
            v.push(format!("  view     {}", self.view_line_after(a)));
        }
        self.session.extend(v);
    }

    /// The session as text, for the clipboard and `@@session.txt`: the
    /// instructions to replay, then the log and the pipeline as comments.
    fn session_text(&self) -> String {
        let mut v = vec![
            format!("# autopilot session · {} · commit {}", self.started_at, self.commit),
            format!(
                "# jev typesafe/jev-1.13 · writer {} · gates: confidence {GATE}, chart kind while typing {GATE_MARK_EARLY}",
                if self.writer.is_some() { "anthropic/claude-haiku-4.5" } else { "off" }
            ),
            format!("# {} decisions, {} sent with Enter · {} changes · ${:.4} spent", self.logged, self.sent.len(), self.changes.len(), self.cost),
            "#".into(),
        ];
        if self.sent.is_empty() {
            v.push("# nothing sent with Enter yet, so nothing to replay".into());
        } else {
            v.push(format!("# sent with Enter; the lines without # replay with: cargo run --release -p lidar-decide --bin autopilot_live -- --snapshot out/replay @@{SESSION_FILE}"));
            v.extend(self.sent.iter().cloned());
        }
        if !self.session.is_empty() {
            v.push("#".into());
            v.push("# every decision, oldest first: the chart when Jev was asked, all its answers with confidence, the cache file".into());
            v.push("# that holds the raw response, what the writer wrote, and what the window did (+ added, - removed)".into());
            v.push("#".into());
            for l in &self.session {
                if !l.starts_with(' ') {
                    v.push("#".into());
                }
                v.push(format!("# {l}"));
            }
            v.push("#".into());
        }
        v.push(format!("# the view when this was copied: {} · {VIEW_FILE}", self.view_line()));
        v.push("# the pipeline now, runnable as it stands:".into());
        v.extend(self.pipeline.iter().enumerate().map(|(i, l)| format!("#   {}{l}", if i == 0 { "" } else { "! " })));
        v.join("\n") + "\n"
    }

    /// Show the data as a table, or write it to Parquet: the stages as
    /// given, then `head 20` or `write`, run as an editor query.
    fn render_data(&mut self, stages: &[String], mut shown: Shown) {
        let kind = shown.render.clone().unwrap_or_else(|| "table".into());
        let sink = if kind == "export" {
            self.exports += 1;
            let _ = std::fs::create_dir_all("out/autopilot_live");
            format!("write out/autopilot_live/export-{}.parquet", self.exports)
        } else {
            format!("head {}", rows_asked(&shown.prefix).unwrap_or(20))
        };
        let text = format!("{} ! {sink}", stages.join(" ! "));
        shown.lines = vec![sink.clone()];
        shown.gate = if kind == "export" { format!("{sink}, the chart is unchanged") } else { "the data as a table, the chart is unchanged".into() };
        shown.colour = ui().th.ok;
        self.log(&shown);
        self.shown = Some(shown);
        let out = self.queried.clone();
        self.applying = true;
        self.rt.spawn(async move {
            *out.lock().unwrap() = Some(editor::query(&text).await);
        });
    }

    /// The overview of the data there is, over the chart.
    fn show_overview(&mut self) {
        let (out, in_chart) = (self.queried.clone(), self.state.dataset.id());
        self.applying = true;
        self.rt.spawn(async move {
            let r = match lidar_decide::layer_pipeline().await {
                Ok((p, _)) => lidar_decide::layer::catalog::overview(&p.ctx, Some(in_chart)).await,
                Err(e) => Err(e.to_string()),
            };
            *out.lock().unwrap() = Some(r);
        });
    }

    /// The view after a decision: the chart it leads to (the table, if one
    /// is open, goes when the chart changes).
    fn view_line_after(&self, a: &Shown) -> String {
        if a.gate.starts_with("Enter:") || a.gate.starts_with("waiting") {
            "unchanged".into()
        } else {
            self.view_line()
        }
    }

    /// What the view shows now, in one line: the chart drawn, or the table.
    fn view_line(&self) -> String {
        if let Some(t) = &self.table {
            let first: Vec<String> = t.rows.iter().take(2).map(|r| r.iter().take(6).cloned().collect::<Vec<_>>().join(" ")).collect();
            return format!("table · {} · columns {} · {}", t.note, fit(&t.columns.join(", "), 80), fit(&first.join(" | "), 100));
        }
        let (s, d) = (&self.state, &self.data);
        let big = |v: f64| if v >= 1e6 { format!("{:.1}M", v / 1e6) } else if v >= 1e3 { format!("{:.0}k", v / 1e3) } else { format!("{v:.0}") };
        let what = match s.mark {
            lidar_decide::layer::model::Mark::Bars | lidar_decide::layer::model::Mark::Pie => {
                format!("{} {}: {}", d.classes.len(), if s.mark == lidar_decide::layer::model::Mark::Pie { "slices" } else { "bars" }, d.classes.iter().map(|c| format!("{} {}", c.0, big(c.2))).collect::<Vec<_>>().join(", "))
            }
            lidar_decide::layer::model::Mark::Line => {
                let lines: std::collections::BTreeSet<i64> = d.flight.iter().map(|r| r.0).collect();
                format!("{} lines, {} points, t 0–{:.0} s", lines.len(), d.flight.len(), d.flight.iter().map(|r| r.1).fold(0.0, f64::max))
            }
            lidar_decide::layer::model::Mark::Heatmap => format!("{} cells, classes × 4 m height bands", d.class_height.len()),
            lidar_decide::layer::model::Mark::Map => {
                let (lo, hi) = d.cells.iter().fold((f64::MAX, f64::MIN), |a, c| (a.0.min(c.2), a.1.max(c.2)));
                format!("{} cells, h {lo:.1}–{hi:.1} m", d.cells.len())
            }
        };
        format!("chart · {} · {what} · \"{}\" · {} items drawn", s.mark.id(), s.title, self.to.items.len())
    }

    /// Have Haiku write the pipeline, in the background.
    fn write(&mut self, mut shown: Shown) {
        let w = self.writer.clone().expect("the writer is on");
        let direction = writer::direction(&shown.decision);
        shown.gate = "Haiku is writing the pipeline…".into();
        shown.colour = accent();
        shown.written = Some(WrittenInfo { direction: direction.clone(), ..Default::default() });
        self.shown = Some(shown.clone());
        self.in_flight += 1;
        self.writing += 1;
        let (state, data, current, out) = (self.state.clone(), self.data.clone(), self.pipeline.join("\n! "), self.written_back.clone());
        let prefix = shown.prefix.clone();
        self.rt.spawn(async move {
            let t = std::time::Instant::now();
            let result = writer::write(&w, &state, &data, &current, &prefix, Some(&direction), 3).await;
            let wall_ms = t.elapsed().as_secs_f64() * 1e3;
            out.lock().unwrap().push(WrittenBack { shown, result, wall_ms });
        });
    }

    /// Apply what the writer wrote, as the editor applies a hand edit.
    async fn collect_written(&mut self, now: Instant) {
        let all: Vec<WrittenBack> = std::mem::take(&mut *self.written_back.lock().unwrap());
        for b in all {
            self.in_flight -= 1;
            self.writing -= 1;
            let mut shown = b.shown;
            let mut info = shown.written.take().unwrap_or_default();
            info.wall_ms = b.wall_ms;
            info.done = true;
            let o = match b.result {
                Ok(o) => o,
                Err(e) => {
                    shown.gate = format!("writer: {e}");
                    shown.colour = ui().th.error;
                    shown.written = Some(info);
                    self.log(&shown);
                    self.shown = Some(shown);
                    continue;
                }
            };
            info.tries = o.attempts.iter().map(|a| (a.written.ms, a.written.cost, a.written.cached, a.refused.clone(), a.written.cache.clone())).collect();
            info.texts = o.attempts.iter().map(|a| a.text.clone()).collect();
            self.cost += o.attempts.iter().filter(|a| !a.written.cached).map(|a| a.written.cost).sum::<f64>();
            let n = info.tries.len();
            let tries = if n == 1 { "1 try".to_string() } else { format!("{n} tries") };
            if o.overview {
                shown.gate = "Haiku: a question about the data, so the overview".into();
                shown.colour = ui().th.ok;
                shown.render = Some("overview".into());
                shown.written = Some(info);
                self.log(&shown);
                self.shown = Some(shown);
                self.show_overview();
                continue;
            }
            if let (Some(_), Some(a)) = (&shown.render, &o.applied) {
                let stages = a.data_stages.clone();
                shown.written = Some(info);
                self.render_data(&stages, shown);
                continue;
            }
            match o.applied {
                Some(a) if a.state == self.state && a.data_stages == self.data_stages => {
                    shown.gate = format!("written by Haiku ({tries}): no change");
                    shown.colour = muted();
                    shown.fold = "folds to the chart as it is".into();
                }
                Some(a) => {
                    let before = self.pipeline.clone();
                    self.remember();
                    self.data = Arc::new(a.data);
                    self.data_stages = a.data_stages;
                    let pipe = self.pipe.clone();
                    let mut p = pipe.lock().await;
                    *p = a.pipeline;
                    self.transition_to(a.state, now);
                    self.snapshot(&p);
                    shown.lines = self.pipeline.iter().filter(|l| !before.contains(l)).cloned().collect();
                    info.removed = before.into_iter().filter(|l| !self.pipeline.contains(l)).collect();
                    shown.removed = info.removed.clone();
                    shown.gate = format!("written by Haiku ({tries}), applied");
                    shown.colour = ui().th.ok;
                    shown.fold = "folds to a state the layer draws".into();
                    self.changes.push((shown.prefix.clone(), "written".into()));
                }
                None => {
                    let why = o.attempts.last().and_then(|a| a.refused.clone()).unwrap_or_default();
                    shown.gate = format!("refused {tries}: {why}");
                    shown.colour = ui().th.error;
                }
            }
            shown.written = Some(info);
            self.log(&shown);
            self.shown = Some(shown);
        }
    }

    /// Keep the pipeline behind the chart now, before it changes.
    fn remember(&mut self) {
        self.history.push(self.pipeline.join("\n! "));
    }

    /// Undo: apply the pipeline before the last change. Reset: apply the
    /// first one, keeping the chart now in the history so an undo brings it
    /// back. Both run through the editor's path, so the chart stays the fold
    /// of its pipeline.
    async fn go_back(&mut self, reset: bool, mut shown: Shown, now: Instant) {
        let text = if reset {
            Some(self.first.clone()).filter(|f| *f != self.pipeline.join("\n! "))
        } else {
            self.history.pop()
        };
        let Some(text) = text else {
            shown.gate = if reset { "already the first chart".into() } else { "nothing to undo".into() };
            shown.colour = muted();
            self.log(&shown);
            self.shown = Some(shown);
            return;
        };
        match editor::apply(&text, &self.base).await {
            Ok(a) => {
                let before = self.pipeline.clone();
                if reset {
                    self.remember();
                }
                self.data = Arc::new(a.data);
                self.data_stages = a.data_stages;
                let pipe = self.pipe.clone();
                let mut p = pipe.lock().await;
                *p = a.pipeline;
                self.transition_to(a.state, now);
                self.snapshot(&p);
                shown.lines = self.pipeline.iter().filter(|l| !before.contains(l)).cloned().collect();
                shown.removed = before.iter().filter(|l| !self.pipeline.contains(l)).cloned().collect();
                shown.gate = if reset { "reset to the first chart".into() } else { format!("undone ({} earlier left)", self.history.len()) };
                shown.colour = ui().th.ok;
                shown.fold = "folds to the earlier state".into();
                self.changes.push((shown.prefix.clone(), if reset { "reset" } else { "undo" }.into()));
            }
            Err(e) => {
                shown.gate = format!("could not go back: {e}");
                shown.colour = ui().th.error;
            }
        }
        self.log(&shown);
        self.shown = Some(shown);
    }

    fn open_editor(&mut self) {
        self.editing = true;
        self.table = None;
        self.code.set(&self.pipeline.join("\n! "));
        self.focused = true;
        self.edit_status = (String::new(), muted());
    }

    fn apply_text(&mut self) {
        if self.applying {
            return;
        }
        self.applying = true;
        self.apply_started = clock();
        self.edit_status = ("applying…".into(), accent());
        let text = self.code.string();
        let (base, out, queried) = (self.base.clone(), self.edited.clone(), self.queried.clone());
        self.rt.spawn(async move {
            match editor::apply(&text, &base).await {
                Err(e) if e == editor::NO_CHART => *queried.lock().unwrap() = Some(editor::query(&text).await),
                r => *out.lock().unwrap() = Some(r),
            }
        });
    }

    async fn collect_edit(&mut self, now: Instant) {
        // A recording shows the apply taking as long as it did.
        if recording() {
            let due = match &*self.edited.lock().unwrap() {
                Some(Ok(a)) => self.apply_started + Duration::from_secs_f64(a.ms / 1e3),
                Some(Err(_)) => self.apply_started,
                None => return,
            };
            if now < due {
                return;
            }
        }
        if let Some(q) = self.queried.lock().unwrap().take() {
            self.applying = false;
            match q {
                Ok(t) => {
                    let what = if t.columns.is_empty() { format!("{} lines", t.text.len()) } else { t.note.clone() };
                    self.edit_status = (format!("query in {:.0} ms · {what} · the chart is unchanged", t.ms), ui().th.ok);
                    if self.editing {
                        self.session.push(format!("{:>6.1} s editor query · {what}", (clock() - self.t0).as_secs_f64()));
                    }
                    self.table = Some(Arc::new(t));
                    let v = self.view_line();
                    self.session.push(format!("  view     {v}"));
                }
                Err(e) => self.edit_status = (e, ui().th.error),
            }
            return;
        }
        let Some(r) = self.edited.lock().unwrap().take() else { return };
        self.applying = false;
        match r {
            Ok(a) => {
                self.edit_status = (format!("applied in {:.0} ms · {}", a.ms, a.note), ui().th.ok);
                self.session.push(format!("{:>6.1} s editor applied · {}", (clock() - self.t0).as_secs_f64(), a.note));
                self.data = Arc::new(a.data);
                self.data_stages = a.data_stages;
                self.changes.push(("editor".into(), "pipeline".into()));
                // The chart changed by hand, not by a decision.
                self.shown = None;
                self.remember();
                let pipe = self.pipe.clone();
                let mut p = pipe.lock().await;
                *p = a.pipeline;
                self.transition_to(a.state, now);
                self.snapshot(&p);
            }
            Err(e) => self.edit_status = (e, ui().th.error),
        }
    }

    fn edit_key(&mut self, key: &Key, text: Option<&str>, m: Mods) -> bool {
        let r = rows(&self.code.text);
        let (row, col) = caret_at(&r, self.code.caret);
        match key {
            Key::Named(NamedKey::Enter) if m.cmd => self.apply_text(),
            Key::Named(NamedKey::Enter) => self.code.insert("\n"),
            Key::Named(NamedKey::Escape) => self.open_editor(),
            Key::Named(NamedKey::ArrowUp) => {
                let to = if row > 0 { r[row - 1].0 + col.min(r[row - 1].1) } else { 0 };
                self.code.move_to(to, m.shift);
            }
            Key::Named(NamedKey::ArrowDown) => {
                let to = if row + 1 < r.len() { r[row + 1].0 + col.min(r[row + 1].1) } else { self.code.text.len() };
                self.code.move_to(to, m.shift);
            }
            _ => return edit(&mut self.code, key, text, m) != Edit::Unhandled,
        }
        true
    }

    /// A key in the autopilot box. Enter asks and clears the box.
    fn input_key(&mut self, key: &Key, text: Option<&str>, m: Mods, now: Instant) -> bool {
        match key {
            Key::Named(NamedKey::Enter) => {
                self.typed_at = None;
                let sent = self.input.string().trim().to_string();
                if !sent.is_empty() {
                    self.sent.push(sent);
                }
                self.ask(true);
                self.input.set("");
            }
            Key::Named(NamedKey::Escape) => {
                self.input.set("");
                self.typed_at = None;
                self.table = None;
            }
            Key::Named(NamedKey::ArrowUp) => self.input.move_to(0, m.shift),
            Key::Named(NamedKey::ArrowDown) => {
                let end = self.input.text.len();
                self.input.move_to(end, m.shift)
            }
            _ => match edit(&mut self.input, key, text, m) {
                Edit::Changed => {
                    self.message.clear();
                    self.typed_at = Some(now);
                }
                Edit::Moved => {}
                Edit::Unhandled => return false,
            },
        }
        true
    }

    fn click_editor(&mut self, p: [f32; 2], shift: bool) {
        let r = rows(&self.code.text);
        let row = (self.scroll + ((p[1] - EDIT_AREA[1] - 8.0) / LH).max(0.0) as usize).min(r.len() - 1);
        let (a, len) = r[row];
        let col = ui().index_at(&self.code.text[a..a + len], p[0] - EDIT_AREA[0] - 8.0, CODE, true);
        // A click after a wrapped row's trailing space lands before it.
        let col = if col == len && a + len < self.code.text.len() && self.code.text[a + len - 1.min(len)] == ' ' && r.get(row + 1).is_some_and(|n| n.0 == a + len) { len.saturating_sub(1) } else { col };
        self.code.move_to(a + col, shift);
    }

    fn click_input(&mut self, p: [f32; 2], shift: bool) {
        let first = input_first(&self.input);
        let col = ui().index_at(&self.input.text[first..], p[0] - INPUT_BOX[0] - 12.0, INPUT, true);
        self.input.move_to(first + col, shift);
    }

    /// Count clicks in a row; the second selects a word, the third a line.
    fn multi_click(&mut self, p: [f32; 2]) -> u32 {
        let now = clock();
        self.clicks = match self.last_click {
            Some((t, q)) if now.duration_since(t).as_secs_f64() < MULTI_CLICK && (p[0] - q[0]).abs() < 5.0 && (p[1] - q[1]).abs() < 5.0 => self.clicks + 1,
            _ => 1,
        };
        self.last_click = Some((now, p));
        self.clicks
    }

    fn transition_to(&mut self, n: State, now: Instant) {
        self.table = None;
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

/// The caret shows solid while typing, and blinks once typing pauses.
fn caret_on(s: &App) -> bool {
    let since = (clock() - s.last_key).as_secs_f64();
    since < 0.6 || ((since - 0.6) * 1.8).fract() < 0.5
}

/// Pipeline text: in the code typeface (the text typeface when themed).
fn mono(s: &str, x: f32, y: f32, color: [f32; 4]) -> SceneMark {
    mono_sized(s, x, y, 11.0, color)
}

fn mono_sized(s: &str, x: f32, y: f32, size: f32, color: [f32; 4]) -> SceneMark {
    SceneTextMark {
        len: 1,
        text: s.to_string().into(),
        x: x.into(),
        y: y.into(),
        font: ui().code.clone().into(),
        font_size: size.into(),
        color: draw::c(color).into(),
        baseline: TextBaseline::Top.into(),
        ..Default::default()
    }
    .into()
}

/// Status text: grey italic in the calm theme, coloured in the neutral one.
fn status(s: &str, x: f32, y: f32, size: f32, colour: [f32; 4], bold: bool, code: bool) -> SceneMark {
    status_on(s, x, y, size, ui().th.muted, colour, bold, code)
}

/// Status text on a given grey (the dark overlay has its own).
#[allow(clippy::too_many_arguments)]
fn status_on(s: &str, x: f32, y: f32, size: f32, grey: [f32; 4], colour: [f32; 4], bold: bool, code: bool) -> SceneMark {
    let th = &ui().th;
    SceneTextMark {
        len: 1,
        text: s.to_string().into(),
        x: x.into(),
        y: y.into(),
        font: if code { ui().code.clone() } else { ui().font.clone() }.into(),
        font_size: size.into(),
        font_style: if th.status_italic { FontStyle::Italic } else { FontStyle::Normal }.into(),
        font_weight: FontWeight::Name(if bold { FontWeightNameSpec::Bold } else { FontWeightNameSpec::Normal }).into(),
        color: draw::c(if th.status_italic { grey } else { colour }).into(),
        baseline: TextBaseline::Top.into(),
        ..Default::default()
    }
    .into()
}

/// A focused field has the accent border (and a glow, in the neutral look).
fn field_frame(r: [f32; 4], focused: bool, marks: &mut Vec<SceneMark>) {
    let [x, y, w, h] = r;
    let th = &ui().th;
    if focused && th.focus_glow {
        marks.push(draw::rect(x - 2.0, y - 2.0, w + 4.0, h + 4.0, [th.accent[0], th.accent[1], th.accent[2], 0.25], None, 8.0));
    }
    marks.push(draw::rect(x, y, w, h, th.background, Some(if focused { th.accent } else { th.line }), ui().radius(6.0)));
    if focused && !th.focus_glow {
        // A 1.5 px border: a second outline half a pixel in.
        marks.push(draw::rect(x + 0.5, y + 0.5, w - 1.0, h - 1.0, [0.0; 4], Some(th.accent), 0.0));
    }
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

fn panel(s: &App, marks: &mut Vec<SceneMark>) {
    let th = &ui().th;
    match th.panel {
        Some(fill) => marks.push(draw::rect(PX - 20.0, 0.0, W - PX + 20.0, H, fill, None, 0.0)),
        None => {
            marks.push(draw::rect(PX - 20.0, 0.0, W - PX + 20.0, H, th.background, None, 0.0));
            marks.push(draw::rule(PX - 20.0, 0.0, PX - 20.0, H, th.line));
        }
    }
    // The copy button flashes when clicked, and says so for a moment.
    let since = s.copied_at.map(|t| (clock() - t).as_secs_f64());
    let copy_on = since.is_some_and(|t| t < 0.25);
    let copy_label = if since.is_some_and(|t| t < 1.5) { "copied ✓" } else { "copy session" };
    for (label, r, on) in [
        ("autopilot", AUTOPILOT_BUTTON, !s.editing),
        ("editor", EDITOR_BUTTON, s.editing),
        ("stats for nerds", NERDS_BUTTON, s.nerds),
        (copy_label, COPY_BUTTON, copy_on),
        ("data", DATA_BUTTON, s.table.as_ref().is_some_and(|t| t.note.contains(" tables, "))),
    ] {
        let [bx, by, bw, bh] = r;
        if th.filled_buttons {
            marks.push(draw::rect(bx, by, bw, bh, if on { th.text } else { [1.0; 4] }, Some(th.line), ui().radius(5.0)));
            marks.push(t(label, bx + 10.0, by + 6.0, 12.0, if on { [1.0; 4] } else { th.text }, on && r != NERDS_BUTTON));
        } else {
            marks.push(draw::rect(bx, by, bw, bh, th.background, Some(if on { th.accent } else { th.line }), 0.0));
            if on {
                marks.push(draw::rect(bx + 0.5, by + 0.5, bw - 1.0, bh - 1.0, [0.0; 4], Some(th.accent), 0.0));
            }
            marks.push(t(label, bx + 10.0, by + 6.0, 12.0, if on { th.accent } else { th.text }, on));
        }
    }
    if s.editing {
        editor_panel(s, marks);
        return;
    }
    let about = if s.writer.is_some() {
        "Jev decides as you type · Enter also: undo, reset, or Haiku writes it"
    } else {
        "Jev 1.13 via OpenRouter · asks 400 ms after typing, and on Enter"
    };
    marks.push(t(about, PX, 58.0, 12.0, muted(), false));
    marks.push(t("Tab: stats for nerds · ⌘E: editor · Esc: clear", PX, 74.0, 12.0, muted(), false));

    let [bx, by, _, _] = INPUT_BOX;
    field_frame(INPUT_BOX, s.focused, marks);
    let f = &s.input;
    let first = input_first(f);
    let max = INPUT_BOX[2] - 24.0;
    let mut end = first;
    let mut w = 0.0;
    while end < f.text.len() && w + ui().cw(f.text[end], INPUT, true) <= max {
        w += ui().cw(f.text[end], INPUT, true);
        end += 1;
    }
    let shown: String = f.text[first..end].iter().collect();
    let (tx, ty) = (bx + 12.0, by + 9.0);
    let x_of = |i: usize| tx + ui().width(&f.text[first..i.clamp(first, end)], INPUT, true);
    if f.text.is_empty() {
        let hint = if s.focused { "type an instruction, then Enter" } else { "click here to type an instruction" };
        marks.push(mono_sized(hint, tx, ty, INPUT, ui().th.kicker));
    } else {
        if let Some((a, b)) = f.selection() {
            let (xa, xb) = (x_of(a), x_of(b));
            if xb > xa {
                marks.push(draw::rect(xa, ty - 2.0, xb - xa, 20.0, ui().th.selection, None, 0.0));
            }
        }
        marks.push(mono_sized(&shown, tx, ty, INPUT, ink()));
    }
    if s.focused && caret_on(s) {
        marks.push(draw::rect(x_of(f.caret) - 0.5, ty - 2.0, 1.6, 20.0, ink(), None, 0.0));
    }
    if s.writing > 0 {
        marks.push(status("Haiku writing…", PX + 310.0, 144.0, 12.0, accent(), false, false));
    } else if s.in_flight > 0 {
        marks.push(status("deciding…", PX + 330.0, 144.0, 12.0, accent(), false, false));
    }

    let mut y = 164.0;
    if let Some(a) = &s.shown {
        marks.push(t(&format!("decision on \"{}\"{}", fit(&a.prefix, 34), if a.complete { " ⏎" } else { "" }), PX, y, 12.0, muted(), false));
        let render = a.decision.answers.get("render").and_then(Value::as_str).filter(|r| *r != "chart");
        let head = pilot::short(&a.decision.answers).replace('/', "  ·  ").replace('_', " ") + &render.map_or(String::new(), |r| format!("  →  {r}"));
        marks.push(t(&head, PX, y + 18.0, 20.0, ink(), true));
        let mut side = vec![];
        if let Some(r) = a.decision.answers.get("render").and_then(Value::as_str) {
            side.push(format!("render {r} {:.2}", a.decision.confidence_of("render").unwrap_or(0.0)));
        }
        if let Some(sp) = a.decision.answers.get("specifics").and_then(Value::as_str) {
            side.push(format!("specifics {sp} {:.2}", a.decision.confidence_of("specifics").unwrap_or(0.0)));
        }
        if !side.is_empty() {
            marks.push(t(&side.join(" · "), PX, y + 44.0, 11.0, muted(), false));
        }
        let latency = if a.decision.cached { format!("cache · {:.0} ms live", a.decision.ms) } else { format!("{:.0} ms", a.wall_ms) };
        marks.push(draw::text(&latency, PX + 410.0, y + 22.0, 12.0, muted(), TextAlign::Right, TextBaseline::Top, false, 0.0));
        y += 64.0;
        for (k, (opt, p)) in a.decision.probs.iter().take(6).enumerate() {
            let yy = y + k as f32 * 24.0;
            let chosen = a.decision.answers.get("action").and_then(Value::as_str) == Some(opt);
            marks.push(t(&opt.replace('_', " "), PX, yy + 2.0, 13.0, if chosen { ink() } else { muted() }, chosen));
            let th = &ui().th;
            marks.push(draw::rect(PX + 100.0, yy + 2.0, 240.0, 14.0, [1.0; 4], Some(th.line), ui().radius(3.0)));
            marks.push(draw::rect(PX + 100.0, yy + 2.0, (240.0 * *p as f32).max(1.0), 14.0, if chosen { th.accent } else { th.line }, None, ui().radius(3.0)));
            marks.push(t(&format!("{p:.2}"), PX + 350.0, yy + 2.0, 12.0, muted(), false));
        }
        y += 6.0 * 24.0 + 8.0;
        marks.push(status(&fit(&a.gate, 60), PX, y, 14.0, a.colour, true, false));
    } else {
        y += 56.0 + 6.0 * 24.0 + 8.0;
        marks.push(t("type an instruction", PX, y, 14.0, muted(), false));
    }
    y += 40.0;
    marks.push(draw::rule(PX, y - 12.0, PX + 410.0, y - 12.0, ui().th.line));
    marks.push(t("changes", PX, y, 12.0, muted(), false));
    for (k, (prefix, short)) in s.changes.iter().rev().take(5).enumerate() {
        let i = ink();
        let c = [i[0], i[1], i[2], 1.0 - 0.15 * k as f32];
        marks.push(t(&fit(prefix, 34), PX, y + 20.0 + k as f32 * 20.0, 12.0, c, false));
        marks.push(t(short, PX + 280.0, y + 20.0 + k as f32 * 20.0, 12.0, c, true));
    }
    let footer = if s.message.is_empty() {
        format!("{} decisions ({} cached) · {} changes · ${:.4} spent", s.decisions, s.cached, s.changes.len(), s.cost)
    } else {
        s.message.clone()
    };
    if !s.notice.is_empty() && s.message.is_empty() {
        marks.push(status(&fit(&s.notice, 70), PX, H - 50.0, 12.0, ui().th.ok, false, false));
    }
    if s.message.is_empty() {
        marks.push(t(&fit(&footer, 64), PX, H - 32.0, 12.0, muted(), false));
    } else {
        marks.push(status(&fit(&footer, 64), PX, H - 32.0, 12.0, ui().th.error, false, false));
    }
}

/// Editor mode: the pipeline as text.
fn editor_panel(s: &App, marks: &mut Vec<SceneMark>) {
    marks.push(t("the chart as a pipeline · ⌘↵ apply · Esc revert · ⌘E autopilot", PX, 64.0, 12.0, muted(), false));
    let [ex, ey, _, eh] = EDIT_AREA;
    field_frame(EDIT_AREA, s.focused, marks);
    let text = &s.code.text;
    let r = rows(text);
    let (row, col) = caret_at(&r, s.code.caret);
    let sel = s.code.selection();
    let visible = ((eh - 16.0) / LH) as usize;
    let x_in = |a: usize, i: usize| ex + 8.0 + ui().width(&text[a..i], CODE, true);
    for (k, (a, len)) in r.iter().enumerate().skip(s.scroll).take(visible) {
        let y = ey + 8.0 + (k - s.scroll) as f32 * LH;
        if let Some((sa, sb)) = sel {
            let (x0, x1) = (sa.max(*a), sb.min(a + len));
            // A selected line break shows as a sliver at the end of its row.
            let tail = if sb > a + len && x1 == a + len { 4.0 } else { 0.0 };
            if x1 > x0 || (tail > 0.0 && x0 <= x1) {
                let xa = x_in(*a, x0);
                marks.push(draw::rect(xa, y - 1.0, x_in(*a, x1) - xa + tail, LH, ui().th.selection, None, 0.0));
            }
        }
        let line: String = text[*a..a + len].iter().collect();
        // Stage names stand out; continuation rows are quieter.
        let starts_stage = *a == 0 || text[a - 1] == '\n';
        marks.push(mono_sized(&line, ex + 8.0, y, CODE, if starts_stage { ink() } else { muted() }));
    }
    if s.focused && row >= s.scroll && row < s.scroll + visible && caret_on(s) {
        let a = r[row].0;
        let (cx, cy) = (x_in(a, a + col), ey + 8.0 + (row - s.scroll) as f32 * LH);
        marks.push(draw::rect(cx - 0.5, cy - 1.0, 1.4, LH, ink(), None, 0.0));
    }
    let mut y = ey + eh + 10.0;
    let (message, colour) = &s.edit_status;
    for line in wrap(message, 62).iter().take(3) {
        marks.push(status(line, PX, y, 12.0, *colour, false, false));
        y += 16.0;
    }
    let help = [
        "bars n --by label · pie n --by label · line n --x t --series line",
        "heatmap n --x band --y label · map --x cx --y cy --value h",
        "color #rrggbb · highlight \"datum.<f> >= <n>\" · clear-highlight",
        "zoom x0..x1 y0..y1 · reset-zoom · data: read, filter, calc, sql · head N",
    ];
    for (k, h) in help.iter().enumerate() {
        marks.push(t(h, PX, H - 84.0 + k as f32 * 16.0, 11.0, muted(), false));
    }
}

/// The chart in one line: mark, table, colour, zoom, emphasis, title.
fn state_line(s: &State) -> String {
    let zoom = match (s.range, s.zoom) {
        (Some(((a, b), (c, d))), _) => format!("zoom {a}..{b} {c}..{d}"),
        (None, Some(q)) => format!("zoom {q:?}"),
        (None, None) => "zoom all".into(),
    };
    let emphasis = match (s.highlight, s.threshold) {
        (false, _) => "no emphasis".to_string(),
        (true, None) => "emphasis top 10 %".into(),
        (true, Some(t)) => format!("emphasis >= {t}"),
    };
    format!("{} of {} · colour {} · {zoom} · {emphasis} · title \"{}\"", s.mark.id(), s.dataset.id(), pilot::colour_name(s.color), s.title)
}

/// One line from a shell command, for the session header.
fn shell(cmd: &str) -> String {
    std::process::Command::new("sh").args(["-c", cmd]).output().ok().map_or(String::new(), |o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// A number of rows in the instruction: "head 2", "5 rows", "10 rijen".
fn rows_asked(s: &str) -> Option<usize> {
    let w: Vec<String> = s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect();
    w.iter().enumerate().find_map(|(i, x)| {
        let n = x.parse::<usize>().ok()?;
        let before = i > 0 && matches!(w[i - 1].as_str(), "head" | "first" | "eerste" | "top");
        let after = w.get(i + 1).is_some_and(|y| matches!(y.as_str(), "rows" | "row" | "rijen" | "rij" | "lines"));
        (before || after).then_some(n.clamp(1, 200))
    })
}

/// The rows of an editor query, over the chart.
fn table_view(tb: &editor::Table, marks: &mut Vec<SceneMark>) {
    let th = &ui().th;
    let (x0, y0, w, h) = (16.0, 16.0, PX - 52.0, H - 32.0);
    marks.push(draw::rect(x0, y0, w, h, th.background, Some(th.line), 0.0));
    let (x, mut y) = (x0 + 16.0, y0 + 14.0);
    marks.push(t("query result", x, y, 13.0, accent(), true));
    marks.push(t(&tb.note, x + 110.0, y + 1.0, 12.0, muted(), false));
    y += 26.0;
    // Rows get denser when there is more to show than fits.
    let lines = tb.rows.len() + tb.text.len() + 3;
    let row: f32 = ((y0 + h - 30.0 - y) / lines as f32).clamp(13.0, 17.0);
    let size: f32 = if row < 15.0 { 11.0 } else { 12.0 };
    let room = ((y0 + h - 30.0 - y) / row) as usize;
    if !tb.columns.is_empty() {
        // Column widths from what they hold, capped; columns that do not fit
        // are named below the table.
        let cap = 24;
        let width = |s: &str| ui().width(&fit(s, cap).chars().collect::<Vec<_>>(), size, true);
        let mut cols: Vec<(usize, f32)> = vec![];
        let mut used = 0.0;
        for (i, c) in tb.columns.iter().enumerate() {
            let cw = tb.rows.iter().take(room).map(|r| width(&r[i])).fold(width(c), f32::max) + 18.0;
            if used + cw > w - 32.0 {
                break;
            }
            cols.push((i, used));
            used += cw;
        }
        for (i, cx) in &cols {
            marks.push(t(&fit(&tb.columns[*i], cap), x + cx, y, size, ink(), true));
        }
        marks.push(draw::rule(x, y + row, x + used, y + row, th.line));
        y += row + 5.0;
        for r in tb.rows.iter().take(room.saturating_sub(1)) {
            for (i, cx) in &cols {
                marks.push(mono_sized(&fit(&r[*i], cap), x + cx, y, size, ink()));
            }
            y += row;
        }
        let hidden: Vec<&str> = tb.columns.iter().skip(cols.len()).map(String::as_str).collect();
        if !hidden.is_empty() {
            y += 6.0;
            marks.push(t(&fit(&format!("{} more columns: {}", hidden.len(), hidden.join(", ")), 150), x, y, 12.0, muted(), false));
            y += row;
        }
        y += 8.0;
    }
    for l in tb.text.iter() {
        if y > y0 + h - 40.0 {
            break;
        }
        marks.push(mono_sized(&fit(l, 150), x, y, size, muted()));
        y += row;
    }
    marks.push(t("the chart is unchanged · ⌘↵ with a chart command draws it · Esc or ⌘E closes this", x, y0 + h - 24.0, 11.0, muted(), false));
}

/// Stats for nerds: an overlay on the chart.
fn nerds(s: &App, now: Instant, marks: &mut Vec<SceneMark>) {
    let (x0, y0, w, h) = (16.0, 16.0, PX - 52.0, H - 32.0);
    let th = &ui().th;
    marks.push(draw::rect(x0, y0, w, h, th.dark_background, None, ui().radius(8.0)));
    let (head, text, dim) = (th.dark_accent, th.dark_text, th.dark_muted);
    let x = x0 + 16.0;
    let mut y = y0 + 14.0;
    let line = |marks: &mut Vec<SceneMark>, label: &str, value: &str, colour: [f32; 4], y: &mut f32| {
        marks.push(mono(label, x, *y, dim));
        marks.push(mono(&fit(value, 108), x + 76.0, *y, colour));
        *y += 15.0;
    };
    let status_line = |marks: &mut Vec<SceneMark>, label: &str, value: &str, colour: [f32; 4], y: &mut f32| {
        marks.push(mono(label, x, *y, dim));
        marks.push(status_on(&fit(value, 108), x + 76.0, *y, 11.0, dim, colour, false, true));
        *y += 15.0;
    };
    // What the last decision added: the accent in the calm theme.
    let fresh = if th.status_italic { th.dark_accent } else { [0.62, 0.90, 0.62, 1.0] };
    marks.push(mono("stats for nerds", x, y, head));
    marks.push(mono(&format!("the whole pipeline behind this chart, runnable as it stands · also in {PIPELINE_FILE}"), x + 130.0, y, dim));
    y += 20.0;
    // One stage per line, wrapped, never cut. Stages the last decision
    // added are green.
    let added = |stage: &str| s.shown.as_ref().is_some_and(|a| a.lines.iter().any(|l| l.split(" ! ").any(|p| p == stage)));
    for (i, stage) in s.pipeline.iter().enumerate() {
        let colour = if i > 0 && added(stage) { fresh } else { text };
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
        // The time in its own right-aligned column: text is not monospace.
        marks.push(draw::text(&format!("{ms:.2} ms"), x0 + w - 24.0, y, 11.0, dim, TextAlign::Right, TextBaseline::Top, false, 0.0));
        line(marks, label, &format!("{:>3}  {}", i + 1, fit(c, 100)), dim, &mut y);
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
        status_line(marks, "gate", &a.gate, a.colour, &mut y);
        if a.lines.is_empty() {
            line(marks, "lines", "none", dim, &mut y);
        }
        for (i, l) in a.lines.iter().enumerate() {
            line(marks, if i == 0 { "lines" } else { "" }, l, fresh, &mut y);
        }
        if !a.fold.is_empty() {
            status_line(marks, "fold", &a.fold, text, &mut y);
        }
        if let Some(w) = &a.written {
            line(marks, "direction", &w.direction, text, &mut y);
            if !w.done {
                line(marks, "writer", "Claude Haiku 4.5 is writing the pipeline…", fresh, &mut y);
            }
            for (i, (ms, cost, cached, refused, _)) in w.tries.iter().enumerate() {
                let what = match refused {
                    None => "accepted".to_string(),
                    Some(r) => format!("refused: {r}"),
                };
                let at = if *cached { "from cache" } else { "live" };
                line(marks, if i == 0 { "writer" } else { "" }, &format!("try {} · {ms:.0} ms · {at} · ${cost:.4} · {what}", i + 1), if refused.is_some() { dim } else { text }, &mut y);
            }
            for (i, r) in w.removed.iter().enumerate() {
                line(marks, if i == 0 { "removed" } else { "" }, &format!("- {r}"), dim, &mut y);
            }
        }
    } else if s.changes.last().is_some_and(|c| c.0 == "editor") {
        line(marks, "edited", "by hand in the editor, not by a decision", text, &mut y);
        status_line(marks, "status", &s.edit_status.0, s.edit_status.1, &mut y);
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
    let now = clock();
    let t0 = std::time::Instant::now();
    if let Some(last) = s.last_frame {
        s.frame_ms = 0.8 * s.frame_ms + 0.2 * (now - last).as_secs_f64() * 1e3;
    }
    s.last_frame = Some(now);
    if s.editing {
        let (row, _) = caret_at(&rows(&s.code.text), s.code.caret);
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
    if let Some(t) = &s.table {
        table_view(t, &mut marks);
    }
    if s.nerds {
        nerds(s, now, &mut marks);
    }
    panel(s, &mut marks);
    s.build_ms = t0.elapsed().as_secs_f64() * 1e3;

    // Keep waking while something moves: a transition, a decision in
    // flight, the caret, or a pending debounce.
    let mut commands = vec![];
    let deadline = match s.typed_at {
        Some(at) if s.input.string().trim() != s.asked => (at + DEBOUNCE).min(now + FRAME * 30),
        _ if s.animating(now) || s.in_flight > 0 || s.applying || s.copied_at.is_some_and(|t| (now - t).as_secs_f64() < 1.6) => now + FRAME,
        _ => now + Duration::from_millis(500),
    };
    s.wake_generation += 1;
    commands.push(RuntimeHostCommand::RequestWakeup {
        key: RuntimeWakeKey::new(WAKE, 0, "frame"),
        deadline,
        generation: s.wake_generation,
    });
    let scene_graph = SceneGraph { marks, width: W, height: H, origin: [0.0; 2] };
    if std::mem::take(&mut s.save_view) {
        // The same scene, headless, at the window's scale: what was on screen.
        let sg = scene_graph.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                use avenger_wgpu::canvas::{Canvas, PngCanvas};
                if let Ok(mut c) = PngCanvas::new(avenger_common::canvas::CanvasDimensions { size: [W, H], scale: 2.0 }, Default::default()).await {
                    if c.set_scene(&sg).is_ok() {
                        if let Ok(img) = c.render().await {
                            let _ = img.save(VIEW_FILE);
                        }
                    }
                }
            });
        });
    }
    SceneBuild {
        scene_graph,
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
        let now = clock();
        let rerender = UpdateStatus { rerender: true, ..Default::default() };
        if std::env::var("AUTOPILOT_DEBUG").is_ok() && !matches!(event, Event::RuntimeWake(_)) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("out/autopilot_live/events.log") {
                let _ = writeln!(f, "{event:?}");
            }
        }
        match event {
            Event::RuntimeWake(w) if w.key.namespace == WAKE => {
                if s.typed_at.is_some_and(|at| now - at >= DEBOUNCE) {
                    s.typed_at = None;
                    s.ask(false);
                }
                s.collect(now).await;
                s.collect_written(now).await;
                s.collect_edit(now).await;
                rerender
            }
            Event::KeyPress(e) => {
                let cmd = e.modifiers.meta || e.modifiers.control;
                if matches!(e.key, Key::Character('e') | Key::Character('E')) && cmd {
                    if s.editing { s.editing = false; s.table = None } else { s.open_editor() }
                    return rerender;
                }
                if matches!(e.key, Key::Named(NamedKey::Tab)) {
                    s.nerds = !s.nerds;
                    return rerender;
                }
                // Modifier keys alone do nothing.
                if matches!(e.key, Key::Named(NamedKey::Super | NamedKey::Control | NamedKey::Shift | NamedKey::Alt | NamedKey::CapsLock)) {
                    return UpdateStatus::default();
                }
                // Typing into an unfocused window focuses the field.
                s.focused = true;
                s.last_key = clock();
                let m = Mods { cmd, shift: e.modifiers.shift, alt: e.modifiers.alt };
                let handled = if s.editing {
                    s.edit_key(&e.key, e.text.as_deref(), m)
                } else {
                    s.input_key(&e.key, e.text.as_deref(), m, now)
                };
                if handled { rerender } else { UpdateStatus::default() }
            }
            // ⌘X, ⌘C and ⌘V: the host reads the system clipboard for a
            // paste, and writes what the app hands back for a copy.
            Event::Clipboard(c) => {
                s.focused = true;
                s.last_key = clock();
                let f = if s.editing { &mut s.code } else { &mut s.input };
                let mut status = rerender;
                match c {
                    ClipboardEvent::Copy | ClipboardEvent::Cut => {
                        let Some((a, b)) = f.selection() else { return UpdateStatus::default() };
                        let text: String = f.text[a..b].iter().collect();
                        status.commands.push(RuntimeHostCommand::WriteClipboard { text });
                        if matches!(c, ClipboardEvent::Cut) {
                            f.delete_selection();
                        }
                    }
                    ClipboardEvent::Paste(text) => {
                        // The autopilot box is one line.
                        let text = if s.editing { text.to_string() } else { text.replace(['\n', '\r'], " ") };
                        f.insert(&text);
                    }
                }
                if !s.editing && !matches!(c, ClipboardEvent::Copy) {
                    s.typed_at = Some(now);
                }
                status
            }
            Event::MouseDown(e) if e.button == MouseButton::Left => {
                let p = e.position;
                if inside(p, NERDS_BUTTON) {
                    s.nerds = !s.nerds;
                } else if inside(p, DATA_BUTTON) {
                    if s.table.as_ref().is_some_and(|t| t.note.contains(" tables, ")) {
                        s.table = None;
                    } else {
                        s.show_overview();
                        s.session.push(format!("{:.1} s, data button: the overview", (clock() - s.t0).as_secs_f64()));
                        s.logged += 1;
                    }
                } else if inside(p, COPY_BUTTON) {
                    let text = s.session_text();
                    let _ = std::fs::create_dir_all("out/autopilot_live");
                    let _ = std::fs::write(SESSION_FILE, &text);
                    s.copied_at = Some(clock());
                    s.save_view = true;
                    s.notice = format!("copied: {} decisions, {} sent with Enter · also in {SESSION_FILE}", s.logged, s.sent.len());
                    let mut status = rerender;
                    status.commands.push(RuntimeHostCommand::WriteClipboard { text });
                    return status;
                } else if inside(p, AUTOPILOT_BUTTON) {
                    s.editing = false;
                } else if inside(p, EDITOR_BUTTON) && !s.editing {
                    s.open_editor();
                } else if s.editing && inside(p, EDIT_AREA) {
                    s.focused = true;
                    s.last_key = clock();
                    s.click_editor(p, e.modifiers.shift);
                    match s.multi_click(p) {
                        2 => s.code.select_word(),
                        n if n >= 3 => s.code.select_line(),
                        // A single click starts a drag that selects.
                        _ => s.dragging = Some(true),
                    }
                } else if !s.editing && inside(p, INPUT_BOX) {
                    s.focused = true;
                    s.last_key = clock();
                    s.click_input(p, e.modifiers.shift);
                    match s.multi_click(p) {
                        2 => s.input.select_word(),
                        n if n >= 3 => s.input.select_line(),
                        _ => s.dragging = Some(false),
                    }
                } else {
                    // A click anywhere else takes the focus away.
                    s.focused = false;
                }
                rerender
            }
            // Dragging with the button down extends the selection from
            // where it was pressed; letting go ends it.
            Event::CursorMoved(e) => match s.dragging {
                Some(true) => {
                    s.click_editor(e.position, true);
                    s.last_key = clock();
                    rerender
                }
                Some(false) => {
                    s.click_input(e.position, true);
                    s.last_key = clock();
                    rerender
                }
                None => UpdateStatus::default(),
            },
            Event::MouseUp(_) => {
                s.dragging = None;
                UpdateStatus::default()
            }
            _ => UpdateStatus::default(),
        }
    }
}

/// One step of a recording script.
enum Act {
    /// Type text at typing speed into the focused field.
    Type(&'static str),
    Key(NamedKey),
    /// Tab: stats for nerds.
    Tab,
    /// ⌘E: switch between autopilot and editor.
    CmdE,
    /// ⌘↵: apply the editor text.
    CmdEnter,
    /// Select the first occurrence of this text in the editor.
    Select(&'static str),
    Wait(f64),
    /// Wait until nothing is pending or moving, then this long.
    Settle(f64),
}

/// The recording: the live window, driven by a script on a virtual clock.
fn script() -> Vec<Act> {
    use Act::*;
    vec![
        Wait(1.5),
        Type("make the bars red"), Wait(0.3), Key(NamedKey::Enter), Settle(1.2),
        // A pause mid-sentence: the decider is asked after 400 ms.
        Type("which share"), Wait(1.0), Type(" does each class have?"), Wait(0.3), Key(NamedKey::Enter), Settle(1.5),
        Type("back to bars please"), Key(NamedKey::Enter), Settle(1.2),
        Type("emphasise the biggest classes"), Key(NamedKey::Enter), Settle(1.2),
        Type("how are the classes spread over height?"), Key(NamedKey::Enter), Settle(1.5),
        Type("how many points did each flight line record over time?"), Key(NamedKey::Enter), Settle(1.5),
        Type("zoom in on the start"), Wait(1.0), Type(" of the lines"), Key(NamedKey::Enter), Settle(1.2),
        Tab, Wait(2.0), Type("zoom back out"), Key(NamedKey::Enter), Settle(2.5), Tab,
        Type("where are the buildings?"), Key(NamedKey::Enter), Settle(1.5),
        Type("mark the tallest buildings"), Key(NamedKey::Enter), Settle(1.2),
        Tab, Wait(4.0), Tab, Wait(0.5),
        // The same chart, by hand: 10 m cells and another threshold.
        CmdE, Wait(2.0),
        Select("floor(x/5)*5 AS cx, floor(y/5)*5 AS cy"), Wait(0.5), Type("floor(x/10)*10 AS cx, floor(y/10)*10 AS cy"), Wait(0.3),
        Select("floor(x/5)*5, floor(y/5)*5"), Wait(0.5), Type("floor(x/10)*10, floor(y/10)*10"), Wait(0.3),
        Select("62.74"), Wait(0.5), Type("70"), Wait(0.8),
        CmdEnter, Settle(2.5),
        CmdE, Wait(0.6),
        Type("kleur de gebouwen groen"), Key(NamedKey::Enter), Settle(1.2),
        Type("zoom to the south-east"), Key(NamedKey::Enter), Settle(1.5),
        Tab, Wait(4.5),
    ]
}

async fn record(mut app: App, dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    use avenger_wgpu::canvas::{Canvas, PngCanvas};
    const FPS: f64 = 30.0;
    std::fs::create_dir_all(dir)?;
    let mut canvas = PngCanvas::new(avenger_common::canvas::CanvasDimensions { size: [W, H], scale: 1.0 }, Default::default()).await?;
    let base = clock();
    let no = Mods { cmd: false, shift: false, alt: false };
    // Typing splits into one key per character.
    let mut acts: std::collections::VecDeque<Act> = Default::default();
    for a in script() {
        match a {
            Act::Type(t) => acts.extend(t.split_inclusive(|_: char| true).map(Act::Type)),
            a => acts.push_back(a),
        }
    }
    let mut ready = base;
    let mut frame = 0usize;
    let mut tail = None;
    loop {
        let now = base + Duration::from_secs_f64(frame as f64 / FPS);
        *VIRTUAL.get().unwrap().lock().unwrap() = now;
        while let Some(a) = acts.front() {
            if now < ready {
                break;
            }
            let busy = app.in_flight > 0 || app.applying || app.animating(now) || app.typed_at.is_some();
            let mut wait = 0.0;
            match a {
                Act::Type(c) => {
                    let key = if *c == " " { Key::Named(NamedKey::Space) } else { Key::Character(c.chars().next().unwrap()) };
                    app.last_key = now;
                    if app.editing {
                        app.edit_key(&key, Some(c), no);
                        wait = 0.03;
                    } else {
                        app.input_key(&key, Some(c), no, now);
                        wait = 0.065;
                    }
                }
                Act::Key(k) => {
                    app.last_key = now;
                    if app.editing { app.edit_key(&Key::Named(*k), None, no); } else { app.input_key(&Key::Named(*k), None, no, now); }
                }
                Act::Tab => app.nerds = !app.nerds,
                Act::CmdE => {
                    if app.editing { app.editing = false } else { app.open_editor() }
                }
                Act::CmdEnter => app.apply_text(),
                Act::Select(t) => {
                    let text = app.code.string();
                    let i = text.find(t).ok_or_else(|| format!("script: `{t}` is not in the editor"))?;
                    let a = text[..i].chars().count();
                    app.code.anchor = Some(a);
                    app.code.caret = a + t.chars().count();
                    app.last_key = now;
                }
                Act::Wait(d) => wait = *d,
                Act::Settle(d) => {
                    if busy {
                        break;
                    }
                    wait = *d;
                }
            }
            acts.pop_front();
            ready = now + Duration::from_secs_f64(wait);
        }
        // Wait (in real time) for decisions and applies in flight, so they
        // can arrive on the virtual clock.
        if app.typed_at.is_some_and(|at| now - at >= DEBOUNCE) {
            app.typed_at = None;
            app.ask(false);
        }
        while app.returned.lock().unwrap().len() < app.in_flight {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        while app.applying && app.edited.lock().unwrap().is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        app.collect(now).await;
        app.collect_written(now).await;
        app.collect_edit(now).await;
        canvas.set_scene(&build(&mut app).scene_graph)?;
        canvas.render().await?.save(format!("{dir}/f{frame:05}.png"))?;
        frame += 1;
        if acts.is_empty() {
            let end = *tail.get_or_insert(frame + FPS as usize / 2);
            if frame >= end {
                break;
            }
        }
    }
    println!(
        "wrote {frame} frames ({:.0} s) to {dir}: {} decisions, {} from cache, {} changes",
        frame as f64 / FPS,
        app.decisions,
        app.cached,
        app.changes.len()
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--record") {
        let _ = VIRTUAL.set(Mutex::new(Instant::now()));
    }
    // The calm theme, or with `--neutral` the look of the recordings.
    let neutral = std::env::args().any(|a| a == "--neutral");
    let ui_ = Ui::new(if neutral { Theme::neutral() } else { Theme::calm() });
    if !neutral {
        let th = &ui_.th;
        draw::set_style(draw::Style {
            font: ui_.font.clone(),
            ink: th.text,
            muted: th.muted,
            grid: [th.line[0], th.line[1], th.line[2], 0.45],
            title: th.accent,
            kicker: Some(("LiDAR tile 0657 6868".into(), th.kicker)),
        });
    }
    let _ = UI.set(ui_);
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let data = Arc::new(runtime.block_on(data::load())?);
    let (mut pipe, taken) = runtime.block_on(layer_pipeline())?;
    let state = State::new(Dataset::Classes);
    for l in package::initial(&state, &data) {
        runtime.block_on(pipe.run(&l))?;
    }
    let state = package::state(&pipe.chart, &data)?;
    let frame = resolve(&state, &data);
    // The recording replays the video's script from the cache, so it runs
    // without the writer; `--no-writer` does the same live.
    let with_writer = args.get(1).map(String::as_str) != Some("--record") && !args.iter().any(|a| a == "--no-writer");
    let client = reqwest::Client::new();
    let mut app = App {
        base: data.clone(),
        data: data.clone(),
        data_stages: package::data_stages(state.dataset),
        pipe: Arc::new(tokio::sync::Mutex::new(Pipeline::new(pipe.ctx.clone()))),
        rt: runtime.handle().clone(),
        jev: Arc::new(Jev { model: "typesafe/jev-1.13", client: client.clone() }),
        questions: Arc::new(pilot::questions()),
        writer: with_writer.then(|| Arc::new(Writer { model: "anthropic/claude-haiku-4.5", client })),
        enter_questions: Arc::new(writer::questions()),
        written_back: Arc::new(Mutex::new(vec![])),
        writing: 0,
        history: vec![],
        first: String::new(),
        sent: vec![],
        session: vec![],
        notice: String::new(),
        exports: 0,
        t0: clock(),
        copied_at: None,
        logged: 0,
        save_view: false,
        started_at: shell("date '+%Y-%m-%d %H:%M'"),
        commit: shell("git rev-parse --short HEAD"),
        taken,
        state,
        from: frame.clone(),
        to: frame,
        started: None,
        dur: 1.0,
        input: Field::default(),
        focused: true,
        last_key: clock(),
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
        last_click: None,
        dragging: None,
        clicks: 0,
        editing: false,
        code: Field::default(),
        scroll: 0,
        edit_status: (String::new(), muted()),
        applying: false,
        apply_started: clock(),
        edited: Arc::new(Mutex::new(None)),
        queried: Arc::new(Mutex::new(None)),
        table: None,
    };
    app.snapshot(&pipe);
    app.first = app.pipeline.join("\n! ");
    app.pipe = Arc::new(tokio::sync::Mutex::new(pipe));

    // `--snapshot <dir> "instruction" ...`: the same path as the window
    // (Enter, gates, pipeline, fold), rendered to PNG with and without stats
    // for nerds, for checking without a display.
    if args.get(1).map(String::as_str) == Some("--record") {
        let dir = args.get(2).cloned().unwrap_or("out/autopilot_live/frames".into());
        return runtime.block_on(record(app, &dir));
    }
    if args.get(1).map(String::as_str) == Some("--snapshot") {
        let dir = args.get(2).ok_or("--snapshot <dir> instructions...")?.clone();
        std::fs::create_dir_all(&dir)?;
        return runtime.block_on(async move {
            use avenger_wgpu::canvas::{Canvas, PngCanvas};
            let mut canvas = PngCanvas::new(avenger_common::canvas::CanvasDimensions { size: [W, H], scale: 1.0 }, Default::default()).await?;
            // `@@session.txt` replays the instructions of a copied session.
            let mut steps: Vec<String> = vec![];
            for a in &args[3..] {
                match a.strip_prefix("@@") {
                    Some(path) => steps.extend(std::fs::read_to_string(path)?.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from)),
                    None => steps.push(a.clone()),
                }
            }
            for (i, text) in steps.iter().enumerate() {
                if let Some(path) = text.strip_prefix('@') {
                    app.open_editor();
                    app.code.set(std::fs::read_to_string(path)?.trim_end());
                    app.apply_text();
                    while app.edited.lock().unwrap().is_none() && app.queried.lock().unwrap().is_none() {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    app.collect_edit(clock()).await;
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
                app.input.set(text);
                app.sent.push(text.clone());
                app.ask(true);
                app.input.set("");
                while app.returned.lock().unwrap().is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                app.shown = None;
                app.message.clear();
                app.collect(clock()).await;
                while app.writing > 0 || app.applying {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    app.collect_written(clock()).await;
                    app.collect_edit(clock()).await;
                }
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
                let _ = std::fs::write(SESSION_FILE, app.session_text());
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
                types: vec![Type::KeyPress, Type::TextInput, Type::Ime, Type::Clipboard, Type::MouseDown, Type::MouseUp, Type::CursorMoved, Type::RuntimeWake],
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

#[cfg(test)]
mod tests {
    use super::*;

    const NO: Mods = Mods { cmd: false, shift: false, alt: false };
    const SHIFT: Mods = Mods { cmd: false, shift: true, alt: false };
    const CMD: Mods = Mods { cmd: true, shift: false, alt: false };
    const ALT: Mods = Mods { cmd: false, shift: false, alt: true };

    fn field(s: &str) -> Field {
        let mut f = Field::default();
        f.set(s);
        f
    }
    fn named(k: NamedKey) -> Key {
        Key::Named(k)
    }
    fn typed(f: &mut Field, s: &str) {
        edit(f, &Key::Character(s.chars().next().unwrap()), Some(s), NO);
    }

    #[test]
    fn a_drag_selects_from_where_it_was_pressed() {
        // Press at 2, drag to 7, then back to 4: the selection is 2..4.
        let mut f = field("filter ground");
        f.move_to(2, false);
        f.move_to(7, true);
        assert_eq!(f.selection(), Some((2, 7)));
        f.move_to(4, true);
        assert_eq!(f.selection(), Some((2, 4)));
        // Dragging back past the press point selects the other way.
        f.move_to(0, true);
        assert_eq!(f.selection(), Some((0, 2)));
    }

    #[test]
    fn arrows_move_and_edit_in_place() {
        let mut f = field("show as pie");
        for _ in 0..3 {
            edit(&mut f, &named(NamedKey::ArrowLeft), None, NO);
        }
        typed(&mut f, "a ");
        assert_eq!(f.string(), "show as a pie");
        edit(&mut f, &named(NamedKey::Backspace), None, NO);
        assert_eq!(f.string(), "show as apie");
    }

    #[test]
    fn shift_arrows_select_and_backspace_deletes_the_selection() {
        let mut f = field("make it red");
        for _ in 0..3 {
            edit(&mut f, &named(NamedKey::ArrowLeft), None, SHIFT);
        }
        assert_eq!(f.selection(), Some((8, 11)));
        edit(&mut f, &named(NamedKey::Backspace), None, NO);
        assert_eq!(f.string(), "make it ");
        typed(&mut f, "g");
        assert_eq!(f.string(), "make it g");
    }

    #[test]
    fn typing_replaces_a_selection_and_arrows_collapse_it() {
        let mut f = field("bars");
        edit(&mut f, &Key::Character('a'), None, CMD);
        assert_eq!(f.selection(), Some((0, 4)));
        typed(&mut f, "p");
        assert_eq!(f.string(), "p");
        let mut g = field("abc");
        edit(&mut g, &named(NamedKey::ArrowLeft), None, SHIFT);
        edit(&mut g, &named(NamedKey::ArrowLeft), None, SHIFT);
        edit(&mut g, &named(NamedKey::ArrowRight), None, NO);
        assert_eq!((g.caret, g.selection()), (3, None));
    }

    #[test]
    fn macos_backspace_as_a_control_character() {
        let mut f = field("abc");
        edit(&mut f, &Key::Character('\u{7f}'), Some("\u{7f}"), NO);
        assert_eq!(f.string(), "ab");
    }

    #[test]
    fn double_and_triple_click_select_a_word_and_a_line() {
        let mut f = field("zoom 657200..657600\n! highlight \"datum.h >= 70\"");
        f.caret = 7;
        f.select_word();
        assert_eq!(f.selection().map(|(a, b)| f.text[a..b].iter().collect::<String>()), Some("657200".into()));
        f.caret = 25;
        f.select_line();
        assert_eq!(f.selection().map(|(a, b)| f.text[a..b].iter().collect::<String>()), Some("! highlight \"datum.h >= 70\"".into()));
    }

    #[test]
    fn word_and_line_deletes() {
        let mut f = field("zoom to the south-east");
        edit(&mut f, &named(NamedKey::Backspace), None, ALT);
        assert_eq!(f.string(), "zoom to the south-");
        edit(&mut f, &named(NamedKey::Backspace), None, CMD);
        assert_eq!(f.string(), "");
        let mut g = field("line one\nline two");
        edit(&mut g, &named(NamedKey::Backspace), None, CMD);
        assert_eq!(g.string(), "line one\n");
    }
}
