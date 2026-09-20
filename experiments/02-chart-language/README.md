# Experiment 2 — the same map in the Avenger chart language

The "class from above" map from experiment 1, written in the experimental
Avenger chart language instead of Rust: 68 lines of chart plus 6 lines
declaring the data, with the axes, legend and layout left to the language.


[`charts/pantin.avenger`](charts/pantin.avenger) describes the "class from above" map in the experimental Avenger chart language, from the [`jonmmease/facet-fresh-start`](https://github.com/jonmmease/avenger/tree/jonmmease/facet-fresh-start) branch. The chart takes 68 lines, plus 6 in [`charts/catalog.avenger`](charts/catalog.avenger). Its `transform sql` aggregates all 17.3M points into 2 m cells, and the language takes care of the axes, the legend and the layout.

```
transform sql {
  query:
    SELECT "cx", "cy", max("z") AS zmax,
           CASE first_value("classification" ORDER BY "z" DESC)
             WHEN 2 THEN 'Ground' ... ELSE 'Other' END AS class
    FROM (SELECT floor("x" / 2.0) * 2.0 + 1.0 AS cx,
                 floor("y" / 2.0) * 2.0 + 1.0 AS cy, "z", "classification"
          FROM input) AS points
    GROUP BY "cx", "cy"
    ORDER BY zmax;
}
mark symbol as cells {
  fill: encoded "class" { legend: { title: 'LiDAR class'; } scale: ordinal { domain: [...]; range: [...]; } }
  x: encoded "cx" { axis: { title: 'Easting − 657 000 (m)'; } scale: linear { domain: [0.0, 1000.0]; } }
  y: encoded "cy" { ... }
}
```

`avenger watch` shows the chart in a native window and redraws it whenever the file is saved. The first open took 1.8–2.1 s; a reload after switching to 4 m cells took 1.1 s.

![avenger watch charts/pantin.avenger](images/avenger_watch.png)

The chart language does not read LAZ directly, so `export_parquet` first writes the tile to `charts/pantin_points.parquet` (about 105 MB, 2–3 s; the file is ignored by git). Things noticed while writing this chart:

- A positional `GROUP BY 1, 2` inside `transform sql` fails to plan: the positions become literals. Grouping by named columns in a subquery works.
- The legend reuses the mark's symbol size, so with `size: 4` the legend dots are very small.
- There is no pan or zoom yet in the language. `watch` reloads the file; it does not navigate.


## Run

The chart language cannot read LAZ, so the tile is exported to Parquet first
(about 105 MB, 2–3 s; the file is ignored by git). Build the `avenger` CLI from
Jon's language branch, then watch the chart:

```sh
git clone --branch jonmmease/facet-fresh-start https://github.com/jonmmease/avenger ../avenger-facet
cargo build --release -p avenger-lang-cli --manifest-path ../avenger-facet/Cargo.toml   # ~12 min, ~4.5 GB target

cargo run --release -p lidar-lang --bin export_parquet -- data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz
cd experiments/02-chart-language/charts && ../../../../avenger-facet/target/release/avenger watch pantin.avenger --scale 2
```

At the time of writing that branch does not build as cloned:
`avenger-typst-label/Cargo.toml` declares three optional `typst*` dependencies
pointing at a local `../../typst` checkout. Remove those dependency lines, empty
the `upstream-typst-probe` feature and delete the `upstream-typst-math-svg-probe`
bin entry, or check Typst out at that path.
