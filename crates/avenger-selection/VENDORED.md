# avenger-selection, carried here

Jon Mease's `avenger-selection` from
[jonmmease/avenger](https://github.com/jonmmease/avenger) at `f4890be`
(BSD-3-Clause, [LICENSE](LICENSE)). Nothing in the workspace depended on the
git crate, so this copy is an ordinary path crate. At this commit it is the
upstream source unchanged; only `Cargo.toml` differs, where the path
dependencies point at the workspace's git pin. All 61 upstream tests pass:

```sh
cargo test --release -p avenger-selection
```

The aim is to extend it until it expresses every selection experiment 7
draws (FINDINGS.md, "Selections in avenger-selection").
