# avenger-altair

Avenger as an opt-in backend for Altair. Altair stays the front end; Avenger
validates the chart and draws it, in-process, with no JavaScript.

```python
import altair as alt
import avenger_altair as av

av.enable()                     # draw with Avenger, validate with Avenger
chart = alt.Chart(df).mark_bar().encode(x="category:N", y="sum(amount):Q")
chart                           # a PNG from Avenger in the notebook
av.save(chart, "bars.png")      # or .svg
av.explain(chart)               # "Avenger draws this chart." or the property that stops it
av.disable()                    # back to what was active before
```

![An Altair bar chart drawn by Avenger](images/bars.png)

The design, and the steps from here to an Altair whose API and validation come
from Avenger, are in [docs/altair-avenger.md](../../docs/altair-avenger.md).

## What it does

- **Validation.** `to_dict()` calls `validate` on the top-level chart; enabled,
  that asks Avenger first (`avenger-vegalite-spec`, typed Vega-Lite 6.4.3). A
  spec Avenger takes is valid; one it does not take is checked by Altair's
  JSON Schema as before, so a valid chart outside Avenger's subset stays
  valid, and an invalid chart still raises Altair's `SchemaValidationError`.
- **Rendering.** A renderer, `"avenger"`, compiles the spec to a native chart
  definition (`avenger-vegalite-compiler`) and draws it. A chart Avenger does
  not draw yet falls back to the renderer that was active; `av.last` says
  which one drew the last chart and why, and `enable(fallback=False)` makes
  that an `AvengerRefusal` with the path instead.
- **Altair's default theme.** Every Altair chart carries
  `config.view.continuousWidth/Height`; Avenger reads no `config` yet, so that
  one config is written out as the width or height Vega-Lite would give a
  continuous axis. Any other config is left for Avenger to refuse.

## Build

```sh
cd crates/avenger-altair && maturin develop --release
cargo run --release -p avenger-altair -- render spec.json out.png 10   # stages, mean of 10
python bench/coverage.py ~/vega/altair/tests/examples_methods_syntax    # the gallery
```

## Measured

26 Sep 2026, Avenger `f4890be`, Altair 6.3.0, Apple Silicon, Python 3.11.6.
An 8-row bar chart (`x="category:N", y="sum(amount):Q"`), warm.

| | Time |
|---|---|
| Altair's JSON Schema validation | 705 µs |
| Avenger's validation, from Python (with the theme written out and `json.dumps`) | 13.5 µs |
| … of which in Rust | 6.5 µs |
| `to_dict()`, Altair as it is | 1,069 µs |
| `to_dict()` with `av.enable()` | 735 µs |
| `to_dict()` for a chart outside Avenger's subset (both checks run) | 1,148 µs |
| draw to PNG: validate, compile, render, export | 0.02 + 0.7 + 4.3 + 14.1 ms |
| draw to SVG | 0.05 + 1.8 + 5.4 + 341 ms |

SVG is the default in Vega's notebooks, but here 139 of its 148 KB is one
embedded font and nearly all its time is spent there, so the renderer
defaults to PNG ([FINDINGS.md](../../FINDINGS.md) 20). `enable(format="svg")`
switches.

Coverage: none of the 117 Altair gallery examples that run here compiles to
Avenger yet; the compiler draws bar charts (bars, ranges, aggregates,
histograms, stacks), and the gallery's first refusals are layers, other marks,
colour and composition (FINDINGS.md 21). A plain Altair bar chart, as above,
is drawn.

## Not built, not checked

- The PNG and SVG were checked to be produced and non-empty, and the PNG by
  eye; they were not compared pixel by pixel with Vega-Lite's rendering.
- PDF: the compiler's `pdf` feature needs rustc 1.92.
- A validator hook in Altair itself; this bridge replaces `Chart.validate` on
  `enable()` and restores it on `disable()`. `LayerChart` and the
  concatenated charts are not hooked, since Avenger draws none of them yet.
- Data goes through Altair's JSON serialisation (`datasets` in the spec); an
  Arrow path is step 4 of the design.
- Interaction and the notebook widget.
