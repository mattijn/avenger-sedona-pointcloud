//! Tests over a whole series: a line brush and a timebox.
//!
//! Not upstream: added in this repo (VENDORED.md). The crate's predicates are
//! row-local, and a filter cannot hold the aggregate or window a series test
//! needs. So the test runs in two steps: one DataFusion query over the
//! chart's series measures each series, and the series that pass become an
//! ordinary `one_of` on the key projection (with partial degrees for a soft
//! test). Cross-filtering, toggles and the split then see a set, and the keys
//! select rows in any relation that shares them, such as the raw points
//! behind an aggregated line chart. On data that grows, the keys are the
//! chart's to recompute.

use std::sync::Arc;

use datafusion::{
    arrow::{
        array::{Array, AsArray, BooleanArray, Float64Array},
        compute::cast,
        datatypes::{DataType, Float64Type},
    },
    common::{Result as DFResult, ScalarValue},
    functions_aggregate::expr_fn::{bool_or, min, sum},
    functions_window::expr_fn::lag,
    logical_expr::{
        col, lit, when, ColumnarValue, Expr, ExprFunctionExt, ScalarFunctionArgs, ScalarUDF,
        ScalarUDFImpl, Signature, Volatility,
    },
    prelude::DataFrame,
};

use crate::{udf::columns, Error, Gesture, ProjectionId, Result, SelectionValue, ValueTest};

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

/// What one query measures of each series: for a line brush, whether it
/// crosses (`hit`) and its nearest vertex's distance (`near`); for a
/// timebox, its vertices within the x-range (`hit`) and those of them inside
/// the box (`near`).
struct Measure {
    key: ScalarValue,
    hit: f64,
    near: f64,
}

impl SeriesTest {
    /// The keys of the series in `rows` that pass, in key order. `key`,
    /// `x` and `y` are expressions over `rows`; x and y are cast to Float64.
    pub async fn keys(
        &self,
        rows: DataFrame,
        key: Expr,
        x: Expr,
        y: Expr,
    ) -> Result<Vec<ScalarValue>> {
        let passes = |m: &Measure| match self {
            SeriesTest::Crosses { .. } => m.hit > 0.0,
            SeriesTest::Within { .. } => m.hit > 0.0 && m.near == m.hit,
        };
        Ok(self
            .measure(rows, key, x, y, [1.0, 1.0])
            .await?
            .into_iter()
            .filter(passes)
            .map(|m| m.key)
            .collect())
    }

    /// Soft selection over whole series (experiment 7's `--soft`): each
    /// series' degree in (0, 1], in key order, leaving out series at 0.
    ///
    /// A line brush gives 1 to a series that crosses it, and otherwise
    /// falls linearly with its nearest vertex's distance to the segment, to 0
    /// at `width`. Distances are measured after multiplying x and y by
    /// `scale`: pixels per data unit, or `1 / span` for the plot's unit
    /// square as experiment 7 measures. A timebox gives the share of a
    /// series' vertices within its x-range whose y lies in its y-range; it
    /// has no distance, and ignores `scale` and `width`.
    pub async fn degrees(
        &self,
        rows: DataFrame,
        key: Expr,
        x: Expr,
        y: Expr,
        scale: [f64; 2],
        width: f64,
    ) -> Result<Vec<(ScalarValue, f64)>> {
        let positive =
            scale.iter().all(|s| s.is_finite() && *s > 0.0) && width.is_finite() && width > 0.0;
        if matches!(self, SeriesTest::Crosses { .. }) && !positive {
            return Err(Error::InvalidValue(
                "a soft line brush needs a positive scale and width".into(),
            ));
        }
        let degree = |m: &Measure| match self {
            SeriesTest::Crosses { .. } if m.hit > 0.0 => 1.0,
            SeriesTest::Crosses { .. } => (1.0 - m.near / width).max(0.0),
            SeriesTest::Within { .. } if m.hit > 0.0 => m.near / m.hit,
            SeriesTest::Within { .. } => 0.0,
        };
        let measured = self.measure(rows, key, x, y, scale).await?;
        Ok(measured
            .into_iter()
            .map(|m| (degree(&m), m.key))
            .filter(|(d, _)| *d > 0.0)
            .map(|(d, k)| (k, d))
            .collect())
    }

