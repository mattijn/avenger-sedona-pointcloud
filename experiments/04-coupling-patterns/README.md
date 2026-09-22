# Experiment 4 — coupling patterns for the chart language

Status: proposal. There is no crate yet; each section ends with the probe that
would test it.

Large simulation systems have a problem that looks a lot like ours.
Independently developed components, each with its own grid, resolution and
clock, have to exchange data without knowing each other's internals. They
solve it with a *coupler*: the components agree on names, a schema and a
cadence, and a declarative configuration states what flows where, how often,
and how it is transformed on the way.

Studying how such coupler-based workflows are put together turned up five
patterns that map onto the Avenger chart language (`jonmmease/facet-fresh-start`)
and onto what experiments 2 and 3 already do by hand. For each one this page
records what was observed, where the language stands today, and a direction to
try. The syntax follows the `.avenger` files in
[experiment 2](../02-chart-language/charts/) and is illustrative, not a design.

| # | Pattern | Direction for Avenger |
|---|---|---|
| 1 | The data flow is readable in one place | A lineage report with rows per edge |
| 2 | Transformations are ordered stacks with a recognisable last resort | Scale fallback chains that report what fell through |
| 3 | Named presets are composed by merging | Exportable presets for scale, axis and legend blocks |
| 4 | Where a reduction runs, and at what resolution, is declared | Output-resolution binning and a mark budget |
| 5 | Data sources are separate processes behind a narrow contract | A live `table flight` with a cadence and a retention window |

## 1. The data flow is readable in one place

**Observed.** In a coupled system the whole topology — which component sends
which field to which, on which grid, how often — sits in one configuration
file. Components barely change when a flow is added, and anyone can see what
moves where without running anything.

**Today.** The language already separates data from charts:
[`catalog.avenger`](../02-chart-language/charts/catalog.avenger) exports the
tables and [`pantin.avenger`](../02-chart-language/charts/pantin.avenger)
imports them. The compiler also keeps dataset and stage lineage indexes
internally (`avenger-lang-compiler/README.md` on that branch). What is missing
is a way to *see* that lineage: which table feeds which transform, which
columns reach which channel through which scale, and how many rows cross each
step.

**Direction.** Expose the lineage the compiler already has, together with row
counts and bytes per edge from the last evaluation:

```
$ avenger graph pantin.avenger
lidar.pantin (parquet)              17.3M rows
  └─ transform sql                     ≤ 250 000 rows   (2 m cells over 1 km²)
       └─ mark symbol cells
            fill ← "class"  via ordinal (6 values)
            x    ← "cx"     via linear  [0, 1000]
            y    ← "cy"     via linear  [0, 1000]
```

The output is a sketch; only the input count and the 2 m cell bound are real. This is the
instrument for patterns 4 and 5: it shows where the data gets small, which is
where the work should happen.

**Probe.** Walk the compiled chart from Rust and print the stage graph with
row counts for `pantin.avenger`. Compare the counts against a direct DataFusion
run of the same SQL.

## 2. Scale fallback chains that report what fell through

**Observed.** Couplers do not configure one interpolation method; they
configure an ordered *stack*. Each method fills what it can and hands the rest
to the next. The stack nearly always ends in a constant chosen to be
*recognisable*, so that a gap shows up as a signal instead of a plausible
value.

**Today.** `pantin.avenger` handles unknown classes itself: the SQL maps every
code except 2–6 to `'Other'` with `CASE … ELSE 'Other'`, and the ordinal scale
lists `'Other'` in its domain. Any value that escapes that `CASE` has no
defined colour. The raster marks do have `null_color` and `non_finite_color`
channels, and the Rust ordinal and linear scales accept a single `default`
option. I have not checked whether `default` can be reached from the language:
the stock-v1 registry document does not list scale options.

**Direction.** A `fallback` list on the scale, tried in order for nulls,
non-finite values, out-of-domain values and unknown categories:

