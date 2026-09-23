//! The `vega-compat` function package: JavaScript's number↔string
//! coercions, which SQL does differently. Used by the compiler's Vega
//! semantics; plain SQL semantics do not need it.

use std::sync::Arc;

use arrow::array::{Array, AsArray, Float64Builder, StringBuilder};
use arrow::datatypes::{DataType, Float64Type};
use datafusion::common::Result;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};

/// JavaScript `Number(string)`: trims whitespace, the empty string is 0,
/// anything unparseable is NaN. Null stays null (callers decide).
pub fn js_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return i64::from_str_radix(h, 16).map_or(f64::NAN, |v| v as f64);
    }
    if t.chars()
        .any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E')
    {
        return f64::NAN;
    }
    t.parse().unwrap_or(f64::NAN)
}

/// JavaScript `String(number)` for the common cases: integers without a
/// decimal point, `NaN`, `Infinity`.
pub fn js_string(x: f64) -> String {
    if x.is_nan() {
        "NaN".into()
    } else if x.is_infinite() {
        if x > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else if x == x.trunc() && x.abs() < 1e21 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct ToNumber(Signature);

impl ScalarUDFImpl for ToNumber {
    fn name(&self) -> &str {
        "js_to_number"
    }
    fn signature(&self) -> &Signature {
        &self.0
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Float64)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let a = args.args[0]
            .cast_to(&DataType::Utf8, None)?
            .to_array(args.number_rows)?;
        let s = a.as_string::<i32>();
        let mut out = Float64Builder::with_capacity(s.len());
        for i in 0..s.len() {
            if s.is_null(i) {
                out.append_null()
            } else {
                out.append_value(js_number(s.value(i)))
            }
        }
        Ok(ColumnarValue::Array(Arc::new(out.finish())))
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct ToString(Signature);

impl ScalarUDFImpl for ToString {
    fn name(&self) -> &str {
        "js_string"
    }
    fn signature(&self) -> &Signature {
        &self.0
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Utf8)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let a = args.args[0].to_array(args.number_rows)?;
        let mut out = StringBuilder::with_capacity(a.len(), a.len() * 6);
        let t = a.data_type().clone();
        if t.is_numeric() {
            let f = arrow::compute::cast(&a, &DataType::Float64)?;
            let f = f.as_primitive::<Float64Type>();
            for i in 0..f.len() {
                out.append_value(if f.is_null(i) {
                    "null".into()
                } else {
                    js_string(f.value(i))
                });
            }
        } else if t == DataType::Boolean {
            let b = a.as_boolean();
            for i in 0..b.len() {
                out.append_value(if b.is_null(i) {
                    "null"
                } else if b.value(i) {
                    "true"
                } else {
                    "false"
                });
            }
        } else {
            let s = arrow::compute::cast(&a, &DataType::Utf8)?;
            let s = s.as_string::<i32>();
            for i in 0..s.len() {
                out.append_value(if s.is_null(i) { "null" } else { s.value(i) });
            }
        }
        Ok(ColumnarValue::Array(Arc::new(out.finish())))
    }
}

pub fn functions() -> Vec<ScalarUDF> {
    vec![
        ScalarUDF::new_from_impl(ToNumber(Signature::any(1, Volatility::Immutable))),
        ScalarUDF::new_from_impl(ToString(Signature::any(1, Volatility::Immutable))),
    ]
}
