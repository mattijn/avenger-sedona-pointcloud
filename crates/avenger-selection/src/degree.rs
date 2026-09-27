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
//! - a tuple of equalities and sets, which has no distance, gives 1 or 0;
//! - a range on a projection without a grid is refused: its data units need
//!   not match another projection's, and the screen is the common measure.
//!
//! Composition follows fuzzy logic: intersected producers and `All` take the
//! least degree, unions and `Any` the greatest, and `Not` its complement, so
//! a degree of exactly 1 is where `predicate` holds.

use std::sync::Arc;

use datafusion::{
    arrow::{
        array::{Array, Float64Array, Int64Array},
        compute::cast,
        datatypes::DataType,
    },
    common::Result as DFResult,
    logical_expr::{
        lit, when, ColumnarValue, Expr, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
    },
};

use crate::{
    resolve::{ResolvedContribution, ResolvedFilter, SelectionStatus},
    ConsumerFilter, EmptySelection, Error, Resolution, Result, SelectionSet, ValueTest,
};

impl ConsumerFilter {
    /// A Float64 expression in [0, 1]: 1 where `predicate` holds, falling
    /// linearly to 0 at `width` logical pixels from the selection. A width of
    /// 0 gives the predicate as 1 or 0.
    pub fn degree(&self, selections: &SelectionSet, width: f64) -> Result<Expr> {
        if !width.is_finite() || width < 0.0 {
            return Err(Error::InvalidValue("a soft width must be finite and not negative".into()));
        }
        resolved(&self.resolve(selections)?, width)
    }
}

fn resolved(filter: &ResolvedFilter, width: f64) -> Result<Expr> {
    Ok(match filter {
        ResolvedFilter::All(fs) => fold(fs.iter().map(|f| resolved(f, width)).collect::<Result<_>>()?, true, 1.0),
        ResolvedFilter::Any(fs) => fold(fs.iter().map(|f| resolved(f, width)).collect::<Result<_>>()?, false, 0.0),
        ResolvedFilter::Not(f) => lit(1.0) - resolved(f, width)?,
        ResolvedFilter::Selection(s) => match s.status {
            SelectionStatus::Inactive => lit(if s.empty == EmptySelection::MatchAll { 1.0 } else { 0.0 }),
            SelectionStatus::AllExcluded => lit(1.0),
            SelectionStatus::Active => fold(
                s.contributions.iter().map(|c| contribution(c, width)).collect::<Result<_>>()?,
                s.resolution == Resolution::Intersect,
                if s.resolution == Resolution::Intersect { 1.0 } else { 0.0 },
            ),
        },
    })
}

/// The least (fuzzy AND) or greatest (fuzzy OR) of the degrees.
fn fold(exprs: Vec<Expr>, least: bool, empty: f64) -> Expr {
    exprs
        .into_iter()
        .reduce(|a, b| {
            let pick_a = if least { a.clone().lt_eq(b.clone()) } else { a.clone().gt_eq(b.clone()) };
            when(pick_a, a).otherwise(b).expect("a CASE with an ELSE")
        })
        .unwrap_or_else(|| lit(empty))
}

