# Experiment 7 — charts driven by decisions

Status: phases A–F run, 24–25 Sep 2026, with Jev 1.13 and Claude Haiku 4.5 through OpenRouter. Phase G (a training run) is not built yet.

Can a chart be driven by a decider that picks from typed options, rather than
by a person writing commands? Two starting points:

- **A chart library designed to be driven by a typed classifier.**
  Classifiers such as [Jev](https://innfactory.ai/en/blog/jev-system-one-model-classifier-not-llm/)
  turn natural language into enums, quickly and cheaply. That makes chart
  edits possible while someone types.
- **A chart that follows a policy.** The chart keeps its current and previous
  state. It evaluates whether it is on track for a *policy* by looking at the
  difference between states, and a decider picks the visual that suits that
  change. The policy can be narrow ("whatever changes, always a bar chart") or
  free (connected to a database, a new geometry column appears, so the decider
  chooses a map).

## Why now

Experiment 6 ended on the question this depends on. Option 2 there replaced
setters with task commands (`bars`, `zoom`, `highlight`, …), and its notes ask
how large that vocabulary has to be: Vega-Lite v5 has 2,423 property
slots. A decider that picks from options needs exactly that vocabulary as its
option list. This experiment puts the vocabulary under real use instead of
arguing about it.

## What Jev is

Checked from public write-ups, not tested yet:

- A non-autoregressive "system one" model from TypeSafe AI. It reads text and
  answers typed questions in one pass instead of generating tokens.
- Three question types: **choice** (from options you supply), **score** (on
  a scale you define) and **binary** (a probability that a statement is true).
- The classes are described in natural language at call time, with no
  training.
- Latency is roughly 70–500 ms, at 4.2 cents per million input tokens; output
  is free. Several questions about one state are answered together in one
  call.

Sources: [innFactory](https://innfactory.ai/en/blog/jev-system-one-model-classifier-not-llm/),
[MindStudio](https://www.mindstudio.ai/blog/jev-system-one-model-classification),
[MindStudio benchmarks](https://www.mindstudio.ai/blog/jev-vs-classic-classifiers-benchmark),
[Nexus Agent](https://agent.nexus/blog/jev-the-model-that-decides),
[awesome-jev](https://github.com/yibie/awesome-jev).

## How the idea maps onto what exists

| Idea | In the experiments so far |
|---|---|
| The chart holds its current and previous state | The CQRS command log; the chart state is a fold of it (experiment 6) |
| Evaluate the difference with the last state | Two folds compared, plus a diff of the data's schema and statistics from the pipeline |
| A policy, narrow or free | The option list handed to the decider: one allowed chart, or the full vocabulary |
| A geometry column appears, so a map | Choosing a coordinate system: a `project` verb over experiment 5's systems |
| A wrong choice must not break the chart | Tasks are validated against the data before they are accepted (experiment 6) |
| Describe the state to the decider | The `describe` read model (experiment 6), extended with data statistics |

## Design

```
data change ─┐
typed text ──┼─► observation ─► decider ─► proposed command ─► task validation ─► log ─► fold ─► render
policy ──────┘   (compact JSON)  (rules │ Jev │ LLM)            (rejects, or accepts)
```

The decider proposes; the existing task validation decides whether a proposal
is accepted. A decider never writes the chart state directly.

- **Observation.** A compact document, with no raw data:
  - the current and previous `describe` of the chart, and the difference;
  - per column of the data: type, cardinality, null share, min/max, and
    whether it is a geometry, a time or a category;
  - the policy, as text;
  - the options the decider may pick from.
- **Policy as a constraint on options.** A narrow policy offers one mark
  (`bars`) plus styling. A free policy offers every mark, `project` and
  `facet`. The policy decides which enums exist at all, so a decider cannot
  wander outside it.
- **A `decide` step.** A pipeline step of kind Command that builds the
  observation, asks a decider and emits the chosen commands through normal
  validation. It works in the CLI, in the live viewer (as an autopilot), and in
  replay.
- **The log stays deterministic.** The log records the accepted command, not
  the sentence or the diff that led to it. A chart made by a non-deterministic
  decider is still replayed exactly (experiment 6's byte-identical checks
  apply unchanged). The observation and the decision are logged beside it, as
  provenance.

## Considerations

1. **Enums cannot carry free arguments.** Jev returns a choice, not text.
   "Zoom to the north-east" is fine if a named region exists; `zoom
   657500..658000` is not something a classifier produces. Arguments must be
   enumerable:
   - fields from the schema;
   - named palettes and colours;
   - named extents (quarters, a selected feature, "fit the data");
   - angles in steps.

   Anything else needs a second step, such as an LLM or a parser for
   numbers in the text. This constraint shapes the vocabulary more than
   anything else: named things over free values.
2. **The vocabulary comes first.** The noun/verb question from experiment 6's
   notes decides the option lists. It covers styling nouns derived from a
   schema, a few intent verbs (`zoom`, `highlight`, `project`, `facet`) and
   patches as the escape hatch. Which of these can be offered to a classifier
   at all? Noun commands with 78 axis options are a large choice question;
   perhaps the answer is questions in two stages (which object, then which
   property).
3. **Stability while typing.** Deciding on every keystroke can make the chart
   flip back and forth. Needed: a confidence threshold (Jev returns
   probabilities), hysteresis (do not switch unless the new choice wins by a
   margin), and debouncing. The flip rate on partial input is a measurement,
   not an afterthought.
4. **When not to act.** Most data changes need no chart change (more rows of
   the same kind). "No change" must be an explicit option, and the most
   common correct answer.
5. **A hosted service.** Jev (and a hosted LLM) means a network call per
   decision, an API key, and data leaving the machine. The key is read from an
   environment variable that the user sets. The observation holds schema and
   summary statistics only, never the LiDAR points. Responses are cached by a
   hash of the observation, so reruns are reproducible and free.
6. **Baselines keep it honest.** A rule-based decider (a geometry column
   means a map, a time column a line, low cardinality means bars) is the floor.
   If Jev does not beat it on the cases that need language, the measurement
   says so.
7. **Not tested yet.** Neither this repository nor the write-ups above have used Jev for
   chart decisions. The latency and cost figures are TypeSafe's and
   reviewers'; they get re-measured here.

## Plan

A crate `lidar-decide` in `experiments/07-chart-decisions`. It builds on
experiment 6's pipeline and chart package, at the same Avenger revision
(#129, `a2241265`).

| Phase | Builds | Measures |
|---|---|---|
| A. Vocabulary as options | Option lists derived from experiment 6's task commands, with enumerable arguments (fields, named extents, palettes, coordinate systems), and the policy as a filter on them | How many options each policy offers; which intents cannot be expressed as an enum |
| B. Observation | `describe` + previous state + diff, and column statistics from DataFusion | Size in tokens per observation; that no raw data leaves the machine |
| C. Deciders | Rules baseline, Jev (choice questions, batched), and a general LLM, behind one `Decider` trait with a response cache | Accuracy against expected commands, latency p50/p95, cost per decision, and rejection rate by task validation |
| D. Cases | Typed instructions ("make the bars red", "only buildings", "zoom to the north-east", "as a map") and data changes on the tile (a geometry column via `ST_Point`, a time column, an outlier, a new class, more rows of the same kind) | Per case: expected command, chosen command, confidence, time |
| E. While typing | The prefixes of each instruction, decided in turn with threshold and hysteresis | Flips per instruction; delay before the right chart appears |
| F. Policies | The same cases under a narrow policy ("always bars") and a free one | Whether the narrow policy holds under every case, and what the free one chooses when a geometry column appears |

Phase G, a training run as the data source, is described under
[Advanced](#advanced-a-training-run-as-the-data-source).

Deliverables, as in experiments 5 and 6: a README with measured results, a
`decide` step in the pipeline, an autopilot switch in a live view, and a
video of a chart following typed text and data changes.

## Run

```sh
set -a; source <folder with your .env>/.env; set +a    # OPENROUTER_API_KEY
cargo run --release -p lidar-decide --bin decide_eval   # phases C, D, F
cargo run --release -p lidar-decide --bin typing        # phase E
```

Every response is cached in [cache/](cache/), keyed by a hash of the decider,
the observation and the questions. Both binaries replay byte-identically
from the cache **without** an API key; only a new or changed case calls the
API. All 108 cached decisions together cost $0.069.

| File | What it holds |
|---|---|
| [src/observe.rs](src/observe.rs) | Column statistics and roles, the data description, the diff, the observation |
| [src/options.rs](src/options.rs) | The questions per policy, and answers → experiment 6 task commands |
| [src/deciders.rs](src/deciders.rs) | `Rules`, `Jev` (OpenRouter Decisions endpoint), `Llm` (OpenRouter chat), the cache |
| [cases.json](cases.json) | 19 instructions and 4 data changes, with expected answers written before any decider ran |
| [results/](results/) | Every decision (`decisions.json`) and the cache files each run used |

## Results

### A. The vocabulary as options

One decision is one call with all questions at once, as Jev bills and
answers them.

| Question | Options (free policy) | Options (narrow policy) |
|---|---|---|
| action | no_change, mark, color, zoom, rotate_labels, highlight, title | the same without mark |
| mark | bars, points, map, keep | – |
| colour | red, blue, orange, green, grey, keep | same |
| region | north_east, north_west, south_east, south_west, all, keep | same |
| angle | tilt, vertical, keep | same |
| subset | top_10, outliers, keep | same |
| **total** | **6 questions, 29 options** | **5 questions, 25 options** |

The decider only chooses intent. Arguments are derived from the data:
- the fields for a new mark come from column roles (a category and a measure
  for bars, two coordinates for a map);
- zoom ranges come from the data's extent;
- highlight thresholds come from the 90th and 99.9th percentiles.

**A title cannot be expressed:** every decider recognised `title` when asked,
and the translation stops there ("needs free text"). The policy is part of the
questions themselves: under the narrow policy `mark` does not exist, so no
decider can change the kind of chart.

### B. The observation

The observation holds the policy, the chart's `describe`, one line per data
column and, where relevant, the data change or the typed instruction. That is
~1,100 input tokens for Jev and ~900 for Haiku, with no raw rows.

For example, the column list for the map:

```
11747 rows. Columns:
- cx (Float64, coordinate): 201 distinct, range 657000..658000
- cy (Float64, coordinate): 201 distinct, range 6867000..6868000
- h (Float64, measure): 1814 distinct, range 45.53..93.65
```

**An observation must be reproducible.** DataFusion's approximate percentile
varies with batch order. With a `UNION ALL` it changed the "extreme value"
line of d03 between runs, and with it the cache key. Exact `percentile_cont`
fixed that. Without a reproducible observation, cached and replayed decisions
drift.

### C–D. Decisions

19 typed instructions on two charts (the height histogram and the buildings
map), including two in Dutch. There are also 4 data changes, each under both
policies. The d03 outlier is one synthetic cell with h = 400 m, added by
`UNION ALL`.

| Decider | Instructions right | Data changes right | Latency p50 / p95 | Cost, 27 decisions |
|---|---|---|---|---|
| rules | 13 / 19 | 8 / 8 (circular, see below) | 0 ms | $0 |
| Jev 1.13 | 18 / 19 | 4 / 8 | 290 / 680 ms | $0.0013 |
| Jev 1.13, confidence < 0.5 → no change | **19 / 19** | **7 / 8** | same | same |
| Claude Haiku 4.5 | 19 / 19 | 6 / 8 | 1,114 / 2,315 ms | $0.033 |

- **Language.** The rules fail on Dutch ("maak de balken rood", "inzoomen op
  het zuidwesten"), on paraphrase ("can we look closer at the lower right
  part", "put the labels upright") and on "something calmer". Both models get
  all of these.
- **Jev's confidence is informative.** Its five wrong answers had confidence
  0.12, 0.26, 0.23, 0.39 and 0.75. Treating anything below 0.5 as "no change"
  fixes four of them without losing a right answer; at 0.6 two right answers
  (0.51, 0.57) are lost.
- **Both models share one failure.** When two new height categories appear
  (d04, free policy), Jev (0.75) and Haiku both choose "highlight outliers".
  New categories are not outliers; the case expects no change.
- **"Doing nothing" is the hard answer for Jev.** For "more rows of the same
  kind" it chose `title` (0.12) and `zoom` with no region (0.26). With the
  threshold both become no change.
- **The rules' 8/8 on data changes measures nothing.** The same code wrote
  the diff text ("new column … (coordinate)", "an extreme value … appeared")
  that the rules match. They are the floor for language, not for data.
- **Cost and speed.** Jev is 26 times cheaper and 3.8 times faster (median)
  than Haiku for the same questions.
- **Validation.** No chosen command was rejected by experiment 6's task
  validation. Some were incomplete (zoom with no region, "unfit"), and titles
  "need free text".

![Charts after Jev's decisions](images/decisions.png)

The gallery ([images/cases/](images/cases/)) also shows what the observation
lacks:
- **"Something calmer" had no visible effect.** Jev chose blue, which the chart
  already was: the observation does not say which colour is current.
- **After d02 turned the scatter into a map, the old title stayed:**
  "height against point count". A mark change should revisit dependent state.
- **The highlighted outlier is one cell of normal size (d03).** Colour alone
  is not enough emphasis.

### E. While typing

Six instructions, decided after every word. A "chart change" counts only
decisions that become a valid command. `zoom` with no region yet changes
nothing.

| Decider | Chart changes over 6 sentences | With confidence ≥ 0.6 | Right at the end |
|---|---|---|---|
| rules | 4 | 4 | 3 of 6 |
| Jev | 14 | 9 | 6 of 6 |
| Haiku | 11 | 11 (no confidence given) | 6 of 6 |

Typical trails (Jev, with confidence):
- "make … the … bars … red": highlight/outliers (0.46) → … → color/red (0.99).
  The threshold removes the early guess, and the chart changes once.
- "can we look closer at the lower right part": after "lower", Jev settles on
  zoom/south_west with 0.96 confidence, and on south_east (1.00) after "right".
  A confident wrong intermediate that no threshold catches; only waiting for a
  pause in typing (debouncing) would.
- "highlight the tallest buildings": after "tallest", both models choose
  outliers, then top_10 after "buildings". The subset flips once, at high
  confidence.

A confidence threshold removes about a third of Jev's changes, but not the
confident wrong intermediates. Deciding while typing needs both a threshold
and debouncing.

### F. Policies

- **The narrow policy held in every case.** "turn this into a scatter plot"
  (i19) and "a geometry column appears" (d02) led every decider to no change,
  because `mark` is not among the options.
- **The free policy chose a map when coordinates and a geometry column
  appeared (d02).** All three deciders did this, Jev with 0.83 confidence.
  That is the "database gets a geometry column" scenario from the plan.
- **Under both policies, an outlier led to highlighting it (d03).** Styling is
  allowed under the narrow policy too.

## What it takes

1. **The option list is the interface.** A typed classifier drives a chart as
   well as a general model does on instructions (19/19 each with a
   threshold), for 4 % of the cost and a quarter of the latency. The work lies
   in the vocabulary: intent as enums, arguments derived from the data.
2. **Confidence is part of the contract.** Jev's probabilities turn "not
   sure" into "no change". Haiku gives none; a model without a calibrated
   confidence needs another way to abstain.
3. **The observation decides as much as the decider.** Missing style state
   (the current colour), dependent state (a title that no longer fits) and
   non-reproducible statistics all showed up as wrong or invisible decisions.
4. **Free text needs a second route.** Titles, predicates and precise ranges
   do not fit in enums.
5. **Typing needs damping at two levels:** a confidence threshold for
   uncertain guesses, and debouncing for confident wrong intermediates.

## Notes for the discussion

- **Deciding intent and deriving arguments is a clean split.** It keeps
  option lists small (29 options) while the chart still gets exact ranges and
  thresholds. A chart library could publish exactly that surface: the
  intents with their named options, and the rules that turn them into
  commands.
- **Jev's `score` question type was not used.** "How well does this chart
  serve the policy?" is a natural next step for the "on track" idea.
- **Not tested:** the video and live autopilot deliverables, phase G, other
  LLMs, and larger or noisier instruction sets. With 19 + 8 cases, one case is
  5 % (instructions) or 12.5 % (data changes); the counts are indicative.

## Advanced: a training run as the data source

Once phases A–F have measured the decisions on the LiDAR cases, the same
machinery can follow a reinforcement-learning training run. There, the state
changes at every step.

- **The data change is the run.** Each training step updates the
  observation: reward, loss, episode length, and statistics of the states the
  agent visits. The decider is asked whether the chart should change. Most of
  the time it should not, and the chart only takes the new data.
- **Regime changes become visible edits.** When something shifts (reward
  jumps, a new behaviour appears, the loss plateaus), the decider picks another
  view: zoom in, highlight the new episodes, switch from a line to a
  distribution. The chart shows not only the numbers moving, but what changed
  in them.
- **The command log is the video's timeline.** Every accepted command is a
  chart state. Folding the log up to entry *n* and rendering it gives frame
  *n*, headlessly, as for the experiment 5 and 6 videos. Because replay is
  deterministic, the video can be rebuilt from the log alone.

Considerations specific to this case:

- **Rate.** A training loop produces far more states than a video has frames,
  or a decider can follow. The chart works on aggregated windows, like the
  rolling aggregate states of experiment 3's live feed. The decider is asked
  per window, not per step, with the hysteresis of consideration 3 so that the
  view does not flicker.
- **Two meanings of "policy".** The agent being trained has a policy, and the
  chart follows a policy too. In this case the terms need distinct names; for
  example the *agent policy* and the *chart policy*.
- **The run as a source.** The run can be a logged file (for example a
  Parquet or Arrow log of steps) replayed through the pipeline, or a live
  stream over Arrow Flight as in experiment 3. A logged run keeps the
  experiment reproducible; a live one tests the rate.

| Phase | Builds | Measures |
|---|---|---|
| G. A training run | A step log of a small RL training run as the source; windowed aggregation; the decider per window; the video from the fold of the command log | Decisions per window and how many are "no change"; whether the regime changes that are visible in the metrics are the ones the chart reacts to; frame rate of the headless render |

## Open questions

- Should a chart library expose its vocabulary *as* the option list, so any
  decider (Jev, an LLM, a UI) drives it through the same typed surface?
- Where does the free text go (titles, predicates, numeric ranges) when the
  decider only returns enums?
- Is "on track for a policy" a score question (Jev's score type: how well does
  this chart serve the policy?) rather than a choice?
- How much of this belongs in Avenger, and how much in an application on top?
