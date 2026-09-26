# Findings for Avenger

What this repo has run into while building real charts on the Avenger stack,
kept as a ledger so it can be rechecked when the stack moves. Every entry says
how it was measured, so a recheck is a command rather than an opinion.

- **Checked against:** `jonmmease/avenger` `f4890be` ([#130](https://github.com/jonmmease/avenger/pull/130), `codex/portable-dataflow-inputs`, the top of the stack after the rebase of 26 Sep 2026, 02:57 CEST), on 26 Sep 2026.
  Previous rounds: `3065e2a` (#130, rebased 25 Sep 19:17 CEST); `602b99c` (#130), 25 Sep 2026; `5f31c58` (#124), 20 Sep 2026.
- **Machine:** Apple Silicon, macOS, wgpu/Metal, Rust 1.89
- **Recheck:** `cargo run --release -p lidar-probes --bin probe_guides` and
  `cargo run --release -p lidar-probes --bin probe_render` print everything
  below except the frame-rate numbers, which come from the streaming viewer
  (`cargo run --release -p lidar-stream --bin stream_live`; see
  [Live charts](#live-charts)).

| # | Area | Finding | Status |
|---|---|---|---|
| 1 | app / geometry | Rebuilding the geometry index dominates data-driven frames | open |
| 2 | app / geometry | `interactive: false` does not avoid that cost | open |
| 3 | winit | Invalidation-driven rebuilds always rebuild geometry | open, by design? |
| 4 | wgpu | Gradient fills do not draw, or draw flat | open |
| 5 | guides | Colorbar ignores its `origin` | open, documented in source |
| 6 | guides | Symbol legend title is placed inside the plot | open |
| 7 | scales | Linear domains must have exactly two stops | open |
| 8 | guides | Axis always set a `band` option | fixed in the stack |
| 9 | format | `NumberFormatContext` removed from `avenger-format-number` | API change, adapted |
| 10 | chart-definition | One constant fill per mark, no colour scale, no line or arc | open, suggestion |
| 11 | chart / chart-definition | No per-item key, so frames cannot be joined for transitions | open, suggestion |
| 12 | chart-definition | The accepted vocabulary is not available as data | open, suggestion |
| 13 | chart-definition | Unsupported properties should be errors with a path and a reason | not checked, suggestion |
| 14 | text / guides | Number formatting must now be configured, and its absence fails at run time | open, adapted |
| 15 | format | The format crates split into `avenger-format` and `*-d3`; two signatures changed | API change, adapted |

Findings 1–7 were re-measured on `602b99c` with the two probes and are
unchanged. The frame-rate numbers under [Live charts](#live-charts) were **not**
re-measured this round, because they need the live window.

## Live charts

The case that produced findings 1–3: a LiDAR feed arriving over Arrow Flight,
aggregated per batch and merged per frame, with the feed thread asking for a
redraw through `RenderInvalidationHub`. Measured with `stream_live`:

| Cells in the frame | Frames per second | Scene build (incl. DataFusion merge) |
|---|---|---|
| ~156k | 4.3 | 12 ms |
| ~40k | 10.8 | 6.5 ms |

Building the scene was never the problem: 12 ms of a 235 ms frame.

### 1. The geometry index is the frame budget

`probes/src/bin/probe_render.rs` builds one symbol mark of *n* instances and times the pieces
separately. Best of three, canvas 900 × 900, scale 1.0:

| Symbols | `set_scene` | `render` | `SceneGraphRTree::from_scene_graph` |
|---|---|---|---|
| 10,000 | 0.8 ms | 4.7 ms | 8.7 ms |
| 50,000 | 1.5 ms | 7.9 ms | 56.3 ms |
| 150,000 | 3.6 ms | 13.9 ms | 181.3 ms |
| 300,000 | 6.7 ms | 21.3 ms | 387.2 ms |

On `602b99c` (25 Sep 2026, single run): 9.5, 57.1, 198.3 and 413.6 ms for the
index, `render` 4.6–20.1 ms. Same shape, within 10 %.

Drawing 300k symbols costs 28 ms; indexing them costs 387 ms, about 1.2 µs per
instance. That matches the live viewer exactly: ~156k cells, ~235 ms per frame.

So the ceiling on an animated or streaming chart is currently the hit-test
index, not the GPU.

### 2. `interactive: false` does not avoid it

The same scene with the enclosing group marked non-interactive:

| Symbols | interactive | `interactive: false` |
|---|---|---|
| 150,000 | 181.3 ms | 177.0 ms |
| 300,000 | 387.2 ms | 365.5 ms |

Unchanged on `602b99c`: 198.3 vs 200.8 ms and 413.6 vs 416.8 ms.

The flag appears to affect what a query returns rather than what gets built, so
a mark that can never be hit still pays for its geometry. (`avenger-geometry`'s
own test is named `noninteractive_marks_are_excluded_from_scene_graph_rtree_but_not_bounds`,
which fits what we measure.)

### 3. Invalidation-driven rebuilds always rebuild geometry

`avenger-winit-wgpu/src/render_invalidation.rs` calls
`rebuild_scene_graph(true)` for an `EvaluationChanged` invalidation, so every
push from a data thread pays finding 1, even when only colours or positions
changed and nothing will be hit-tested before the next push. Still the case on
`602b99c` (read in the source, lines 121 and 163).

**New upstream, not yet measured by us:** `avenger-chart` now has *retained
symbols*. When both positions bind fields through unclamped linear scales, a
domain change (pan, zoom) reuses the base position arrays and, in wgpu, the
instance buffers; `frame.geometry_report()` records builds and reuses. Its
README says a *source snapshot* change still rebuilds positions, so a stream
of new data would not benefit, and it does not say whether the hit-test index
is rebuilt. Measuring that is the next thing to do for findings 1–3.

Questions rather than prescriptions, since the design intent may be deliberate:

- Could the invalidation say whether geometry changed, the way `UpdateStatus`
  does for event-driven updates?
- Could `interactive: false` skip instance geometry construction, not only
  index membership?
- Could the index be built lazily on the first hit-test after a rebuild, so a
  stream that never hit-tests never pays?

**What we did instead:** roll the retained aggregate states up to a coarser
grid whenever the window holds more than ~55k cells, which keeps the mark count
bounded and took the viewer from 3 to 11 fps. That is a fine workaround for a
map, but it trades detail for frames, and it is not available to a chart whose
marks are the data.

### 4. Gradient fills do not draw, or draw flat

Two symptoms, both reproducible:

- **Minimal scene, nothing drawn.** A `SceneRectMark` with
  `fill: GradientIndex(0)`, the gradient declared on its parent group, renders
  nothing at all; the identical rect with a solid fill renders. Tried with
  `x1` of 0.25, 1, 180 and 360 (to cover both normalized and pixel
  conventions), at canvas scale 1.0 and 2.0, and with the gradient on the
  direct parent and on a grandparent group. `probe_render`, section 1.
- **Colorbar, flat last stop.** `make_colorbar_marks` produces a proper ramp in
  the scene — the scale's stops are correct, we print them — but the rendered
  bar is a single colour, `#ECF8B1`, which is the last stop of the range.
  `probe_guides`, then sample the bar's pixels in `out/probe_guides.png`.

Both still present on `602b99c`: every probe case is flat, and the colorbar in
`out/probe_guides.png` is still one colour, `#ECF8B1`.

Both look like the gradient atlas or its texture coordinates in
`avenger-wgpu/src/marks/gradient.rs`, but we have not chased it further. Our
charts draw colorbars as stacked solid rects instead.

## Guides and scales

Re-verified on #124 and again on `602b99c` (#130) with `probe_guides`; all three are still present, and all
three still need a workaround in this repo's charts.

### 5. Colorbar ignores its `origin`

`make_colorbar_marks(scale, title, [320.0, 10.0], config)` returns a group at
`[0, 0]`, so the caller must wrap it to place it. The parameter is marked dead
in the source: `_origin: [f32; 2], // Unused - we always start at (0, 0) now`.
If that is now the intended contract, dropping the parameter would say so more
clearly than ignoring it.

### 6. Symbol legend title sits inside the plot

For a 300-wide plot, entries are placed correctly at x = 308 while the title is
drawn at x = 8, on top of the chart. Our charts pass `title: None` and draw the
title themselves.

### 7. Linear domains must have exactly two stops

`numeric_interval_domain` errors when `domain.len() != 2`, so a piecewise
colour ramp with uneven stops (a common need for elevation or intensity) cannot
be expressed; ranges have to be resampled to evenly spaced stops first.

### 8. Fixed: axis `band` option

`make_numeric_axis_marks` used to set a `band` option unconditionally, which
`LinearScale` rejected. It now sets it only when the scale has one. Kept here
so the ledger records the fix.

### 9. `NumberFormatContext` removed

`PreparedNumberFormat::new(spec, overrides, NumberFormatContext::new(&locale))`
became `PreparedNumberFormat::new(spec, overrides, &locale)` somewhere in the
formatting rework (`5d1518f` … `e86810b`). One call site in experiment 6
(`experiments/06-pipelines/src/packages/vega_format.rs`), a one-line change.
Recorded as drift, not as a defect: the new signature is simpler.

## From the repin to `3065e2a`

### 14. Number formatting fails at run time when not configured

After #139 ("require explicit number formatter setup"), the workspace built
with no warning about it, and then seven binaries failed when they drew an
axis: `InvalidAxisLabelFormat("number formatting is not configured")`
(probe_guides, cross_section, topviews, explorer, cqrs_check, stream_live;
explorer and stream_live as panics). The short entry points
`make_numeric_axis_marks`, `make_colorbar_marks` and `ChartOptions::default()`
use `default_text_engine()`, which has no formatter. Upstream's own examples
build one: a `NumberFormatRegistry` with `D3NumberFormatProvider`, and
`default_text_engine().with_number_formatting(NumberFormatConfig::new("d3"), …)`.
This repo now does the same once, in `lidar_common::text_engine()`, with
wrappers of the same names.

The suggestion: an entry point that compiles and cannot draw a number is a
trap. Either `default_text_engine()` could come with D3 formatting, or the
short entry points could take the engine, so the requirement shows at compile
time. Measured by running every binary in the cycle.

### 15. The format crates split

`avenger-format-number` and `avenger-format-datetime` became `avenger-format`
(the traits) plus `avenger-format-number-d3` and `avenger-format-datetime-d3`.
For this repo: the crate names;
`PreparedDateTimeFormat::format_zoned` now returns `Result<String, _>` instead
of a value with `.text`; and `TextMeasurementConfig`'s five locale fields became
`number_format` and `datetime_format`. Three small changes, found by the
compiler.

All other findings were re-measured on `3065e2a` and are unchanged: the index
costs 392 ms for 300k symbols (`interactive: false` the same), gradients draw
flat, and the guide pitfalls remain.

## From the repin to `f4890be`, and Altair on Avenger

Findings 1–7 re-measured on `f4890be` are unchanged: the geometry index
costs 383 ms for 300k symbols (392 ms before; `interactive: false` the
same), gradients draw flat, and the three guide pitfalls remain.

### 17. Number formatting moved again, and scales now carry it

Three changes, found by the compiler. `avenger_text::NumberFormatRegistry`
and `NumberFormatConfig` are gone; the idiom is
`ScaleFormatting::d3(Default::default(), Default::default())`, applied to a
text engine with `configure_text_engine` and to chart options with
`ChartOptions::with_formatting` (which also gained a `scale_formatting`
field). `PreparedNumberFormat::new` and `PreparedDateTimeFormat::new` each
lost one argument. And a scale now needs its own `with_formatting`: a
colorbar built with a formatted text engine but an unformatted scale fails
with `InvalidAxisLabelFormat("number formatting is not configured")`
(`probe_guides` did, until `lidar_common` formatted the scale too). Two
places must now agree on formatting for one axis. **Measure:**
`cargo run --release -p lidar-probes --bin probe_guides`.

### 18. The Vega-Lite front end fails on axis labels with default options

`Chart::from_vegalite(&spec, &datasets, VegaLiteOptions::default())`, then
`render`, fails for Altair's plain bar chart with "Invalid axis label format:
number formatting is not configured"; with
`VegaLiteOptions { chart: ChartOptions::default().with_formatting(ScaleFormatting::d3(..)), .. }`
it draws. This is finding 14 reached through the Vega-Lite front end; a
default that formats numbers the way Vega-Lite does would suit a Vega-Lite
compiler. The compiler README's example uses the default options; that
example itself was not run here. **Measure:** in
`crates/avenger-altair/src/lib.rs`, drop the `chart:` option and run
`avenger-altair render <spec> out.png`.

### 19. Every Altair chart carries `config`, which the spec types refuse

Altair's default theme adds `"config": {"view": {"continuousWidth": 300,
"continuousHeight": 300}}` to every chart, and `avenger-vegalite-spec`
refuses the key (`config: unknown field`), so no Altair chart passes as
written. `avenger-altair` writes that one config out as Vega-Lite applies it
(a width or height of 300 for a continuous axis) and leaves any other config
to be refused. Reading `config.view` (and ignoring, or reporting, the rest)
would let Altair's output through unchanged. **Measure:**
`python crates/avenger-altair/bench/coverage.py ~/vega/altair/tests/examples_methods_syntax`
(19 of 117 gallery examples stop at `config` even after the default theme is
written out).

### 20. SVG export spends 340 ms on the font

For an 8-bar chart, validating takes 0.05 ms, compiling 0.7–1.8 ms,
rendering 4–5 ms, PNG export 14 ms, and SVG export 341 ms. The SVG is 148 KB,
of which 139 KB is one embedded font, subset and encoded as WOFF2 in
`avenger-svg`'s `font_face_css`; the time is most likely there (not
profiled). An option to reference fonts instead of embedding them, or a
cached subset, would make SVG the cheap format it is in Vega. **Measure:**
`avenger-altair render <spec> out.svg 10` and `… out.png 10` print the
stages.

### 21. What Altair's gallery needs first

Of Altair's 120 gallery examples (`tests/examples_methods_syntax`), 117 run
here and none compiles to Avenger yet. The first refusal of each, with the
default theme written out and `config` set aside: `layer` 41, `mark.type`
41 (marks other than bar), `encoding.color` 8, composition (`vconcat`,
`hconcat`, `concat`, `facet`, `repeat`) 19, sorting by another channel
(`sort: "-x"`) 3, and one each of `joinaggregate`, `mark.binSpacing`,
`mark.cursor`, `data.format.parse`, `encoding.row`. By this measure the
order that opens the most of Altair is: other marks and layering, then
colour, then composition. The same script reruns it; `coverage.json` has
every example.

### 22. The materialisation charge counts shared buffers once per batch

`ExecutionConfig::max_materialized_bytes` (256 MB by default) is charged by
what active queries produce. For a Vega-Lite histogram (`bin`, `count()`) over
one float column, the smallest budget that draws grows roughly quadratically:
83 bytes a row at 30k rows, 216 at 100k, 628 at 300k, about 2,000 at 1M and
about 5,900 at 3M (17.7 GB, while the process peaks 121 MB above where it
started). So the default refuses a histogram over 1M rows ("active
materialization exceeds the 268435456 byte budget").

**Where.** `avenger-datafusion-dataflow/src/runtime.rs`, where each batch a
query streams is charged:

```rust
self.reservation.charge(
    batch.get_array_memory_size().saturating_add(std::mem::size_of::<RecordBatch>()),
)?;
```

`get_array_memory_size` counts the whole buffers a column refers to, and a
batch that is a slice of a larger array shares them. Logged per node (a
scratch copy with a counter at that line): at 1M rows every byte of the
1,968 MB is charged by one node, `__vl_bins_5`, the compiler's bin
assignment, whose 123 batches of about 8,192 rows each hold 0.26 MB and are
charged about 16 MB, the buffers of all 1M rows. Handing the input over in
8,192-row batches only shrinks it (524 MB at 1M, 4.5 GB at 3M): the bin
node's batches still share buffers of 3.6 to 7.6 MB each.

**Measured fix.** Charging each column's `to_data().get_slice_memory_size()`
instead makes the charge linear and exact: 3.2 MB at 100k rows, 32 MB at 1M,
97 MB at 3M, 32 bytes a row, which is the bin node's four 8-byte columns. The
change is [`crates/avenger-altair/bench/charge-slices.patch`](crates/avenger-altair/bench/charge-slices.patch)
(against `f4890be`, behind an environment variable, with the logging).
`materialize_partition` and `MaterializedValue::size` (`inputs.rs`) use
`get_array_memory_size` the same way and were not measured. Proposed as
[jonmmease/avenger#141](https://github.com/jonmmease/avenger/pull/141), on
`codex/datafusion-dataflow`, with a regression test in Avenger itself:
`cargo test -p avenger-datafusion-dataflow --test materialization_budget`
(a projection over one 100k-row batch, with a budget of four times the
column, fails with `ResourceExhausted` without the change and passes with
it; the crate's 134 tests pass).
**Measure:** `python crates/avenger-altair/bench/budget.py 30000 100000 300000 1000000`
bisects the smallest budget that draws, for JSON rows and for Arrow.

With the fix on `f4890be` (the Python bridge built against it), Avenger's
own 256 MB default draws the histogram over 1M and 3M rows, as JSON rows and
as Arrow, and the times are the same as with the budget raised (51 and 88 ms
with Arrow); the fix changes the accounting, not the work.
`LARGE_ALL=1 python crates/avenger-altair/bench/large.py 1000000 3000000`
with `AVENGER_MAX_MATERIALIZED_BYTES=268435456`.

With the budget raised, the same histogram over 1M values draws in 48 ms
from a pandas frame passed as Arrow, and over 3M in 83 ms (matplotlib's
`hist`: 37 and 52 ms; Altair with VegaFusion: 519 and 482 ms).

### 23. The spec types accept a JSON array where an object belongs

A derived serde `Deserialize` for a struct also accepts a sequence, with the
fields in declaration order, and `deny_unknown_fields` does not stop it. So
`UnitSpec::from_json` takes `"encoding": []` as an empty encoding, and
`"encoding": [{"field": "category", "type": "nominal"}, {"field": "amount", "type": "quantitative"}]`
as x and y, and the compiler draws it; the same holds for `axis`, `scale`,
`data.format` and every other object. Vega-Lite refuses these. Found by
checking a JSON Schema generated from the same types against them (24):
this is the only disagreement in 3,008 specs. A `Deserialize` that calls
`deserialize_map`, or a check on the JSON value before typing it, would
close it. **Measure:** `python crates/avenger-altair/bench/schema_parity.py <avenger checkout>`.

### 24. The spec types can export their schema, with CEL for the rest

Altair generates its API from Vega-Lite's JSON Schema; to generate it from
Avenger instead, Avenger's spec types need to export one. A `schema` feature
on `avenger-vegalite-spec` does it with `schemars` (derives on 31 types, the
schema of the five with their own `Deserialize` by hand), and puts what JSON
Schema cannot state (an ordered extent, strictly increasing steps, distinct
names and aliases) as CEL in `x-avenger-rules`, Kubernetes-style. The
result is 18.8 KB against the 1.9 MB of Vega-Lite's schema that Altair ships, and a plain `jsonschema` run
on it takes 155 µs where Altair's takes 705 µs. The change is
[`crates/avenger-altair/schema/vegalite-spec-schema.patch`](crates/avenger-altair/schema/vegalite-spec-schema.patch)
(against `f4890be`; the crate's tests pass with and without the feature).
Not proposed upstream yet.

Altair's own `generate_schema_wrapper.py` reads the result: 35 core classes
and 9 channel classes, with shorthand, and a chart built with them is
validated against Avenger's schema and drawn by Avenger, the same PNG byte
for byte as from Altair's API. Two names had to be Vega-Lite's
(`FacetedEncoding`, `RepeatRef`), and the mark and config mixins need
Vega-Lite's `MarkDef` and `Config`. Rules between fields must be CEL, not
constraint-only `anyOf` branches, which the generator reads as a union of
types. **Measure:** `python crates/avenger-altair/bench/altair_generator.py <altair checkout> <out>`.

Also: the compiler's `pdf` feature pulls in `krilla` 0.8.2, which needs
rustc 1.92, so the bridge builds with `svg` and `png` only on this machine's
1.89.

## From experiment 7: charts driven by typed decisions

Experiment 7 drives one chart from typed text (a classifier, and an LLM that
writes the pipeline). Its [README](experiments/07-chart-decisions/README.md#feedback-for-avenger)
has the full feedback; the points about Avenger itself:

### 10. One constant fill per mark, no colour scale, no line or arc

At `602b99c`, `RectEncoding` and `SymbolEncoding` take `fill: String`, and
`Scale` kinds are linear, band and point. A pie, a line per series, a heatmap
with a colour scale and a log axis could not be described, so the experiment
draws them on the scenegraph with its own layer (`src/layer/`), although
`avenger-scales` has log, pow and symlog. Checked by reading
`avenger-chart-definition/src/model.rs`.

### 11. No per-item key

What makes the experiment's chart feel alive is a key from the data on every
item: bars curl into a pie, split into a heatmap cell by cell, and come back.
`avenger-chart` keeps an identity per plot, not per item. A `key` channel,
carried into the rendered frame, would let a host animate between any two
frames. Checked by reading; the transitions themselves are measured in the
experiment's video.

### 12. The vocabulary as data

A classifier needs the option list, and an LLM the grammar; both were written
by hand from what the layer accepts. A chart definition that could list its
marks, channels with allowed types, and properties would let deciders,
editors and prompts be generated, and not drift from the renderer. This repo now does
that for its own pipelines: [`crates/avenger-validate`](crates/avenger-validate/)
reads the vocabulary from one spec and checks a pipeline in about 10 µs, from
Rust, Python and wasm, with the rules exported as CEL. The spec is still
written by hand, not exported by the steps themselves.

### 13. Refuse unsupported properties, with a path

The LLM loop is safe because everything the layer cannot draw is refused with
a reason, and the model fixed its tries from that text. The Vega-Lite
compiler's `CompileError::path()` does this. **Not checked:** what
`ChartDefinition::finish()` does with a property it cannot honour.

### 16. `geo` 0.29 keeps SedonaDB's spatial predicates out

A structure-aware lasso (CloudLasso) on the tile wanted `st_contains` from
SedonaDB. The predicates (`st_contains`, `st_within`, `st_intersects`) are
in `sedona-geo` only; `sedona-functions`, which this repo links, has
constructors, accessors and affine transforms but no predicate. `sedona-geo`
cannot join the workspace: it needs `geo` 0.33 → `i_overlay` 4.5 →
`i_float ~1.16`, while `avenger-geo`, `avenger-geometry` and
`avenger-guides` at `3065e2a` need `geo` 0.29 → `i_overlay` 1.9 →
`i_float ~1.6`. Two `geo` versions could coexist, but not two 1.x versions
of `i_float`, so resolution fails. Moving Avenger to `geo` 0.33 would let a
host put SedonaDB's spatial SQL next to it; the lasso now tests the polygon in
Rust on the drawn cells instead. **Measure:** add
`sedona-geo = { git = "https://github.com/apache/sedona-db", rev = "2f3e378…" }`
to `experiments/06-pipelines/Cargo.toml` and run `cargo metadata --format-version 1 >/dev/null`
(it fails on `i_float`); `cargo tree -i geo@0.29.3 --depth 1` lists the
Avenger crates.

Also from experiment 7, as things that worked well: `RequestWakeup` with
`RuntimeWake` for background work, `WriteClipboard`, `MouseUp` and
`CursorMoved` for text fields (and for a lens that follows the cursor),
`Clip::Path` for a round magnifier, and headless `PngCanvas` rendering of the same
`SceneGraph`, which made a video that rebuilds frame for frame from a cache.

## What worked well

Worth saying, because it is the part that does not generate issues:

- **Repinning #120 → #124 needed no code changes.** Nine crates, no API drift
  that reached us.
- **Repinning #124 → #130 across a rewritten stack needed one line.** The stack
  was rebased on 24 Sep 2026 (neither old pin is on any branch now), 180
  commits moved, and 20 crates came along with a single signature change
  (finding 9). Every experiment's binaries ran; the renders of experiments 1
  and 5 match, except `maps.png`, which differs by scattered pixels between
  two runs on the *same* revision (overlapping symbols drawn in a
  non-deterministic row order), so it is not an Avenger change.
- **`RenderInvalidationHub` is a good fit for live data.** A data thread calls
  `request_render` and the window rebuilds; invalidations coalesce sensibly
  (1022 batches over 25 s produced 140 rebuilds).
- **`avenger-datafusion-aggregate-state` generalises past brushing.** We use it
  for a rolling window: fold each arriving batch once (2–7 ms for 16k points),
  merge the retained states per frame (2–11 ms), and — the part that saved the
  viewer — re-roll the *same* states up to a coarser grid when there are too
  many cells. PR #123 motivates it with brush preaggregation; streaming is a
  second use with no changes needed.
- **Version alignment pays off.** Arrow 58.3 / DataFusion 54 matches SedonaDB
  `main`, so LiDAR batches reach Avenger scales with no conversion, and it also
  matches `re_datafusion` 0.38.1, which makes Rerun interop realistic (its
  MSRV of 1.96 is the only blocker for us).

## Chart language (`jonmmease/facet-fresh-start`)

From an earlier round, **not re-verified on the current branch**:

- A positional `GROUP BY 1, 2` inside `transform sql` fails to plan; the
  positions become literals. Named columns in a subquery work.
- The legend reuses the mark's symbol size, so small marks give an unreadable
  legend.
- No pan or zoom in the language yet.
- `avenger-typst-label` declares optional `typst*` dependencies pointing at a
  local `../../typst` checkout, so the branch does not build as cloned.
- `avenger-chart`'s README describes transforms as stubs, which the code has
  moved past.

## What this round did not check

- After the repin to `f4890be`: the renders of experiments 1–3 ran, but were
  not compared with the committed images; the live viewers were not opened.

- The live frame rates (`stream_live` without `--snapshots`); the headless
  snapshots ran and look the same.
- The chart-language branch (`jonmmease/facet-fresh-start`). It still exists,
  but the stack now has its own `avenger-chart`, `avenger-chart-definition` and
  `avenger-vegalite-compiler`, which may make it moot.
- Experiment 7 calls to Jev and the LLM: every decision was replayed from the
  cache, without an API key, so the tables match by construction. What was
  checked is that the chart layer and pipelines still build and fold
  correctly on the new stack (`layer_roundtrip`: 44 states, 1936 transitions).

## Next round

What we would like to measure when the stack next moves:

1. Whether findings 1–3 change; the same two probes and the same viewer numbers.
2. Whether a mark's data can be updated without a full scene rebuild — the
   thing that would make streaming charts cheap.
3. The chart-language items above, against whatever the language branch has
   become.
