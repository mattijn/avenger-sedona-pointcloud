//! A degree of interest in [0, 1], beside the yes-or-no predicate.
//!
//! Not upstream: added in this repo (VENDORED.md). Soft selection (Doleisch
//! et al. 2003; experiment 7's `--soft`) fades what lies near a brush instead
//! of cutting it off. A row's degree is 1 where the predicate holds and
//! falls linearly to 0 at `width` logical pixels from the selection:
//!
//! - a tuple of ranges on gridded projections measures its distance in
//!   pixels, per projection the gap between the row's cell and the range
//!   times the cell size, combined as a Euclidean length. A contribution
//!   takes its nearest tuple, which for a lasso is its nearest run of cells;
//! - a tuple of equalities and sets, which has no distance, gives 1 or 0,
//!   or a partial degree for keys the value carries one for;
//! - a range on a projection without a grid is refused: its data units need
//!   not match another projection's, and the screen is the common measure.
//!
//! Composition follows fuzzy logic: intersected producers and `All` take the
//! least degree, unions and `Any` the greatest, and `Not` its complement, so
//! a degree of exactly 1 is where `predicate` holds.

use std::sync::Arc;

use datafusion::{
    arrow::{
        array::{Array, Float64Array},
        datatypes::{DataType, Int64Type},
    },
    common::{Result as DFResult, ScalarValue},
    logical_expr::{
        lit, when, ColumnarValue, Expr, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature,
        Volatility,
    },
};

use crate::{
    resolve::{ResolvedContribution, ResolvedFilter, SelectionStatus},
    udf::columns,
    values::{canonical_values, scalar_cmp},
    ConsumerFilter, EmptySelection, Error, ProducerDefinition, ProjectionId, Resolution, Result,
    SelectionSet, ValueTest,
};

impl ConsumerFilter {
    /// A Float64 expression in [0, 1]: 1 where `predicate` holds, falling
    /// linearly to 0 at `width` logical pixels from the selection. A width of
    /// 0 gives the predicate as 1 or 0.
    pub fn degree(&self, selections: &SelectionSet, width: f64) -> Result<Expr> {
        check_width(width)?;
        resolved(&self.resolve(selections)?, width)
    }

    /// The focused producer's degree alone, over `keys`: expressions for its
    /// interaction dimensions (pixel cells for gridded projections), such as
    /// the columns of states stored for preaggregation. With the other
    /// producers applied as the split's fixed predicate at warm-up, a fading
    /// chart sums `count × degree` per group over the stored states.
    pub fn focus_degree(
        &self,
        selections: &SelectionSet,
        focus: &ProducerDefinition,
        width: f64,
        keys: Vec<Expr>,
    ) -> Result<Expr> {
        check_width(width)?;
        let Some(c) = selections
            .get(focus.selection())?
            .contributions
            .get(focus.address())
        else {
            return Err(Error::InvalidValue(
                "focus_degree needs a contribution from the focus".into(),
            ));
        };
        if c.producer() != focus {
            return Err(Error::InvalidDefinition(
                "the focus differs from the producer that contributed".into(),
            ));
        }
        if keys.len() != focus.projections().len() {
            return Err(Error::InvalidMapping(format!(
                "focus_degree needs {} keys, one per projection, and was given {}",
                focus.projections().len(),
                keys.len()
            )));
        }
        contribution(
            &ResolvedContribution {
                contribution: c.clone(),
                projections: keys,
            },
            width,
        )
    }
}

fn check_width(width: f64) -> Result<()> {
    if width.is_finite() && width >= 0.0 {
        Ok(())
    } else {
        Err(Error::InvalidValue(format!(
            "a soft width is finite and not negative, not {width}"
        )))
    }
}

