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
use avenger_eventstream::window::{ClipboardEvent, Key, MouseButton, NamedKey};
use avenger_geometry::rtree::SceneGraphRTree;
use avenger_scenegraph::marks::mark::SceneMark;
use avenger_scenegraph::marks::text::SceneTextMark;
use avenger_scenegraph::scene_graph::SceneGraph;
use avenger_text::measurement::TextMeasurementConfig;
use avenger_text::types::{FontStyle, FontWeight, FontWeightNameSpec, TextAlign, TextBaseline, TextSyntaxMode};
use avenger_wgpu::canvas::CanvasConfig;
use lidar_decide::deciders::{Decider, Decision, Jev};
use lidar_decide::layer::anim::{still, transition};
use lidar_decide::layer::model::{resolve, Data, Dataset, Frame, State};
use lidar_decide::layer::theme::Theme;
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
/// The buttons in the panel header.
const NERDS_BUTTON: [f32; 4] = [PX + 300.0, 24.0, 110.0, 26.0];
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
    input: Field,
    /// Whether the text field (the autopilot box, or the editor) has
    /// keyboard focus: a blue border and a blinking caret.
    focused: bool,
    last_key: std::time::Instant,
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
    last_click: Option<(std::time::Instant, [f32; 2])>,
    clicks: u32,
    // Editor mode.
    editing: bool,
    code: Field,
    scroll: usize,
    edit_status: (String, [f32; 4]),
    applying: bool,
    edited: Arc<Mutex<Option<Result<editor::Applied, String>>>>,
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
                colour: if next.is_some() { ui().th.ok } else if gate.starts_with("waiting") { accent() } else { muted() },
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
                        shown.colour = ui().th.error;
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
        self.code.set(&self.pipeline.join("\n! "));
        self.focused = true;
        self.edit_status = (String::new(), muted());
    }

    fn apply_text(&mut self) {
        if self.applying {
            return;
        }
        self.applying = true;
        self.edit_status = ("applying…".into(), accent());
        let text = self.code.string();
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
                self.edit_status = (format!("applied in {:.0} ms · {}", a.ms, a.note), ui().th.ok);
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
                self.ask(true);
                self.input.set("");
            }
            Key::Named(NamedKey::Escape) => {
                self.input.set("");
                self.typed_at = None;
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
        let now = std::time::Instant::now();
        self.clicks = match self.last_click {
            Some((t, q)) if now.duration_since(t).as_secs_f64() < MULTI_CLICK && (p[0] - q[0]).abs() < 5.0 && (p[1] - q[1]).abs() < 5.0 => self.clicks + 1,
            _ => 1,
        };
        self.last_click = Some((now, p));
        self.clicks
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

/// The caret shows solid while typing, and blinks once typing pauses.
fn caret_on(s: &App) -> bool {
    let since = s.last_key.elapsed().as_secs_f64();
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
    for (label, r, on) in [
        ("autopilot", AUTOPILOT_BUTTON, !s.editing),
        ("editor", EDITOR_BUTTON, s.editing),
        ("stats for nerds", NERDS_BUTTON, s.nerds),
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
    marks.push(t("Jev 1.13 via OpenRouter · asks 400 ms after typing, and on Enter", PX, 58.0, 12.0, muted(), false));
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
    if s.in_flight > 0 {
        marks.push(status("deciding…", PX + 330.0, 144.0, 12.0, accent(), false, false));
    }

    let mut y = 164.0;
    if let Some(a) = &s.shown {
        marks.push(t(&format!("decision on \"{}\"{}", fit(&a.prefix, 34), if a.complete { " ⏎" } else { "" }), PX, y, 12.0, muted(), false));
        marks.push(t(&pilot::short(&a.decision.answers).replace('/', "  ·  ").replace('_', " "), PX, y + 18.0, 20.0, ink(), true));
        let latency = if a.decision.cached { format!("cache · {:.0} ms live", a.decision.ms) } else { format!("{:.0} ms", a.wall_ms) };
        marks.push(draw::text(&latency, PX + 410.0, y + 22.0, 12.0, muted(), TextAlign::Right, TextBaseline::Top, false, 0.0));
        y += 56.0;
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
        "zoom x0..x1 y0..y1 · reset-zoom · data: read, filter, calc, sql",
    ];
    for (k, h) in help.iter().enumerate() {
        marks.push(t(h, PX, H - 84.0 + k as f32 * 16.0, 11.0, muted(), false));
    }
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
    let now = Instant::now();
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
                // Modifier keys alone do nothing.
                if matches!(e.key, Key::Named(NamedKey::Super | NamedKey::Control | NamedKey::Shift | NamedKey::Alt | NamedKey::CapsLock)) {
                    return UpdateStatus::default();
                }
                // Typing into an unfocused window focuses the field.
                s.focused = true;
                s.last_key = std::time::Instant::now();
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
                s.last_key = std::time::Instant::now();
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
                } else if inside(p, AUTOPILOT_BUTTON) {
                    s.editing = false;
                } else if inside(p, EDITOR_BUTTON) && !s.editing {
                    s.open_editor();
                } else if s.editing && inside(p, EDIT_AREA) {
                    s.focused = true;
                    s.last_key = std::time::Instant::now();
                    s.click_editor(p, e.modifiers.shift);
                    match s.multi_click(p) {
                        2 => s.code.select_word(),
                        n if n >= 3 => s.code.select_line(),
                        _ => {}
                    }
                } else if !s.editing && inside(p, INPUT_BOX) {
                    s.focused = true;
                    s.last_key = std::time::Instant::now();
                    s.click_input(p, e.modifiers.shift);
                    match s.multi_click(p) {
                        2 => s.input.select_word(),
                        n if n >= 3 => s.input.select_line(),
                        _ => {}
                    }
                } else {
                    // A click anywhere else takes the focus away.
                    s.focused = false;
                }
                rerender
            }
            _ => UpdateStatus::default(),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        input: Field::default(),
        focused: true,
        last_key: std::time::Instant::now(),
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
        clicks: 0,
        editing: false,
        code: Field::default(),
        scroll: 0,
        edit_status: (String::new(), muted()),
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
                    app.code.set(std::fs::read_to_string(path)?.trim_end());
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
                app.input.set(text);
                app.ask(true);
                app.input.set("");
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
                types: vec![Type::KeyPress, Type::TextInput, Type::Ime, Type::Clipboard, Type::MouseDown, Type::RuntimeWake],
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
