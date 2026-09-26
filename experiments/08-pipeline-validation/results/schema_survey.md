# One source of truth for pipeline step definitions: a survey

Date: 2026-09-26. Scope: turning a Rust step registry (GDAL-style `a ! b ! c` pipelines on DataFusion) into validators for Python first, and maybe TypeScript later, across four layers:

- **L1**: per-argument checks (pattern, range, enum).
- **L2**: pipeline-level checks (step order, state-dependent rules, input/output type chaining).
- **L3**: embedded expression languages (Vega expressions, SQL).
- **L4**: data-dependent checks (does the field exist in the Arrow schema, does the SQL plan).

Every claim below has a URL. Anything marked **[unverified]** was not checked this session. Anything marked **[ran]** was run here, and the commands are listed in section 0.

---

## Summary

1. **No schema language covers more than L1 plus a slice of L2.** JSON Schema, CUE, Pkl, KCL, TypeSpec, Smithy, protovalidate and LinkML all handle L1 well. Some (CUE, KCL, Nickel, protovalidate/CEL) can also express cross-field rules. None of them has a notion of state threaded through an ordered sequence of steps. And none can parse an embedded language or plan SQL against a schema.
2. **Every mature system with typed step chaining does the L2–L4 checks in engine code, not in the schema.** That covers GDAL, Nushell, CWL, WDL, openEO, Kubeflow and actionlint. The schema describes the steps; a checker written in a real language folds over the steps.
   - GDAL: [ran] the check is C++ in `gdalalg_abstract_pipeline.cpp`, and `--json-usage` does not export it.
   - Nushell: `input_output_types` in Rust, checked by the parser and at run time.
   - openEO: the JS validator compares a step's declared return schema against the next step's parameter schema. `datacube` matches anything, and the dimension constraints are never checked. [ran] The real validation is a server-side `POST /validation`.
3. **The best-supported approach for our case is to ship the Rust validator itself to Python and TS, not to generate reimplementations.** The Rust validator (tokeniser, registry, Vega parser, DataFusion planner) would be exposed through pyo3/maturin and wasm-bindgen. PRQL, regorus, rudof, KCL and GDAL all ship bindings to one native core. Alongside it, export a declarative spec for editors and for lightweight clients: JSON Schema per step, a table of step kind, input and output types, and CEL preconditions. Generated validators then cover L1 and most of L2, and anything they miss falls back to the native core.
4. **CEL is the one rule language that ran the same rule in Rust, Python and JS here.** [ran] The same highlight-needs-bar-or-line rule gave identical results in cel-rust 0.14.5, cel-python 0.5.0 and @marcbachmann/cel-js 8.0.0. Kubernetes and protovalidate are large-scale precedents for putting CEL in a schema. CEL does not replace the fold over steps, but it is a good way to write each step's precondition on the state.
5. **datafusion-python does L4 schema-only planning in Python.** [ran] Register an empty RecordBatch with the right schema. `ctx.sql(q)` then returns the output schema or a planning error without executing anything. The caveat is that custom UDFs (vega-format, sedona, …) must be registered as signature-only stubs, or planning fails on unknown functions.
6. **tree-sitter is for editors, not validation.** [ran] tree-sitter-sql 0.3.11 parses `SELECT label FROM WHERE` with no error node. Use it for highlighting and structure, with injections for SQL and Vega. Do not treat it as the L3 validator.

---

## 0. What I ran

Everything ran in `…/scratchpad`, with a venv under `scratchpad/venv`.

