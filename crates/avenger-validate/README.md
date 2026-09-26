# avenger-validate

Validate a chart pipeline before it runs, from Rust, from Python and in the
browser, with the same code. The pipelines are the GDAL-style ones of
experiments 6 and 7:

```
read tile.laz ! sql "SELECT classification, count(*) AS n FROM input GROUP BY classification"
  ! bars n --by label ! color green
```

```json
{"valid": false, "steps": 4, "layers": [0, 1, 2, 3, 4], "issues": [
  {"layer": 4, "step": 2, "name": "bars", "arg": "--by", "span": [105, 110],
   "message": "no field `label` in the data here (classification, n)"},
  {"layer": 1, "step": 3, "name": "color", "arg": "colour", "span": [119, 124],
   "message": "`green` is not a #rrggbb colour"}]}
```

An agent that writes charts needs this answer before anything is drawn, and
needs it fast enough to ask on every try: the verdict is back in about 10 µs.
The same checks are meant to sit under Altair, so a Python user can opt in to
Avenger as the backend and get errors that name the step and the argument.

## The layers

The layers come from [experiment 8](../../experiments/08-pipeline-validation/),
which sorted the 39 pipelines experiment 7's chart refused by what it takes to
catch them.

| Layer | Checks | How |
|---|---|---|
| 0. syntax | quotes, steps split at `!` | the parser experiment 6 runs with (it now lives here) |
| 1. arguments | each value against its type, flags that exist, positionals present | a pattern or choice per type ([`spec/steps.json`](spec/steps.json)) |
| 2. order and state | each step's precondition on the pipeline state | a CEL rule per step (`highlight` needs a mark with emphasis) |
| 3. expressions | embedded Vega expressions parse | experiment 6's Vega parser (now here) |
| 4. data | each `sql` step plans against the schema that reaches it; each channel names a field | DataFusion against a table with no rows (feature `sql`) |

Every issue carries its layer, step, argument, message, and the byte range in
the text. The steps are data, in [`spec/steps.json`](spec/steps.json); a
different vocabulary is a different spec (`Spec::from_json`).

## Use

```rust
let v = avenger_validate::Validator::default();   // LAS columns as the source
let report = v.check(text);                        // or v.check_json(&steps)
```

```sh
cd crates/avenger-validate && maturin develop --release
```

```python
import avenger_validate as av
v = av.Validator()                                 # or Validator(schema=[("x", "float64"), ...])
v.check(text)["issues"]
v.check_many(texts); v.check_steps([{"step": "chart", "args": ["bar"], "flags": {"x": "label:N"}}])
```

```sh
cargo build --release -p avenger-validate --lib --target wasm32-unknown-unknown --no-default-features --features wasm
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/avenger_validate.wasm
```

```js
import init, {Validator, exportCel} from './pkg/avenger_validate.js'
await init()
new Validator().check(text).issues
```

```sh
avenger-validate check "<pipeline>"          # the report as JSON; exit 1 when invalid
avenger-validate export-cel > bundle.json    # layers 1 and 2 as CEL
avenger-validate corpus experiments/07-chart-decisions/results 20   # verdicts and timings
```

## Without the library: the CEL export

`export-cel` writes layers 1 and 2 as a JSON bundle of CEL expressions: per
step, a check over `v` for each argument, the `requires` rule over `state` and
`step`, and what the step `sets`; the bundle's `about` states the algorithm. Any CEL implementation can then validate a pipeline given as
data (`[{"step", "args", "flags"}]`), with no Rust. Two reference drivers, of
about 60 lines each, are in [`cel/`](cel/): `avenger_validate_cel.py`
(cel-python) and `avenger_validate_cel.mjs` (cel-js, `@marcbachmann/cel-js`).

Each type is one regular expression, checked natively with the `regex` crate
and exported as the same CEL `matches`, so the two cannot drift. Checked on
every run of `cargo test -p avenger-validate`: each type on 38 values agrees
with its CEL, and the bundle run through cel-rust gives the native layer 1 and
2 issues on all 269 pipelines. [`cel/parity.py`](cel/parity.py) checks
cel-python and cel-js the same way: 269 of 269 agree, in both.

Layers 3 and 4 are not in CEL: a Vega expression needs a parser and a query
needs a planner.

## Measured

Experiment 7's corpus: all 269 pipelines its writers produced (77 distinct),
39 of them refused by the chart. Apple Silicon (M-series), macOS, Rust 1.89,
Python 3.11.6, Node 22.23.2; 26 Sep 2026. Mean per pipeline, warm means after
one pass.

| Way | Layers | Per pipeline | Refused and flagged | Accepted but flagged |
|---|---|---|---|---|
| experiment 8's benchmark, native, on this machine | 0–4 | 205 µs | 39 of 39 | 0 |
| **avenger-validate, native, first pass** | 0–4 | **22.1 µs** | 39 of 39 | 0 |
| avenger-validate, native, warm | 0–4 | 10.2 µs | | |
| from Python (pyo3), one call a pipeline | 0–4 | 13.8 µs | 39 of 39 | 0 |
| from Python, one call for all 269 | 0–4 | 11.4 µs | | |
| wasm in Node | 0–3 | 8.5 µs | 36 of 39 | 0 |
| CEL export in cel-js | 1–2 | 3.6 µs | | |
| CEL export in cel-python | 1–2 | 922 µs | | |

Warm, per layer (native): parse 3.4 µs, arguments 0.4, state 0.8, expressions
1.5, data 2.6. Start-up: 7.7 ms native and from Python, 34 ms for the wasm
module in Node. The wasm module is 3.0 MB, 877 KB gzipped, without `wasm-opt`.
The 3 refusals wasm misses are the ones only layer 4 sees.

Where the factor of 10 over experiment 8 comes from, as measured along the
way (the first step compares experiment 8's benchmark with this crate before
the memo, not an isolated change):

- **CEL, 40 → 0.8 µs.** Experiment 8 built a CEL context per step, which
  registers every built-in function; now one base context is built once and
  each step gets an inner scope (40 → 8 µs). A rule is a pure function of the
  state and the step's arguments, and the state has two fields, so each
  outcome is kept by `(rule, state, arguments)` (8 → 0.8 µs).
- **DataFusion, 144 → 2.6 µs warm, 12.5 µs on the first pass.** Plans are kept
  by SQL text and input schema. A writer's retries repeat the data stages:
  the corpus has 269 attempts but 77 distinct pipelines.

The numbers are not comparable with experiment 8's README, which ran in a
Linux container.

## Not built, not checked

- Layer 4 in wasm. DataFusion compiles to wasm, but its size and speed there
  were not measured; the wasm build leaves the layer out and says so in
  `layers`.
- The spec is still written by hand. Experiments 6 and 7's `Step` trait could
  declare its arguments and export this file; it does not yet. The checks are
  simpler than experiment 7's own: the agreement above is on whether a
  pipeline is refused, not on the reason.
- Messages per rule. A spec entry can carry a `message`, and none does yet,
  so a failed rule prints the rule itself.
- The functions experiment 6 registers (`vega-format`, Sedona's `st_*`) are
  unknown to layer 4, so a `sql` step that calls one fails to plan. None of
  the corpus does.
- `unit` is stricter than in experiment 8 (`1e-1` is no longer a number in
  0..1), so that CEL, Python and JavaScript can agree on one pattern.
- Wheels and an npm package are not published; the Altair integration itself
  is not started.