fn contribution(c: &ResolvedContribution, width: f64) -> Result<Expr> {
    let tuples = c.contribution.effective_value().as_tuples();
    let producer = c.contribution.producer();
    let ranges = tuples.iter().flatten().any(|(_, t)| matches!(t, ValueTest::Range { .. }));
    let partial = c.contribution.value().partial.as_ref();
    if !ranges {
        let p = crate::predicate::contribution(c);
        // Rows the tuples leave out take their key's partial degree, if any.
        let rest = match partial {
            None => lit(0.0),
            Some(k) => {
                let i = producer.projections().iter().position(|p| p.id() == &k.projection).expect("checked when applied");
                let key = c.projections[i].clone();
                let mut degrees = k.degrees.iter();
                match degrees.next() {
                    None => lit(0.0),
                    Some((v, d)) => {
                        let mut case = when(key.clone().eq(lit(v.clone())), lit(*d));
                        for (v, d) in degrees {
                            case = case.when(key.clone().eq(lit(v.clone())), lit(*d));
                        }
                        case.otherwise(lit(0.0))?
                    }
                }
            }
        };
        return Ok(when(p, lit(1.0)).otherwise(rest)?);
    }
    if partial.is_some() {
        return Err(Error::InvalidDefinition("partial degrees go with keys, not with ranges".into()));
    }
    let mut sizes = Vec::new();
    for p in producer.projections() {
        match producer.pixel_grid(p.id()) {
            Some(g) => sizes.push(g.size()),
            None => {
                return Err(Error::InvalidDefinition(format!(
                    "a degree needs a pixel grid on {}, since its ranges are measured in pixels",
                    p.id()
                )))
            }
        }
    }
    let boxes = crate::cell_boxes::boxes(tuples).ok_or_else(|| {
        Error::InvalidDefinition("a degree over pixel grids needs range comparisons on every projection".into())
    })?;
    Ok(ScalarUDF::from(CellDegree {
        boxes,
        sizes: sizes.iter().map(|s| s.to_bits()).collect(),
        width: width.to_bits(),
        signature: Signature::uniform(sizes.len(), vec![DataType::Int64], Volatility::Immutable),
    })
    .call(c.projections.clone()))
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct CellDegree {
    boxes: Vec<Vec<(i64, i64)>>,
    sizes: Vec<u64>,
    width: u64,
    signature: Signature,
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
        let rows = args.number_rows;
        let cols = args
            .args
            .iter()
            .map(|a| Ok(cast(&a.clone().into_array(rows)?, &DataType::Int64)?))
            .collect::<DFResult<Vec<_>>>()?;
        let cols: Vec<&Int64Array> = cols.iter().map(|c| c.as_any().downcast_ref::<Int64Array>().unwrap()).collect();
        let sizes: Vec<f64> = self.sizes.iter().map(|s| f64::from_bits(*s)).collect();
        let width = f64::from_bits(self.width);
        let hull: Vec<(i64, i64)> = match self.boxes.first() {
            None => Vec::new(),
            Some(first) => (0..first.len())
                .map(|d| self.boxes.iter().fold((i64::MAX, i64::MIN), |(lo, hi), b| (lo.min(b[d].0), hi.max(b[d].1))))
                .collect(),
        };
        let gap = |c: i64, (lo, hi): (i64, i64)| if c < lo { (lo as f64) - c as f64 } else if c > hi { c as f64 - hi as f64 } else { 0.0 };
        let out: Float64Array = (0..rows)
            .map(|i| {
                if cols.iter().any(|c| c.is_null(i)) {
                    return Some(0.0);
                }
                // Beyond the width from the boxes' bounding box: no box is nearer.
                let lb: f64 = hull.iter().zip(&cols).zip(&sizes).map(|((r, c), s)| (gap(c.value(i), *r) * s).powi(2)).sum();
                if hull.is_empty() || (width > 0.0 && lb >= width * width) || (width == 0.0 && lb > 0.0) {
                    return Some(0.0);
                }
                let mut d2 = f64::INFINITY;
                for b in &self.boxes {
                    let e: f64 = b.iter().zip(&cols).zip(&sizes).map(|((r, c), s)| (gap(c.value(i), *r) * s).powi(2)).sum();
                    d2 = d2.min(e);
                    if d2 == 0.0 {
                        break;
                    }
                }
                let d = d2.sqrt();
                Some(if d == 0.0 { 1.0 } else if width == 0.0 { 0.0 } else { (1.0 - d / width).max(0.0) })
            })
            .collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
    }
}

/// Degrees in (0, 1) for keys a value selects only in part (not upstream).
#[derive(Clone, Debug)]
pub(crate) struct KeyDegrees {
    pub(crate) projection: crate::ProjectionId,
    pub(crate) degrees: Vec<(datafusion::common::ScalarValue, f64)>,
}
impl PartialEq for KeyDegrees {
    fn eq(&self, other: &Self) -> bool {
        self.projection == other.projection
            && self.degrees.len() == other.degrees.len()
            && self.degrees.iter().zip(&other.degrees).all(|((a, x), (b, y))| a == b && x.to_bits() == y.to_bits())
    }
}
impl Eq for KeyDegrees {}
impl KeyDegrees {
    /// Checked against the producer, sorted by key, one degree per key.
    pub(crate) fn canonical(self, producer: &crate::ProducerDefinition) -> Result<Self> {
        if !producer.projections().iter().any(|p| p.id() == &self.projection) {
            return Err(Error::InvalidValue(format!("partial degrees name {}, which the producer does not project", self.projection)));
        }
        if producer.pixel_grid(&self.projection).is_some() {
            return Err(Error::InvalidValue(format!("partial degrees need an ungridded key, and {} has a pixel grid", self.projection)));
        }
        if let Some((_, d)) = self.degrees.iter().find(|(_, d)| !(d.is_finite() && *d > 0.0 && *d < 1.0)) {
            return Err(Error::InvalidValue(format!("a partial degree lies in (0, 1), not {d}")));
        }
        let mut keys: Vec<_> = self.degrees.iter().map(|(k, _)| k.clone()).collect();
        let n = keys.len();
        keys = crate::values::canonical_values(keys)?;
        if keys.len() != n {
            return Err(Error::InvalidValue("partial degrees name a key twice".into()));
        }
        let mut degrees = self.degrees;
        degrees.sort_by(|a, b| crate::values::scalar_cmp(&a.0, &b.0));
        Ok(Self { projection: self.projection, degrees })
    }
}
