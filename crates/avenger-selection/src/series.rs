//! Tests over a whole series: a line brush and a timebox.
//!
//! Not upstream: added in this repo (VENDORED.md). The crate's predicates are
//! row-local, and a filter cannot hold the aggregate or window a series test
//! needs. So the test runs in two steps: one DataFusion query over the
//! chart's series finds the keys that pass, and those keys become an ordinary
//! `one_of` on the key projection. Cross-filtering, toggles and the split then
//! see a set, and the keys select rows in any relation that shares them,
//! such as the raw points behind an aggregated line chart. On data that
//! grows, the keys are the chart's to recompute.

use std::sync::Arc;

use datafusion::{
    arrow::{
        array::{Array, BooleanArray, Float64Array},
        compute::cast,
        datatypes::DataType,
    },
    common::{Result as DFResult, ScalarValue},
    functions_aggregate::expr_fn::sum,
    functions_window::expr_fn::lag,
    logical_expr::{
        col, lit, when, ColumnarValue, Expr, ExprFunctionExt, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature,
        Volatility,
    },
    prelude::DataFrame,
};

use crate::{Error, ProjectionId, Result, SelectionValue, ValueTest};

/// A test that a series passes or fails as a whole.
#[derive(Clone, Debug, PartialEq)]
pub enum SeriesTest {
    /// A line brush (Konyha et al. 2006): the series whose polyline, its
    /// vertices ordered by x, crosses the segment `from`–`to`.
    Crosses { from: [f64; 2], to: [f64; 2] },
    /// A timebox (Hochheiser & Shneiderman 2004): the series with at least
    /// one vertex whose x lies in `x`, and whose every such vertex has its y
    /// in `y`. Both ranges are closed.
    Within { x: (f64, f64), y: (f64, f64) },
}

impl SeriesTest {
    /// The keys of the series in `rows` that pass, in key order. `key`,
    /// `x` and `y` are expressions over `rows`; x and y are cast to Float64.
    pub async fn keys(&self, rows: DataFrame, key: Expr, x: Expr, y: Expr) -> Result<Vec<ScalarValue>> {
        let f64 = |e: Expr| datafusion::logical_expr::cast(e, DataType::Float64);
        let rows = rows.select(vec![key.alias("k"), f64(x).alias("x"), f64(y).alias("y")])?;
        let passing = match self {
            SeriesTest::Crosses { from, to } => {
                if from.iter().chain(to).any(|c| !c.is_finite()) {
                    return Err(Error::InvalidValue("a line brush needs finite end points".into()));
                }
                // The offset must be given: `lag(_, None, _)` returned the row
                // itself here (DataFusion 54.1), not the one before it.
                let prev = |c: &str| {
                    lag(col(c), Some(1), None)
                        .partition_by(vec![col("k")])
                        .order_by(vec![col("x").sort(true, false)])
                        .build()
                };
                rows.window(vec![prev("x")?.alias("px"), prev("y")?.alias("py")])?
                    .filter(crosses(*from, *to).call(vec![col("px"), col("py"), col("x"), col("y")]))?
                    .aggregate(vec![col("k")], vec![])?
            }
            SeriesTest::Within { x, y } => {
                let ok = |(lo, hi): (f64, f64)| lo.is_finite() && hi.is_finite() && lo <= hi;
                if !ok(*x) || !ok(*y) {
                    return Err(Error::InvalidValue("a timebox needs finite, ordered ranges".into()));
                }
                let in_x = col("x").between(lit(x.0), lit(x.1));
                let in_y = col("y").between(lit(y.0), lit(y.1));
                let one = |c: Expr| when(c, lit(1_i64)).otherwise(lit(0_i64));
                rows.aggregate(
                    vec![col("k")],
                    vec![sum(one(in_x.clone())?).alias("n_in"), sum(one(in_x.and(std::ops::Not::not(in_y)))?).alias("n_out")],
                )?
                .filter(col("n_in").gt(lit(0_i64)).and(col("n_out").eq(lit(0_i64))))?
                .select(vec![col("k")])?
            }
        };
        let mut keys = Vec::new();
        for batch in passing.sort(vec![col("k").sort(true, false)])?.collect().await? {
            let k = batch.column(0);
            for i in 0..k.len() {
                keys.push(ScalarValue::try_from_array(k, i)?);
            }
        }
        Ok(keys)
    }

    /// The keys as a selection value on the producer's key projection.
    pub fn value(key: &ProjectionId, keys: Vec<ScalarValue>) -> SelectionValue {
        SelectionValue::tuple([(key.clone(), ValueTest::OneOf(keys))])
    }
}

fn crosses(from: [f64; 2], to: [f64; 2]) -> ScalarUDF {
    ScalarUDF::from(Crosses {
        segment: [from[0], from[1], to[0], to[1]].map(f64::to_bits),
        signature: Signature::uniform(4, vec![DataType::Float64], Volatility::Immutable),
    })
}

/// Whether the vertex pair (px, py)–(x, y) crosses the segment, touching
/// included. Orientation signs do not change under positive scaling of
/// either axis, so data units serve as well as pixels.
#[derive(Debug, PartialEq, Eq, Hash)]
struct Crosses {
    segment: [u64; 4],
    signature: Signature,
}
impl ScalarUDFImpl for Crosses {
    fn name(&self) -> &str {
        "avenger_selection_crosses"
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> DFResult<DataType> {
        Ok(DataType::Boolean)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> DFResult<ColumnarValue> {
        let rows = args.number_rows;
        let cols = args
            .args
            .iter()
            .map(|a| Ok(cast(&a.clone().into_array(rows)?, &DataType::Float64)?))
            .collect::<DFResult<Vec<_>>>()?;
        let c: Vec<&Float64Array> = cols.iter().map(|a| a.as_any().downcast_ref::<Float64Array>().unwrap()).collect();
        let [q1x, q1y, q2x, q2y] = self.segment.map(f64::from_bits);
        let o = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        let (q1, q2) = ([q1x, q1y], [q2x, q2y]);
        let out: BooleanArray = (0..rows)
            .map(|i| {
                if c.iter().any(|a| a.is_null(i)) {
                    return Some(false);
                }
                let (p1, p2) = ([c[0].value(i), c[1].value(i)], [c[2].value(i), c[3].value(i)]);
                Some(o(q1, q2, p1) * o(q1, q2, p2) <= 0.0 && o(p1, p2, q1) * o(p1, p2, q2) <= 0.0)
            })
            .collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
    }
}
