# Altair on Avenger

Status: a design and a first bridge, 26 Sep 2026. The bridge is
[`crates/avenger-altair`](../crates/avenger-altair/); the measurements are in
its README.

Altair stays the front end: the Python API people write charts with. Avenger
becomes the engine underneath: it validates the chart and it draws it, natively
and in-process. For a person at a notebook that means charts without a browser
round trip; for an agent that writes charts it means a verdict in
microseconds, with an error that names the property, on every try.

## How Altair works today

1. **The API is generated from the Vega-Lite JSON Schema.**
   `tools/generate_schema_wrapper.py` reads `vega-lite-schema.json` and writes
   `altair/vegalite/v6/schema/core.py` (1.5 MB), `channels.py` (1.1 MB),
   `_config.py`, `mixins.py` and `_typing.py`. Every class carries its piece
   of the schema.
2. **Validation is `jsonschema` against that schema.** `to_dict()` calls
   `SchemaBase.validate` on the top-level class with the whole spec, which
   calls `validate_jsonschema` with the root schema
   (`altair/utils/schemapi.py`). There is no plug-in point: the validator is
   the JSON Schema.
3. **Rendering is Vega-Lite and Vega in JavaScript**, in the notebook
   frontend, or through vl-convert for `save`. Renderers are plug-ins
   (`alt.renderers.register`), so rendering already has a hook.

Measured with Altair 6.3.0 (Vega-Lite 6.4.1) on an 8-row bar chart, warm:
the JSON Schema validation alone takes 705 µs and `to_dict()` 1,069 µs, before
anything is drawn. With `avenger_altair.enable()`, Avenger validates the same
spec in 13.5 µs from Python (6.5 µs of it in Rust), and `to_dict()` takes
735 µs: the rest of it is Altair's own Python layer walking its objects and
serialising the data, which is what steps 3 and 4 below are about.

## Where it goes

Avenger has its own Vega-Lite front end: `avenger-vegalite-spec` (typed Serde
structs for a subset of Vega-Lite 6.4.3, with validation whose errors carry a
path, `encoding.x.bin.step: expected a positive finite number`) and
`avenger-vegalite-compiler` (the spec into a native `ChartDefinition`, which
`avenger-chart` evaluates with DataFusion and draws to SVG, PNG or PDF).

The steps, each usable on its own:

1. **An opt-in backend, no change to Altair** (built:
   `avenger_altair.enable()`). A renderer draws with Avenger; Altair's
   `validate` on the top-level chart asks Avenger first, and only a spec
   Avenger does not take goes through the JSON Schema. A chart Avenger does
   not draw yet falls back to the renderer that was active, and
   `explain(chart)` names the property.
2. **A validator hook in Altair.** Rendering has `alt.renderers`; validation
   needs the same: `alt.validators.register("avenger", fn)` and
   `alt.validators.enable(...)`, called where `validate_jsonschema` is called
   today. The bridge then stops patching a class method. This is a small PR
   to Altair, and the first one this needs.
3. **The API generated from Avenger.** Avenger's spec types become the
   source: they export a schema (for example with `schemars`) that
   `generate_schema_wrapper.py` reads instead of Vega-Lite's. As long as
   Avenger's grammar is Vega-Lite's, Altair's API does not change for its
   users; what changes is where the classes, their docstrings and their
   validation come from. The JSON Schema route stays available as an
   opt-out.
4. **Data without JSON.** Today a DataFrame becomes inline JSON rows (or a
   file) in the spec. Avenger reads Arrow: a pandas, Polars or PyArrow frame
   can reach DataFusion through the Arrow C data interface without being
   written out, as a named `TableSnapshot`. This is where the largest time
   goes for real data, and where native rendering pays most.
5. **Interaction in the notebook**: a widget (anywidget) that hosts an
   Avenger canvas, so selections and parameters work without Vega.

## What Avenger needs

Measured on 26 Sep 2026 at `f4890be`: none of the 117 gallery examples that
run here compiles to Avenger yet. The first refusal of each is `layer` (41),
a mark other than bar (41), `encoding.color` (8), composition (19), and a
handful of single properties. [FINDINGS.md](../FINDINGS.md) 19–21 has the
list and the command.

Today the compiler draws bar charts only: bars, ranges, aggregates,
histograms, stacks, a `gte` filter and numeric parameters. Altair's gallery
is the measure of what comes next; the bridge's coverage script runs the 120
examples of `tests/examples_methods_syntax` and groups the first refusal of
each by property, so the order of work follows use. That list is for Jon;
this repo will hand it back through [FINDINGS.md](../FINDINGS.md).

## What Altair needs

- A validator registry next to the renderer registry (step 2).
- The schema source configurable in `generate_schema_wrapper.py` (step 3).
- A data path that hands Arrow tables to a backend instead of serialising
  rows (step 4); `alt.data_transformers` is the natural place.

None of this is sent upstream from here without asking first; the text of
each proposal is drafted in this repo.