```
fill: encoded "class" {
  scale: ordinal {
    domain: ['Ground', 'Low vegetation', 'Medium vegetation', 'High vegetation', 'Building'];
    range: ['#8c6e4d', '#b8d666', '#59ad54', '#216e38', '#cc4040'];
    fallback: [case_insensitive, constant '#9ea3ad'];
  }
  legend: {
    title: 'LiDAR class';
    fallback_entry: 'Other';     -- one legend row, labelled with the count
  }
}
```

Two things matter more than the syntax. First, the scale should report which
step produced each value, so the legend can show "Other (n)" and a debug
overlay can highlight rows that fell through. Second, a loud sentinel — magenta,
or not drawing the mark — should be an option, because a plausible grey hides
a broken `CASE` for as long as nobody looks.

**Probe.** Add a case to `probes/probe_guides` that feeds the ordinal scale a
class outside its domain, and records what colour comes out and whether the
legend mentions it.

## 3. Exportable presets for scale, axis and legend blocks

**Observed.** Coupling configurations define reusable bundles once — an
interpolation stack, a time configuration, a source-and-target pair — and
assemble each flow by merging several of them and overriding a field or two.
A change to a preset reaches every flow that uses it.

**Today.** The language has definitions with typed slots and defaults for
marks, tools and transforms, and chart-level `format` and `time` defaults. It
has no reusable bundle for the blocks that repeat most. In `pantin.avenger` the
x and y channels carry the same scale block, word for word:

```
scale: linear {
  domain: [0.0, 1000.0];
  zero: false;
}
```

Every chart of this tile will repeat it again, and the same goes for axis
titles in metres and the class legend.

**Direction.** Let a catalogue export named blocks, and let a use site
reference a block and override it:

```
-- catalog.avenger
export preset scale as tile_metres: linear {
  domain: [0.0, 1000.0];
  zero: false;
}
export preset scale as lidar_class: ordinal {
  domain: ['Ground', 'Low vegetation', 'Medium vegetation', 'High vegetation', 'Building', 'Other'];
  range: ['#8c6e4d', '#b8d666', '#59ad54', '#216e38', '#cc4040', '#9ea3ad'];
}

-- pantin.avenger
x: encoded "cx" { scale: lidar.tile_metres; axis: { title: 'Easting − 657 000 (m)'; } }
y: encoded "cy" { scale: lidar.tile_metres { reverse: false; } }
```

Presets resolve at compile time, like the existing definitions, so they cost
nothing when the chart runs, and the compiler can type-check them against the
channel they are used on. The shared palette in `common/` could then live in
the catalogue instead of being duplicated between Rust and `.avenger`.

**Probe.** A compile-only test: two charts sharing one preset, one with an
override. Check that expansion produces the same canonical DSL as writing the
blocks out by hand.

## 4. Output-resolution binning and a mark budget

**Observed.** This pattern carried the most weight. In the coupled workflows
studied, the largest gain came from an *ordering* decision rather than a
faster algorithm: do the cheap reduction on the coarse source data *before*
anything is transferred, and run the expensive mapping afterwards, on less
data. The side that performs the mapping is also a declared setting, not an
implementation detail.

**Today.** Both earlier experiments already follow this rule, by hand.

- Experiment 2's `transform sql` folds all 17.3M points into 2 m cells before
  the mark sees them. The chart would not be drawable otherwise.
- Experiment 3 folds each batch into per-cell aggregate *states* once, and
  merges states per frame, so raw points are never revisited. When the window
  holds more than about 55k cells it rolls states up to 4 m, 8 m and so on,
  because rebuilding the geometry index costs about 1.2 µs per mark
  ([FINDINGS.md](../../FINDINGS.md)). With a 20 s window that took the viewer
  from 3 to about 11 frames per second.

That roll-up is a placement and resolution decision written in Rust, and the
cell size in experiment 2 is fixed at 2 m in the SQL. The language has
`transform rasterize_2d`, whose `extent` can follow
`viewport.x.domain`, but in the examples I found `bins` is a fixed count.

