// Not upstream: tests for `SelectionValue::polygon` (VENDORED.md).
mod common;

use avenger_scales_datafusion::BuiltinScale;
use avenger_selection::*;
use common::*;
use datafusion::{
    arrow::array::{ArrayRef, Float64Array, Int64Array, TimestampMillisecondArray},
    common::ScalarValue,
};
use std::{collections::HashMap, sync::Arc};

fn numbers(values: &[f64]) -> ArrayRef {
    Arc::new(Float64Array::from(values.to_vec()))
}
fn linear(domain: [f64; 2], range: [f64; 2], size: f64) -> PixelGrid {
    PixelGrid::new(BuiltinScale::Linear, numbers(&domain), numbers(&range), HashMap::new(), 0.0, size).unwrap()
}
fn in_ring(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let mut inside = false;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + ring.len() - 1) % ring.len()]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}
// A lasso that doubles back on itself: concave, with a notch from the top.
const RING: [[f64; 2]; 7] = [[20.0, 15.0], [180.0, 22.0], [170.0, 180.0], [110.0, 175.0], [100.0, 60.0], [85.0, 170.0], [30.0, 150.0]];

/// Scattered points in data units, a map in metres whose screen y runs down.
fn scatter() -> (Vec<f64>, Vec<f64>) {
    let mut s = 12345_u64;
    let mut next = || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..4000).map(|_| (1000.0 + 500.0 * next(), 2000.0 + 400.0 * next())).unzip()
}

#[tokio::test]
async fn selects_exactly_the_cells_whose_centre_lies_inside() {
    for size in [1.0, 2.5] {
        let (gx, gy) = (linear([1000.0, 1500.0], [0.0, 200.0], size), linear([2000.0, 2400.0], [200.0, 0.0], size));
        let (u, v) = (ProjectionId::new("x").unwrap(), ProjectionId::new("y").unwrap());
        let lasso = producer("lasso", view("map"), &["x", "y"]).with_pixel_grids([(u.clone(), gx.clone()), (v.clone(), gy.clone())]).unwrap();
        let value = SelectionValue::polygon((&u, &gx), (&v, &gy), &RING).unwrap();
        let n_tuples = value.as_tuples().len();
        let s = state(Resolution::Intersect).set(&lasso, value).unwrap();
        let (xs, ys) = scatter();
        let rows = batch(vec![
            ("id", Arc::new(Int64Array::from_iter_values(0..xs.len() as i64))),
            ("x", numbers(&xs)),
            ("y", numbers(&ys)),
        ]);
        let got = selected(rows, membership().predicate(&s).unwrap()).await;
        // The reference: each point's cell, through the grid's own kernel,
        // then that cell's centre tested against the ring.
        let centre = |g: &PixelGrid, x: f64| (g.cell(&ScalarValue::Float64(Some(x))).unwrap().unwrap() as f64 + 0.5) * size;
        let want: Vec<i64> = (0..xs.len())
            .filter(|&i| in_ring([centre(&gx, xs[i]), centre(&gy, ys[i])], &RING))
            .map(|i| i as i64)
            .collect();
        assert!(want.len() > 500, "the ring should take a good share: {}", want.len());
        assert_eq!(got, want, "cell size {size}, {n_tuples} tuples");
    }
}

#[tokio::test]
async fn a_ring_off_the_grid_selects_nothing_and_stays_active() {
    let g = linear([0.0, 100.0], [0.0, 100.0], 1.0);
    let (u, v) = (ProjectionId::new("x").unwrap(), ProjectionId::new("y").unwrap());
    // Thinner than a cell: no centre falls inside.
    let value = SelectionValue::polygon((&u, &g), (&v, &g), &[[10.2, 10.2], [10.4, 10.2], [10.3, 10.4]]).unwrap();
    assert!(value.as_tuples().is_empty());
    let lasso = producer("lasso", view("map"), &["x", "y"]).with_pixel_grids([(u, g.clone()), (v, g)]).unwrap();
    let s = state(Resolution::Intersect).set(&lasso, value).unwrap();
    let rows = batch(vec![("id", Arc::new(Int64Array::from(vec![0_i64]))), ("x", numbers(&[10.3])), ("y", numbers(&[10.3]))]);
    assert!(selected(rows, membership().predicate(&s).unwrap()).await.is_empty());
}

#[test]
fn rejects_what_it_cannot_invert() {
    let g = linear([0.0, 100.0], [0.0, 100.0], 1.0);
    let (u, v) = (ProjectionId::new("x").unwrap(), ProjectionId::new("y").unwrap());
    assert!(SelectionValue::polygon((&u, &g), (&v, &g), &[[0.0, 0.0], [1.0, 1.0]]).is_err());
    let t = PixelGrid::new(
        BuiltinScale::Time,
        Arc::new(TimestampMillisecondArray::from(vec![0_i64, 1000])),
        numbers(&[0.0, 100.0]),
        HashMap::new(),
        0.0,
        1.0,
    )
    .unwrap();
    assert!(SelectionValue::polygon((&u, &t), (&v, &g), &RING).is_err());
}
