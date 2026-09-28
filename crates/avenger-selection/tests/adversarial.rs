// Not upstream: edge cases for the additions (VENDORED.md).
mod common;
mod support;

use avenger_scales_datafusion::{avenger_scales::scalar::Scalar, BuiltinScale};
use avenger_selection::*;
use common::*;
use datafusion::{
    arrow::array::{Int64Array, RecordBatch},
    common::ScalarValue,
    logical_expr::col,
    prelude::SessionContext,
};
use std::{collections::HashMap, sync::Arc};
use support::*;

fn series(rows: &[(i64, f64, f64)]) -> RecordBatch {
    batch(vec![
        (
            "line",
            Arc::new(Int64Array::from(
                rows.iter().map(|r| r.0).collect::<Vec<_>>(),
            )),
        ),
        ("t", numbers(&rows.iter().map(|r| r.1).collect::<Vec<_>>())),
        ("n", numbers(&rows.iter().map(|r| r.2).collect::<Vec<_>>())),
    ])
}
async fn crossing(rows: &[(i64, f64, f64)], from: [f64; 2], to: [f64; 2]) -> Vec<ScalarValue> {
    let df = SessionContext::new().read_batch(series(rows)).unwrap();
    SeriesTest::Crosses { from, to }
        .keys(df, col("line"), col("t"), col("n"))
        .await
        .unwrap()
}

#[tokio::test]
async fn collinear_segments_cross_only_where_they_overlap() {
    // Line 1 lies on y = 0 from 0 to 1; the brush on y = 0 from 2 to 3.
    assert!(
        crossing(&[(1, 0.0, 0.0), (1, 1.0, 0.0)], [2.0, 0.0], [3.0, 0.0])
            .await
            .is_empty()
    );
    // Overlapping, and touching end to end, do cross.
    assert_eq!(
        crossing(&[(1, 0.0, 0.0), (1, 2.0, 0.0)], [1.0, 0.0], [3.0, 0.0])
            .await
            .len(),
        1
    );
    assert_eq!(
        crossing(&[(1, 0.0, 0.0), (1, 2.0, 0.0)], [2.0, 0.0], [3.0, 0.0])
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn a_point_brush_takes_only_the_series_through_it() {
    // (5, 5) lies on line 1's segment, and on line 2's supporting line only.
    let rows = [(1, 0.0, 0.0), (1, 10.0, 10.0), (2, 0.0, 0.0), (2, 1.0, 1.0)];
    assert_eq!(
        crossing(&rows, [5.0, 5.0], [5.0, 5.0]).await,
        vec![ScalarValue::Int64(Some(1))]
    );
}

#[tokio::test]
async fn a_lasso_may_leave_a_clamped_plot() {
    let mut opts = HashMap::new();
    opts.insert("clamp".to_string(), Scalar::from(true));
    let g = PixelGrid::new(
        BuiltinScale::Linear,
        numbers(&[0.0, 100.0]),
        numbers(&[0.0, 100.0]),
        opts,
        0.0,
        1.0,
    )
    .unwrap();
    let p = producer("lasso", view("map"), &["x", "y"])
        .with_pixel_grids([(pid("x"), g.clone()), (pid("y"), g)])
        .unwrap();
    // Half of it beyond the right edge of the plot.
    let ring = [[80.0, 10.0], [140.0, 10.0], [140.0, 30.0], [80.0, 30.0]];
    let v =
        SelectionValue::polygon(&p, &pid("x"), &pid("y"), &ring).expect("a lasso over the edge");
    let s = state(Resolution::Intersect).set(&p, v).unwrap();
    let rows = batch(vec![
        ("id", Arc::new(Int64Array::from(vec![0_i64, 1, 2]))),
        ("x", numbers(&[85.5, 99.5, 50.5])),
        ("y", numbers(&[20.5, 20.5, 20.5])),
    ]);
    assert_eq!(
        selected(rows, membership().predicate(&s).unwrap()).await,
        vec![0, 1]
    );
}

#[test]
fn a_huge_lasso_is_refused_not_expanded() {
    let g = PixelGrid::new(
        BuiltinScale::Linear,
        numbers(&[0.0, 100.0]),
        numbers(&[0.0, 100.0]),
        HashMap::new(),
        0.0,
        1.0,
    )
    .unwrap();
    let p = producer("lasso", view("map"), &["x", "y"])
        .with_pixel_grids([(pid("x"), g.clone()), (pid("y"), g)])
        .unwrap();
    let ring = [[0.0, 0.0], [10.0, 0.0], [10.0, 1e9], [0.0, 1e9]];
    // Refused before any row is built: the check comes first.
    assert!(SelectionValue::polygon(&p, &pid("x"), &pid("y"), &ring).is_err());
}

#[test]
fn partial_degrees_are_not_lost_in_the_log() {
    let p = producer("keys", view("lines"), &["line"]);
    let v = SelectionValue::tuple([(pid("line"), ValueTest::one_of([1_i64]))])
        .with_partial(pid("line"), [(ScalarValue::Int64(Some(2)), 0.5)]);
    // Without a gesture there is nothing to redraw them from.
    assert!(SelectionUpdate::set(&p, v).to_json().is_err());
}

#[tokio::test]
async fn cells_far_apart_are_found_by_search() {
    // Nine cells up to ten million apart: too sparse for the dense table.
    let g = PixelGrid::new(
        BuiltinScale::Linear,
        numbers(&[0.0, 100.0]),
        numbers(&[0.0, 100.0]),
        HashMap::new(),
        0.0,
        1.0,
    )
    .unwrap();
    let p = producer("cells", view("map"), &["x", "y"])
        .with_pixel_grids([(pid("x"), g.clone()), (pid("y"), g)])
        .unwrap();
    let cells: Vec<Vec<i64>> = (0..9).map(|k| vec![k * 1_250_000, 3]).collect();
    let s = state(Resolution::Intersect)
        .set(
            &p,
            SelectionValue::cells(&p, &[&pid("x"), &pid("y")], cells).unwrap(),
        )
        .unwrap();
    let rows = batch(vec![
        ("id", Arc::new(Int64Array::from(vec![0_i64, 1, 2, 3]))),
        ("x", numbers(&[0.5, 2_500_000.5, 2_500_001.5, 10_000_000.5])),
        ("y", numbers(&[3.5, 3.5, 3.5, 3.5])),
    ]);
    assert_eq!(
        selected(rows, membership().predicate(&s).unwrap()).await,
        vec![0, 1, 3]
    );
}
