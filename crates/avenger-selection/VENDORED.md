# avenger-selection, carried here

Jon Mease's `avenger-selection` from
[jonmmease/avenger](https://github.com/jonmmease/avenger) at `f4890be`
(BSD-3-Clause, [LICENSE](LICENSE)), with additions that are not upstream.
Nothing in the workspace depended on the git crate, so this copy is an
ordinary path crate. [UPSTREAM.diff](UPSTREAM.diff) is the whole difference;
`Cargo.toml` changed only to point the path dependencies at the workspace's
git pin. The upstream tests pass alongside the new ones:

```sh
cargo test --release -p avenger-selection
```

The aim is to extend it until it expresses every selection experiment 7
draws with its own code (FINDINGS.md, "Selections in avenger-selection").

The additions so far:

- **`SelectionValue::polygon`** (`src/polygon.rs`, `tests/polygon.rs`): a
  lasso drawn in pixels as one tuple per run of pixel cells, over two of the
  producer's gridded projections. A row is selected when its cell's centre lies inside
  (even-odd). No new kind of test: resolution, cross-filtering, toggles and
  the split see ranges, as for a two-dimensional brush.
- **One lookup for many cell tuples** (`src/cell_boxes.rs`, and three lines
  in `tuples_predicate`): eight tuples or more of Int64 cell ranges compile
  to one function that evaluates each projection once, instead of an OR in
  which every term repeats its cell expression (FINDINGS.md 25: 5.3 s → 269
  ms for an 80-tuple lasso over 17.3M points). Membership is unchanged.

- **`ConsumerFilter::degree`** (`src/degree.rs`, `tests/degree.rs`): a
  Float64 degree of interest in [0, 1] beside the predicate, for soft
  selection. 1 exactly where the predicate holds, falling linearly to 0 at a
  width in logical pixels, measured from the row's cell on gridded
  projections (FINDINGS.md 26).

- **`SeriesTest`** (`src/series.rs`, `tests/series.rs`): a line brush
  (`Crosses`) and a timebox (`Within`) over a whole series, run as one query
  that returns the passing keys; `SeriesTest::value` makes them a `one_of`
  on the key projection (FINDINGS.md 27). `SeriesTest::degrees` and
  `soft_value` give soft series, with partial degrees per key
  (`SelectionValue::with_partial`, read by `degree`; FINDINGS.md 30).

- **`SelectionValue::cells`** (`src/polygon.rs`): a set of grid cells, such
  as CloudLasso's voxels, as one tuple per cell (FINDINGS.md 28). It and
  `polygon` read their grids from the producer.
- **`Gesture`** (`src/gesture.rs`): what was drawn, carried by a value and
  kept by a contribution through `set`, dropped by `toggle` (FINDINGS.md 29).
- **The selection log** (`src/log.rs`, `tests/log.rs`): `SelectionUpdate::to_json`,
  and `LogEntry::from_json` against a `Producers` registry of the chart's
  definitions. It records what was drawn: a value with a gesture is written
  as the gesture; replay redraws a lasso (`SelectionValue::from_gesture`) and
  hands the chart's kinds back as `Drawn`. Adds `serde_json` as a dependency
  (FINDINGS.md 29).

- **`ConsumerFilter::focus_degree`** (`src/degree.rs`): the focused
  producer's degree over given keys, such as stored preaggregation states,
  for a fading chart (FINDINGS.md 31).

Measured by `cargo run --release -p lidar-probes --bin probe_selection -- <tile>`.

Regenerate the diff against a checkout of upstream at the pin:

```sh
git -C ../avenger archive f4890be avenger-selection | tar -x -C /tmp
diff -ruN --exclude=VENDORED.md --exclude=LICENSE --exclude=UPSTREAM.diff \
  /tmp/avenger-selection crates/avenger-selection > crates/avenger-selection/UPSTREAM.diff
```
