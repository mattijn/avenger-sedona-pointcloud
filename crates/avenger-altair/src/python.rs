//! `avenger_altair._native`: the Rust side of the Python package.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use crate::{Format, Refusal};

fn refusal<'py>(py: Python<'py>, r: &Refusal) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("path", &r.path)?;
    d.set_item("message", &r.message)?;
    d.set_item("stage", r.stage)?;
    Ok(d)
}

/// `validate(spec_json) -> None | {"path", "message", "stage"}`
#[pyfunction]
fn validate<'py>(py: Python<'py>, spec: &str) -> PyResult<Option<Bound<'py, PyDict>>> {
    match crate::validate(spec) {
        Ok(_) => Ok(None),
        Err(r) => Ok(Some(refusal(py, &r)?)),
    }
}

/// `render(spec_json, format="svg", scale=2.0, base_dir=None) -> (bytes, None) | (None, refusal)`
#[pyfunction]
#[pyo3(signature = (spec, format = "svg", scale = 2.0, base_dir = None))]
fn render<'py>(py: Python<'py>, spec: &str, format: &str, scale: f32, base_dir: Option<String>) -> PyResult<(Option<Bound<'py, PyBytes>>, Option<Bound<'py, PyDict>>)> {
    let f = match format {
        "svg" => Format::Svg,
        "png" => Format::Png,
        other => return Err(PyValueError::new_err(format!("format is svg or png, not {other}"))),
    };
    let spec = spec.to_string();
    // Rendering does not touch Python objects; let other threads run.
    let r = py.allow_threads(move || crate::render(&spec, f, scale, base_dir.as_deref()));
    match r {
        Ok(b) => Ok((Some(PyBytes::new(py, &b)), None)),
        Err(e) => Ok((None, Some(refusal(py, &e)?))),
    }
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(validate, m)?)?;
    m.add_function(wrap_pyfunction!(render, m)?)?;
    Ok(())
}
