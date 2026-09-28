//! A polygon drawn on the screen, such as a lasso, and a set of grid cells,
//! as ordinary tuples.
//!
//! Not upstream: added in this repo (VENDORED.md). A polygon becomes one
//! tuple per run of pixel cells: the cell row on `v` and a range of cells on
//! `u`. Everything downstream — resolution, cross-filtering, toggles and the
//! preaggregation split — sees ranges on gridded projections, as for a
//! two-dimensional brush.

use datafusion::{arrow::datatypes::DataType, common::ScalarValue};

use crate::{
    Error, Gesture, PixelGrid, ProducerDefinition, ProjectionId, Result, SelectionValue, ValueTest,
};

/// Polygons over more rows of cells than this are refused.
const MAX_ROWS: i64 = 1 << 20;

impl SelectionValue {
    /// Select the rows whose pixel cell has its centre inside `ring`.
    ///
    /// `ring` is in chart-local logical pixels, the space the pointer draws
    /// in, and is closed implicitly. Insideness uses the even-odd rule. Both
    /// grids, read from the producer so that value and definition cannot
    /// disagree, must be linear with a Float32 or Float64 domain and no
    /// options beyond `clamp` and `round: false`.
    ///
    /// One tuple per run of cells, so the size of the predicate grows with
    /// the polygon's height in cells and with its concavity; a polygon over
    /// more than a million rows of cells is refused. On a clamped grid, where
    /// data beyond the plot lands in its edge cells, only the cells data can
    /// reach are listed.
    pub fn polygon(
        producer: &ProducerDefinition,
        u: &ProjectionId,
        v: &ProjectionId,
        ring: &[[f64; 2]],
    ) -> Result<Self> {
        if ring.len() < 3 || ring.iter().flatten().any(|c| !c.is_finite()) {
            return Err(Error::InvalidValue(
                "a polygon needs three or more finite points".into(),
            ));
        }
        let (gu, gv) = (
            Inverse::new(grid_of(producer, u)?)?,
            Inverse::new(grid_of(producer, v)?)?,
        );
        let lo = ring.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
        let hi = ring.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);
        let (first, last) = (gv.first_centre(lo), gv.last_centre(hi));
        if last.saturating_sub(first) >= MAX_ROWS {
            return Err(Error::InvalidValue(format!(
                "a polygon over {} rows of cells; at most {MAX_ROWS}",
                last.saturating_sub(first) + 1
            )));
        }
        let mut tuples = Vec::new();
        for row in first..=last {
            let xs = crossings(ring, gv.centre(row));
            for pair in xs.chunks_exact(2) {
                let (c0, c1) = (gu.first_centre(pair[0]), gu.last_centre(pair[1]));
                if c0 <= c1 {
                    tuples.push(vec![
                        (u.clone(), gu.cells(c0, c1)?),
                        (v.clone(), gv.cells(row, row)?),
                    ]);
                }
            }
        }
        let gesture = Gesture::new("polygon", ring.iter().copied()).on([u.clone(), v.clone()]);
        Ok(SelectionValue::tuples(tuples).with_gesture(gesture))
    }

    /// Draw a gesture again with the crate's own kinds: `"polygon"` on the
    /// two projections it names. None for other kinds, which are the chart's
    /// to redraw (a line brush or CloudLasso depends on the data as well).
    pub fn from_gesture(producer: &ProducerDefinition, gesture: &Gesture) -> Option<Result<Self>> {
        match (gesture.kind(), gesture.projections()) {
            // Keep the gesture as drawn, named numbers included.
            ("polygon", [u, v]) => Some(
                Self::polygon(producer, u, v, gesture.points())
                    .map(|value| value.with_gesture(gesture.clone())),
            ),
            _ => None,
        }
    }

    /// Select the rows in the given cells: one tuple per cell, over one
    /// gridded projection per dimension, such as the voxels CloudLasso keeps.
    /// Each cell lists its index per projection, in the order of `ids`; the
    /// grids come from the producer and follow the rules of `polygon`.
    pub fn cells(
        producer: &ProducerDefinition,
        ids: &[&ProjectionId],
        cells: impl IntoIterator<Item = Vec<i64>>,
    ) -> Result<Self> {
        let grids = ids
            .iter()
            .map(|id| Inverse::new(grid_of(producer, id)?))
            .collect::<Result<Vec<_>>>()?;
        let tuples = cells
            .into_iter()
            .map(|cell| {
                if cell.len() != grids.len() {
                    return Err(Error::InvalidValue(format!(
                        "a cell needs {} indices, one per grid",
                        grids.len()
                    )));
                }
                cell.iter()
                    .zip(ids)
                    .zip(&grids)
                    .map(|((c, id), g)| Ok(((*id).clone(), g.cells(*c, *c)?)))
                    .collect()
            })
            .collect::<Result<Vec<Vec<_>>>>()?;
        Ok(SelectionValue::tuples(tuples))
    }
}

