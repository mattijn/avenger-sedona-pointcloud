//! The `vega-format` function package: Vega's `format` and `timeFormat`
//! as DataFusion functions, backed by Jon's d3 formatters
//! (`avenger-format-number`, `avenger-format-datetime`).
//!
//! Registered under their Vega names, so Vega expressions call them as they
//! are, with snake_case aliases for SQL (`vega_format`, `vega_time_format`).

use std::sync::Arc;

use arrow::array::{Array, ArrayRef, AsArray, StringBuilder};
use arrow::datatypes::{DataType, Float64Type, TimeUnit, TimestampMillisecondType};
use avenger_format_datetime_d3::{
    DateTimeFormatContext, DateTimeLocaleRegistry, PreparedDateTimeFormat,
};
use avenger_format_number_d3::{NumberLocaleRegistry, PreparedNumberFormat};
use datafusion::common::{exec_err, Result, ScalarValue};
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};

/// The format pattern must be a constant: it is prepared once per batch.
fn pattern(args: &ScalarFunctionArgs, name: &str) -> Result<String> {
    match args.args.get(1) {
        Some(ColumnarValue::Scalar(v)) => Ok(match v {
            ScalarValue::Utf8(s) | ScalarValue::LargeUtf8(s) | ScalarValue::Utf8View(s) => {
                s.clone().unwrap_or_default()
            }
            ScalarValue::Null => String::new(),
            other => return exec_err!("{name}: the pattern must be a string, got {other}"),
        }),
        Some(_) => exec_err!("{name}: the pattern must be a constant"),
        None => Ok(String::new()),
    }
}

fn map_values(
    value: &ColumnarValue,
    rows: usize,
    f: impl Fn(&ArrayRef, usize) -> Option<String>,
) -> Result<ColumnarValue> {
    let array = value.to_array(rows)?;
    let mut out = StringBuilder::with_capacity(array.len(), array.len() * 8);
    for i in 0..array.len() {
        match if array.is_null(i) { None } else { f(&array, i) } {
            Some(s) => out.append_value(s),
            None => out.append_null(),
        }
    }
    Ok(ColumnarValue::Array(Arc::new(out.finish())))
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct Format {
    signature: Signature,
    aliases: Vec<String>,
}

impl ScalarUDFImpl for Format {
    fn name(&self) -> &str {
        "format"
    }
    fn aliases(&self) -> &[String] {
        &self.aliases
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Utf8)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let spec = pattern(&args, "format")?;
        let locale = NumberLocaleRegistry::with_builtins()
            .resolve("en-US")
            .map_err(|e| datafusion::error::DataFusionError::Execution(e.to_string()))?;
        let fmt = PreparedNumberFormat::new(Some(&spec), Default::default(), &locale)
        .map_err(|e| datafusion::error::DataFusionError::Execution(format!("format: {e}")))?;
        let value = args.args[0].cast_to(&DataType::Float64, None)?;
        map_values(&value, args.number_rows, |a, i| {
            Some(fmt.format(a.as_primitive::<Float64Type>().value(i)).text)
        })
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct TimeFormat {
    signature: Signature,
    aliases: Vec<String>,
}

impl ScalarUDFImpl for TimeFormat {
    fn name(&self) -> &str {
        "timeFormat"
    }
    fn aliases(&self) -> &[String] {
        &self.aliases
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Utf8)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let spec = pattern(&args, "timeFormat")?;
        let locale = DateTimeLocaleRegistry::with_builtins()
            .resolve("en-US")
            .map_err(|e| datafusion::error::DataFusionError::Execution(e.to_string()))?;
        // Vega formats in local time; this package uses UTC so results do not
        // depend on the machine. utcFormat is the exact match.
        let fmt = PreparedDateTimeFormat::new(
            Some(&spec),
            Default::default(),
            DateTimeFormatContext::new(&locale, chrono_tz::UTC),
        )
        .map_err(|e| datafusion::error::DataFusionError::Execution(format!("timeFormat: {e}")))?;
        // Numbers are epoch milliseconds, as in JavaScript.
        let value = match args.args[0].data_type() {
            t if t.is_numeric() => args.args[0]
                .cast_to(&DataType::Int64, None)?
                .cast_to(&DataType::Timestamp(TimeUnit::Millisecond, None), None)?,
            _ => args.args[0].cast_to(&DataType::Timestamp(TimeUnit::Millisecond, None), None)?,
        };
        map_values(&value, args.number_rows, |a, i| {
            let ms = a.as_primitive::<TimestampMillisecondType>().value(i);
            chrono::DateTime::from_timestamp_millis(ms).and_then(|t| fmt.format_zoned(t).ok())
        })
    }
}

/// The package's functions, ready to register on a session.
pub fn functions() -> Vec<ScalarUDF> {
    let signature = Signature::any(2, Volatility::Immutable);
    vec![
        ScalarUDF::new_from_impl(Format {
            signature: signature.clone(),
            aliases: vec!["vega_format".into()],
        }),
        ScalarUDF::new_from_impl(TimeFormat {
            signature,
            aliases: vec!["vega_time_format".into(), "utcFormat".into()],
        }),
    ]
}
