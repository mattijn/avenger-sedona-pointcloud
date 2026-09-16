# avenger-sedona-pointcloud

Scatterplots of a real LiDAR point cloud, with no JSON and no browser in between:

Static charts, an interactive explorer with pan and zoom, and a benchmark of window queries.

```
tile.copc.laz ─► sedona-pointcloud (DataFusion file format)
              ─► SQL ─► Arrow arrays
              ─► Avenger scales, axes, legends ─► wgpu ─► PNG
```

- **Data:** [`sedona-pointcloud`](https://github.com/apache/sedona-db/tree/main/rust/sedona-pointcloud) registers LAS/LAZ (and therefore COPC) as a DataFusion file format, so the point cloud is queried with plain SQL.
- **Charts:** [Avenger](https://github.com/jonmmease/avenger) scales take the resulting Arrow arrays directly, with no conversion.
- **Rendering:** Avenger's offscreen `PngCanvas`, using wgpu.

The input is one 1 km² [IGN LiDAR HD](https://geoservices.ign.fr/lidarhd) tile (17.3 million points) north-east of Paris.

## Charts

### Cross-section

`cross_section`: every point in a 2 m wide west–east strip (27,313 points), coloured by LiDAR class.

![Cross-section](docs/images/cross_section.png)

### Top views

`topviews` aggregates all 17.3 million points into 1 m cells with one `GROUP BY` query (about 1.2 s). Each cell becomes one point, about 1 million per chart.

| Surface height (highest point per cell) | Class of the highest point |
|---|---|
| ![Surface height](docs/images/top_height.png) | ![Class from above](docs/images/top_class.png) |

| Mean return intensity | Every raw point in a 120 × 120 m window |
|---|---|
| ![Intensity](docs/images/top_intensity.png) | ![Zoom](docs/images/zoom_points.png) |

The core of the top-view query:

```sql
SELECT floor(x - 657000) + 0.5                        AS cx,
       floor(y - 6867000) + 0.5                       AS cy,
       max(z)                                         AS zmax,
       first_value(classification ORDER BY z DESC)    AS top_class,
       avg(intensity)                                 AS intensity
FROM 'data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz'
GROUP BY 1, 2
```

### Interactive explorer

`explorer` opens a window: drag to pan, scroll to zoom.

- **At startup:** the tile is loaded into an in-memory DataFusion table (about 2 s), and overview levels of 1, 2, 4, 8 and 16 m cells are built from it (about 0.4 s).
- **While you drag or scroll:** only the view changes. Avenger applies GPU-side scale adjustments, so no data is queried or re-uploaded, and symbols grow with the zoom so the current data stays readable.
- **About 150 ms after the view settles:** the explorer queries data for the new view and swaps it in.
  - When zoomed out, it uses the matching overview level (about 250k cells, 8–11 ms).
  - When zoomed in to at least 3 px per metre, it uses the raw points for the view plus a 20% margin, drawn low to high and capped at 3M points (242,824 points in 37 ms).

![Overview → zoom → reload](docs/images/explorer_zoom.png)

*Top row: overview, zoomed to 350 m before the reload, after the reload (1 m cells). Bottom row: zoomed to 100 m before the reload, after the reload (raw points). These frames come from `explorer … --snapshots out`, which renders the same scene headlessly.*

### Fetching a map window: benchmark

`bench_window` compares four ways to fetch the points in a window from this tile (17.3M points):

| Method | 120 × 120 m | 500 × 500 m | Notes |
|---|---|---|---|
| sedona-pointcloud, full scan | 1.1 s | 1.1 s | baseline |
| sedona-pointcloud + chunk statistics | 69 ms | 491 ms | The first query takes 0.84 s because it builds the statistics. `las.persist_statistics` writes a 25 KB `.stats` sidecar, so a new session takes 72 ms. |
| COPC octree via `copc-rs` | 270 ms | 2.9 s | Point-by-point decoding is the bottleneck. Selecting by resolution works: the whole tile at 2 m is 1.9M points in 0.7 s. |
| In-memory table (DataFusion `MemTable`) | 4 ms | 7 ms | Loading takes 1–2 s, and all points stay in RAM. |

Chunk statistics work well on COPC files because every LAZ chunk is an octree node, so a spatial filter skips most of the file. `copc-rs` is taken from git: the crates.io release (0.5.0) does not build with current `las`.

## Run

```sh
./scripts/download_tile.sh     # ~105 MB into data/
mkdir -p out
cargo run --release --bin cross_section -- data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz out/cross_section.png
cargo run --release --bin topviews      -- data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz out
cargo run --release --bin explorer      -- data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz
cargo run --release --bin bench_window  -- data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz
```

Rendering needs a GPU adapter that wgpu can use (Metal, Vulkan or DX12).

Timings measured on an Apple Silicon Mac:

| Step | Time |
|---|---|
| Full-scan aggregation over 17.3M points | 1.2 s |
| Filtered query (strip or window) | 1.0 s (full scan; chunk statistics not enabled) |
| Rendering 1M points to a 2× PNG | 0.2–0.3 s |

## Versions and pitfalls

This repo is a snapshot of work in progress on both sides.

- **Arrow and DataFusion versions must match.** Avenger's scales take `ArrayRef`, so both crates need the *same* Arrow version.
  - The `sedona-pointcloud` 0.4.1 release uses Arrow 57 and DataFusion 52.5.
  - Avenger `main` has `arrow = "*"`.
  - This repo therefore pins SedonaDB `main` (DataFusion 54.1, Arrow 58.3) together with the Avenger core PR stack (DataFusion 54, Arrow 58.3).
- **The Avenger stack is a set of open PRs.** The pinned commit (`34372f7`) may disappear if the branch is rebased. If it does, update the `rev` values in `Cargo.toml`.
- **Minimum Rust version.** With Rust 1.89, generate the lockfile with `CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback cargo update`, because the newest `ordered-float` needs Rust 1.90.
- **Workarounds for issues found in the Avenger stack:**
  - `make_colorbar_marks` draws at (0, 0), so the caller must position it. In `PngCanvas` its fill also renders as a single colour. `topviews` draws its own colorbar from stacked rects plus an axis.
  - The symbol legend title is placed inside the plot area, so these examples draw it separately.
  - `LinearScale` accepts only a two-value domain. Colour ramps use evenly spaced stops.
- **An issue on Avenger `main`** (fixed in the stack): `make_numeric_axis_marks` always sets a `band` option, which `LinearScale` rejects.

## Data

The LiDAR HD data is © IGN (Institut national de l'information géographique et forestière) and published as open data; check the [IGN LiDAR HD terms](https://geoservices.ign.fr/lidarhd) for the current licence and attribution. The tile is downloaded from the host used by [Giro3D's massive point cloud example](https://giro3d.org/latest/examples/massive-point-cloud.html) and is not stored in this repository.
