# avenger-coords

One coordinate-system trait for charts and maps, and marks drawn through any
system. It grew in [experiment 5](../../experiments/05-coordinate-systems/),
which measures it and explains why it is shaped this way; this crate is that
code with the tile left out, so later experiments can use it instead of
writing it again.

A coordinate system is a list of channels, a point transform, a line transform
and one flag, `is_rectilinear`. Grids, tick labels, morphs and the fallback
from rect instances to paths are derived from those.

```rust
use avenger_coords::{Bend, CoordinateSystem};
use avenger_coords::draw;

let cs = Bend { width: 400.0, height: 300.0, t: 0.5 };   // halfway to polar
let p = cs.project(&[0.25, 0.5]);                          // unit x, y → pixels
```

## What is here

| System | Channels | Rectilinear | Use |
|---|---|---|---|
| `Cartesian` | x, y | yes | the default |
| `Polar` | θ ← x, r ← y | no | rose from bars, donut from a stacked share bar |
| `Spatial { projector }` | lon, lat | no | maps, through `avenger-geo` |
| `Cartesian3d { yaw, elevation }` | x, y, z (z = 0 by default) | no | a 2D chart on a tilted floor |
| `Bend { t }` | x, y | only at t = 0 | **the morph from Cartesian (t = 0) to polar (t = 1)** |
| `Blend(a, b, t)` | those of `a` | no | a morph between similar systems (map projections, lenses) |
| `Paired(a, b, t)` | `a`'s, then `b`'s | no | a morph between different inputs (metres ↔ lon/lat) |
| `Fisheye`, `Hyperbolic`, `Twirl` | x, y | no | lenses |
| `Nested { outer, radius }` | outer's, then θ, r | no | a pie at every point (vega-lite#7848) |

Every channel is in unit space except lon/lat, which are degrees. `draw` holds
`points`, `rects`, `rings`, `shape`, `polylines`, `grid` and `title`: each
takes a `&dyn CoordinateSystem` and returns scenegraph marks, rect instances
when the system is rectilinear and paths otherwise.

**For a morph, use `Bend`, not `Blend`.** `Blend` interpolates screen
positions, and between a strip and a ring the middle frames fold.
`Bend` interpolates the transform itself (curvature, radius, rotation), so
every frame is a valid ring segment (experiment 5's `morph.png`). Animate it by
stepping `t` from a `RequestWakeup` every 16 ms, as `coords_live` does.

## What is not here

These were listed as open in experiment 5 and are still open:

- **no inverse.** Nothing maps pixels back to input, so picking and brushing
  through a non-Cartesian system (a brush on a donut) have to add it.
- no clipping to the plot: `Bend`'s middle frames spill past their panel.
- no local scale (Jacobian), so symbol sizes do not follow a lens.
- no scales: channels are expected in unit space already.

## Used by

- [experiment 5](../../experiments/05-coordinate-systems/): `coords`,
  `coords_live`, `glyphs`
