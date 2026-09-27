# The bigger picture

Nine experiments, four crates and one findings ledger, drawn as one system.
The views follow the [C4 model](https://c4model.com) (context, containers,
components, and dynamic views for the main paths), then a
[Wardley map](https://learnwardleymapping.com) for what evolves where, then a
map of the questions the experiments asked. Diagrams are Mermaid, so GitHub
draws them and they change in the same commits as the code.

State of 27 Sep 2026, Avenger pinned at `f4890be` (#130). Numbers are copied
from the experiment READMEs, with the machine they were measured on where it
matters; they are not re-measured here.

## The one-sentence version

Every experiment is a new front door onto the same spine: a lazy DataFusion
plan over the tile, Arrow in between, and Avenger drawing natively. What grew
around that spine, experiment by experiment, is a **vocabulary as data**: the
steps, the chart commands and Vega-Lite's types written down once, validated
in microseconds everywhere, and followed by whatever drives the chart, a
person, a classifier, an LLM or Altair.

## 1. Context

Who and what the repo touches.

```mermaid
flowchart TB
  user(["Analyst<br/>HKV / SVASEK engineer"])
  agent(["Agent<br/>an LLM that writes charts"])
  jon(["Jon Mease<br/>Avenger author"])

  repo["avenger-sedona-pointcloud<br/>experiments, crates, FINDINGS.md"]

  avenger[["Avenger<br/>jonmmease/avenger, an open PR stack"]]
  sedona[["SedonaDB<br/>sedona-pointcloud, sedona-functions"]]
  df[["DataFusion 54 / Arrow 58"]]
  ign[("IGN LiDAR HD tile<br/>COPC/LAZ, 17.3M points")]
  or[["OpenRouter<br/>Jev 1.13, Claude Haiku 4.5"]]
  altair[["Altair 6 / Vega-Lite 6"]]
  refs[["GDAL, PDAL, VegaFusion, Vega<br/>references, not linked"]]

  user -- "asks questions of the point cloud" --> repo
  agent -- "writes pipelines" --> repo
  repo -- "FINDINGS.md: measured findings" --> jon
  jon -- "pushes commits; we repin" --> avenger
  repo -- "draws with" --> avenger
  repo -- "reads points with" --> sedona
  sedona --> df
  avenger --> df
  repo -- "downloads" --> ign
  repo -- "decisions" --> or
  altair -- "charts validated and drawn by" --> repo
  repo -. "syntax and lessons from" .-> refs
```

The repo has two outputs of equal weight: charts that work, and a ledger of
what Avenger should change, each entry with the command that measures it.

## 2. Containers

What runs, and how it is run. Solid arrows are crate dependencies; dashed
arrows are "grew out of" or "reads the output of".

```mermaid
flowchart TB
  subgraph exp["experiments (each on lidar-common: the LAS session, palette, text)"]
    direction LR
    e1["01 lidar-charts<br/>PNGs, explorer, window bench"]
    e2["02 lidar-lang<br/>.avenger chart"]
    e3["03 lidar-stream<br/>Flight server + live window"]
    e4["04 coupling patterns<br/>proposal"]
    e5["05 lidar-coords<br/>coordinate systems"]
    e6["06 lidar-pipeline<br/>pipeline CLI"]
    e7["07 lidar-decide<br/>autopilot, evaluations"]
    e8["08 validation bench<br/>outside the workspace"]
    e9["09 Altair on Avenger<br/>design"]
  end

  subgraph crates["crates"]
    direction LR
    val["avenger-validate<br/>CLI · Python · wasm · CEL"]
    alt["avenger-altair<br/>spike: Python, CLI, notebook"]
    vls["avenger-vegalite-spec<br/>Jon's types + schema export"]
    vlp["avenger-vegalite-py<br/>1 MB validator"]
  end

  probes["lidar-probes<br/>re-measures FINDINGS 1–7"]

  e3 -. "aggregate states" .-> e4
  e5 -. "Bend" .-> e7
  e7 --> e6
  e7 -. "269 pipelines" .-> e8
  e6 --> val
  e8 -. "became" .-> val
  e9 -. "grew" .-> alt & vlp
  alt --> vls
  vlp --> vls
```

| Container | Runs as | Output |
|---|---|---|
| lidar-charts | binaries | PNG, winit window |
| lidar-stream | gRPC server + window | frames, MP4 |
| lidar-pipeline | CLI | PNG, pipeline JSON, chart-definition bytes |
| lidar-decide | window + evaluation binaries | window, session log, results JSON/MD |
| avenger-validate | CLI, pyo3 module, wasm (3.0 MB) | diagnostics JSON, CEL bundle |
| avenger-altair | pyo3 module (89 MB), CLI, anywidget | PNG, SVG, notebook view |
| avenger-vegalite-py | pyo3 module (1 MB) | `None` or `(path, message)` |

## 3. Components

### The spine

```mermaid
flowchart LR
  tile[("tile.copc.laz")] --> sp["sedona-pointcloud<br/>DataFusion file format"]
  sp --> plan["lazy LogicalPlan<br/>SQL, filters, Vega expressions"]
  plan --> arrow["Arrow arrays"]
  arrow --> scales["avenger-scales, guides"]
  scales --> sg["scenegraph"]
  sg --> gidx["geometry index<br/>hit testing"]
  sg --> wgpu["avenger-wgpu"]
  wgpu --> out["window · PNG · SVG · frames"]

  flight["Arrow Flight"] --> states["aggregate states<br/>fold per batch, merge per frame"] --> arrow
  vl["Vega-Lite JSON"] --> vlc["avenger-vegalite-compiler"] --> cd["ChartDefinition + dataflow"] --> arrow
```

The one measured bottleneck on this spine sits after the data and before the
GPU: rebuilding the geometry index costs about 1.2 µs a mark, 387 ms for
300k symbols against 21 ms to render them (FINDINGS 1–3).

### The vocabulary and its checks

```mermaid
flowchart TB
  spec["spec/steps.json<br/>steps, argument types,<br/>CEL requires / sets"]
  vlt["avenger-vegalite-spec<br/>Vega-Lite types"]

  subgraph v["avenger-validate"]
    l0["0 syntax"] --> l1["1 arguments"] --> l2["2 order and state (CEL)"] --> l3["3 Vega expressions"] --> l4["4 data: SQL planned on empty tables"]
  end

  spec --> v
  vlt --> js["JSON Schema + x-avenger-rules CEL"]
  v --> cel["CEL bundle"]
  v --> py["pyo3"] & wasm["wasm"]
  js --> portable["portable validator<br/>jsonschema + cel-python / ajv + cel-js"]
  vlt --> vlp["avenger-vegalite-py"]
```

Two vocabularies, one pattern: types in Rust, rules that JSON Schema cannot
state exported as CEL, the native validator shipped to Python and the browser,
and a portable fallback for anyone without the binary.

### The chart layer of experiment 7

```mermaid
flowchart LR
  text["typed text"] --> pilot["pilot: Jev's questions"]
  pilot -->|"covered, confidence ≥ 0.5"| lines["pipeline lines"]
  pilot -->|"otherwise"| writer["writer: Haiku"] --> lines
  lines --> check["validate"]
  check -->|"refused, with reason"| writer
  check --> fold["fold the command log<br/>into State"]
  fold --> resolve["resolve(state, data)<br/>keyed items"]
  resolve --> anim["transition(a, b, t)"]
  anim --> draw["draw through a coordinate system<br/>(experiment 5's Bend)"]
```

## 4. Dynamic views

### A decision, from text to pixels

```mermaid
sequenceDiagram
  actor U as Analyst
  participant P as Pilot (Jev)
  participant W as Writer (Haiku)
  participant V as Validator
  participant D as DataFusion
  participant C as Chart layer
  U->>P: "only buildings, as a map"
  P->>P: choose from options (≈290 ms p50)
  alt options cover it
    P->>V: pipeline lines
  else they don't
    P->>W: direction + current pipeline
    W->>V: pipeline text (≈1.1 s)
    V-->>W: refused: reason names the way that works
    W->>V: second try
  end
  V->>D: plan (lazy)
  D->>C: Arrow
  C->>U: keyed transition, drawn
```

### Altair, drawn by Avenger

```mermaid
sequenceDiagram
  actor A as Python user
  participant Al as Altair
  participant S as avenger-vegalite-spec
  participant K as vegalite-compiler + dataflow
  participant R as avenger-chart
  A->>Al: alt.Chart(df).mark_bar()...
  Al->>S: validate in 6.5 µs, against 705 µs for the JSON Schema
  S-->>Al: ok, or the path of the property
  Al->>K: spec + Arrow via PyCapsule (no JSON rows)
  K->>R: ChartDefinition, TableSnapshot
  R-->>A: PNG in 19 ms, or the previous renderer as fallback
```

### A live feed

```mermaid
sequenceDiagram
  participant S as stream_server
  participant L as stream_live
  participant R as Avenger
  loop every batch
    S->>L: Arrow Flight batch, GPS-time order
    L->>L: fold into max/count states per 2 m cell (2–7 ms)
  end
  loop every frame, ≤ 25/s
    L->>L: merge the rolling window (2–11 ms), roll up above ~55k cells
    L->>R: request_render
    R->>R: rebuild geometry index (dominates)
  end
```

## 5. Wardley map

The user need at the top, the commodity at the bottom; evolution from genesis
on the left to commodity on the right. Paste the block into
[onlinewardleymaps.com](https://onlinewardleymaps.com) to draw it.

```
title Questioning a point cloud with charts
anchor Analyst [0.98, 0.45]
anchor Agent [0.98, 0.20]

component Answer to a question [0.90, 0.35]
component Altair API [0.80, 0.78]
component Decider (Jev, Haiku) [0.78, 0.22]
component Pipeline language [0.70, 0.15]
component Validator [0.64, 0.12]
component Vocabulary as data [0.58, 0.08]
component Chart layer, transitions [0.56, 0.10]
component Coordinate systems [0.50, 0.14]
component Vega-Lite compiler [0.52, 0.32]
component Avenger scenegraph, scales, guides [0.44, 0.38]
component Geometry index [0.38, 0.30]
component Aggregate states [0.40, 0.28]
component LLM API [0.62, 0.55]
component CEL [0.46, 0.58]
component JSON Schema [0.46, 0.85]
component sedona-pointcloud [0.30, 0.30]
component DataFusion [0.30, 0.68]
component Arrow Flight [0.26, 0.66]
component Arrow [0.20, 0.86]
component wgpu [0.14, 0.72]
component COPC/LAZ tile [0.10, 0.80]
component GPU [0.04, 0.95]

Analyst->Answer to a question
Agent->Answer to a question
Analyst->Altair API
Answer to a question->Decider (Jev, Haiku)
Answer to a question->Pipeline language
Decider (Jev, Haiku)->LLM API
Decider (Jev, Haiku)->Vocabulary as data
Pipeline language->Validator
Validator->Vocabulary as data
Validator->CEL
Altair API->Vega-Lite compiler
Altair API->JSON Schema
Pipeline language->Chart layer, transitions
Chart layer, transitions->Coordinate systems
Chart layer, transitions->Avenger scenegraph, scales, guides
Vega-Lite compiler->Avenger scenegraph, scales, guides
Avenger scenegraph, scales, guides->Geometry index
Avenger scenegraph, scales, guides->wgpu
Pipeline language->DataFusion
Aggregate states->DataFusion
Aggregate states->Arrow Flight
DataFusion->sedona-pointcloud
sedona-pointcloud->COPC/LAZ tile
DataFusion->Arrow
wgpu->GPU

evolve Validator 0.45
evolve Vocabulary as data 0.40
evolve Vega-Lite compiler 0.55
evolve Avenger scenegraph, scales, guides 0.60
evolve sedona-pointcloud 0.55

note genesis: built here [0.75, 0.05]
note custom: Avenger, moving to product [0.35, 0.45]
note commodity: use, don't build [0.12, 0.90]
note FINDINGS 1–3: the frame budget [0.35, 0.22]
```

What the map says:

- **Genesis is where this repo adds value:** the pipeline language, the
  vocabulary as data, the validator, the decider loop, the chart layer with
  transitions, coordinate systems as one trait. None of it exists as a product
  elsewhere; GDAL comes closest for pipelines, and it does not export its step
  types.
- **Avenger is custom-built and moving towards product**, and the ledger is
  how this repo pushes it: every finding is a friction point on that move
  (the geometry index, the spec types refusing Altair's `config`, 0 of 117
  gallery examples compiling).
- **The bottom is commodity and should stay bought:** Arrow, DataFusion,
  Flight, wgpu, JSON Schema, CEL. The measured wins came from keeping data in
  Arrow across boundaries (1M rows: 48 ms via Arrow, 2.2 s as JSON rows) and
  from native code (validation: 6.5 µs against 705 µs).
- **What should move right next:** the validator and the Vega-Lite additions,
  from this repo into Avenger or a published crate (`avenger-vegalite-spec`'s
  schema export, #141 for the dataflow charge). Doctrine: once a genesis piece
  works, hand it to the component that will own it.

## 6. The questions, and what each answer grew

```mermaid
flowchart TB
  q1["1 · Can Avenger draw 17.3M points from SQL?<br/><b>yes</b>; window fetch 1.1 s → 69 ms with chunk statistics"]
  q2["2 · Can the chart language say it in less code?<br/><b>74 lines</b>; first open 1.8–2.1 s"]
  q3["3 · Can it keep up with a live feed?<br/><b>11 fps at ~40k cells</b>; the geometry index is the budget"]
  q4["4 · Which coupling patterns help?<br/><b>proposal</b>: pixel binning, mark budget, Flight table"]
  q5["5 · One coordinate system for charts and maps?<br/><b>~20-line resolve</b>; Lambert-93 within 0.27 px"]
  q6["6 · Vega expressions + SQL in one lazy pipeline?<br/><b>67 % of Vega compiles</b>; lazy 95 ms vs eager 992 ms"]
  q7["7 · Can a classifier or an LLM drive a chart?<br/><b>Jev 19/19 at 290 ms</b>; writer 25/25; $0.91 in total"]
  q8["8 · Can the vocabulary be validated everywhere, fast?<br/><b>39/39 refusals</b>; 10 µs warm in the crate"]
  q9["9 · Can Avenger be Altair's engine?<br/><b>validation 6.5 µs vs 705 µs</b>; 0/117 gallery compiles yet"]

  f[("FINDINGS.md<br/>24 entries")]

  q1 --> q2 & q3 & q5
  q3 --> q4
  q1 --> q6
  q6 --> q7
  q5 --> q7
  q7 --> q8
  q6 --> q8
  q8 --> q9
  q1 & q3 & q6 & q7 & q9 -.-> f
```

Read top to bottom, the questions moved from *can it draw* (1–3), to *how
should charts be described* (4–6), to *who drives the chart* (7), to *how is
what they write checked, fast, in every language* (8–9). The next question on
that line is where the vocabulary lives: in this repo, in Avenger, or in a
crate of its own that Altair, the pipeline and a decider all read.

## Not in these views

- The Code level of C4. It changes with every experiment; the READMEs name
  the files.
- A deployment view beyond the table in section 2. Nothing here is deployed;
  everything runs on one machine.
- Positions on the Wardley map are judgements, not measurements. The
  evolution axis in particular is an opinion to argue with.
- Numbers come from different machines (Apple Silicon for most, a Linux
  container for experiment 8's benchmark) and from different Avenger
  revisions; each README says which.
