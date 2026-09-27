# Experiment 8 — validating a pipeline, in Rust and elsewhere

Status: research and a benchmark, 26 Sep 2026. Not tied to an Avenger
revision: nothing here draws.

Experiment 7's writers produce pipelines in a GDAL-style syntax
(`read … ! sql "…" ! chart bar --x label:N --y n:Q ! highlight "…"`), and the
chart refuses what it cannot draw. Of 269 recorded writer attempts, 39 were
refused. This experiment asks three things:

1. Can the step vocabulary be written down once, so that validators in other
   languages (Python first) are generated from it rather than rewritten?
2. How do GDAL and PDAL do this, and what else is used for it?
3. How fast is each layer of validation, natively in Rust, from Python via
   pyo3, and in pure Python?

## The four layers

Sorting experiment 7's 39 refusals by what it takes to catch them gives four
layers, and the rest of this README uses them.

| Layer | What catches it | Refusals | Examples |
|---|---|---|---|
| 1. argument | a type, pattern, range or choice per argument | 15 | `color: 'green' is not a #rrggbb colour` (12), `lens --focus 20,100000` outside 0..1 (3) |
| 2. order and state | step order, and what the current mark allows | 16 | "this mark has no emphasis" (15), "`sql` comes after the chart commands" (1) |
| 3. expressions | a parser for the embedded Vega expressions | 5 | `vega: expected ')' … at offset 19` |
| 4. data | field names and SQL, against the schema that reaches the step | 3 | `No field named z`, a missing GROUP BY |

(The count was made over `experiments/07-chart-decisions/results/*decisions.json`;
a refusal is counted once, under the reason the chart gave.)

## What GDAL does

Run against GDAL 3.13.3 (conda-forge), with the source compared at the 3.11,
3.12 and 3.13 tags.

- **`gdal <command> --json-usage`** (since 3.11) prints each algorithm and its
  arguments as JSON: `name`, `type`, `required`, `default`, `choices`,
  `min_value`/`max_value`, `min_count`/`max_count`, `dataset_type`, and since
  3.13 `mutual_exclusion_group`, `mutual_dependency_group` and `depends_on`.
  `gdal --json-usage` gives the whole tree: 304 algorithms, 3,267 arguments.
  [`results/gdal/raster_reproject.json`](results/gdal/raster_reproject.json)
  is one of them.
- **`gdal pipeline --json-usage`** lists the steps a pipeline accepts under
  `pipeline_algorithms` (83 in `gdal pipeline`, 39 raster, 37 vector).
- **The JSON Schema GDAL ships, `gdal_algorithm.schema.json`, validates the
  usage output, not a command.** No per-algorithm schema is published; RFC 104
  says the JSON is meant for generating user interfaces. Two R packages
  (`gdalcli`, `gdalraster`) generate their wrappers from it. No Python or
  TypeScript generator was found.
- **What the JSON leaves out:** which data type a step consumes and produces,
  which steps may come first or last, and the `!` and `[ … ]` grammar. In
  `gdal pipeline` every step's `full_path` is `[]`, so raster and vector `read`
  are told apart only by their `url`. GDAL checks chaining in C++:
  `read in.tif ! reproject ! buffer 1` fails with "Step 'buffer' expects a
  vector input dataset, but previous step 'reproject' generates a raster
  output dataset". Custom checks such as a valid CRS are also invisible in the
  JSON ([`results/gdal/validation_probe.txt`](results/gdal/validation_probe.txt)).
- **Python** (3.11+) can walk `gdal.GetGlobalAlgorithmRegistry()` and gets what
  the JSON lacks (positional arguments, short names, aliases), but has no
  getter for min/max ([`results/gdal/walk_registry.py`](results/gdal/walk_registry.py)).
- **`.gdalg.json`** is `{"type":"gdal_streamed_alg","command_line":"…"}`, a
  string, like our `to_json()`. Its schema spells `additional_properties`
  instead of `additionalProperties`, so unknown keys pass; confirmed with
  `jsonschema`. Not reported upstream.

## What PDAL does

Run against PDAL 2.10.2 and pdal-python 3.5.5, with the source read at those
tags. PDAL has no schema: an option exists, with a type, only as C++ code in
its stage (`ProgramArgs`, `TArg<T>`, `operator>>`), so an option can only be
checked by building the stage.

`pdal pipeline --validate` is not a separate checker. It parses the JSON,
builds every stage and runs `prepare()` (options, `initialize()`, the point
layout), stopping before points are read. So it opens the input files, and
writers are not checked. 35 broken pipelines were run through it and through
a real execute; the fixtures are in [`results/pdal/pipelines/`](results/pdal/pipelines/),
the results in [`results/pdal/summary.tsv`](results/pdal/summary.tsv), and
[`results/pdal/run_matrix.sh`](results/pdal/run_matrix.sh) reruns them.

