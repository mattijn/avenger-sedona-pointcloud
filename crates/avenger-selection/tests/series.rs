// Not upstream: tests for `SeriesTest` (VENDORED.md).
mod common;

use avenger_selection::*;
use common::*;
use datafusion::{
    arrow::array::{Float64Array, Int64Array, RecordBatch},
    common::ScalarValue,
    logical_expr::col,
    prelude::SessionContext,
};
use std::sync::Arc;

fn series(rows: &[(i64, f64, f64)]) -> RecordBatch {
    batch(vec![
        ("id", Arc::new(Int64Array::from_iter_values(0..rows.len() as i64))),
        ("line", Arc::new(Int64Array::from(rows.iter().map(|r| r.0).collect::<Vec<_>>()))),
        ("t", Arc::new(Float64Array::from(rows.iter().map(|r| r.1).collect::<Vec<_>>()))),
        ("n", Arc::new(Float64Array::from(rows.iter().map(|r| r.2).collect::<Vec<_>>()))),
    ])
}
async fn keys(test: &SeriesTest, rows: RecordBatch) -> Vec<i64> {
    let df = SessionContext::new().read_batch(rows).unwrap();
    test.keys(df, col("line"), col("t"), col("n"))
        .await
        .unwrap()
        .into_iter()
        .map(|k| match k {
            ScalarValue::Int64(Some(v)) => v,
            other => panic!("{other:?}"),
        })
        .collect()
}
// Line 1 is a tent, given out of order; line 2 flat and high; line 3 a short
// flat piece ending on the brush; line 4 a single vertex.
const ROWS: [(i64, f64, f64); 8] = [(1, 4.0, 0.0), (1, 0.0, 0.0), (1, 2.0, 2.0), (2, 0.0, 5.0), (2, 4.0, 5.0), (3, 0.0, 1.0), (3, 1.0, 1.0), (4, 1.0, 1.0)];

#[tokio::test]
async fn a_line_brush_takes_the_series_that_cross_it() {
    let brush = SeriesTest::Crosses { from: [1.0, -1.0], to: [1.0, 3.0] };
    // Ordered by x, line 1 crosses at t = 1; out of row order it would pair
    // (4, 0) with (0, 0) and miss nothing either, so a second brush checks
    // the order: it meets only the segment (0, 0)–(4, 0) that sorting removes.
    assert_eq!(keys(&brush, series(&ROWS)).await, vec![1, 3]);
    let low = SeriesTest::Crosses { from: [3.0, -0.5], to: [3.5, -0.1] };
    assert_eq!(keys(&low, series(&ROWS)).await, Vec::<i64>::new());
}

#[tokio::test]
async fn a_timebox_takes_the_series_that_stay_inside_over_its_x_range() {
    let tb = SeriesTest::Within { x: (1.0, 3.0), y: (0.0, 3.0) };
    // Line 2 has no vertex in the x-range, so it does not qualify.
    assert_eq!(keys(&tb, series(&ROWS)).await, vec![1, 3, 4]);
    let tight = SeriesTest::Within { x: (1.0, 3.0), y: (1.5, 3.0) };
    assert_eq!(keys(&tight, series(&ROWS)).await, vec![1]);
}

#[tokio::test]
async fn keys_select_rows_elsewhere_and_match_experiment_7() {
    // Random walks, tested here and by experiment 7's own loop.
    let mut s = 99_u64;
    let mut next = || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut rows = Vec::new();
    for line in 0..300_i64 {
        let mut y = 50.0 + 30.0 * next();
        for t in 0..40 {
            y += 8.0 * (next() - 0.5);
            rows.push((line, t as f64 + 0.5 * next(), y));
        }
    }
    let crossing = |p1: [f64; 2], p2: [f64; 2], q1: [f64; 2], q2: [f64; 2]| {
        let o = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        o(q1, q2, p1) * o(q1, q2, p2) <= 0.0 && o(p1, p2, q1) * o(p1, p2, q2) <= 0.0
    };
    let by_line = |l: i64| {
        let mut pts: Vec<[f64; 2]> = rows.iter().filter(|r| r.0 == l).map(|r| [r.1, r.2]).collect();
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        pts
    };
    let (a, b) = ([12.0, 90.0], [20.0, 40.0]);
    let want_seg: Vec<i64> = (0..300).filter(|l| by_line(*l).windows(2).any(|w| crossing(w[0], w[1], a, b))).collect();
    let (x, y) = ((14.0, 18.0), (40.0, 75.0));
    let want_tb: Vec<i64> = (0..300)
        .filter(|l| {
            let within: Vec<[f64; 2]> = by_line(*l).into_iter().filter(|p| p[0] >= x.0 && p[0] <= x.1).collect();
            !within.is_empty() && within.iter().all(|p| p[1] >= y.0 && p[1] <= y.1)
        })
        .collect();
    let seg = keys(&SeriesTest::Crosses { from: a, to: b }, series(&rows)).await;
    let tb = keys(&SeriesTest::Within { x, y }, series(&rows)).await;
    assert!(!want_seg.is_empty() && want_seg.len() < 300 && !want_tb.is_empty() && want_tb.len() < 300);
    assert_eq!(seg, want_seg);
    assert_eq!(tb, want_tb);

    // The keys as a selection, applied to another relation that shares them.
    let key = ProjectionId::new("line").unwrap();
    let p = producer("brush", view("lines"), &["line"]);
    let st = state(Resolution::Intersect).set(&p, SeriesTest::Crosses { from: a, to: b }.value(&key, seg.iter().map(|k| ScalarValue::Int64(Some(*k))).collect())).unwrap();
    let raw = batch(vec![
        ("id", Arc::new(Int64Array::from_iter_values(0..300))),
        ("line", Arc::new(Int64Array::from_iter_values(0..300))),
    ]);
    assert_eq!(selected(raw, cross(view("points")).predicate(&st).unwrap()).await, seg);
}

