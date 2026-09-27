//! A polygon drawn on the screen, such as a lasso, as ordinary tuples.
//!
//! Not upstream: added in this repo (VENDORED.md). The polygon becomes one
//! tuple per run of pixel cells: the cell row on `v` and a range of cells on
//! `u`. Everything downstream — resolution, cross-filtering, toggles and the
//! preaggregation split — sees ranges on two gridded projections, as for a
//! two-dimensional brush.

use datafusion::{arrow::datatypes::DataType, common::ScalarValue};

use crate::{Error, PixelGrid, ProjectionId, Result, SelectionValue, ValueTest};

impl SelectionValue {
    /// Select the rows whose pixel cell has its centre inside `ring`.
    ///
    /// `ring` is in chart-local logical pixels, the space the pointer draws
    /// in, and is closed implicitly. Insideness uses the even-odd rule. Both
    /// grids must be linear with a Float32 or Float64 domain and no options
    /// beyond `clamp` and `round: false`, and the producer must declare the
    /// two projections with these grids.
    ///
    /// One tuple per run of cells, so the size of the predicate grows with
    /// the polygon's height in cells and with its concavity.
    pub fn polygon(
        u: (&ProjectionId, &PixelGrid),
        v: (&ProjectionId, &PixelGrid),
        ring: &[[f64; 2]],
    ) -> Result<Self> {
        if ring.len() < 3 || ring.iter().flatten().any(|c| !c.is_finite()) {
            return Err(Error::InvalidValue(
                "a polygon needs three or more finite points".into(),
            ));
        }
        let (gu, gv) = (Inverse::new(u.1)?, Inverse::new(v.1)?);
        let lo = ring.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
        let hi = ring.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);
        let mut tuples = Vec::new();
        for row in gv.first_centre(lo)..=gv.last_centre(hi) {
            let y = gv.centre(row);
            let mut xs: Vec<f64> = Vec::new();
            for i in 0..ring.len() {
                let (a, b) = (ring[i], ring[(i + ring.len() - 1) % ring.len()]);
                if (a[1] > y) != (b[1] > y) {
                    xs.push((b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0]);
                }
            }
            xs.sort_by(f64::total_cmp);
            for pair in xs.chunks_exact(2) {
                let (c0, c1) = (gu.first_centre(pair[0]), gu.last_centre(pair[1]));
                if c0 > c1 {
                    continue;
                }
                let (a, b) = (gu.value(c0, u.1)?, gu.value(c1, u.1)?);
                let r = gv.value(row, v.1)?;
                tuples.push([
                    (u.0.clone(), ValueTest::Range { lower: incl(&a, &b, true), upper: incl(&a, &b, false) }),
                    (v.0.clone(), ValueTest::Range { lower: std::ops::Bound::Included(r.clone()), upper: std::ops::Bound::Included(r) }),
                ]);
            }
        }
        Ok(SelectionValue::tuples(tuples))
    }
}

// Decreasing scales map the first cell to the larger value.
fn incl(a: &ScalarValue, b: &ScalarValue, lower: bool) -> std::ops::Bound<ScalarValue> {
    let a_first = a.partial_cmp(b).is_some_and(|o| o.is_le());
    std::ops::Bound::Included(if a_first == lower { a.clone() } else { b.clone() })
}

/// A linear grid, run backwards: from a cell to a data value that the
/// grid's own kernel maps into that cell.
struct Inverse {
    d: (f64, f64),
    r: (f64, f64),
    origin: f64,
    size: f64,
    data_type: DataType,
}
impl Inverse {
    fn new(grid: &PixelGrid) -> Result<Self> {
        let invalid = |m: &str| Error::InvalidValue(format!("polygon grids {m}"));
        if grid.builtin() != avenger_scales_datafusion::BuiltinScale::Linear {
            return Err(invalid("must be linear"));
        }
        if grid.options().keys().any(|k| k != "clamp" && k != "round") {
            return Err(invalid("take only clamp and round: false"));
        }
        let data_type = grid.domain().data_type().clone();
        if !matches!(data_type, DataType::Float32 | DataType::Float64) {
            return Err(invalid("need a Float32 or Float64 domain"));
        }
        let pair = |a: &datafusion::arrow::array::ArrayRef| -> Result<(f64, f64)> {
            let f = |i| -> Result<f64> {
                match ScalarValue::try_from_array(a, i)?.cast_to(&DataType::Float64)? {
                    ScalarValue::Float64(Some(x)) => Ok(x),
                    _ => Err(invalid("need non-null numeric endpoints")),
                }
            };
            Ok((f(0)?, f(1)?))
        };
        Ok(Self { d: pair(grid.domain())?, r: pair(grid.range())?, origin: grid.origin(), size: grid.size(), data_type })
    }
    fn centre(&self, cell: i64) -> f64 {
        self.origin + (cell as f64 + 0.5) * self.size
    }
    /// The first cell whose centre lies beyond a pixel coordinate.
    fn first_centre(&self, p: f64) -> i64 {
        ((p - self.origin) / self.size - 0.5).floor() as i64 + 1
    }
    /// The last cell whose centre lies before it.
    fn last_centre(&self, p: f64) -> i64 {
        ((p - self.origin) / self.size - 0.5).ceil() as i64 - 1
    }
    /// A value at the cell's centre, checked against the grid's kernel.
    fn value(&self, cell: i64, grid: &PixelGrid) -> Result<ScalarValue> {
        let p = self.centre(cell);
        let x = self.d.0 + (p - self.r.0) / (self.r.1 - self.r.0) * (self.d.1 - self.d.0);
        let value = ScalarValue::Float64(Some(x)).cast_to(&self.data_type)?;
        match grid.cell(&value)? {
            Some(c) if c == cell => Ok(value),
            other => Err(Error::InvalidValue(format!(
                "cell {cell} did not survive the grid's kernel (got {other:?})"
            ))),
        }
    }
}
