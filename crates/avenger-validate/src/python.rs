//! The Python module `avenger_validate` (pyo3, built with maturin).
//!
//! ```python
//! from avenger_validate import Validator, export_cel
//! v = Validator()                       # LAS columns as the source schema
//! r = v.check('read t.laz ! sql "SELECT classification FROM input" ! bars n --by label')
//! r["valid"], r["issues"]               # each issue: layer, step, name, arg, message, span
//! ```

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::{Issue, Report};

fn issue_dict<'py>(py: Python<'py>, i: &Issue) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("layer", i.layer)?;
    d.set_item("step", i.step)?;
    d.set_item("name", &i.name)?;
    d.set_item("arg", &i.arg)?;
    d.set_item("message", &i.message)?;
    d.set_item("span", i.span)?;
    Ok(d)
}

fn report_dict<'py>(py: Python<'py>, r: &Report) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("valid", r.valid)?;
    d.set_item("steps", r.steps)?;
    d.set_item("layers", &r.layers)?;
    let issues = PyList::empty(py);
    for i in &r.issues {
        issues.append(issue_dict(py, i)?)?;
    }
    d.set_item("issues", issues)?;
    Ok(d)
}

#[cfg(feature = "sql")]
fn arrow_type(t: &str) -> PyResult<datafusion::arrow::datatypes::DataType> {
    use datafusion::arrow::datatypes::DataType as D;
    Ok(match t.to_ascii_lowercase().as_str() {
        "bool" | "boolean" => D::Boolean,
        "int8" => D::Int8,
        "int16" => D::Int16,
        "int32" => D::Int32,
        "int64" | "int" => D::Int64,
        "uint8" => D::UInt8,
        "uint16" => D::UInt16,
        "uint32" => D::UInt32,
        "uint64" => D::UInt64,
        "float32" => D::Float32,
        "float64" | "float" | "double" => D::Float64,
        "utf8" | "string" | "str" => D::Utf8,
        "date32" | "date" => D::Date32,
        "timestamp" => D::Timestamp(datafusion::arrow::datatypes::TimeUnit::Nanosecond, None),
        other => return Err(pyo3::exceptions::PyValueError::new_err(format!("unknown column type `{other}`"))),
    })
}

/// `Validator(schema=None, data=True)`. `schema` maps the source's column
/// names to types (`"float64"`, `"uint8"`, `"utf8"`, …); by default the LAS
/// point columns. `data=False` leaves out layer 4 (SQL and fields).
#[pyclass(unsendable, module = "avenger_validate")]
struct Validator {
    inner: crate::Validator,
}

#[pymethods]
impl Validator {
    #[new]
    #[pyo3(signature = (schema = None, data = true))]
    fn new(schema: Option<Vec<(String, String)>>, data: bool) -> PyResult<Self> {
        #[allow(unused_mut)]
        let mut inner = crate::Validator::default();
        #[cfg(feature = "sql")]
        if let Some(cols) = schema {
            use datafusion::arrow::datatypes::{Field, Schema};
            let fields = cols.iter().map(|(n, t)| Ok(Field::new(n, arrow_type(t)?, true))).collect::<PyResult<Vec<_>>>()?;
            inner = inner.with_source_schema(std::sync::Arc::new(Schema::new(fields)));
        }
        #[cfg(not(feature = "sql"))]
        let _ = schema;
        if !data {
            inner = inner.without_data();
        }
        Ok(Validator { inner })
    }

    /// Check a pipeline given as text.
    fn check<'py>(&self, py: Python<'py>, text: &str) -> PyResult<Bound<'py, PyDict>> {
        report_dict(py, &self.inner.check(text))
    }

    /// Check many pipelines in one call.
    fn check_many<'py>(&self, py: Python<'py>, texts: Vec<String>) -> PyResult<Bound<'py, PyList>> {
        let out = PyList::empty(py);
        for t in &texts {
            out.append(report_dict(py, &self.inner.check(t))?)?;
        }
        Ok(out)
    }

    /// Check a pipeline given as data: a list of `{"step", "args", "flags"}`,
    /// as JSON text or as Python objects.
    fn check_steps<'py>(&self, py: Python<'py>, steps: Bound<'py, PyAny>) -> PyResult<Bound<'py, PyDict>> {
        let text: String = match steps.extract::<String>() {
            Ok(s) => s,
            Err(_) => py.import("json")?.call_method1("dumps", (steps,))?.extract()?,
        };
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
        report_dict(py, &self.inner.check_json(&v))
    }

    /// The text as steps, in the form `check_steps` and the CEL export take
    /// (JSON text).
    fn parse(&self, text: &str) -> PyResult<String> {
        crate::parse(text).map(|c| crate::calls_to_json(&c).to_string()).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.message))
    }
}

/// Layers 1 and 2 as a CEL bundle (JSON text), for validating without this library.
#[pyfunction]
fn export_cel() -> String {
    crate::export::cel_bundle(&crate::Spec::builtin()).to_string()
}

/// The built-in step spec (JSON text).
#[pyfunction]
fn spec_json() -> &'static str {
    crate::spec::STEPS_JSON
}

#[pymodule]
fn avenger_validate(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Validator>()?;
    m.add_function(wrap_pyfunction!(export_cel, m)?)?;
    m.add_function(wrap_pyfunction!(spec_json, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