#[tokio::test]
async fn soft_series_follow_experiment_7() {
    // Line 1 crosses the brush; line 2 passes 1 unit above its top end;
    // line 5 passes 3 units away; line 2's nearest vertex decides.
    let rows = [(1, 0.0, 0.0), (1, 2.0, 2.0), (2, 0.0, 4.0), (2, 2.0, 4.0), (5, 0.0, 6.0), (5, 2.0, 6.0)];
    let brush = SeriesTest::Crosses { from: [1.0, -1.0], to: [1.0, 3.0] };
    let df = SessionContext::new().read_batch(series(&rows)).unwrap();
    let d = brush.degrees(df, col("line"), col("t"), col("n"), [1.0, 1.0], 2.0).await.unwrap();
    // Line 2: nearest vertex (0, 4) or (2, 4) is sqrt(1 + 1) from (1, 3).
    let want = 1.0 - 2f64.sqrt() / 2.0;
    assert_eq!(d.len(), 2);
    assert_eq!(d[0], (ScalarValue::Int64(Some(1)), 1.0));
    assert_eq!(d[1].0, ScalarValue::Int64(Some(2)));
    assert!((d[1].1 - want).abs() < 1e-12);
    // A timebox gives the share inside.
    let tb = SeriesTest::Within { x: (0.0, 2.0), y: (0.0, 5.0) };
    let df = SessionContext::new().read_batch(series(&rows)).unwrap();
    let d = tb.degrees(df, col("line"), col("t"), col("n"), [1.0, 1.0], 1.0).await.unwrap();
    assert_eq!(d, vec![(ScalarValue::Int64(Some(1)), 1.0), (ScalarValue::Int64(Some(2)), 1.0)]);
    let tb = SeriesTest::Within { x: (0.0, 2.0), y: (0.0, 1.0) };
    let df = SessionContext::new().read_batch(series(&rows)).unwrap();
    let d = tb.degrees(df, col("line"), col("t"), col("n"), [1.0, 1.0], 1.0).await.unwrap();
    assert_eq!(d, vec![(ScalarValue::Int64(Some(1)), 0.5)]);

    // As a value: the predicate takes line 1, the degree line 2 in part,
    // on rows of another relation keyed by line.
    let key = ProjectionId::new("line").unwrap();
    let p = producer("brush", view("lines"), &["line"]);
    let df = SessionContext::new().read_batch(series(&rows)).unwrap();
    let d = brush.degrees(df, col("line"), col("t"), col("n"), [1.0, 1.0], 2.0).await.unwrap();
    let st = state(Resolution::Intersect).set(&p, brush.soft_value(&key, d, [1.0, 1.0], 2.0)).unwrap();
    let raw = batch(vec![("id", Arc::new(Int64Array::from(vec![0_i64, 1, 2]))), ("line", Arc::new(Int64Array::from(vec![1_i64, 2, 5])))]);
    let f = cross(view("points"));
    let out = SessionContext::new().read_batch(raw.clone()).unwrap()
        .select(vec![f.degree(&st, 1.0).unwrap().alias("d")]).unwrap().collect().await.unwrap();
    let got = out[0].column(0).as_any().downcast_ref::<Float64Array>().unwrap().values().to_vec();
    assert_eq!(got[0], 1.0);
    assert!((got[1] - want).abs() < 1e-12);
    assert_eq!(got[2], 0.0);
    assert_eq!(selected(raw, f.predicate(&st).unwrap()).await, vec![0]);
    let g = st.contributions(&id()).unwrap().next().unwrap().value().gesture().unwrap().clone();
    assert_eq!((g.param("soft"), SeriesTest::from_gesture(&g)), (Some(2.0), Some(brush)));
}