    fn check(&self) -> Result<()> {
        let ordered = |(lo, hi): (f64, f64)| lo.is_finite() && hi.is_finite() && lo <= hi;
        match self {
            SeriesTest::Crosses { from, to } if from.iter().chain(to).any(|c| !c.is_finite()) => {
                Err(Error::InvalidValue(
                    "a line brush needs finite end points".into(),
                ))
            }
            SeriesTest::Within { x, y } if !ordered(*x) || !ordered(*y) => Err(
                Error::InvalidValue("a timebox needs finite, ordered ranges".into()),
            ),
            _ => Ok(()),
        }
    }

    /// One query over `rows`: a `Measure` per series, in key order.
    async fn measure(
        &self,
        rows: DataFrame,
        key: Expr,
        x: Expr,
        y: Expr,
        scale: [f64; 2],
    ) -> Result<Vec<Measure>> {
        self.check()?;
        let f64 = |e: Expr| datafusion::logical_expr::cast(e, DataType::Float64);
        let rows = rows.select(vec![key.alias("k"), f64(x).alias("x"), f64(y).alias("y")])?;
        let measured = match self {
            SeriesTest::Crosses { from, to } => {
                // The offset must be given: `lag(_, None, _)` returned the row
                // itself here (DataFusion 54.1), not the one before it.
                let prev = |c: &str| {
                    lag(col(c), Some(1), None)
                        .partition_by(vec![col("k")])
                        .order_by(vec![col("x").sort(true, false)])
                        .build()
                };
                let crossing = udf(Crosses {
                    segment: segment_bits(*from, *to),
                    signature: float_args(4),
                });
                let near = udf(Distance {
                    segment: segment_bits(*from, *to),
                    scale: scale.map(f64::to_bits),
                    signature: float_args(2),
                });
                rows.window(vec![prev("x")?.alias("px"), prev("y")?.alias("py")])?
                    .aggregate(
                        vec![col("k")],
                        vec![
                            bool_or(crossing.call(vec![col("px"), col("py"), col("x"), col("y")]))
                                .alias("hit"),
                            min(near.call(vec![col("x"), col("y")])).alias("near"),
                        ],
                    )?
            }
            SeriesTest::Within { x, y } => {
                let in_x = col("x").between(lit(x.0), lit(x.1));
                let in_y = col("y").between(lit(y.0), lit(y.1));
                let count =
                    |c: Expr| -> DFResult<Expr> { Ok(sum(when(c, lit(1.0)).otherwise(lit(0.0))?)) };
                rows.aggregate(
                    vec![col("k")],
                    vec![
                        count(in_x.clone())?.alias("hit"),
                        count(in_x.and(in_y))?.alias("near"),
                    ],
                )?
            }
        };
        let mut out = Vec::new();
        for batch in measured
            .sort(vec![col("k").sort(true, false)])?
            .collect()
            .await?
        {
            let number = |name: &str| -> Result<Float64Array> {
                let c = batch
                    .column_by_name(name)
                    .ok_or_else(|| Error::InvalidValue(format!("no {name} column")))?;
                Ok(cast(c, &DataType::Float64)
                    .map_err(datafusion::common::DataFusionError::from)?
                    .as_primitive::<Float64Type>()
                    .clone())
            };
            let (hit, near, keys) = (number("hit")?, number("near")?, batch.column(0));
            for i in 0..batch.num_rows() {
                out.push(Measure {
                    key: ScalarValue::try_from_array(keys, i)?,
                    hit: if hit.is_null(i) { 0.0 } else { hit.value(i) },
                    near: if near.is_null(i) {
                        f64::INFINITY
                    } else {
                        near.value(i)
                    },
                });
            }
        }
        Ok(out)
    }

    /// Soft degrees as a selection value: the series at 1 as tuples, the
    /// rest as partial degrees, and the gesture with `soft` (the width) and
    /// `sx`, `sy` (the scale), so a log can draw it again.
    pub fn soft_value(
        &self,
        key: &ProjectionId,
        degrees: Vec<(ScalarValue, f64)>,
        scale: [f64; 2],
        width: f64,
    ) -> SelectionValue {
        let (full, part): (Vec<_>, Vec<_>) = degrees.into_iter().partition(|(_, d)| *d >= 1.0);
        SelectionValue::tuple([(
            key.clone(),
            ValueTest::OneOf(full.into_iter().map(|(k, _)| k).collect()),
        )])
        .with_partial(key.clone(), part)
        .with_gesture(
            self.gesture()
                .with_param("soft", width)
                .with_param("sx", scale[0])
                .with_param("sy", scale[1]),
        )
    }

