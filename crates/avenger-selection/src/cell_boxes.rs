//! Many tuples of cell ranges, tested with one function call per row.
//!
//! Not upstream: added in this repo (VENDORED.md). A contribution of many
//! tuples, each a range of Int64 cells on every projection — what a lasso
//! becomes through `SelectionValue::polygon` — compiles by default to an OR
//! of ANDs in which every term repeats its projection's cell expression.
//! DataFusion does not share those across the branches of an OR, so the cell
//! function would run six times per tuple and row (FINDINGS.md 25). Here
//! each projection is evaluated once, and the row is looked up in the boxes.
//! Membership is the same: a null cell matches no box.

use std::{ops::Bound, sync::Arc};

use datafusion::{
    arrow::{
        array::{Array, BooleanArray},
        datatypes::{DataType, Int64Type},
    },
    common::{Result as DFResult, ScalarValue},
    logical_expr::{
        ColumnarValue, Expr, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
    },
};

use crate::{udf::columns, values::Tuple, ValueTest};

/// Below this many tuples the ordinary expression stays, readable and cheap.
pub(crate) const MIN_TUPLES: usize = 8;

/// Inclusive cell bounds per projection, or None if a term is not an Int64
/// range (exact values, sets, other types): those keep the ordinary form.
pub(crate) fn boxes(tuples: &[Tuple]) -> Option<Vec<Vec<(i64, i64)>>> {
    let bound = |b: &Bound<ScalarValue>, lower: bool| -> Option<i64> {
        match b {
            Bound::Unbounded => Some(if lower { i64::MIN } else { i64::MAX }),
            Bound::Included(ScalarValue::Int64(Some(v))) => Some(*v),
            Bound::Excluded(ScalarValue::Int64(Some(v))) => {
                if lower {
                    v.checked_add(1)
                } else {
                    v.checked_sub(1)
                }
            }
            _ => None,
        }
    };
    tuples
        .iter()
        .map(|t| {
            t.iter()
                .map(|(_, test)| match test {
                    ValueTest::Range { lower, upper } => {
                        Some((bound(lower, true)?, bound(upper, false)?))
                    }
                    _ => None,
                })
                .collect()
        })
        .collect()
}

pub(crate) fn predicate(boxes: Vec<Vec<(i64, i64)>>, projections: &[Expr]) -> Expr {
    // Index on a dimension where every box is one cell wide, if there is one:
    // a lasso's rows of cells, or voxels.
    let index = (0..projections.len())
        .find(|&d| boxes.iter().all(|b| b[d].0 == b[d].1))
        .map(|d| (d, Index::new(&boxes, d)));
    ScalarUDF::from(CellBoxes {
        boxes,
        index,
        signature: Signature::uniform(
            projections.len(),
            vec![DataType::Int64],
            Volatility::Immutable,
        ),
    })
    .call(projections.to_vec())
}

/// The boxes by their cell on the key dimension: a dense table when the
/// cells are close together, as a lasso's rows are, else sorted for search.
#[derive(Debug, PartialEq, Eq, Hash)]
enum Index {
    Dense { first: i64, boxes: Vec<Vec<u32>> },
    Sorted(Vec<(i64, Vec<u32>)>),
}
impl Index {
    fn new(boxes: &[Vec<(i64, i64)>], d: usize) -> Self {
        let mut by_key: std::collections::BTreeMap<i64, Vec<u32>> = Default::default();
        for (k, b) in boxes.iter().enumerate() {
            by_key.entry(b[d].0).or_default().push(k as u32);
        }
        let (Some((&lo, _)), Some((&hi, _))) = (by_key.first_key_value(), by_key.last_key_value())
        else {
            return Index::Sorted(Vec::new());
        };
        let span = usize::try_from(hi.abs_diff(lo))
            .ok()
            .and_then(|s| s.checked_add(1))
            .filter(|&s| s <= 4 * by_key.len() + 4096);
        match span {
            Some(span) => {
                let mut dense = vec![Vec::new(); span];
                for (k, v) in by_key {
                    dense[k.abs_diff(lo) as usize] = v;
                }
                Index::Dense {
                    first: lo,
                    boxes: dense,
                }
            }
            None => Index::Sorted(by_key.into_iter().collect()),
        }
    }
    fn get(&self, key: i64) -> &[u32] {
        match self {
            Index::Dense { first, boxes } => (key >= *first)
                .then(|| boxes.get(key.abs_diff(*first) as usize))
                .flatten()
                .map_or(&[], Vec::as_slice),
            Index::Sorted(v) => v
                .binary_search_by_key(&key, |e| e.0)
                .map_or(&[], |i| v[i].1.as_slice()),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct CellBoxes {
    boxes: Vec<Vec<(i64, i64)>>,
    /// The key dimension and its index, when there is one.
    index: Option<(usize, Index)>,
    signature: Signature,
}
impl ScalarUDFImpl for CellBoxes {
    fn name(&self) -> &str {
        "avenger_selection_cell_boxes"
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> DFResult<DataType> {
        Ok(DataType::Boolean)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> DFResult<ColumnarValue> {
        let cols = columns::<Int64Type>(&args)?;
        let values: Vec<&[i64]> = cols.iter().map(|c| c.values().as_ref()).collect();
        let nulls = cols.iter().any(|c| c.null_count() > 0);
        let inside = |b: &[(i64, i64)], i: usize| {
            b.iter()
                .zip(&values)
                .all(|((lo, hi), c)| (*lo..=*hi).contains(&c[i]))
        };
        let hit = |i: usize| -> bool {
            if nulls && cols.iter().any(|c| c.is_null(i)) {
                return false;
            }
            match &self.index {
                Some((d, index)) => index
                    .get(values[*d][i])
                    .iter()
                    .any(|k| inside(&self.boxes[*k as usize], i)),
                None => self.boxes.iter().any(|b| inside(b, i)),
            }
        };
        let out: BooleanArray = (0..args.number_rows).map(|i| Some(hit(i))).collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
    }
}
