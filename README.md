# avenger-sedona-pointcloud

Scatterplots of a real LiDAR point cloud, with no JSON and no browser in between:

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

## Run

```sh
./scripts/download_tile.sh     # ~105 MB into data/
mkdir -p out
cargo run --release --bin cross_section -- data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz out/cross_section.png
cargo run --release --bin topviews      -- data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz out
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