fn grid_of<'a>(producer: &'a ProducerDefinition, id: &ProjectionId) -> Result<&'a PixelGrid> {
    producer.pixel_grid(id).ok_or_else(|| {
        Error::InvalidDefinition(format!(
            "{id} needs a pixel grid in the producer's definition"
        ))
    })
}

/// Where the horizontal line at `y` crosses the ring's edges, left to right:
/// even-odd pairs of these bound the inside.
fn crossings(ring: &[[f64; 2]], y: f64) -> Vec<f64> {
    let mut xs: Vec<f64> = (0..ring.len())
        .map(|i| (ring[i], ring[(i + ring.len() - 1) % ring.len()]))
        .filter(|(a, b)| (a[1] > y) != (b[1] > y))
        .map(|(a, b)| (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0])
        .collect();
    xs.sort_by(f64::total_cmp);
    xs
}

/// A linear grid, run backwards: from a cell to a data value that the
/// grid's own kernel maps into that cell.
struct Inverse<'a> {
    grid: &'a PixelGrid,
    domain: (f64, f64),
    range: (f64, f64),
    data_type: DataType,
    /// On a clamped grid, the cells the data can reach.
    clamp: Option<(i64, i64)>,
}
impl<'a> Inverse<'a> {
    fn new(grid: &'a PixelGrid) -> Result<Self> {
        let invalid = |m: &str| Error::InvalidValue(format!("polygon and cell grids {m}"));
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
        // PixelGrid::new has checked that both are two finite numbers.
        let pair = |a: &datafusion::arrow::array::ArrayRef| -> Result<(f64, f64)> {
            let f = |i| -> Result<ScalarValue> {
                Ok(ScalarValue::try_from_array(a, i)?.cast_to(&DataType::Float64)?)
            };
            match (f(0)?, f(1)?) {
                (ScalarValue::Float64(Some(a)), ScalarValue::Float64(Some(b))) => Ok((a, b)),
                _ => Err(invalid("need numeric endpoints")),
            }
        };
        let clamped = grid.options().get("clamp").is_some_and(|c| {
            ScalarValue::try_from_array(&c.0, 0)
                .is_ok_and(|v| v == ScalarValue::Boolean(Some(true)))
        });
        let clamp = if clamped {
            let cell = |i| -> Result<i64> {
                grid.cell(&ScalarValue::try_from_array(grid.domain(), i)?)?
                    .ok_or_else(|| invalid("have a domain the kernel maps"))
            };
            let (a, b) = (cell(0)?, cell(1)?);
            Some((a.min(b), a.max(b)))
        } else {
            None
        };
        Ok(Self {
            grid,
            domain: pair(grid.domain())?,
            range: pair(grid.range())?,
            data_type,
            clamp,
        })
    }
    fn centre(&self, cell: i64) -> f64 {
        self.grid.origin() + (cell as f64 + 0.5) * self.grid.size()
    }
    /// The first cell whose centre lies beyond a pixel coordinate, within
    /// the cells the data reaches.
    fn first_centre(&self, p: f64) -> i64 {
        let c = ((p - self.grid.origin()) / self.grid.size() - 0.5).floor() as i64 + 1;
        self.clamp.map_or(c, |(lo, _)| c.max(lo))
    }
    /// The last cell whose centre lies before it.
    fn last_centre(&self, p: f64) -> i64 {
        let c = ((p - self.grid.origin()) / self.grid.size() - 0.5).ceil() as i64 - 1;
        self.clamp.map_or(c, |(_, hi)| c.min(hi))
    }
    /// The closed range of values that selects cells `c0` to `c1` on this grid.
    fn cells(&self, c0: i64, c1: i64) -> Result<ValueTest> {
        let (a, b) = (self.value(c0)?, self.value(c1)?);
        let (lo, hi) = if self.grid.is_decreasing() {
            (b, a)
        } else {
            (a, b)
        };
        Ok(ValueTest::range(lo..=hi))
    }
    /// A value at the cell's centre, checked against the grid's kernel.
    fn value(&self, cell: i64) -> Result<ScalarValue> {
        let (d, r) = (self.domain, self.range);
        let x = d.0 + (self.centre(cell) - r.0) / (r.1 - r.0) * (d.1 - d.0);
        let value = ScalarValue::Float64(Some(x)).cast_to(&self.data_type)?;
        match self.grid.cell(&value)? {
            Some(c) if c == cell => Ok(value),
            other => Err(Error::InvalidValue(format!(
                "cell {cell} did not survive the grid's kernel (got {other:?})"
            ))),
        }
    }
}
