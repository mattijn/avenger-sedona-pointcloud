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
  lasso drawn in pixels as one tuple per run of pixel cells, over two
  gridded projections. A row is selected when its cell's centre lies inside
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

Measured by `cargo run --release -p lidar-probes --bin probe_selection -- <tile>`.

Regenerate the diff against a checkout of upstream at the pin:

```sh
git -C ../avenger archive f4890be avenger-selection | tar -x -C /tmp
diff -ruN --exclude=VENDORED.md --exclude=LICENSE --exclude=UPSTREAM.diff \
  /tmp/avenger-selection crates/avenger-selection > crates/avenger-selection/UPSTREAM.diff
```