| Experiment | Command / file | Result |
|---|---|---|
| openEO subtypes | `git clone Open-EO/openeo-processes` (HEAD 98ae014, 2026-01-07) + a Python walk over `*.json` | 118 processes. Subtype counts: `datacube` 76, `process-graph` 14, `date-time` 11, … `raster-cube`/`vector-cube` no longer appear in process files, only in `meta/subtype-schemas.json`. Cube shape is now `{"subtype":"datacube","dimensions":[{"type":"spatial","axis":["x","y"]}]}`, e.g. `aggregate_spatial` takes spatial x/y and returns a `geometry` dimension. |
| openEO chaining check | `git clone Open-EO/openeo-js-processgraphs` (HEAD 5294a7b, 2026-02-26), read `src/jsonschema.js:309-372` and `src/process.js:102-112` | For a `from_node` argument, the validator checks `isSchemaCompatible(param.schema, process.returns.schema)`. For objects it returns true if either side is `datacube`, and there is a `// ToDo: Check properties`. `dimensions` is referenced nowhere in `src/`, so dimension typing is not checked client-side. |
| openEO Python client | read the installed `openeo` 0.52.0 | `openeo/processes.py` header says "automatically generated" by `internal/processes/generator.py` from the spec. Validation is a POST to `/validation` (`rest/connection.py:1132-1152`, `auto_validate=True`). Band names are checked by hand against collection metadata (`rest/datacube.py:157-160, 467-468`), and `aggregate_spatial` returns `VectorCube` via Python type hints. |
| GDAL pipeline | `genv/bin/gdal` (3.13.3, already present in the scratchpad from an earlier run; I re-ran it myself) | `gdal pipeline read in.tif ! reproject --output-crs EPSG:3857 ! buffer 1 ! write x.gpkg` gave `Step 'buffer' expects a vector input dataset, but previous step 'reproject' generates a raster output dataset`. `gdal pipeline --json-usage` has `pipeline_algorithms` whose args carry `type`, `choices`, `min_value`/`max_value`, `mutual_exclusion_group`, `depends_on`, … The pipeline step entries carry no input/output dataset type; the check is C++ at `apps/gdalalg_abstract_pipeline.cpp:1440-1470` (GDAL HEAD 7617801). |
| PDAL | `penv/bin/pdal` 2.10.2 | `pdal --options filters.range --showjson` gives only name/description/default, no types. `pdal pipeline --validate` on a pipeline with a bogus option returned `"filters.range: Unexpected argument 'bogus'."`, `valid:false`. |
| CEL in three languages | `cel_test.py`, `rtest/src/main.rs`, `jstest/t.mjs` | Rules for `#rrggbb`, `focus in [0,1]`, highlight-needs-bar/line and field-in-schema gave identical true/false results in cel-python 0.5.0, cel 0.14.5 (Rust) and @marcbachmann/cel-js 8.0.0. cel-python accepted `x + "a"` at compile time and failed only at evaluation, so it does no static type check. |
| schemars | `rtest` (schemars 1.2.2) | `#[schemars(regex(pattern=…))]`, `range(min,max)`, `length(equal=2)` and a lowercase enum all emitted as JSON Schema 2020-12 (`pattern`, `minimum`/`maximum`, `minItems`/`maxItems`, `enum`). |
| L4 planning in Python | `df_test.py` (datafusion 54.0.0) | Empty RecordBatch registered as `t`: `SELECT label, count(*) AS n FROM t GROUP BY label` gives schema `label: string, n: int64 not null`. `SELECT lbl FROM t` gives `Schema error: No field named lbl. Valid fields are t.label, t.z, t.classification.` A missing GROUP BY gives a planning error. sqlglot 30.19 `qualify(..., validate_qualify_columns=True)` also catches `lbl`, but sqlglot has **no DataFusion dialect** (its dialect list was printed). |
| tree-sitter | `tree-sitter` 0.26 + `tree-sitter-sql` 0.3.11 + `tree-sitter-javascript` | `SELECT label FROM WHERE` and `SELECT FROM FROM t` both parse with `has_error=False`: the grammar is permissive and reads keywords as identifiers. `datum.z > && 2` under the JS grammar gives `has_error=True`. |
| End-to-end prototype | `proto_fold.py` | Spec-as-data: per-step JSON Schema (L1), CEL `requires` folded over `{phase, mark}` state (L2), DataFusion planning of the `sql` step whose output schema feeds `chart --x` field checks (L4). It correctly flags `--color red` (L1), `highlight` after `chart point` (L2), `--focus 0.3,1.6` (L1), `chart` before `read` (L2), `SELECT lbl` (L4) and `chart bar --x z:Q` after an aggregate that dropped `z` (L4). |

---

## (a) Comparison table

Legend: **Y** = native, **P** = partial or only with effort, **N** = no. "Rust as source?" means whether the definitions can originate in Rust code or a Rust toolchain. Maturity is a rough reading of adoption.

