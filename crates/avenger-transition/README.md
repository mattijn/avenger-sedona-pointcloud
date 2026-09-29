# avenger-transition

Transitions of keyed items in a plot's unit square, before any coordinate
system. It grew in [experiment 7](../../experiments/07-chart-decisions/)'s
chart layer, where one chart object animates from each state to the next;
this crate is the part of it that knows nothing about that chart's model.

Draw the result through [`avenger-coords`](../avenger-coords/): the items
come out in unit space, and `phases` says how far to bend.

```rust
use avenger_transition::{join, phases, related, Plane, Timing, ease};
use avenger_coords::{Bend, CoordinateSystem};

let (bend, g) = phases(Plane::Cartesian, Plane::Polar, t);   // t from 0 to 1
let e = ease(t);
let items = join(&from, &to, Timing { t, g, exit: 1.0 - e, enter: e });
let cs = Bend { width: 300.0, height: 300.0, t: bend };
```

## What is here

- **`join(a, b, timing)`**: D3's object constancy. An item in both frames
  moves, resizes and recolours. A new item whose parent was in the old
  frame starts as a slice of that parent (a class bar splits into its
  heatmap cells), and the reverse collapses cells back into their bar. Any
  other item fades in or out.
- **`phases(a, b, t)`**: between Cartesian and polar, the items move to
  their new layout in the first half and the plane bends in the second, or
  the reverse. Moving and bending at once folds the shapes.
- **`bins`**: a histogram as keyed items, as bars (x is the value) or as one
  stacked bar (x is the running share of the count), the layout that bends
  into a donut. `interval` puts a value range, such as a brush, into either
  layout as one rect. `position` and `value_at` convert between value and
  unit x in both directions, so a brush drawn on the donut becomes a value
  range again.

**Why a brush moves along.** A brush edge at a value sits at the same
fraction of its bin in both layouts, and `join` moves both ends of every
rect linearly, so the edge stays at that place within its bar through the
whole transition. Stacking keeps the bins in order, so the brush stays one
rect: a band over the bars, an arc of the donut. The `bins` tests check the
first part.

## What is not here

- Experiment 7's axes, titles, legends, lenses and views stay in its
  `anim.rs`: they depend on its chart model.
- No easing other than smoothstep.

## Used by

- [experiment 7](../../experiments/07-chart-decisions/): `layer/anim.rs`
  (the 45 frames of `layer_demo` are byte-identical to before the move)
