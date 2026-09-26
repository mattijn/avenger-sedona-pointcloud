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

/// A table from anything with `__arrow_c_stream__` (pandas, Polars, PyArrow):
/// the Arrow PyCapsule interface, without copying through JSON.
fn table(obj: &Bound<'_, PyAny>) -> PyResult<avenger_datafusion_dataflow::TableSnapshot> {
    use arrow::ffi_stream::{ArrowArrayStreamReader, FFI_ArrowArrayStream};
    use arrow::record_batch::RecordBatchReader;
    use pyo3::types::PyCapsule;
    let capsule = obj.call_method0("__arrow_c_stream__")?;
    let capsule = capsule.downcast::<PyCapsule>()?;
    // Move the stream out of the capsule; the capsule then releases an empty one.
    let stream = unsafe { std::ptr::replace(capsule.pointer() as *mut FFI_ArrowArrayStream, FFI_ArrowArrayStream::empty()) };
    let reader = ArrowArrayStreamReader::try_new(stream).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let schema = reader.schema();
    let batches = reader.collect::<Result<Vec<_>, _>>().map_err(|e| PyValueError::new_err(e.to_string()))?;
    avenger_datafusion_dataflow::TableSnapshot::from_batches(schema, batches).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// `render(spec_json, format="png", scale=2.0, base_dir=None, tables=None) -> (bytes, None) | (None, refusal)`;
/// `tables` maps a data name to a frame with `__arrow_c_stream__`.
#[pyfunction]
#[pyo3(signature = (spec, format = "png", scale = 2.0, base_dir = None, tables = None))]
fn render<'py>(py: Python<'py>, spec: &str, format: &str, scale: f32, base_dir: Option<String>, tables: Option<Bound<'py, PyDict>>) -> PyResult<(Option<Bound<'py, PyBytes>>, Option<Bound<'py, PyDict>>)> {
    let f = match format {
        "svg" => Format::Svg,
        "png" => Format::Png,
        other => return Err(PyValueError::new_err(format!("format is svg or png, not {other}"))),
    };
    let mut bound = crate::Tables::new();
    for (k, v) in tables.iter().flat_map(|d| d.iter()) {
        bound.insert(k.extract()?, table(&v)?);
    }
    let spec = spec.to_string();
    // Rendering does not touch Python objects; let other threads run.
    let r = py.allow_threads(move || crate::render_timed(&spec, f, scale, base_dir.as_deref(), &bound, &mut crate::Stages::default()));
    match r {
        Ok(b) => Ok((Some(PyBytes::new(py, &b)), None)),
        Err(e) => Ok((None, Some(refusal(py, &e)?))),
    }
}

fn batches(obj: &Bound<'_, PyAny>) -> PyResult<Vec<arrow::record_batch::RecordBatch>> {
    Ok(table(obj)?.batches().to_vec())
}

/// `Live(spec_json, first_frame)`: a chart compiled once over a named table;
/// `append(frame)` adds rows, `render(format, scale)` draws the rows so far.
#[pyclass(unsendable, module = "avenger_altair._native")]
struct Live {
    inner: crate::Live,
}

#[pymethods]
impl Live {
    #[new]
    fn new(spec: &str, first: Bound<'_, PyAny>) -> PyResult<Self> {
        let snapshot = table(&first)?;
        crate::Live::new(spec, snapshot).map(|inner| Live { inner }).map_err(|r| PyValueError::new_err(r.to_string()))
    }

    /// Add the rows of a frame; returns the number of rows now.
    fn append(&mut self, py: Python<'_>, frame: Bound<'_, PyAny>) -> PyResult<usize> {
        let b = batches(&frame)?;
        let inner = &mut self.inner;
        py.allow_threads(|| inner.append(b)).map_err(|r| PyValueError::new_err(r.to_string()))
    }

    #[getter]
    fn rows(&self) -> usize {
        self.inner.rows()
    }

    #[pyo3(signature = (format = "png", scale = 2.0))]
    fn render<'py>(&self, py: Python<'py>, format: &str, scale: f32) -> PyResult<Bound<'py, PyBytes>> {
        let f = match format {
            "svg" => Format::Svg,
            "png" => Format::Png,
            other => return Err(PyValueError::new_err(format!("format is svg or png, not {other}"))),
        };
        let inner = &self.inner;
        let bytes = py.allow_threads(|| inner.render(f, scale)).map_err(|r| PyValueError::new_err(r.to_string()))?;
        Ok(PyBytes::new(py, &bytes))
    }
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(validate, m)?)?;
    m.add_function(wrap_pyfunction!(render, m)?)?;
    m.add_class::<Live>()?;
    Ok(())
}
