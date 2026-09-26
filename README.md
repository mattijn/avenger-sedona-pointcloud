# avenger-sedona-pointcloud

Experiments in drawing a real LiDAR point cloud with [Avenger](https://github.com/jonmmease/avenger),
queried straight from [`sedona-pointcloud`](https://github.com/apache/sedona-db/tree/main/rust/sedona-pointcloud)
on DataFusion. No JSON and no browser in between:

```
tile.copc.laz ─► sedona-pointcloud (DataFusion file format)
              ─► SQL ─► Arrow arrays
              ─► Avenger scales, axes, legends ─► wgpu ─► window or PNG
```

The input is one 1 km² [IGN LiDAR HD](https://geoservices.ign.fr/lidarhd) tile
of 17.3 million points, north-east of Paris.

Avenger's scales take Arrow arrays directly, so the only reason this works
without conversion is that both sides agree on Arrow 58.3 and DataFusion 54.

## The experiments

| | Experiment | What it asks |
|---|---|---|
| 1 | [charts of a point cloud](experiments/01-charts/) | Can Avenger draw 17.3M points from a SQL query — statically, interactively, and how fast can a map window be fetched? |
| 2 | [the same map in the chart language](experiments/02-chart-language/) | Can the experimental Avenger chart language express the same chart, in less code? |
| 3 | [a live feed over Arrow Flight](experiments/03-streaming/) | Can Avenger keep up with a live sensor feed, using rolling aggregate states? |
| 4 | [coupling patterns](experiments/04-coupling-patterns/) | Which patterns from coupler-based workflows would improve the chart language? (proposal, no crate yet) |
| 5 | [one coordinate system](experiments/05-coordinate-systems/) | What does a general coordinate transform take, with Cartesian as the default and map projections as options? Includes a live viewer and two videos. |
| 6 | [Vega expressions, SQL and pipelines](experiments/06-pipelines/) | Can Vega expressions compile to DataFusion, chain with SQL and tool steps in one lazy GDAL-style pipeline, and split into commands and queries? |
| 7 | [charts driven by decisions](experiments/07-chart-decisions/) | Can a fast typed classifier (Jev) or an LLM drive a chart through the task vocabulary, from typed text and data changes, under a policy? |
| 8 | [validating a pipeline](experiments/08-pipeline-validation/) | Can the step vocabulary be written once and validated from Rust, Python and elsewhere, how do GDAL and PDAL do it, and how fast is each layer? (research and a benchmark) |

Out of experiment 8 came a crate of its own, [`crates/avenger-validate`](crates/avenger-validate/):
a pipeline validator for Rust, Python (pyo3) and the browser (wasm), with its
rules exported as CEL, that returns a verdict in about 10 µs.

![the class map](experiments/01-charts/images/top_class.png)

## Reviewing Avenger

These experiments double as a review of the stack. [FINDINGS.md](FINDINGS.md)
is the ledger: what we ran into, with the measurement behind each entry, so
that when Avenger moves the next pass is a recheck rather than a rediscovery.

```sh
cargo run --release -p lidar-probes --bin probe_guides   # guide and scale pitfalls
cargo run --release -p lidar-probes --bin probe_render   # render, geometry index, gradients
```

Avenger is developed as a stack of open PRs, so this is a cycle: Jon pushes,
we repin to the new top of the stack, rerun everything, and record what
changed. [CLAUDE.md](CLAUDE.md) is the runbook for that round — where the
stack lives, how to repin, what to rerun, what to write down, and the
gotchas this machine has already produced. Read it first if you are picking
the repo up cold.

## Layout

```
common/                         the LAS session and the shared palette
crates/avenger-validate/        the pipeline validator: Rust, Python, wasm, CEL
experiments/01-charts/          static charts, explorer, window benchmark
experiments/02-chart-language/  the .avenger chart and its Parquet export
experiments/03-streaming/       the Flight server and the live viewer
experiments/04-coupling-patterns/  proposals for the chart language
experiments/05-coordinate-systems/ one coordinate-system trait for charts and maps
experiments/06-pipelines/       Vega expressions, SQL and chained pipelines
experiments/07-chart-decisions/ charts driven by a decider (Jev, an LLM, rules)
experiments/08-pipeline-validation/ how to validate a pipeline, and how fast
probes/                         re-measures everything in FINDINGS.md
data/                           the tile (downloaded, not in git)
scripts/download_tile.sh
```

Every experiment is its own crate, so `cargo run -p lidar-charts …` builds only
what that experiment needs. The Avenger revision is pinned once, in the
workspace `Cargo.toml`, which is also where a repin happens.

## Getting the data

```sh
./scripts/download_tile.sh     # ~105 MB into data/
```

Rendering needs a GPU adapter that wgpu can use (Metal, Vulkan or DX12).

## Versions and pitfalls

This repo is a snapshot of work in progress on both sides.

- **Arrow and DataFusion versions must match.** Avenger's scales take
  `ArrayRef`, so both crates need the *same* Arrow version.
  - The `sedona-pointcloud` 0.4.1 release uses Arrow 57 and DataFusion 52.5.
  - Avenger `main` has `arrow = "*"`.
  - This repo therefore pins SedonaDB `main` (DataFusion 54.1, Arrow 58.3)
    together with the Avenger core PR stack (DataFusion 54.1, Arrow 58.4, which
    resolve to one Arrow and one DataFusion in the lockfile).
- **The Avenger stack is a set of open PRs.** This repo pins the top of that
  stack, `602b99c` ([#130](https://github.com/jonmmease/avenger/pull/130),
  `codex/portable-dataflow-inputs`), for every experiment. A pinned commit may
  disappear if the branch is rebased;
  if it does, update the revision in the workspace `Cargo.toml`. Moving from
  #120 to #124 needed no code changes here; moving #124 to #130, after the
  stack was rebased, needed one line.
- **Minimum Rust version.** With Rust 1.89, generate the lockfile with
  `CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback cargo update`, because
  the newest `ordered-float` needs Rust 1.90.
- **Workarounds the charts still carry**, all re-tested against #130 and all
  still needed — see [FINDINGS.md](FINDINGS.md) for the evidence:
  - `make_colorbar_marks` draws at (0, 0) whatever `origin` says, and its
    gradient renders as one flat colour, so `topviews` draws its own colorbar
    from stacked rects plus an axis.
  - The symbol legend title is placed inside the plot area, so these charts
    draw the title separately.
  - `LinearScale` accepts only a two-value domain, so colour ramps use evenly
    spaced stops.

Timings measured on an Apple Silicon Mac:

| Step | Time |
|---|---|
| Full-scan aggregation over 17.3M points | 1.2 s |
| Filtered query (strip or window) | 1.0 s (full scan; chunk statistics not enabled) |
| Rendering 1M points to a 2× PNG | 0.2–0.3 s |
| Streaming: fold one 16k-point batch into cell states | 2–7 ms |
| Streaming: merge a 12 s window into the current frame | 2–11 ms |
| Streaming: whole frame, ~40k cells | ~90 ms (about 11 fps) |

## Data

The LiDAR HD data is © IGN (Institut national de l'information géographique et
forestière) and published as open data; check the
[IGN LiDAR HD terms](https://geoservices.ign.fr/lidarhd) for the current licence
and attribution. The tile is downloaded from the host used by
[Giro3D's massive point cloud example](https://giro3d.org/latest/examples/massive-point-cloud.html)
and is not stored in this repository.