| Case | `--validate` |
|---|---|
| unknown option, bad integer, bad enum, unknown dimension in `where`, undefined `inputs` tag | caught, clear message |
| `step: "10abc"` on a double, `compression: "maybe"` | **accepted**; read as 10 and as false |
| `limits: "Z[0:ten]"` | no JSON; `PDAL: Missing ')' or ']'.`, naming no stage or option |
| a filter using a dimension **before** the stage that creates it | **valid**, then runs wrong (0 or all points pass) |
| missing input file | caught, because validating opens it |
| missing output directory | valid; fails only on execute |
| `[]` | segmentation fault |

`--validate` exits 0 whether the pipeline is valid or not. `--options
<stage> --showjson` gives only `name`, `description` and `default`: no types,
choices or ranges. pdal-python generates its stage classes from that at import
time, and `Pipeline.validate()` was removed in 3.0; every error first appears
at `execute()`. A JSON Schema for pipelines was asked for in 2018
([PDAL#2184](https://github.com/PDAL/PDAL/issues/2184)) and is still open.

PDAL's pipeline *format* is the better half: one JSON object per stage, with
`tag` and `inputs` for branches, where GDAL and our `to_json()` keep a command
line as a string. A structured form validates with a plain schema, without a
parser.

The lessons, stated as what a validator should do that PDAL does not: keep
the layers apart; declare options rather than discover them by running code;
parse values strictly; check fields along the real edges between steps, not
against one global layout; return structured errors with a step, an option
and a position, and a non-zero exit code; stay free of side effects.

## What else is used

A survey of schema languages, rule languages, code-first generators, CLI
specs, grammars, workflow standards and geo process specifications, scored
against the four layers. The full report, with a source for every claim, is
[`schema_survey.md`](schema_survey.md); the experiments it mentions ran in a
scratch directory of the session, not in this repo.

| Approach | 1 | 2 | 3 | 4 | Rust as source |
|---|---|---|---|---|---|
| JSON Schema from Rust types (`schemars`), checked by `jsonschema`/pydantic | ✓ | partly | – | – | ✓ |
| GDAL `--json-usage` | ✓ | – (chaining checked, not exported) | – | – | – |
| CEL rules (cel-rust, cel-python, cel-js) | ✓ | ✓ as a fold over state | – | with the schema as input | ✓ |
| CUE, Pkl, TypeSpec, Smithy, protovalidate | ✓ | partly | – | – | no or partly |
| openEO process specifications | ✓ | shallow in the client; the server validates | – | server | – |
| CWL, WDL, Nextflow | ✓ | ✓ typed step inputs and outputs, in the engine | – | – | – |
| tree-sitter grammar with SQL/Vega injections | – | syntax | syntax | – | – |
| datafusion-python, planning against empty tables | – | – | SQL | ✓ | – |
| the Rust validator itself, via pyo3 and wasm | ✓ | ✓ | ✓ | ✓ | ✓ |

What the survey ran:

- One CEL rule set (`#rrggbb`, 0..1, "highlight only if the mark allows it",
  "field exists") gave identical results in cel-rust 0.14.5, cel-python 0.5.0
  and cel-js 8.0.0.
- datafusion-python 54.0 caught experiment 7's layer 4 refusals (`No field
  named …`, the missing GROUP BY) against a table with no rows.
- tree-sitter-sql accepted `SELECT label FROM WHERE`: useful for an editor, not
  as a validator.
- openEO's JS validator accepts any data cube where a data cube is expected,
  with `// ToDo: Check properties` in the code; real validation is left to the
  server.

No schema language covers more than layer 1 and a slice of layer 2. Every
system with typed step chaining (GDAL, CWL, WDL, Nushell, PRQL) does layers 2
to 4 in engine code. The closest prior art to this repo is GDAL itself: the
same `!` syntax, typed step chaining with precise errors, and Python gets the
checks through bindings to the same C++ core.

## The benchmark

To compare speed on equal terms, the step vocabulary of experiment 7 is
written once, as data, in [`spec/steps.json`](spec/steps.json): per step its
positional arguments and flags with a type, a CEL `requires` over the
pipeline state, and what it `sets`. Two validators read that file and run the
same four layers over the same corpus, all 269 pipelines experiment 7's
writers produced:

- [`bench/`](bench/): Rust. Layer 0 is a copy of experiment 6's
  `parse_pipeline`, layer 2 runs the CEL rules with `cel` 0.14.5, layer 3 uses
  experiment 6's Vega parser, layer 4 plans each `sql` step with DataFusion
  54.1 against an empty table carrying the schema that reaches it, then checks
  the channels' fields. The same crate builds as a Python module with the
  `python` feature (pyo3, via maturin).
- [`python/validate.py`](python/validate.py): pure Python, with cel-python and
  datafusion-python 54.0. No layer 3: there is no Python parser for Vega
  expressions.

The input table carries the LAS columns the pipelines use, with LAS types; the
real tile's schema from sedona-pointcloud was not read for this.

```sh
cd experiments/08-pipeline-validation/bench
cargo run --release -- 50                          # native Rust
pip install maturin && maturin develop --release   # the same crate as a Python module
python ../python/bench_pyo3.py 50
pip install cel-python "datafusion==54.*" pyarrow
python ../python/validate.py 5                     # pure Python
```

Measured on 26 Sep 2026 in a Linux container (Intel Xeon, 2.8 GHz, 4 cores;
Rust 1.94.1, Python 3.11.15), so not comparable with the macOS timings
elsewhere in this repo. Mean per pipeline over the 269, after one warm-up
pass: 50 repeats for Rust and pyo3, 10 for pure Python.

| Layer | Rust, native | pure Python |
|---|---|---|
| 0. parse the text | 12.8 µs | 71.7 µs |
| 1. arguments | 1.8 µs | 14.2 µs |
| 2. order and state (CEL) | 71.9 µs | 908.1 µs |
| 3. Vega expressions | 2.7 µs | not available |
| 4. SQL planning and fields (DataFusion) | 361.7 µs | 663.1 µs |
| **total** | **470 µs** | **1,667 µs** |
| the same Rust, called from Python (pyo3), one call per pipeline | 523 µs | |
| the same, one call for all 269 | 508 µs | |

Start-up, once: the Rust validator loads the spec and makes a DataFusion
session in 3 ms (1.9 + 1.0 ms), or 3.2 ms as a pyo3 `Validator()`. Pure Python
takes 138 ms, most of it compiling the CEL rules with cel-python (126 ms).

All three give the same verdicts: every one of the 39 refused pipelines is
flagged, and none of the 230 accepted ones. The pure Python version reaches
39 without layer 3 because each of the five pipelines with a bad Vega
expression also breaks a layer 2 rule. Errors by layer, Rust: 15 in layer 1,
21 in layer 2, 5 in layer 3, 8 in layer 4 (a pipeline can have several). The
flagged pipelines and their errors are in
[`results/rust_flagged.txt`](results/rust_flagged.txt).

What this says about speed:

- **Rust is faster on every layer, and the pyo3 boundary is cheap.** Called
  from Python the same code costs 523 µs against 470 µs natively, and is 3.2×
  faster than pure Python while also checking layer 3. Without layer 4, Rust
  takes 89 µs a pipeline and pure Python 994 µs: 11×.
- **Layer 4 is the cost either way.** Planning SQL in DataFusion is 77 % of the
  Rust time, and pure Python calls the same Rust engine through
  datafusion-python, which is why the gap there is only 1.8×. The corpus has
  269 attempts but 77 distinct pipelines, so caching each plan by its SQL text
  and input schema would remove most of it; not built.
- **Layer 2 is CEL's cost, not the rules'.** cel-rust spends 72 µs a pipeline
  converting the state to CEL values and evaluating; cel-python spends 908 µs.
  The same rules as a Rust fold compiled from the spec would likely cost well
  under a microsecond per step; not measured.
- **Start-up matters for short-lived processes.** 3 ms against 138 ms.

For a validator that runs on every keystroke in an editor, or between an LLM
writer and the chart as in experiment 7, the Rust core is the fast option,
and pyo3 keeps it fast from Python. wasm, for the browser, was not measured.

## Follow-up: the crate

The Rust validator is now a crate of its own,
[`crates/avenger-validate`](../../crates/avenger-validate/), with Python (pyo3)
and wasm bindings and the rules exported as CEL. Experiment 6's parsers moved
into it, so the pipeline that runs and the validator read the text the same
way. On the same corpus and one machine (Apple Silicon), this experiment's
benchmark takes 205 µs a pipeline and the crate 22 µs on a first pass, 10 µs
warm, with the same verdicts; the crate's README has the measurements and
where the difference comes from. The benchmark here now reads the Vega parser
from the crate.

## Follow-up: Altair on Avenger

The same question, a fast check with an error that names its place, applies
to Altair: it validates every chart against the whole Vega-Lite JSON Schema
(705 µs for an 8-row bar chart) before anything is drawn.
[`crates/avenger-altair`](../../crates/avenger-altair/), a spike, makes
Avenger an opt-in backend: `avenger_altair.enable()` has Altair's validation ask
Avenger's own Vega-Lite types first (13.5 µs for the same chart) and draws
with Avenger, falling back to the JSON Schema and the usual renderer for a
chart Avenger does not draw yet. [`altair-avenger.md`](altair-avenger.md)
lays out the steps from there to an Altair whose API and validation come
from Avenger; [FINDINGS.md](../../FINDINGS.md) 19–21 has what Avenger needs
for it, measured on Altair's gallery.

## Not built, not checked

- A wasm build. DataFusion compiles to wasm (the survey cites about 30 MB raw,
  under 10 MB compressed); neither its size nor its speed was measured here.
- A declarative registry inside the real `Step` trait of experiments 6 and 7.
  `spec/steps.json` was written by hand from experiment 7's README and code;
  it is what such a registry would export, not an export.
- The same verdict as experiment 7's chart on each pipeline. The spec's rules
  are simpler than the chart's own checks; the agreement below is on whether a
  pipeline was refused, not on the reason.
- `datafusion-python` with our UDFs (vega-format, sedona) registered as
  signature-only stubs, which planning a real pipeline would need.
- A tree-sitter grammar with injections for SQL and Vega.
- Why PDAL's request for a JSON Schema per stage
  ([PDAL#3209](https://github.com/PDAL/PDAL/issues/3209)) was closed.
- GDAL 3.11 and 3.12 binaries; only 3.13.3 was run.
