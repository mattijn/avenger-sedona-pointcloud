# Experiment 7 — charts driven by decisions

Status: plan. No crate yet.

Can a chart be driven by a decider that picks from typed options, rather than
by a person writing commands? Two starting points from the discussion:

- **Jon:** [Jev](https://innfactory.ai/en/blog/jev-system-one-model-classifier-not-llm/)
  is a classifier that turns natural language into enums: fast, cheap, and
  typed. What would a chart library designed to be driven by it look like?
  Someone at Hex wired up a demo where chart edits happen while you type.
- **Mattijn:** the chart keeps its current and previous state. It evaluates
  whether it is on track for a *policy* by looking at the difference between
  states, and a decider picks the visual that suits that change. The policy can
  be narrow ("whatever changes, always a bar chart") or free (connected to a
  database, a new geometry column appears, so the decider chooses a map).

## Why now

Experiment 6 ended on the question this depends on. Option 2 there replaced
setters with task commands (`bars`, `zoom`, `highlight`, …), and the notes for
Jon ask how large that vocabulary has to be: Vega-Lite v5 has 2,423 property
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
7. **Not tested yet.** Neither we nor the write-ups above have used Jev for
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

Deliverables, as in experiments 5 and 6: a README with measured results, a
`decide` step in the pipeline, an autopilot switch in a live view, and a
video of a chart following typed text and data changes.

## Open questions for the discussion

- Should a chart library expose its vocabulary *as* the option list, so any
  decider (Jev, an LLM, a UI) drives it through the same typed surface?
- Where does the free text go (titles, predicates, numeric ranges) when the
  decider only returns enums?
- Is "on track for a policy" a score question (Jev's score type: how well does
  this chart serve the policy?) rather than a choice?
- How much of this belongs in Avenger, and how much in an application on top?
