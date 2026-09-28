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
fn fold(exprs: Vec<Expr>, least: bool) -> Expr {
    exprs
        .into_iter()
        .reduce(|a, b| {
            let pick_a = if least {
                a.clone().lt_eq(b.clone())
            } else {
                a.clone().gt_eq(b.clone())
            };
            when(pick_a, a).otherwise(b).expect("a CASE with an ELSE")
        })
        .unwrap_or_else(|| lit(if least { 1.0 } else { 0.0 }))
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

#[derive(Debug, PartialEq, Eq, Hash)]
struct CellDegree {
    boxes: Vec<Vec<(i64, i64)>>,
    /// Pixels per cell, per projection, as bits.
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
        let cols = columns::<Int64Type>(&args)?;
        let sizes: Vec<f64> = self.sizes.iter().map(|s| f64::from_bits(*s)).collect();
        let width = f64::from_bits(self.width);
        // The boxes' bounding box: a row beyond the width from it is at 0.
        let hull: Vec<(i64, i64)> = match self.boxes.first() {
            None => Vec::new(),
            Some(first) => (0..first.len())
                .map(|d| {
                    self.boxes.iter().fold((i64::MAX, i64::MIN), |(lo, hi), b| {
                        (lo.min(b[d].0), hi.max(b[d].1))
                    })
                })
                .collect(),
        };
        // Squared distance in pixels from row i's cells to a box.
        let dist2 = |b: &[(i64, i64)], i: usize| -> f64 {
            b.iter()
                .zip(&cols)
                .zip(&sizes)
                .map(|((&(lo, hi), c), s)| {
                    let v = c.value(i);
                    (lo.saturating_sub(v).max(v.saturating_sub(hi)).max(0) as f64 * s).powi(2)
                })
                .sum()
        };
        let degree = |i: usize| -> f64 {
            if hull.is_empty() || cols.iter().any(|c| c.is_null(i)) {
                return 0.0;
            }
            let near = dist2(&hull, i);
            if near > 0.0 && near >= width * width {
                return 0.0;
            }
            let mut d2 = f64::INFINITY;
            for b in &self.boxes {
                d2 = d2.min(dist2(b, i));
                if d2 == 0.0 {
                    break;
                }
            }
            let d = d2.sqrt();
            if d == 0.0 {
                1.0
            } else if width == 0.0 {
                0.0
            } else {
                (1.0 - d / width).max(0.0)
            }
        };
        let out: Float64Array = (0..args.number_rows).map(|i| Some(degree(i))).collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
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
