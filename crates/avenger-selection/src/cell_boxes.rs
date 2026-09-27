//! Many tuples of cell ranges, tested with one function call per row.
//!
//! Not upstream: added in this repo (VENDORED.md). A contribution of many
//! tuples, each a range of Int64 cells on every projection — what a lasso
//! becomes through `SelectionValue::polygon` — compiles by default to an OR
//! of ANDs in which every term repeats its projection's cell expression.
//! DataFusion does not share those across the branches of an OR, so a
//! lasso of 80 rows of cells evaluated 480 pixel-cell calls per row.
//! Here each projection is evaluated once, and the row is looked up in the
//! boxes. Membership is the same: a null cell matches no box.

use std::{collections::HashMap, ops::Bound, sync::Arc};

use datafusion::{
    arrow::{
        array::{Array, BooleanArray, Int64Array},
        compute::cast,
        datatypes::DataType,
    },
    common::{Result as DFResult, ScalarValue},
    logical_expr::{ColumnarValue, Expr, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility},
};

use crate::{values::Tuple, ValueTest};

/// Below this many tuples the ordinary expression stays, readable and cheap.
pub(crate) const MIN_TUPLES: usize = 8;

/// Inclusive cell bounds per projection, or None if a term is not an Int64
/// range (exact values, sets, other types): those keep the ordinary form.
pub(crate) fn boxes(tuples: &[Tuple]) -> Option<Vec<Vec<(i64, i64)>>> {
    let bound = |b: &Bound<ScalarValue>, lower: bool| -> Option<i64> {
        match b {
            Bound::Unbounded => Some(if lower { i64::MIN } else { i64::MAX }),
            Bound::Included(ScalarValue::Int64(Some(v))) => Some(*v),
            Bound::Excluded(ScalarValue::Int64(Some(v))) => if lower { v.checked_add(1) } else { v.checked_sub(1) },
            _ => None,
        }
    };
    tuples
        .iter()
        .map(|t| {
            t.iter()
                .map(|(_, test)| match test {
                    ValueTest::Range { lower, upper } => Some((bound(lower, true)?, bound(upper, false)?)),
                    _ => None,
                })
                .collect()
        })
        .collect()
}

pub(crate) fn predicate(boxes: Vec<Vec<(i64, i64)>>, projections: &[Expr]) -> Expr {
    let n = projections.len();
    // Index on a dimension where every box is one cell wide, if there is one.
    let key = (0..n).find(|&d| boxes.iter().all(|b| b[d].0 == b[d].1));
    ScalarUDF::from(CellBoxes {
        boxes,
        key,
        signature: Signature::uniform(n, vec![DataType::Int64], Volatility::Immutable),
    })
    .call(projections.to_vec())
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct CellBoxes {
    boxes: Vec<Vec<(i64, i64)>>,
    key: Option<usize>,
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
        let rows = args.number_rows;
        let cols = args
            .args
            .iter()
            .map(|a| Ok(cast(&a.clone().into_array(rows)?, &DataType::Int64)?))
            .collect::<DFResult<Vec<_>>>()?;
        let cols: Vec<&Int64Array> = cols.iter().map(|c| c.as_any().downcast_ref::<Int64Array>().unwrap()).collect();
        let inside = |b: &Vec<(i64, i64)>, i: usize| b.iter().zip(&cols).all(|((lo, hi), c)| (*lo..=*hi).contains(&c.value(i)));
        let index: Option<HashMap<i64, Vec<usize>>> = self.key.map(|d| {
            let mut m: HashMap<i64, Vec<usize>> = HashMap::new();
            for (k, b) in self.boxes.iter().enumerate() {
                m.entry(b[d].0).or_default().push(k);
            }
            m
        });
        let out: BooleanArray = (0..rows)
            .map(|i| {
                if cols.iter().any(|c| c.is_null(i)) {
                    return Some(false);
                }
                Some(match (&index, self.key) {
                    (Some(m), Some(d)) => m.get(&cols[d].value(i)).is_some_and(|ks| ks.iter().any(|k| inside(&self.boxes[*k], i))),
                    _ => self.boxes.iter().any(|b| inside(b, i)),
                })
            })
            .collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
    }
}