fn resolved(filter: &ResolvedFilter, width: f64) -> Result<Expr> {
    let each = |fs: &[ResolvedFilter]| {
        fs.iter()
            .map(|f| resolved(f, width))
            .collect::<Result<Vec<_>>>()
    };
    Ok(match filter {
        ResolvedFilter::All(fs) => fold(each(fs)?, true),
        ResolvedFilter::Any(fs) => fold(each(fs)?, false),
        ResolvedFilter::Not(f) => lit(1.0) - resolved(f, width)?,
        ResolvedFilter::Selection(s) => match s.status {
            SelectionStatus::Inactive => lit(if s.empty == EmptySelection::MatchAll {
                1.0
            } else {
                0.0
            }),
            SelectionStatus::AllExcluded => lit(1.0),
            SelectionStatus::Active => fold(
                s.contributions
                    .iter()
                    .map(|c| contribution(c, width))
                    .collect::<Result<_>>()?,
                s.resolution == Resolution::Intersect,
            ),
        },
    })
}

/// The least (fuzzy AND) or greatest (fuzzy OR) of the degrees; with none,
/// 1 or 0, as `predicate::combine` gives true or false.
///
/// One `least(…)` or `greatest(…)` over all of them, not nested
/// `CASE WHEN a <= b THEN a ELSE b`: the CASE repeats each inner degree, so its
/// size doubled with every contribution, and DataFusion's common subexpression
/// elimination then refused the plan (the extracted CASE nullable in the
/// physical plan and not in the logical one; experiment 10's window, with two
/// brushes and a line brush). Degrees are never null, so both give the same.
fn fold(mut exprs: Vec<Expr>, least: bool) -> Expr {
    use datafusion::functions::core::expr_fn;
    match exprs.len() {
        0 => lit(if least { 1.0 } else { 0.0 }),
        1 => exprs.remove(0),
        _ if least => expr_fn::least(exprs),
        _ => expr_fn::greatest(exprs),
    }
}

