# Experiment 7 — charts driven by decisions

Status: built and measured 24–25 Sep 2026, on Avenger `602b99c` (#130), with
Jev 1.13 and Claude Haiku 4.5 through OpenRouter. A training run as the data
source (phase G) is not built.

Can a chart be driven by what someone types, rather than by commands they
write? A typed classifier, [Jev](https://innfactory.ai/en/blog/jev-system-one-model-classifier-not-llm/),
turns text into choices from options we supply, in about 300 ms. Where the
options cannot say what was meant (a title, "taller than 70 m", "10 m cells"),
Claude Haiku writes the pipeline instead. Either way, what reaches the chart is
a pipeline that has been validated against the data, and the chart drawn is the
fold of that pipeline. One chart object moves between bars, a pie, a heatmap, a
time series and a map, with keyed transitions.

[video/autopilot_tour.mp4](video/autopilot_tour.mp4) (68 s) shows it:

![The tour, one frame per step](images/tour_sheet.png)

## The window

```sh
set -a; source <folder with your .env>/.env; set +a    # OPENROUTER_API_KEY
cargo run --release -p lidar-decide --bin autopilot_live
```

Three columns: the chart; **Jev**, right of it, with its last decision (the
action, how sure it was of each option, what the window did) and the changes
so far; and the panel, with the box to type in and the pipeline under it.

- **Autopilot.** Type what you want and press Enter. Jev reads while you type
  (400 ms after the last key) and on Enter; the line under the box says what
  Enter will do. What Jev's options cover applies at once; the rest goes to
  Haiku, and a banner over the chart says it is writing.
- **The pipeline** under the box follows every decision, with the stages the
  last one added in the accent colour, so the effect of each decision shows
  as text next to the chart. Click in it to edit, ⌘↵ applies; an edit is kept
  when the autopilot changes the chart, and Esc takes the running pipeline
  again. ⌘E moves between the box and the pipeline.
- **Undo and reset** are instructions like any other ("undo", "go back",
  "start over").
- **Table, export, overview.** "Show head 5 as a table" shows the rows behind
  the chart, "export this to parquet" writes them, and "what data is there?"
  (or the **data** button) lists every table with its columns, types and
  ranges. The chart is left as it is.
- **Stats for nerds** (Tab): the whole pipeline behind the chart, over it and
  see-through, with Jev's answers, the writer's tries and timings.
- **Queries in the pipeline box.** Plain SQL over the named tables
  (`SHOW TABLES`, `SELECT * FROM cells LIMIT 5`, `information_schema`), a
  text that ends in `head N`, or `overview`, show a table over the chart.
- **Copy session** puts the session on the clipboard and in
  `out/autopilot_live/session.txt`: per decision, the chart it saw, every answer
  of Jev's with its confidence, the cache files of the raw responses, what the
  writer wrote and what the view showed; a PNG of the view goes beside it.
  `--snapshot <dir> @@session.txt` replays it.

Text fields edit as text fields should: arrows, ⌥ and ⌘ with them, ⇧ to
select, double and triple click, dragging, and the clipboard.

![Stats for nerds after "use 10 m cells instead of 5 m"](images/writer_nerds.png)

## How a decision becomes a chart

```
typed text ─► Jev ─┬─ its options say it all ─────────────► pipeline lines ─┐
                   └─ otherwise: its reading as direction ─► Haiku writes ──┤
                                                                            ▼
                               validate every stage, fold to a chart state (or refuse, with the reason)
                                                                            ▼
                                                          keyed transition to the new chart
```

- **Jev answers typed questions**, all in one call: the action (`mark`,
  `color`, `zoom`, `highlight`, `title`, `transform`, `undo`, `reset`, …) and
  its argument (which mark, colour, quarter, subset), how to render (`chart`,
  `table`, `export`, `overview`), and whether the text carries specifics of its
  own that no option holds. The observation holds the chart's state and table
  sizes, never rows.
- **The route.** Jev's options apply when its confidence is at least 0.5, it
  saw no specifics, and the answer fits the chart. Otherwise Haiku gets the
  grammar, the tables and their pipelines, the chart now, the pipeline now,
  the instruction and Jev's chosen change, and writes the whole new pipeline.
  Haiku may also answer `overview` to a question about the data.
- **Nothing written is trusted.** The pipeline runs through the editor's
  path; a refusal goes back to Haiku with its reason, up to three tries.
- **While typing, only Jev decides**, with two gates: confidence ≥ 0.5, and
  ≥ 0.9 for a change of chart kind before the sentence is complete. What only
  Enter can do (a transform, undo, reset, a title, a table) is held and named.
- **Every response is cached**, keyed by a hash of the model, the observation
  and the questions (or the prompt). Everything here replays without a key
  once it has been asked, including the video.

## The pipeline

Data stages first, then one mark, then its properties. The marks use
Vega-Lite's names, with channels as `--channel field:type` (N, O, Q, T; a type
left out comes from the Arrow type). Aggregation is a `sql` stage before the
mark.

```
read out/layer/classes.parquet
! chart bar --x label:N --y n:Q --color label:N
! set x.axis.title "LiDAR class"
! set y.scale.type log
```

| Mark | Channels |
|---|---|
| `chart bar` | `--x f:N` (or O), `--y f:Q`, `--color` the same field as x or `#hex` |
| `chart arc` | `--theta f:Q --color f:N` |
| `chart line` | `--x f:Q` (or T), `--y f:Q`, `--color f:N` (or `--detail`) |
| `chart rect` | `--x f:O` (or Q), `--y f:N`, `--color f:Q` (viridis, log) |
| `chart point` | `--x f:Q --y f:Q --color f:Q` |

| Property | Where |
|---|---|
| `set x.axis.title "…"`, `set y.axis.title "…"` | all but arc |
| `set y.scale.type log` | bar |
| `set x.scale.domain a,b` (or y) | line and point; one axis keeps the other's extent |
| `title "…"`, `color #hex`, `highlight "datum.f >= n"`, `clear-highlight`, `zoom x0..x1 y0..y1`, `reset-zoom` | as the mark allows |

Data stages are `read <file>`, `filter --vega "…"`, `sql "… FROM input …"`;
`head N` ends a text that is a query. Experiment 6's short forms (`bars n --by
label`, `set x.title`) still work. What the layer cannot draw is refused, never
approximated: a `--size` channel on a point, `label:Q` on a bar's x,
`chart area`, `set x.axis.labelAngle`, a log scale on a map
(`layer_roundtrip` prints each case).

The tile and the four tables derived from it (`classes`, `flight`,
`class_height`, `cells`) are named tables in every session, with
`information_schema` on. A pipeline whose data stages are those of one of the
four tables reads the cached Parquet file instead of the tile: about 2 ms
instead of about 1 s.

![The overview](images/overview.png)

## The chart layer

`avenger-chart-definition` at #130 has two marks, rectangles and circle
symbols, each with one constant fill and no colour scale, so a pie, a line
per series and a heatmap cannot be described there. [src/layer/](src/layer/) is
a small chart layer on `avenger-scenegraph`, drawn by `avenger-wgpu`. From
`facet-fresh-start` it takes two ideas (marks separate from their coordinate
system; a line mark split into series by a key) and adds a **key from the data
on every item**, so two frames can be joined item by item:
- items with the same key move, resize and recolour;
- a heatmap cell grows out of its class bar, its parent, and collapses back;
- between Cartesian and polar, a stacked bar curls into a donut through
  experiment 5's `Bend`: first the layout, then the bend;
- a shared linear axis tweens its domain; guides and titles that change fade
  in sequence.

| File | What it does |
|---|---|
| `model.rs` | The chart state and `resolve(state, data)`: a frame of keyed items |
| `anim.rs` | `transition(a, b, t)` |
| `draw.rs` | Frames to marks, through `Bend` |
| `package.rs` | The `layer` pipeline package, decisions → pipeline lines, and the fold back to a state |
| `pilot.rs` | Jev's observation and questions, and its answers applied to the state |
| `writer.rs` | The extra questions, the writer's prompt, write → apply → retry |
| `editor.rs` | A pipeline text or query run on its own, drawn from its result |
| `catalog.rs` | The named tables and the overview |

## Results

The cases were written before any decider ran on them, with the expected
answers; later cases say so in the case file. Counts are small (one case is
4–5 %), so they are indicative. Every decision is in [results/](results/).

### Jev against rules and a general model

27 decisions on the first chart vocabulary ([cases.json](cases.json)): 19
typed instructions and 4 data changes under two policies.

| Decider | Instructions | Data changes | Latency p50 / p95 | Cost |
|---|---|---|---|---|
| rules | 13 / 19 | 8 / 8 (circular: the rules read the diff text the same code wrote) | 0 ms | $0 |
| Jev 1.13, confidence < 0.5 → no change | **19 / 19** | **7 / 8** | 290 / 680 ms | $0.0013 |
| Claude Haiku 4.5 | 19 / 19 | 6 / 8 | 1,114 / 2,315 ms | $0.033 |

- Rules fail on Dutch, paraphrase ("can we look closer at the lower right
  part") and "something calmer"; both models get them.
- Jev's confidence is informative: its wrong answers had 0.12–0.75, and
  below 0.5 as "no change" fixes four of five.
- Both models chose "highlight outliers" when two new categories appeared.
- A narrow policy ("always bars") held in every case, since `mark` is not
  among its options; a free one chose a map when coordinates and a geometry
  column appeared (Jev 0.83).
- An observation must be reproducible: approximate percentiles changed with
  batch order, and with them the cache key; exact `percentile_cont` fixed it.

On the chart layer's vocabulary (five marks, colour, zoom, emphasis;
[cases_layer.json](cases_layer.json)): Jev 24/24 at 278 ms p50, Haiku 24/24 at
1,140 ms, rules 16/24. Jev also reads indirect questions: "which share does
each class have?" is a pie, "where are the buildings?" a map.

### While typing

Deciding after every word ([results/typing.md](results/typing.md)): a
confidence threshold removes about a third of Jev's changes, but not a
confident wrong intermediate ("lower right" is south-west at "lower", 0.96).
Debouncing (400 ms) and the 0.9 gate for a change of chart kind take care of
those: over 12 instructions, one change each, all right, several before the
sentence ends ("which share" → pie, 0.98). Jev is not fully deterministic:
asked again, some confidences moved by 0.01. The cache makes the runs
reproducible.

### Jev steers, Haiku writes

22 cases ([cases_writer.json](cases_writer.json)), each with the state it
must fold to and a reference pipeline that `writer_eval --check` proves
reachable: 6 that Jev's options cover, 16 that need text of their own (titles,
thresholds, ranges, cell sizes, filters).

| Way | Covered by options | Own text | Writer calls | Accepted first try | Latency p50 | Cost |
|---|---|---|---|---|---|---|
| Jev's options only | 6 / 6 | 2 / 16 | 0 | – | 302 ms | $0.0013 |
| Haiku writes alone | 5 / 6 | 15 / 16 | 22 | 21 / 22 | 1,639 ms | $0.071 |
| routed (the window) | **6 / 6** | **16 / 16** | 18 | 17 / 18 | 1,651 ms | $0.060 |

- Haiku alone drew "which share does each class have?" as bars, and "draw
  the buildings as a 3D model" as a map of the raw tile; with Jev's reading
  neither happened.
- Jev's `render` answer was right in 21 of 21 cases; its action missed three
  of those (a table asked as `mark/keep`), which the route does not need.
- The prompt matters as much as the model: a palette written as
  `green #59a14f` gave `color green`, and "the top left" of a time series an
  arbitrary range, until the prompt said "the hex, not the name" and listed the
  quarters. Refusals that name the problem are what let Haiku fix a try.
- Earlier rounds of the prompt are kept in `results/writer_v*.md`.

Undo and reset, on 13 cases including four lookalikes ("reset the zoom",
"remove the emphasis", "go back to the bar chart"): 13/13.

### Cost

560 decisions (482 by Jev, 78 by Haiku as a decider) and 277 written
pipelines were made to build and measure this: $0.91 in all, from the cache
files.

## Feedback for Avenger

What building this on the stack taught us, for [FINDINGS.md](../../FINDINGS.md)
and Jon. Each point says where it comes from; the unchecked ones say so.

1. **Chart definitions cannot describe most charts people ask for.** At #130,
   `RectEncoding` and `SymbolEncoding` take one constant `fill: String`, and
   scales are linear, band or point. A pie, a line per series, a heatmap with a
   colour scale and a log axis all had to be drawn on the scenegraph directly,
   although `avenger-scales` has log, pow and symlog. A field-driven colour
   channel with a scale, and line and arc marks, would have covered all five
   charts here.
2. **Items need keys for charts to morph.** The one thing our layer adds that
   makes it feel alive is a key from the data on every item, so a frame can
   be joined to the next: bars curl into a pie, split into a heatmap, and come
   back. `avenger-chart` has an identity per plot, not per item, and its
   retained symbols reuse positions within one mark across a domain change. A
   key channel (Vega's `key`), carried into the rendered frame, would let a
   host animate between any two frames.
3. **Publish the vocabulary as data.** A classifier needs the option list, and
   a writer needs the grammar. Both were written by hand here, from the marks,
   channels, types and properties the layer accepts. If a chart definition
   could list what it accepts (marks, channels with allowed types, properties
   per channel), deciders, editors and prompts could be generated from it,
   and could not drift from what the renderer draws.
4. **Refuse, with a path and a reason.** The loop is safe because anything the
   layer cannot draw is an error that names the problem ("chart point: the layer
   has no `size` channel for this mark; it takes x, y, color"). Haiku fixed
   refused tries from that text alone: with the first prompt, four colours
   written as names were each right on the next try, and in the last round
   the one refused try was too. The Vega-Lite compiler's
   `CompileError::path()` goes this way; the same from
   `ChartDefinition::finish()` for unsupported properties, rather than
   dropping them, would make generated charts trustworthy.
5. **What worked well, and should stay.** `RuntimeHostCommand::RequestWakeup`
   with `RuntimeWake` events let decisions, writes and queries run in the
   background without the window ever waiting. `WriteClipboard`, `MouseUp`
   and `CursorMoved` were all that text fields needed. And the same
   `SceneGraph` rendered headlessly by `PngCanvas` made `--snapshot`, the
   recordings and the video possible: the tour rebuilds from the cache frame
   for frame, without a key.
6. **Text measurement per glyph.** Caret, selection and wrapping need the
   advance of each character. We measure `|c|` minus `||` with
   `measure_bounds` and cache it, which works but is a workaround. Not
   checked: whether `avenger-widgets`' `text_input.rs` (#117) would replace
   our fields.
7. **Gradients still draw flat** (finding 4), so the heatmap's colour legend
   is drawn as stacked rectangles.

## Not built, not checked

- A training run as the data source (phase G); data events (a new column, an
  outlier) are measured on the first vocabulary but not wired to the window.
- Other writer models; instructions that change the data beyond filters and
  cell size; Jev's `score` question type ("how well does this chart serve the
  policy?").
- A mark change resets the title and the scale and axis properties to the
  new mark's defaults; they do not travel with the chart.
- Registering the named tables costs every new pipeline a little:
  `layer_roundtrip`, which builds thousands, went from 3.9 s to 10.1 s.

## Open questions

- Should a chart library expose its vocabulary *as* the option list, so any
  decider (Jev, an LLM, a UI) drives it through the same typed surface?
- Is "on track for a policy" a score rather than a choice?
- How much of this belongs in Avenger, and how much in an application on top?

## Reference

```sh
cargo run --release -p lidar-decide --bin decide_eval       # Jev, rules and Haiku on cases.json
cargo run --release -p lidar-decide --bin typing            # deciding after every word
cargo run --release -p lidar-decide --bin layer_eval        # the chart layer's vocabulary
cargo run --release -p lidar-decide --bin writer_eval       # Jev steers, Haiku writes; --check: references only
cargo run --release -p lidar-decide --bin layer_roundtrip   # decisions ↔ pipeline, and what is refused
cargo run --release -p lidar-decide --bin autopilot_live -- --snapshot <dir> "instruction" … | @file | @@session.txt
cargo run --release -p lidar-decide --bin autopilot_live -- --tour out/autopilot_live/tour
ffmpeg -framerate 30 -i out/autopilot_live/tour/f%05d.png -c:v libx264 -preset slow \
    -pix_fmt yuv420p -crf 24 -movflags +faststart experiments/07-chart-decisions/video/autopilot_tour.mp4
cargo test --release -p lidar-decide --bin autopilot_live   # text editing
```

`--record` plays the first recording's script ([video/autopilot.mp4](video/autopilot.mp4)),
without the writer. `--no-writer` runs the window without it; `--neutral` gives
the recordings' look. `AUTOPILOT_DEBUG=1` logs input events.

| File | What it holds |
|---|---|
| [src/deciders.rs](src/deciders.rs) | `Rules`, `Jev` (OpenRouter Decisions endpoint), `Llm` and `Writer` (OpenRouter chat), the cache |
| [src/observe.rs](src/observe.rs), [src/options.rs](src/options.rs) | The first vocabulary: column statistics, the observation, the questions per policy |
| [src/layer/](src/layer/) | The chart layer, see above |
| [src/bin/autopilot_live.rs](src/bin/autopilot_live.rs) | The window, snapshots, recordings |
| [cases.json](cases.json), [cases_layer.json](cases_layer.json), [cases_writer.json](cases_writer.json) | The cases, with expected answers |
| [cache/](cache/) | Every response, by hash |
| [results/](results/) | Every decision, and the tables above |
