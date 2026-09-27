//! `avenger_vegalite.validate(spec_json) -> None | (path, message)`: Avenger's
//! Vega-Lite types (`avenger-vegalite-spec`) for Python, without the compiler,
//! the renderer or DataFusion.

use pyo3::prelude::*;

#[pyfunction]
fn validate(spec: &str) -> Option<(String, String)> {
    avenger_vegalite_spec::UnitSpec::from_json(spec).err().map(|e| (e.path().to_string(), e.message().to_string()))
}

#[pymodule]
fn avenger_vegalite(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(validate, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))
}