fn contribution(c: &ResolvedContribution, width: f64) -> Result<Expr> {
    let tuples = c.contribution.effective_value().as_tuples();
    let producer = c.contribution.producer();
    let partial = c.contribution.value().partial.as_ref();
    if !tuples
        .iter()
        .flatten()
        .any(|(_, t)| matches!(t, ValueTest::Range { .. }))
    {
        let rest = partial.map_or(Ok(lit(0.0)), |k| key_degrees(c, k))?;
        return Ok(when(crate::predicate::contribution(c), lit(1.0)).otherwise(rest)?);
    }
    if partial.is_some() {
        return Err(Error::InvalidValue(
            "partial degrees go with keys, not with ranges".into(),
        ));
    }
    let sizes = producer
        .projections()
        .iter()
        .map(|p| {
            producer.pixel_grid(p.id()).map(|g| g.size().to_bits()).ok_or_else(|| {
                Error::InvalidDefinition(format!("a degree needs a pixel grid on {}, since its ranges are measured in pixels", p.id()))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let boxes = crate::cell_boxes::boxes(tuples).ok_or_else(|| {
        Error::InvalidDefinition(
            "a degree over pixel grids needs range comparisons on every projection".into(),
        )
    })?;
    let signature = Signature::uniform(sizes.len(), vec![DataType::Int64], Volatility::Immutable);
    Ok(ScalarUDF::from(CellDegree {
        boxes,
        sizes,
        width: width.to_bits(),
        signature,
        table: Default::default(),
    })
    .call(c.projections.clone()))
}

/// The partial degrees of a key contribution, as `CASE key WHEN … END`.
fn key_degrees(c: &ResolvedContribution, k: &KeyDegrees) -> Result<Expr> {
    let i = c
        .contribution
        .producer()
        .projections()
        .iter()
        .position(|p| p.id() == &k.projection)
        .ok_or_else(|| {
            Error::InvalidValue(format!(
                "partial degrees name {}, which the producer does not project",
                k.projection
            ))
        })?;
    let key = &c.projections[i];
    let mut arms = k
        .degrees
        .iter()
        .map(|(v, d)| (key.clone().eq(lit(v.clone())), lit(*d)));
    let Some((w, t)) = arms.next() else {
        return Ok(lit(0.0));
    };
    Ok(arms
        .fold(when(w, t), |mut case, (w, t)| case.when(w, t))
        .otherwise(lit(0.0))?)
}

struct CellDegree {
    boxes: Vec<Vec<(i64, i64)>>,
    /// Pixels per cell, per projection, as bits.
    sizes: Vec<u64>,
    width: u64,
    signature: Signature,
    /// Each cell's degree near the boxes, built on first use (see `Table`).
    table: std::sync::OnceLock<Option<Table>>,
}
impl std::fmt::Debug for CellDegree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CellDegree").field("boxes", &self.boxes.len()).field("sizes", &self.sizes).field("width", &self.width).finish()
    }
}
// The table follows from the boxes, sizes and width, so it takes no part.
impl PartialEq for CellDegree {
    fn eq(&self, other: &Self) -> bool {
        (&self.boxes, &self.sizes, self.width, &self.signature) == (&other.boxes, &other.sizes, other.width, &other.signature)
    }
}
impl Eq for CellDegree {}
impl std::hash::Hash for CellDegree {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        (&self.boxes, &self.sizes, self.width, &self.signature).hash(h);
    }
}

/// The degree of every cell within the width of the boxes' bounding box,
/// computed once, so a row costs a lookup instead of a distance to every box.
/// A lasso is some two hundred boxes (runs of cells), and measuring each row
/// against all of them took 450 ms over 10M rows where the predicate took 35.
/// Built box by box: each box updates only the cells within the width of it.
/// The distances are the same arithmetic as the loop, so the degrees are too.
struct Table {
    lo: Vec<i64>,
    dims: Vec<usize>,
    degrees: Vec<f64>,
}
/// Above this many cells the table is not built and each row is measured.
const TABLE_CELLS: usize = 1 << 21;

impl CellDegree {
    fn sizes(&self) -> Vec<f64> {
        self.sizes.iter().map(|s| f64::from_bits(*s)).collect()
    }
    /// The boxes' bounding box.
    fn hull(&self) -> Vec<(i64, i64)> {
        match self.boxes.first() {
            None => Vec::new(),
            Some(first) => (0..first.len())
                .map(|d| {
                    self.boxes.iter().fold((i64::MAX, i64::MIN), |(lo, hi), b| {
                        (lo.min(b[d].0), hi.max(b[d].1))
                    })
                })
                .collect(),
        }
    }
    /// Squared distance in pixels from a row's cells to a box.
    fn dist2(b: &[(i64, i64)], cells: impl Iterator<Item = i64>, sizes: &[f64]) -> f64 {
        b.iter()
            .zip(cells)
            .zip(sizes)
            .map(|((&(lo, hi), v), s)| {
                (lo.saturating_sub(v).max(v.saturating_sub(hi)).max(0) as f64 * s).powi(2)
            })
            .sum()
    }
    fn degree_at(d2: f64, width: f64) -> f64 {
        let d = d2.sqrt();
        if d == 0.0 {
            1.0
        } else if width == 0.0 {
            0.0
        } else {
            (1.0 - d / width).max(0.0)
        }
    }
    fn table(&self) -> Option<&Table> {
        self.table.get_or_init(|| self.build()).as_ref()
    }
    fn build(&self) -> Option<Table> {
        let (sizes, width, hull) = (self.sizes(), f64::from_bits(self.width), self.hull());
        if hull.is_empty() || sizes.iter().any(|s| !(*s > 0.0)) {
            return None;
        }
        // Cells within the width of a box, per projection.
        let reach: Vec<i64> = sizes.iter().map(|s| (width / s).ceil() as i64).collect();
        let lo: Vec<i64> = hull.iter().zip(&reach).map(|(h, r)| h.0.checked_sub(*r)).collect::<Option<_>>()?;
        let dims: Vec<usize> = hull.iter().zip(&reach).zip(&lo)
            .map(|((h, r), l)| h.1.checked_add(*r).and_then(|hi| hi.checked_sub(*l)).and_then(|n| usize::try_from(n + 1).ok()))
            .collect::<Option<_>>()?;
        let cells = dims.iter().try_fold(1usize, |n, d| n.checked_mul(*d))?;
        if cells > TABLE_CELLS {
            return None;
        }
        let mut d2 = vec![f64::INFINITY; cells];
        let mut at = vec![0i64; dims.len()];
        for b in &self.boxes {
            let from: Vec<i64> = b.iter().zip(&reach).zip(&lo).map(|((b, r), l)| (b.0 - r).max(*l)).collect();
            let to: Vec<i64> = b.iter().zip(&reach).zip(lo.iter().zip(&dims)).map(|((b, r), (l, n))| (b.1 + r).min(l + *n as i64 - 1)).collect();
            if from.iter().zip(&to).any(|(f, t)| f > t) {
                continue;
            }
            at.copy_from_slice(&from);
            'cells: loop {
                let index = at.iter().zip(&lo).zip(&dims).fold(0usize, |i, ((a, l), n)| i * n + (a - l) as usize);
                let d = Self::dist2(b, at.iter().copied(), &sizes);
                if d < d2[index] {
                    d2[index] = d;
                }
                // The next cell of the box's reach, the last projection fastest.
                for k in (0..at.len()).rev() {
                    if at[k] < to[k] {
                        at[k] += 1;
                        continue 'cells;
                    }
                    at[k] = from[k];
                }
                break;
            }
        }
        let degrees = d2.into_iter().map(|d| if d.is_finite() { Self::degree_at(d, width) } else { 0.0 }).collect();
        Some(Table { lo, dims, degrees })
    }
}

