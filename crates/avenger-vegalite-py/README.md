# avenger-vegalite (Python)

Avenger's Vega-Lite validation for Python and nothing else: the typed spec of
`avenger-vegalite-spec` behind pyo3, without the compiler, the renderer or
DataFusion. It is the piece an Altair validator hook needs.

```python
import avenger_vegalite
avenger_vegalite.validate(json_text)   # None, or (path, message)
```

```sh
cd crates/avenger-vegalite-py && maturin build --release
```

Measured on 26 Sep 2026 (Apple Silicon, Python 3.11.6, Avenger `f4890be`),
an 8-row Altair bar chart, next to the same call in `avenger-altair`, whose
module also carries the renderer:

| | `avenger_vegalite` | `avenger_altair._native` |
|---|---|---|
| validate | 7.1 µs | 6.3 µs |
| module | 1.0 MB (wheel 387 KB) | 89.4 MB |
| import | 3.1 ms | 268 ms |
| crates in the dependency tree | 33 (serde, serde_json, serde_path_to_error, thiserror, pyo3) | the whole stack |

The validation is the same work in both; what differs is what comes with it.
