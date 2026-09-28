// Not upstream: tests for `SelectionValue::polygon` (VENDORED.md).
mod common;
mod support;

use avenger_scales_datafusion::BuiltinScale;
use avenger_selection::*;
use common::*;
use datafusion::{
    arrow::array::{Int64Array, TimestampMillisecondArray},
    common::ScalarValue,
};
use std::{collections::HashMap, sync::Arc};
use support::*;

fn in_ring(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let mut inside = false;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + ring.len() - 1) % ring.len()]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    inside
}
// A lasso that doubles back on itself: concave, with a notch from the top.
const RING: [[f64; 2]; 7] = [
    [20.0, 15.0],
    [180.0, 22.0],
    [170.0, 180.0],
    [110.0, 175.0],
    [100.0, 60.0],
    [85.0, 170.0],
    [30.0, 150.0],
];

/// Scattered points in data units, a map in metres whose screen y runs down.
fn scatter() -> (Vec<f64>, Vec<f64>) {
    let mut next = uniform(12345);
    (0..4000)
        .map(|_| (1000.0 + 500.0 * next(), 2000.0 + 400.0 * next()))
        .unzip()
}

#[tokio::test]
async fn selects_exactly_the_cells_whose_centre_lies_inside() {
    for size in [1.0, 2.5] {
        let (gx, gy) = (
            linear([1000.0, 1500.0], [0.0, 200.0], size),
            linear([2000.0, 2400.0], [200.0, 0.0], size),
        );
        let (u, v) = (
            ProjectionId::new("x").unwrap(),
            ProjectionId::new("y").unwrap(),
        );
        let lasso = producer("lasso", view("map"), &["x", "y"])
            .with_pixel_grids([(u.clone(), gx.clone()), (v.clone(), gy.clone())])
            .unwrap();
        let value = SelectionValue::polygon(&lasso, &u, &v, &RING).unwrap();
        let n_tuples = value.as_tuples().len();
        let s = state(Resolution::Intersect).set(&lasso, value).unwrap();
        let (xs, ys) = scatter();
        let rows = batch(vec![
            (
                "id",
                Arc::new(Int64Array::from_iter_values(0..xs.len() as i64)),
            ),
            ("x", numbers(&xs)),
            ("y", numbers(&ys)),
        ]);
        let got = selected(rows, membership().predicate(&s).unwrap()).await;
        // The reference: each point's cell, through the grid's own kernel,
        // then that cell's centre tested against the ring.
        let centre = |g: &PixelGrid, x: f64| {
            (g.cell(&ScalarValue::Float64(Some(x))).unwrap().unwrap() as f64 + 0.5) * size
        };
        let want: Vec<i64> = (0..xs.len())
            .filter(|&i| in_ring([centre(&gx, xs[i]), centre(&gy, ys[i])], &RING))
            .map(|i| i as i64)
            .collect();
        assert!(
            want.len() > 500,
            "the ring should take a good share: {}",
            want.len()
        );
        assert_eq!(got, want, "cell size {size}, {n_tuples} tuples");
    }
}

#[tokio::test]
async fn a_ring_off_the_grid_selects_nothing_and_stays_active() {
    let g = linear([0.0, 100.0], [0.0, 100.0], 1.0);
    let (u, v) = (
        ProjectionId::new("x").unwrap(),
        ProjectionId::new("y").unwrap(),
    );
    // Thinner than a cell: no centre falls inside.
    let lasso = producer("lasso", view("map"), &["x", "y"])
        .with_pixel_grids([(u.clone(), g.clone()), (v.clone(), g)])
        .unwrap();
    let value =
        SelectionValue::polygon(&lasso, &u, &v, &[[10.2, 10.2], [10.4, 10.2], [10.3, 10.4]])
            .unwrap();
    assert!(value.as_tuples().is_empty());
    let s = state(Resolution::Intersect).set(&lasso, value).unwrap();
    let rows = batch(vec![
        ("id", Arc::new(Int64Array::from(vec![0_i64]))),
        ("x", numbers(&[10.3])),
        ("y", numbers(&[10.3])),
    ]);
    assert!(selected(rows, membership().predicate(&s).unwrap())
        .await
        .is_empty());
}

#[test]
fn rejects_what_it_cannot_invert() {
    let g = linear([0.0, 100.0], [0.0, 100.0], 1.0);
    let (u, v) = (
        ProjectionId::new("x").unwrap(),
        ProjectionId::new("y").unwrap(),
    );
    let ok = producer("lasso", view("map"), &["x", "y"])
        .with_pixel_grids([(u.clone(), g.clone()), (v.clone(), g.clone())])
        .unwrap();
    assert!(SelectionValue::polygon(&ok, &u, &v, &[[0.0, 0.0], [1.0, 1.0]]).is_err());
    // A projection without a grid in the definition.
    let ungridded = producer("lasso", view("map"), &["x", "y"]);
    assert!(SelectionValue::polygon(&ungridded, &u, &v, &RING).is_err());
    let t = PixelGrid::new(
        BuiltinScale::Time,
        Arc::new(TimestampMillisecondArray::from(vec![0_i64, 1000])),
        numbers(&[0.0, 100.0]),
        HashMap::new(),
        0.0,
        1.0,
    )
    .unwrap();
    let timed = producer("lasso", view("map"), &["x", "y"])
        .with_pixel_grids([(u.clone(), t), (v.clone(), g)])
        .unwrap();
    assert!(SelectionValue::polygon(&timed, &u, &v, &RING).is_err());
}

#[tokio::test]
async fn cells_select_exactly_their_rows() {
    let (gx, gy) = (
        linear([0.0, 100.0], [0.0, 100.0], 4.0),
        linear([0.0, 100.0], [0.0, 100.0], 4.0),
    );
    let p = producer("voxels", view("map"), &["x", "y"])
        .with_pixel_grids([(pid("x"), gx.clone()), (pid("y"), gy.clone())])
        .unwrap();
    let cells = vec![vec![1, 2], vec![3, 4], vec![3, 5]];
    let v = SelectionValue::cells(&p, &[&pid("x"), &pid("y")], cells.clone()).unwrap();
    let s = state(Resolution::Intersect).set(&p, v).unwrap();
    let pts: Vec<(f64, f64)> = (0..25)
        .flat_map(|i| (0..25).map(move |j| (i as f64 * 4.0 + 1.3, j as f64 * 4.0 + 2.1)))
        .collect();
    let rows = batch(vec![
        (
            "id",
            Arc::new(Int64Array::from_iter_values(0..pts.len() as i64)),
        ),
        ("x", numbers(&pts.iter().map(|p| p.0).collect::<Vec<_>>())),
        ("y", numbers(&pts.iter().map(|p| p.1).collect::<Vec<_>>())),
    ]);
    let want: Vec<i64> = pts
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            cells.contains(&vec![
                (p.0 / 4.0).floor() as i64,
                (p.1 / 4.0).floor() as i64,
            ])
        })
        .map(|(i, _)| i as i64)
        .collect();
    assert_eq!(want.len(), 3);
    assert_eq!(
        selected(rows, membership().predicate(&s).unwrap()).await,
        want
    );
}