impl ScalarUDFImpl for CellDegree {
    fn name(&self) -> &str {
        "avenger_selection_cell_degree"
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> DFResult<DataType> {
        Ok(DataType::Float64)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> DFResult<ColumnarValue> {
        let cols = columns::<Int64Type>(&args)?;
        let table = self.table();
        let (sizes, width, hull) = (self.sizes(), f64::from_bits(self.width), self.hull());
        let mut cells = vec![0i64; cols.len()];
        let out: Float64Array = (0..args.number_rows)
            .map(|i| {
                if hull.is_empty() || cols.iter().any(|c| c.is_null(i)) {
                    return Some(0.0);
                }
                cells.iter_mut().zip(&cols).for_each(|(v, c)| *v = c.value(i));
                Some(match table {
                    Some(t) => t.degree(&cells),
                    None => self.measured(&cells, &sizes, width, &hull),
                })
            })
            .collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
    }
}

impl CellDegree {
    /// A row's degree measured against every box, without the table.
    fn measured(&self, cells: &[i64], sizes: &[f64], width: f64, hull: &[(i64, i64)]) -> f64 {
        // The boxes' bounding box: a row beyond the width from it is at 0.
        let near = Self::dist2(hull, cells.iter().copied(), sizes);
        if near > 0.0 && near >= width * width {
            return 0.0;
        }
        let mut d2 = f64::INFINITY;
        for b in &self.boxes {
            d2 = d2.min(Self::dist2(b, cells.iter().copied(), sizes));
            if d2 == 0.0 {
                break;
            }
        }
        Self::degree_at(d2, width)
    }
}
impl Table {
    /// A row's degree from the table: 0 outside it, beyond every box's reach.
    fn degree(&self, cells: &[i64]) -> f64 {
        let mut index = 0usize;
        for ((c, l), n) in cells.iter().zip(&self.lo).zip(&self.dims) {
            let k = c.saturating_sub(*l);
            if k < 0 || k >= *n as i64 {
                return 0.0;
            }
            index = index * n + k as usize;
        }
        self.degrees[index]
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;

    /// The table gives, bit for bit, the degree the loop measures, on every
    /// cell around random boxes (2D runs like a lasso's, and 3D voxels), with
    /// uneven cell sizes and widths, including a width of 0.
    #[test]
    fn the_table_agrees_with_measuring_every_box() {
        let mut s = 11u64;
        let mut next = move |n: i64| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 33) % n as u64) as i64
        };
        for case in 0..40 {
            let dims = if case % 2 == 0 { 2 } else { 3 };
            let boxes: Vec<Vec<(i64, i64)>> = (0..1 + next(60))
                .map(|_| (0..dims).map(|_| { let a = next(80) - 20; (a, a + next(12)) }).collect())
                .collect();
            // In 3D a small reach, so the table stays within `TABLE_CELLS`.
            let sizes: Vec<f64> = (0..dims).map(|_| if dims == 2 { [1.0, 0.5, 2.5, 4.0][next(4) as usize] } else { [1.0, 2.5, 4.0][next(3) as usize] }).collect();
            let width = if dims == 2 { [0.0f64, 3.0, 10.0, 60.0][next(4) as usize] } else { [0.0f64, 3.0, 10.0][next(3) as usize] };
            let udf = CellDegree {
                boxes,
                sizes: sizes.iter().map(|s| s.to_bits()).collect(),
                width: width.to_bits(),
                signature: Signature::uniform(dims, vec![DataType::Int64], Volatility::Immutable),
                table: Default::default(),
            };
            let t = udf.table().expect("a table for a small reach");
            let hull = udf.hull();
            // Every cell of the table and two beyond it on each side.
            let (from, to): (Vec<i64>, Vec<i64>) = t.lo.iter().zip(&t.dims).map(|(l, n)| (l - 2, l + *n as i64 + 1)).unzip();
            let mut at = from.clone();
            'cells: loop {
                let (a, b) = (t.degree(&at), udf.measured(&at, &sizes, width, &hull));
                assert_eq!(a.to_bits(), b.to_bits(), "case {case}, cell {at:?}: table {a}, measured {b}");
                for k in (0..dims).rev() {
                    if at[k] < to[k] {
                        at[k] += 1;
                        continue 'cells;
                    }
                    at[k] = from[k];
                }
                break;
            }
        }
    }

    /// Beyond `TABLE_CELLS` there is no table, and each row is measured.
    #[test]
    fn a_reach_too_large_for_a_table_measures_each_row() {
        let udf = CellDegree {
            boxes: vec![vec![(0, 10), (0, 10), (0, 10)]],
            sizes: vec![0.5f64.to_bits(); 3],
            width: 60.0f64.to_bits(),
            signature: Signature::uniform(3, vec![DataType::Int64], Volatility::Immutable),
            table: Default::default(),
        };
        assert!(udf.table().is_none());
        let (sizes, hull) = (udf.sizes(), udf.hull());
        assert_eq!(udf.measured(&[5, 5, 5], &sizes, 60.0, &hull), 1.0);
        assert_eq!(udf.measured(&[10 + 60, 5, 5], &sizes, 60.0, &hull), 1.0 - 30.0 / 60.0);
    }
}