    /// The keys as a selection value on the producer's key projection,
    /// carrying the test as its gesture (`"segment"` or `"timebox"`).
    pub fn value(&self, key: &ProjectionId, keys: Vec<ScalarValue>) -> SelectionValue {
        SelectionValue::tuple([(key.clone(), ValueTest::OneOf(keys))]).with_gesture(self.gesture())
    }

    /// The test as a gesture, in the series' data units.
    pub fn gesture(&self) -> Gesture {
        match self {
            SeriesTest::Crosses { from, to } => Gesture::new("segment", [*from, *to]),
            SeriesTest::Within { x, y } => Gesture::new("timebox", [[x.0, y.0], [x.1, y.1]]),
        }
    }

    /// The test back from its gesture, as a replayed log gives it.
    pub fn from_gesture(g: &Gesture) -> Option<Self> {
        match (g.kind(), g.points()) {
            ("segment", [a, b]) => Some(SeriesTest::Crosses { from: *a, to: *b }),
            ("timebox", [a, b]) => Some(SeriesTest::Within {
                x: (a[0], b[0]),
                y: (a[1], b[1]),
            }),
            _ => None,
        }
    }
}

fn segment_bits(from: [f64; 2], to: [f64; 2]) -> [u64; 4] {
    [from[0], from[1], to[0], to[1]].map(f64::to_bits)
}
fn float_args(n: usize) -> Signature {
    Signature::uniform(n, vec![DataType::Float64], Volatility::Immutable)
}
fn udf(f: impl ScalarUDFImpl + 'static) -> ScalarUDF {
    ScalarUDF::new_from_impl(f)
}

/// Whether the vertex pair (px, py)–(x, y) meets the segment, touching
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
        let c = columns::<Float64Type>(&args)?;
        let [ax, ay, bx, by] = self.segment.map(f64::from_bits);
        let meets = |i: usize| {
            !c.iter().any(|a| a.is_null(i))
                && segments_meet(
                    [c[0].value(i), c[1].value(i)],
                    [c[2].value(i), c[3].value(i)],
                    [ax, ay],
                    [bx, by],
                )
        };
        let out: BooleanArray = (0..args.number_rows).map(|i| Some(meets(i))).collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
    }
}

/// The distance from (x, y) to the segment, after scaling both axes.
#[derive(Debug, PartialEq, Eq, Hash)]
struct Distance {
    segment: [u64; 4],
    scale: [u64; 2],
    signature: Signature,
}
impl ScalarUDFImpl for Distance {
    fn name(&self) -> &str {
        "avenger_selection_segment_distance"
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> DFResult<DataType> {
        Ok(DataType::Float64)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> DFResult<ColumnarValue> {
        let c = columns::<Float64Type>(&args)?;
        let [ax, ay, bx, by] = self.segment.map(f64::from_bits);
        let [sx, sy] = self.scale.map(f64::from_bits);
        let (a, b) = ([ax * sx, ay * sy], [bx * sx, by * sy]);
        let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
        let distance = |i: usize| {
            (!c[0].is_null(i) && !c[1].is_null(i)).then(|| {
                let (px, py) = (c[0].value(i) * sx, c[1].value(i) * sy);
                let t = (((px - a[0]) * vx + (py - a[1]) * vy) / (vx * vx + vy * vy).max(1e-12))
                    .clamp(0.0, 1.0);
                (px - (a[0] + t * vx)).hypot(py - (a[1] + t * vy))
            })
        };
        let out: Float64Array = (0..args.number_rows).map(distance).collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
    }
}

/// Whether segments p1–p2 and q1–q2 share a point, touching included. The
/// orientation test alone takes collinear segments to meet wherever they
/// lie on the same line; those, and a segment that is a point, meet only
/// where a point lies within the other segment's extent.
pub(crate) fn segments_meet(p1: [f64; 2], p2: [f64; 2], q1: [f64; 2], q2: [f64; 2]) -> bool {
    let o = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    let within = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        c[0] >= a[0].min(b[0])
            && c[0] <= a[0].max(b[0])
            && c[1] >= a[1].min(b[1])
            && c[1] <= a[1].max(b[1])
    };
    let (d1, d2, d3, d4) = (o(q1, q2, p1), o(q1, q2, p2), o(p1, p2, q1), o(p1, p2, q2));
    (d1 * d2 < 0.0 && d3 * d4 < 0.0)
        || (d1 == 0.0 && within(q1, q2, p1))
        || (d2 == 0.0 && within(q1, q2, p2))
        || (d3 == 0.0 && within(p1, p2, q1))
        || (d4 == 0.0 && within(p1, p2, q2))
}
