// Not upstream: tests for `ConsumerFilter::degree` (VENDORED.md).
mod common;

use avenger_scales_datafusion::BuiltinScale;
use avenger_selection::*;
use common::*;
use datafusion::{
    arrow::array::{Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch},
    logical_expr::Expr,
    prelude::SessionContext,
};
use std::{collections::HashMap, sync::Arc};

fn numbers(values: &[f64]) -> ArrayRef {
    Arc::new(Float64Array::from(values.to_vec()))
}
fn grid(size: f64) -> PixelGrid {
    // Data 0..100 on 0..100 px: a data unit is a pixel.
    PixelGrid::new(BuiltinScale::Linear, numbers(&[0.0, 100.0]), numbers(&[0.0, 100.0]), HashMap::new(), 0.0, size).unwrap()
}
fn pid(name: &str) -> ProjectionId {
    ProjectionId::new(name).unwrap()
}
fn brush(size: f64) -> ProducerDefinition {
    producer("brush", view("map"), &["x", "y"]).with_pixel_grids([(pid("x"), grid(size)), (pid("y"), grid(size))]).unwrap()
}
fn rect(x: (f64, f64), y: (f64, f64)) -> SelectionValue {
    SelectionValue::tuple([(pid("x"), ValueTest::range(x.0..=x.1)), (pid("y"), ValueTest::range(y.0..=y.1))])
}
fn points(xy: &[(f64, f64)]) -> RecordBatch {
    batch(vec![
        ("id", Arc::new(Int64Array::from_iter_values(0..xy.len() as i64))),
        ("x", numbers(&xy.iter().map(|p| p.0).collect::<Vec<_>>())),
        ("y", numbers(&xy.iter().map(|p| p.1).collect::<Vec<_>>())),
    ])
}
async fn eval(rows: RecordBatch, degree: Expr, pred: Expr) -> Vec<(f64, bool)> {
    let out = SessionContext::new().read_batch(rows).unwrap().select(vec![degree.alias("d"), pred.alias("p")]).unwrap().collect().await.unwrap();
    out.iter()
        .flat_map(|b| {
            let d = b.column(0).as_any().downcast_ref::<Float64Array>().unwrap().clone();
            let p = b.column(1).as_any().downcast_ref::<BooleanArray>().unwrap().clone();
            (0..b.num_rows()).map(move |i| (d.value(i), p.value(i)))
        })
        .collect()
}

#[tokio::test]
async fn falls_off_with_the_distance_in_pixels() {
    let p = brush(2.0);
    let s = state(Resolution::Intersect).set(&p, rect((20.0, 40.0), (20.0, 40.0))).unwrap();
    // Inside; 6 px right of the brush (3 cells of 2 px); 6 px right and 8 px up
    // (a 10 px diagonal); beyond the width.
    let rows = points(&[(30.0, 30.0), (46.5, 30.0), (46.5, 48.5), (80.0, 80.0)]);
    let got = eval(rows, membership().degree(&s, 20.0).unwrap(), membership().predicate(&s).unwrap()).await;
    let d: Vec<f64> = got.iter().map(|g| g.0).collect();
    assert_eq!(d, vec![1.0, 1.0 - 6.0 / 20.0, 1.0 - 10.0 / 20.0, 0.0]);
    assert_eq!(got.iter().map(|g| g.1).collect::<Vec<_>>(), vec![true, false, false, false]);
}

#[tokio::test]
async fn a_degree_of_one_is_exactly_the_predicate() {
    // A brush, and a lasso of many cell runs (the lookup path).
    let p = brush(1.0);
    let ring = [[10.0, 10.0], [90.0, 20.0], [60.0, 50.0], [85.0, 90.0], [15.0, 70.0]];
    let lasso = SelectionValue::polygon((&pid("x"), &grid(1.0)), (&pid("y"), &grid(1.0)), &ring).unwrap();
    assert!(lasso.as_tuples().len() >= 8);
    let mut s = 7_u64;
    let mut next = || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (s >> 11) as f64 / (1u64 << 53) as f64 * 110.0 - 5.0
    };
    let xy: Vec<(f64, f64)> = (0..3000).map(|_| (next(), next())).collect();
    for value in [rect((20.0, 45.0), (30.0, 60.0)), lasso] {
        let st = state(Resolution::Intersect).set(&p, value).unwrap();
        for width in [0.0, 5.0, 25.0] {
            let got = eval(points(&xy), membership().degree(&st, width).unwrap(), membership().predicate(&st).unwrap()).await;
            assert!(got.iter().any(|g| g.1) && got.iter().any(|g| !g.1));
            for (d, pred) in &got {
                assert!((0.0..=1.0).contains(d));
                assert_eq!(*d == 1.0, *pred, "width {width}: degree {d}");
                if width == 0.0 {
                    assert!(*d == 0.0 || *d == 1.0);
                }
            }
        }
    }
}

#[tokio::test]
async fn keys_give_one_or_zero_and_composition_is_fuzzy() {
    let keys = producer("keys", view("legend"), &["class"]);
    let p = brush(1.0);
    let s = SelectionSet::new([(id(), Resolution::Intersect)]).unwrap()
        .set(&keys, values("class", [2_i64.into(), 6_i64.into()])).unwrap()
        .set(&p, rect((0.0, 10.0), (0.0, 10.0))).unwrap();
    let rows = batch(vec![
        ("id", Arc::new(Int64Array::from(vec![0_i64, 1, 2]))),
        ("class", Arc::new(Int64Array::from(vec![6_i64, 6, 3]))),
        ("x", numbers(&[5.0, 15.5, 5.0])),
        ("y", numbers(&[5.0, 5.0, 5.0])),
    ]);
    // Intersected: the least of the key's 1/0 and the brush's degree.
    let got = eval(rows.clone(), membership().degree(&s, 10.0).unwrap(), membership().predicate(&s).unwrap()).await;
    assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), vec![1.0, 1.0 - 5.0 / 10.0, 0.0]);
    // Not: the complement.
    let not = filter(view("other"), SelectionFilter::Not(Box::new(SelectionFilter::membership(&id(), EmptySelection::MatchAll))));
    let got = eval(rows, not.degree(&s, 10.0).unwrap(), not.predicate(&s).unwrap()).await;
    assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), vec![0.0, 0.5, 1.0]);
}

#[test]
fn refuses_ranges_without_a_grid_and_bad_widths() {
    let plain = interval("brush", "x");
    let s = state(Resolution::Intersect).set(&plain, between("x", 0, 10)).unwrap();
    assert!(matches!(membership().degree(&s, 5.0), Err(Error::InvalidDefinition(_))));
    let s = state(Resolution::Intersect);
    assert!(membership().degree(&s, f64::NAN).is_err());
    assert!(membership().degree(&s, -1.0).is_err());
    // Inactive: the empty policy, as a degree.
    assert_eq!(format!("{}", membership().degree(&s, 5.0).unwrap()), format!("{}", datafusion::logical_expr::lit(1.0)));
}