| Approach | L1 arg | L2 order/state/types | L3 embedded lang | L4 data | Rust as source? | Python / TS targets | Maturity |
|---|---|---|---|---|---|---|---|
| **JSON Schema** (2020-12) | Y | P (fixed positions via `prefixItems`, `if/then`; no state threading) | N | N | Y via schemars [ran] | Py: jsonschema, pydantic via datamodel-code-generator; TS: ajv, zod converters | Very high |
| **CUE** | Y | P (unification and cross-field constraints; no sequential state) | N | N | P: Go-only engine; `cue-rs` wraps libcue via cgo | cue-py is "upcoming"; exports JSON Schema/OpenAPI | Medium–high |
| **Pkl** (Apple) | Y | P (type constraints with arbitrary expressions) | N | N | N | Official bindings: Java, Kotlin, Swift, Go only; community `pkl-python` 0.1.19 | Medium |
| **KCL** | Y (`check:` blocks) | P | N | N | Y (engine is Rust, Rust API) | SDKs for Python (`kcl-lib` 0.13), Node, Go, Java, .NET | Medium (CNCF sandbox) |
| **Nickel** | Y (contracts) | P | N | N | Y (engine is Rust) | No JSON Schema export yet (issue #2667); Python bindings [unverified] | Low–medium |
| **Dhall** | P (types, no refinements) | N | N | N | P (`dhall-rust`) [unverified] | `dhall` 0.1.16 on PyPI | Low |
| **Jsonnet** | N (templating, no validation) | N | N | N | P | many | High, but off-target |
| **TypeSpec** | Y (decorators emit JSON Schema constraints) | N | N | N | N (TS compiler) | Emits JSON Schema 2020-12, OpenAPI, Protobuf; Python/TS clients in preview | Medium–high (1.0 GA) |
| **Smithy** | Y (`@pattern`, `@range`, `@length`) | N | N | N | Y: smithy-rs generates constrained Rust types (but Smithy IDL is the source) | smithy-python, smithy-typescript | High in AWS |
| **Protobuf + protovalidate** | Y (standard rules and CEL) | P (message-level CEL across fields) | N | N | P: prost-protovalidate / protocheck (community) | protovalidate-py, protovalidate-es (official) | High |
| **Avro / Arrow schema** | N (types and nullability only) | N | N | Y as the *carrier* of L4 schemas | Y (arrow-rs) | pyarrow, arrow-js | Very high (as data schema) |
| **LinkML** | Y | P (rules; SHACL gen) | N | N | P (Rust generator listed) | pydantic, TypeScript, JSON Schema, SHACL generators | Medium (science) |
| **XSD / RelaxNG / Schematron** | Y | P (Schematron XPath assertions) | N | N | N | lxml (Py) [unverified details] | High but legacy for this use |
| **SHACL** | Y | P (SPARQL constraints) | N | N | Y: rudof (Rust) + pyrudof | pyshacl, pyrudof | Medium (RDF-centric) |
| **CEL** (rules) | Y | Y as per-step *preconditions* on a state object, the way K8s transition rules use `oldSelf`; the fold stays in host code | N | P (if the schema is passed in as a variable) [ran] | Y (cel-rust) [ran] | cel-python [ran], Google cel-expr-python (Mar 2026), cel-js [ran], protovalidate-es | High (K8s, Envoy, protovalidate) |
| **OPA / Rego** | Y | Y (whole-document policies over the step list) | N | P | Y: regorus (Rust) with Python (pyo3) and WASM bindings | via regorus | High (OPA); regorus medium |
| **JSON Logic** | P | P | N | N | Y (jsonlogic-rs) | JS, Python, many | Medium; weak typing |
| **schemars** (Rust → JSON Schema) | Y [ran] | N | N | N | **Y** | via JSON Schema tooling | High |
| **typify** (JSON Schema → Rust) | n/a (inverse direction) | | | | reverse | – | Medium (Oxide) |
| **specta / ts-rs** (Rust → TS types) | N (types, not constraints) | N | N | N | Y | TS (specta: TS production-ready; Python/JSON Schema/Zod partial) | Medium–high |
| **pyo3 + maturin + pyo3-stub-gen** | Y (runs Rust code) | Y | Y | Y | **Y** | Python (typed `.pyi`) | High |
| **wasm-bindgen / wasm-pack** | Y | Y | Y | Y | **Y** | TS/JS; DataFusion does build to wasm (datafusion-wasm, ~30 MB raw / <10 MB compressed per maintainers) | Medium–high |
| **uniffi** | Y | Y | Y | Y | Y | Python, Kotlin, Swift, Ruby; **no JS** built in | High (Mozilla) |
| **pydantic / zod** (target-side) | Y | P (model validators) | N | N | – (targets) | Py / TS | Very high |
| **Vega-Lite → Altair codegen** | Y (JSON Schema) | N (runtime warnings in VL) | N | N | TS source via ts-json-schema-generator | Python (Altair via `generate_schema_wrapper.py`) | Very high; precedent for spec → Python API |
| **jdx `usage` (KDL)** | Y (`choices`, `validate=` expr-lang expressions, `requires`/`conflicts`) | N | N | N | Y via `clap_usage` | completions, docs, man; no Py/TS validators | Medium |
| **OpenCLI** | P (`arity`, `acceptedValues`) | N | N | N | N | – | Low (v0.1 draft) |
| **Fig / Amazon Q specs** | P (suggestions, generators) | N | N | N | N | TS spec files | Medium, vendor-controlled |
| **carapace-spec** | P (completion macros) | N | N | N | N | YAML + JSON Schema for the spec | Medium |
| **clap introspection** | Y at run time in Rust (value parsers, ranges) | N | N | N | Y | via clap_complete, clap_mangen, clap_usage → KDL | High |
| **click / typer `to_info_dict`** | P | N | N | N | N | Python-side export | High |
| **GDAL `--json-usage`** | Y (type, choices, min/max, mutual exclusion, depends_on) [ran] | P: checked in C++, not exported [ran] | P (engine-specific) | Y at run time | C++ source | Python via bindings of the same C++ (`gdal.Algorithm(...).GetUsageAsJSON()`) | High |
| **tree-sitter** | N | N (syntax only) | P: injections give structure and highlighting; permissive grammars accept invalid SQL [ran] | N | Y (grammar is JS/JSON, runtime is C with Rust bindings) | Python, Node, WASM bindings | Very high (editors) |
| **Lark / ANTLR / pest / Ohm** | – | P (after parse) | Y if a grammar exists | N | pest: Rust-only; ANTLR Rust target is a community fork | Lark: Python; ANTLR: Py/TS official; Ohm: JS | High |
| **sqlparser-rs / DataFusion** | – | – | Y (SQL) | Y (planning) | Y | datafusion-python [ran], datafusion-wasm | High |
| **sqlglot** | – | – | Y (SQL; no DataFusion dialect) [ran] | P (`qualify` with a schema) [ran] | N | Python only | High |
| **VegaFusion / our own Vega parser** | – | – | Y (Vega expressions, Rust) | Y (compiles to DataFusion exprs) | Y | VegaFusion ships Python and WASM builds | Medium–high |
| **CWL** (schema-salad + cwltool) | Y | Y (static source/sink type checker) | P (JS expressions evaluated, not typed) | N | N | Python reference impl | High (bioinformatics) |
| **WDL** (miniwdl / womtool) | Y | Y (full static type check) | N | N | N (Rust `sprocket` exists [unverified]) | Python (miniwdl), Java (womtool) | High |
| **Nextflow nf-schema** | Y (JSON Schema params, samplesheets) | N | N | P (samplesheet columns) | N | Groovy plugin | High |
| **Galaxy tool XML** | Y | Y (editor marks connections compatible or incompatible by datatype) | N | N | N | Python / JS | High |
| **GitHub Actions**: schemastore JSON Schema vs **actionlint** | schema: Y | actionlint: Y (`needs:`, step outputs, reusable workflows) | actionlint: Y (type-checks `${{ }}`) | P (known contexts) | N | Go | Very high; the clearest case of "schema is not enough" |
| **Kubeflow Pipelines** | Y | Y (compile-time I/O type check; `Artifact` acts as any) | N | N | N | Python DSL | High |
| **openEO** processes | Y (per-param JSON Schema + `subtype`) [ran] | P: `from_node` return vs param schema; `datacube` is a wildcard; dimensions unchecked client-side [ran] | P (child process graphs as callbacks) | P: band names checked against collection metadata in the Python client [ran]; server `/validation` | N | Python client code generated from spec [ran]; JS validator | Medium–high (EO) |
| **OGC API – Processes** | Y (inputs described with JSON Schema) | N in Part 1 (Part 3 workflows [unverified]) | N | N | N | – | High (standard) |
| **QGIS Processing** | Y (typed `QgsProcessingParameter*`) | P (model designer) [unverified details] | P (expressions) | Y at run time | N | PyQGIS | Very high |
| **WhiteboxTools** `--toolparameters` | P (`ExistingFile:Raster`, `OptionList`) | N | N | N | **Y** (the tools are written in Rust) | R/Python front-ends wrap the binary | Medium |
| **GRASS** `--interface-description` / `--json` | Y | N | N | N | N (C) | pygrass reads the XML | High |
| **PDAL** `--options --showjson` | P (name/default only) [ran] | P (`--validate` checks options) [ran] | P (`where` expressions) | N | N | python-pdal | High |
| **Nushell** signatures | Y (`SyntaxShape`) | **Y** (`input_output_types`, parse-time plus run-time) | P | N | **Y** | – (in-process; LSP `nu --lsp`) | High |
| **Kusto.Language** | Y | Y | Y | **Y** (you give it `DatabaseSymbol`s; schema-aware diagnostics and completion) | N (C#, bridged to TS) | TS (monaco-kusto) | High |
| **PRQL** | – | Y (typed pipeline → SQL) | – | P | **Y** (Rust compiler) | Python and JS (wasm) bindings of the same compiler | Medium–high |

---

## (b) The most promising combinations

### 1. Native core, many bindings, plus an exported spec (recommended)

**Recipe.** Keep one Rust crate, `pipeline-check`, that owns:

- the tokeniser (today's `parse_pipeline` in `experiments/06-pipelines/src/pipeline.rs`);
- a declarative step registry;
- the Vega expression parser (`experiments/06-pipelines/src/vega/parse.rs`);
- a planning-only DataFusion session.

It exposes one function, `check(pipeline_text, input_arrow_schema) -> Vec<Diagnostic{step, span, layer, message}>`. Ship that function in three ways:

- **Python**: pyo3 + maturin, with `.pyi` files from pyo3-stub-gen.
- **TypeScript**: wasm-bindgen / wasm-pack.
- **Editors**: an LSP server (`tower-lsp`) over the same function.

**Why.** It is the only option that gives identical answers at all four layers, because L3 and L4 need the real parsers and the real planner. Generating those in Python is not realistic: there is no Python Vega-expression parser, and sqlglot has no DataFusion dialect [ran]. The precedents are:

- PRQL, whose Rust compiler ships as Python and JS (wasm) packages.
- regorus, a Rust Rego engine with pyo3 and WASM bindings.
- rudof/pyrudof and KCL, both Rust engines with Python SDKs.
- GDAL, whose chaining check exists only in C++ yet reaches Python because Python calls the same code.
- openEO, which after years of client-side validators still relies on a server-side `/validation` for anything beyond shallow schema compatibility [ran].

**Costs.** Wheel and wasm size: DataFusion wasm is about 30 MB raw and under 10 MB compressed, per the maintainers. There is also a build matrix. You can cut size by building L4 behind a feature flag.

### 2. A declarative spec exported from Rust, validated natively in Python/TS (for L1 and most of L2)

**Recipe.** Each step is a Rust struct with `#[derive(Deserialize, JsonSchema)]`. That gives L1 through schemars: pattern, range, enum and length all emit correctly [ran]. Beside it goes a small declarative table:

- `kind` (source/transform/command/query, as in today's `Kind` enum);
- `input` and `output` types, in the style of GDAL `GetInputType()` or Nushell `input_output_types`;
- `requires` as a **CEL** expression over `state`;
- `sets` as state updates, e.g. `mark = $mark`;
- a list of `field_args` to check against the current Arrow schema.

Export all of it as one JSON document, similar in shape to GDAL's `--json-usage`. On the Python side:

- `jsonschema`, or pydantic models via datamodel-code-generator, for L1;
- cel-python (or Google's cel-expr-python) plus a 30-line fold for L2;
- datafusion-python for SQL steps, planning against a schema-only table for L4 [ran].

TS uses the same pieces: ajv, cel-js and a fold. `proto_fold.py` is a working 90-line demonstration.

**Why.** It needs no native binary in the client, it gives cheap editor completions and documentation, and it is transparent to readers. CEL is the rule language with the best cross-language story, since the same rule gave identical results in Rust, Python and JS [ran]. It also has Kubernetes and protovalidate as precedents for "rules live in the schema".

**Limits.** L3 for Vega expressions has no Python parser; you either ship the Rust parser (which is combination 1) or maintain a Lark or tree-sitter port. L4 in datafusion-python needs your UDFs registered as signature-only stubs, or planning fails on unknown functions. Version drift is a further risk: pip gave datafusion 54.0 while the repo pins 54.1. Keep the ordering rule in the fold, not in the schema: JSON Schema cannot express "data steps before chart commands" for arbitrary lengths.

### 3. A tree-sitter grammar with injections for the editor layer

**Recipe.** Write `tree-sitter-avpipe`: steps, flags, quoted strings, `!`. Add `injections.scm` rules that inject `sql` into `sql "…"` arguments and a JavaScript grammar into `--vega "…"` arguments. That works because Vega expressions are a restricted esprima/JS subset, per vega-expression. The result gives highlighting, folding and structural queries in Neovim, Helix, Zed and VS Code, and Python/Node/WASM bindings come for free.

**Why not as a validator.** tree-sitter-sql parsed `SELECT label FROM WHERE` with no error [ran], and the JS grammar would accept JS that Vega forbids, such as assignment. So pair it with combination 1 (LSP diagnostics) or combination 2 (spec-driven completion).

### 4. Adopt a CLI-spec format for the argument surface

**Recipe.** Model each step's flags either with clap derive or with a GDAL-like argument declaration: type, choices, min/max, mutual exclusion, depends_on. Then emit a `--json-usage` document, or a jdx `usage` KDL spec via `clap_usage`. You get shell completions, markdown/man docs and a spec that `usage` can lint. `usage` even supports per-arg `validate=` expressions in expr-lang.

**Why.** Our syntax is GDAL's, and GDAL already solved "describe step arguments as JSON" with a published JSON Schema for the usage document (`https://gdal.org/gdal_algorithm.schema.json`, found in the scratchpad from an earlier run). Following its field names (`min_value`, `max_value`, `choices`, `mutual_exclusion_group`, `depends_on`, `dataset_type`) costs nothing and helps GDAL users. Add the two fields GDAL does not export for pipeline steps, `input_type` and `output_type` [ran]. This covers L1 only; it is a way to shape combination 2, not a substitute for it.

### 5. openEO-style process JSON, if interoperability with EO tooling matters

Process JSON puts per-parameter JSON Schema plus `subtype` and, since openEO 2.0, `dimensions` constraints on the `datacube` subtype [ran]. The Python client is generated from these files [ran]. The lesson for us is cautionary. Their JS validator's chaining check treats `datacube` as a wildcard and never inspects `dimensions` [ran], so type chaining written in JSON Schema stayed shallow while the real checks moved server-side. Borrow the vocabulary (a subtype plus a structural refinement like `dimensions`, which maps well onto "Arrow table with fields X"), not the validation strategy.

**Recommended mix.** Build combination 1 for correctness, export the spec from combination 2 (shaped like combination 4) for completions, docs and binary-free clients, and add combination 3 when an editor matters. In all of them the Rust registry is the single source: the spec JSON is emitted from it, and CEL `requires` strings live next to the step definitions in Rust.

---

## (c) Prior art closest to our exact problem

Ranked by closeness to "typed step chaining with a text pipeline syntax, with definitions in the host language".

1. **GDAL `gdal pipeline`** (3.11+; `--json-usage` since 3.12). It uses the same `!` syntax, and each step is a C++ `GDALAlgorithm` declaring typed args with choices and min/max, emitted as JSON [ran]. Steps have input/output types (raster/vector), and the pipeline rejects mismatches with a precise message [ran]. Python gets the same checks by calling the same core. The gap is that chaining types are not in the exported JSON.
2. **Nushell** (Rust). Commands register a `Signature` with `SyntaxShape` args and `input_output_types`. The parser type-checks pipelines at parse time, and since PR #14741 pipeline input types are also checked at run time. Of the systems surveyed, it is closest to "Rust registry as source with parse-time chaining checks". It has no cross-language export beyond `nu --lsp`.
3. **PRQL** (Rust). A text pipeline language compiled to SQL. It ships the compiler itself to Python and JS rather than generating validators, which is the approach recommended above.
4. **Kusto.Language (KQL).** A piped query language whose analyzer takes a schema (`DatabaseSymbol` in `GlobalState`) and gives schema-aware diagnostics and completions. That is our L4 in an editor. The C# core is bridged to TS for monaco-kusto.
5. **openEO process graphs.** Typed EO processes with subtype-based chaining and a generated Python client. The client-side chaining check is shallow [ran]. JSON graph, not text syntax.
6. **CWL / WDL / Kubeflow.** Typed step I/O with static checkers: cwltool's `static_checker` for source/sink compatibility, miniwdl's full type check, and KFP's compile-time type check. Different domain, same "checker over declared step types" structure.
7. **actionlint.** The canonical proof that a JSON Schema (schemastore) catches L1 while L2 and L3 need a real checker. actionlint type-checks `${{ }}` expressions and step outputs.
8. **VegaFusion** (by the Avenger author). A Rust Vega expression parser compiling to DataFusion expressions, with WASM compatibility. It is the nearest existing L3+L4 component in Rust for Vega expressions, as an alternative or reference for the repo's own parser.

---

## Not verified / open

- Dhall's current Rust status; Nickel Python bindings; OGC API Processes Part 3 workflow chaining; WPS DescribeProcess; SAGA; Snakemake; Argo/Tekton (CRD OpenAPI plus CEL presumably, not checked); Schematron/RelaxNG/XSD tooling specifics; exactly how Galaxy decides datatype-subclass compatibility; whether the WhiteboxTools R/Python wrappers are generated from `--toolparameters`; the QGIS model designer's connection typing; Fig's maintenance status after the Amazon Q transition.
- I did not build a tree-sitter grammar with actual injections, a pyo3 or wasm build of the repo's pipeline crate, or datafusion-python stub UDFs. These are the next cheap experiments if combination 1 or 2 is pursued.
- JSON Schema's inability to express "all data steps precede all chart steps" for arbitrary-length arrays is my analysis of the 2020-12 keywords (`prefixItems`, `items`, `contains`), not a cited statement.

---

## Sources

**Schema languages**
- JSON Schema 2020-12: https://json-schema.org/draft/2020-12
- CUE ↔ JSON Schema/OpenAPI: https://cuelang.org/docs/concept/how-cue-works-with-json-schema/ , https://cuelang.org/docs/concept/how-cue-works-with-openapi/
- cue-py: https://github.com/cue-lang/cue-py (issue https://github.com/cue-lang/cue/issues/3100)
- cue-rs (libcue via cgo): https://docs.rs/cue-rs/latest/cue_rs/
- Pkl bindings (Java, Kotlin, Swift, Go): https://pkl-lang.org/main/current/language-bindings.html
- KCL validation and SDKs: https://www.kcl-lang.io/docs/user_docs/guides/validation , https://www.kcl-lang.io/docs/reference/xlang-api/rust-api , https://github.com/kcl-lang/kcl
- Nickel: https://nickel-lang.org/ ; contracts to JSON Schema: https://github.com/nickel-lang/nickel/issues/2667
- TypeSpec: https://typespec.io/docs/emitters/json-schema/reference/ , https://typespec.io/docs/emitters/clients/introduction/
- Smithy constraint traits: https://smithy.io/2.0/spec/constraint-traits.html ; smithy-rs constrained types: https://smithy-lang.github.io/smithy-rs/design/rfcs/rfc0025_constraint_traits.html
- protovalidate: https://github.com/bufbuild/protovalidate , https://protovalidate.com/cel/how-cel-works/ ; Rust: https://docs.rs/prost-protovalidate/latest/prost_protovalidate/ , https://lib.rs/crates/protocheck
- LinkML generators: https://linkml.io/linkml/generators/index.html , https://linkml.io/linkml/generators/pydantic.html , https://linkml.io/linkml/generators/typescript.html
- rudof / pyrudof (SHACL/ShEx in Rust): https://github.com/rudof-project/rudof , https://pypi.org/project/pyrudof/0.3.21/
- Avro spec: https://avro.apache.org/docs/1.12.0/specification/ ; Arrow schema: https://arrow.apache.org/docs/format/Columnar.html (both from prior knowledge; not fetched this session)

**Rule languages**
- CEL in Kubernetes: https://kubernetes.io/blog/2022/09/29/enforce-immutability-using-cel/ , https://github.com/kubernetes/enhancements/blob/master/keps/sig-api-machinery/2876-crd-validation-expression-language/README.md
- cel-rust: https://github.com/cel-rust/cel-rust , https://crates.io/crates/cel
- cel-python (cloud-custodian): https://github.com/cloud-custodian/cel-python
- cel-expr-python (Google): https://opensource.googleblog.com/2026/03/announcing-cel-expr-python-the-common-expression-language-in-python-now-open-source.html , https://github.com/cel-expr/cel-python
- regorus: https://github.com/microsoft/regorus , https://github.com/microsoft/regorus/tree/main/bindings/wasm
- JSON Logic: https://jsonlogic.com/operations.html , https://github.com/jwadhams/json-logic-js/

**Code-first generation**
- schemars: https://docs.rs/schemars
- typify: https://github.com/oxidecomputer/typify
- specta: https://github.com/specta-rs/specta
- pyo3-stub-gen: https://github.com/jij-inc/pyo3-stub-gen
- uniffi: https://github.com/mozilla/uniffi-rs/blob/main/README.md
- Altair codegen: https://github.com/vega/altair/blob/main/tools/generate_schema_wrapper.py , https://altair-viz.github.io/user_guide/internals.html
- ts-json-schema-generator: https://github.com/vega/ts-json-schema-generator

**CLI specs**
- jdx usage: https://github.com/jdx/usage , https://usage.jdx.dev/spec/reference/arg , https://usage.jdx.dev/spec/integrations/clap
- OpenCLI: https://github.com/spectreconsole/open-cli , https://opencli.org/
- Fig: https://github.com/withfig/autocomplete
- carapace-spec: https://github.com/carapace-sh/carapace-spec
- click `to_info_dict`: https://click.palletsprojects.com/api/

**Grammars**
- tree-sitter injections: https://tree-sitter.github.io/tree-sitter/3-syntax-highlighting.html ; bindings: https://tree-sitter.github.io/tree-sitter/
- tree-sitter-sql: https://github.com/derekstride/tree-sitter-sql
- ANTLR targets: https://github.com/antlr/antlr4/blob/master/doc/targets.md ; Rust fork: https://github.com/antlr4rust/antlr4
- Langium: https://langium.org/
- vega-expression: https://github.com/vega/vega/tree/main/packages/vega-expression
- VegaFusion parser: https://docs.rs/vegafusion-core/latest/vegafusion_core/expression/parser/index.html , https://vegafusion.io/about/technology.html
- DataFusion wasm: https://github.com/apache/datafusion/issues/13715 , https://github.com/datafusion-contrib/datafusion-wasm-bindings
- PRQL bindings: https://prql-lang.org/book/project/bindings/python.html , https://prql-lang.org/book/project/bindings/javascript.html

**Workflows**
- cwltool checker: https://cwltool.readthedocs.io/en/latest/autoapi/cwltool/checker/index.html
- miniwdl check: https://miniwdl.readthedocs.io/en/latest/check.html
- nf-schema: https://nextflow-io.github.io/nf-schema/latest/nextflow_schema/nextflow_schema_specification/
- Galaxy editor connections: https://training.galaxyproject.org/training-material/topics/galaxy-interface/tutorials/workflow-editor/tutorial.html
- actionlint: https://github.com/rhysd/actionlint/blob/main/docs/checks.md
- Kubeflow type checking: https://www.kubeflow.org/docs/components/pipelines/user-guides/core-functions/compile-a-pipeline/ , https://www.kubeflow.org/docs/components/pipelines/user-guides/data-handling/artifacts/

**Geo**
- openEO processes: https://github.com/Open-EO/openeo-processes
- openeo-js-processgraphs: https://github.com/Open-EO/openeo-js-processgraphs
- openEO validation endpoint discussion: https://github.com/Open-EO/openeo-python-client/issues/404 , https://github.com/Open-EO/openeo-api/issues/144
- openeo-pg-parser-networkx: https://github.com/Open-EO/openeo-pg-parser-networkx
- OGC API – Processes Part 1: https://docs.ogc.org/is/18-062r2/18-062r2.html
- QGIS: https://docs.qgis.org/3.44/en/docs/user_manual/processing/standalone.html , https://qgis.org/pyqgis/3.44/core/QgsProcessingParameterDefinition.html
- WhiteboxTools: https://github.com/jblindsay/whitebox-tools/blob/master/readme.txt , https://cran.r-project.org/web/packages/whitebox/vignettes/datasets.html
- GRASS g.parser: https://grass.osgeo.org/grass-stable/manuals/g.parser.html
- GDAL pipeline and `--json-usage`: https://gdal.org/en/latest/programs/gdal_pipeline.html , https://gdal.org/en/stable/programs/gdal_cli_from_python.html
- PDAL pipeline validate: https://pdal.io/en/stable/pipeline.html

**Pipelines with text syntax / editors**
- Nushell: https://www.nushell.sh/contributor-book/plugins.html , https://github.com/nushell/nushell/pull/14741
- Kusto.Language schemas: https://learn.microsoft.com/en-us/kusto/api/netfx/kusto-language-define-schemas?view=microsoft-fabric , https://github.com/Azure/monaco-kusto
- yaml-language-server: https://github.com/redhat-developer/yaml-language-server
