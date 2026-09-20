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

![the class map](experiments/01-charts/images/top_class.png)

## Reviewing Avenger

These experiments double as a review of the stack. [FINDINGS.md](FINDINGS.md)
is the ledger: what we ran into, with the measurement behind each entry, so
that when Avenger moves the next pass is a recheck rather than a rediscovery.

```sh
cargo run --release -p lidar-probes --bin probe_guides   # guide and scale pitfalls
cargo run --release -p lidar-probes --bin probe_render   # render, geometry index, gradients
```

## Layout

```
common/                         the LAS session and the shared palette
experiments/01-charts/          static charts, explorer, window benchmark
experiments/02-chart-language/  the .avenger chart and its Parquet export
experiments/03-streaming/       the Flight server and the live viewer
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
    together with the Avenger core PR stack (DataFusion 54, Arrow 58.3).
- **The Avenger stack is a set of open PRs.** This repo pins the top of that
  stack, `5f31c58` ([#124](https://github.com/jonmmease/avenger/pull/124),
  `codex/selection`). A pinned commit may disappear if the branch is rebased;
  if it does, update the revision in the workspace `Cargo.toml`. Moving from
  #120 to #124 needed no code changes here.
- **Minimum Rust version.** With Rust 1.89, generate the lockfile with
  `CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback cargo update`, because
  the newest `ordered-float` needs Rust 1.90.
- **Workarounds the charts still carry**, all re-tested against #124 and all
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
