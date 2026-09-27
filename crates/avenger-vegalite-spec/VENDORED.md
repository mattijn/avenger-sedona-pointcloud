# avenger-vegalite-spec, carried here

Jon Mease's `avenger-vegalite-spec` from
[jonmmease/avenger](https://github.com/jonmmease/avenger) at `f4890be`
(BSD-3-Clause, [LICENSE](LICENSE)), with additions that are not upstream yet.
The workspace uses it in place of the git crate through
`[patch."https://github.com/jonmmease/avenger"]`, so Avenger's own compiler
reads these types too. [UPSTREAM.diff](UPSTREAM.diff) is the whole difference.

The additions:

- **`schema` feature** (`src/schema.rs`, `examples/json_schema.rs`): a JSON
  Schema of the types, with the rules JSON Schema cannot state as CEL in
  `x-avenger-rules` (FINDINGS.md 24).
- **`config`** (`Config`, `ViewConfig`): `config.view` with its continuous
  sizes, step, stroke and fill, which Altair's default theme sets on every
  chart (FINDINGS.md 19). The compiler does not size by it yet;
  `avenger-altair` writes the continuous sizes out as width and height.
- **`camera`** (`Camera`): flat, a Sarkar-Brown fisheye (`focus`, `radius`,
  `distortion`), or a tilt into 3D (`yaw`, `elevation`). Not Vega-Lite, whose
  `view` is the plot background. The compiler ignores it; `avenger-altair`
  draws it.

Regenerate the schema the bridge ships with:

```sh
cargo run -p avenger-vegalite-spec --features schema --example json_schema \
  > crates/avenger-altair/python/avenger_altair/avenger-vegalite.schema.json
```