**Direction.** Two additions, both declarative.

```
transform rasterize_2d as cells {
  x: "x";  y: "y";
  value: "z";  agg: max;
  x_dim: { bins: plot.width;  extent: [viewport.x.domain.start, viewport.x.domain.end]; }
  y_dim: { bins: plot.height; extent: [viewport.y.domain.start, viewport.y.domain.end]; }
}

mark symbol as cells {
  budget: 55000;        -- above this, roll up to a coarser grid
  ...
}
```

Binning at the plot's pixel size makes the cell count depend on the output,
not on the input. A 620 × 620 plot never needs more than 384 400 cells,
however many points the tile has. A mark budget turns experiment 3's
hand-written roll-up into a rule the runtime applies, which needs aggregates
that can be merged at any resolution — exactly what the merge states from
`avenger-datafusion-aggregate-state` provide. The lineage report from
pattern 1 then shows how much data crossed each step, so a placement choice
can be measured instead of guessed.

**Probe.** Extend `bench_window` with a pixel-binned variant of the class map
and record, next to the current numbers: time to first frame (currently
1.8–2.1 s under `avenger watch`), peak memory, frame time while panning, and a
visual comparison against `images/top_class.png` at equal size.

## 5. A live `table flight` with a cadence and a retention window

**Observed.** In coupled workflows data preparation runs as separate
processes, often in Python, next to the main program. The contract is
deliberately small: a component name, field names, a schema, and an exchange
cadence written as an ISO 8601 duration. The *receiver* drives the clock. Each
provider also has a dry-run mode, so it can be developed and tested without
the host.

**Today.** Experiment 3 is this pattern in Rust: `stream_server` replays the
tile over Arrow Flight, `stream_live` owns the rolling window and asks for
data with a ticket (`{"speed": 4.0}`). The chart language cannot express any
of it. Its table kinds on the branch are `arrow`, `csv`, `delta`, `inline`,
`json`, `parquet` and `sql`, none of which is live.

**Direction.** A Flight table that states the contract, with the rolling
window and merge states from experiment 3 as properties rather than code:

```
export schema tables as feed {
  table flight as scan {
    endpoint: 'http://127.0.0.1:50051';
    ticket: { speed: 4.0; };
    schema: { x: DOUBLE; y: DOUBLE; z: DOUBLE; classification: TINYINT; gps_time: DOUBLE; };
    keep: 'PT20S';                  -- retention window, on gps_time
    fold: {
      key: [floor("x" / 2.0), floor("y" / 2.0)];
      states: [maxState("z"), countState()];
    }
  }
}
```

A schema mismatch becomes a load-time error that names the endpoint and the
column, rather than a rendering failure later. `avenger watch --dry-run` could
substitute a generator that emits batches matching the declared schema, so a
chart can be written before the server exists, and the server can be tested
against the schema without a viewer. The `fold` block also gives pattern 4
something to push down: the server could send states instead of points.

**Probe.** Point a `table arrow` at a file of batches captured from
`stream_server` to check that the language can chart one frame of the feed.
That separates "can it draw this data" from "can it stream".

## Not carried over

The process machinery of coupled simulations — launch layouts across many
programs, partitioning over ranks, node scaling — solves a different problem
from an interactive viewer and is not proposed here. Spherical regridding is
out of scope too; if it matters later, it belongs in the geo work as a
transform, not in the language.

## Suggested order

1. **Fallback chains (2).** The smallest change, and the probe fits in the
   existing `probe_guides`.
2. **Presets (3).** Compile-time only, and removes duplication that every
   chart of this tile will have.
3. **Output-resolution binning (4).** Measured with `bench_window`; the mark
   budget follows once binning works.
4. **Lineage report (1).** Most useful once there are placement choices to
   compare.
5. **Flight table (5).** Builds on 1 and 4, and has the largest surface.