/// Degrees in (0, 1) for keys a value selects only in part.
#[derive(Clone, Debug)]
pub(crate) struct KeyDegrees {
    pub(crate) projection: ProjectionId,
    pub(crate) degrees: Vec<(ScalarValue, f64)>,
}
impl PartialEq for KeyDegrees {
    fn eq(&self, other: &Self) -> bool {
        self.projection == other.projection
            && self.degrees.len() == other.degrees.len()
            && self
                .degrees
                .iter()
                .zip(&other.degrees)
                .all(|((a, x), (b, y))| a == b && x.to_bits() == y.to_bits())
    }
}
impl Eq for KeyDegrees {}
impl KeyDegrees {
    /// Checked against the producer, keys normalised as the crate's sets
    /// normalise them, sorted, one degree per key.
    pub(crate) fn canonical(self, producer: &ProducerDefinition) -> Result<Self> {
        let invalid = |m: String| Err(Error::InvalidValue(m));
        if !producer
            .projections()
            .iter()
            .any(|p| p.id() == &self.projection)
        {
            return invalid(format!(
                "partial degrees name {}, which the producer does not project",
                self.projection
            ));
        }
        if producer.pixel_grid(&self.projection).is_some() {
            return invalid(format!(
                "partial degrees need an ungridded key, and {} has a pixel grid",
                self.projection
            ));
        }
        let mut degrees = Vec::with_capacity(self.degrees.len());
        for (k, d) in self.degrees {
            if !(d.is_finite() && d > 0.0 && d < 1.0) {
                return invalid(format!("a partial degree lies in (0, 1), not {d}"));
            }
            degrees.push((canonical_values(vec![k])?.remove(0), d));
        }
        degrees.sort_by(|a, b| scalar_cmp(&a.0, &b.0));
        if degrees
            .windows(2)
            .any(|w| w[0].0.data_type() != w[1].0.data_type())
        {
            return invalid("partial degrees use one key type".into());
        }
        if degrees
            .windows(2)
            .any(|w| scalar_cmp(&w[0].0, &w[1].0).is_eq())
        {
            return invalid("partial degrees name a key twice".into());
        }
        Ok(Self {
            projection: self.projection,
            degrees,
        })
    }
}
