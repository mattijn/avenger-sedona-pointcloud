// Not upstream: tests for gestures, the selection log and `cells` (VENDORED.md).
mod common;

use avenger_scales_datafusion::BuiltinScale;
use avenger_selection::*;
use common::*;
use datafusion::{
    arrow::array::{ArrayRef, Float64Array, Int64Array},
    arrow::datatypes::TimeUnit,
    common::ScalarValue,
    logical_expr::col,
};
use std::{collections::HashMap, ops::Bound, sync::Arc};

fn numbers(values: &[f64]) -> ArrayRef {
    Arc::new(Float64Array::from(values.to_vec()))
}
fn grid(size: f64) -> PixelGrid {
    PixelGrid::new(BuiltinScale::Linear, numbers(&[0.0, 100.0]), numbers(&[0.0, 100.0]), HashMap::new(), 0.0, size).unwrap()
}
fn pid(name: &str) -> ProjectionId {
    ProjectionId::new(name).unwrap()
}
fn map_producer(name: &str) -> ProducerDefinition {
    producer(name, view("map"), &["x", "y"]).with_pixel_grids([(pid("x"), grid(1.0)), (pid("y"), grid(1.0))]).unwrap()
}
const RING: [[f64; 2]; 5] = [[10.0, 10.0], [90.0, 20.0], [60.0, 50.0], [85.0, 90.0], [15.0, 70.0]];

#[test]
fn a_gesture_survives_set_and_leaves_with_toggle() {
    let lasso = map_producer("lasso");
    let value = SelectionValue::polygon(&lasso, &pid("x"), &pid("y"), &RING).unwrap();
    assert_eq!(value.gesture().unwrap().kind(), "polygon");
    let value = value.clone().with_gesture(value.gesture().unwrap().clone().with_param("structure", 0.3));
    let s = state(Resolution::Intersect).set(&lasso, value).unwrap();
    let c = s.contributions(&id()).unwrap().next().unwrap();
    let g = c.value().gesture().unwrap();
    assert_eq!((g.points(), g.param("structure")), (&RING[..], Some(0.3)));
    // Toggling a tuple changes what the outline stood for: the gesture goes.
    let keys = producer("keys", view("legend"), &["class"]);
    let s = state(Resolution::Intersect)
        .set(&keys, values("class", [2_i64.into(), 6_i64.into()]).with_gesture(Gesture::new("legend", [])))
        .unwrap()
        .toggle(&keys, values("class", [3_i64.into()]))
        .unwrap();
    assert!(s.contributions(&id()).unwrap().next().unwrap().value().gesture().is_none());
}

#[tokio::test]
async fn a_log_replays_to_the_same_predicates() {
    let other = SelectionId::new("other").unwrap();
    let lasso = map_producer("lasso");
    let brush = map_producer("brush");
    let keys = producer("keys", view("legend"), &["class", "name"]);
    let when = ProducerDefinition::new(other.clone(), ProducerId::new("time").unwrap(), view("clock"),
        [Projection::new(pid("t"), col("t")).unwrap()]).unwrap();
    let defs = [lasso.clone(), brush.clone(), keys.clone(), when.clone()];
    let ts = |v: i64| ScalarValue::TimestampMillisecond(Some(v), Some("UTC".into()));
    let updates = vec![
        SelectionUpdate::set(&lasso, SelectionValue::polygon(&lasso, &pid("x"), &pid("y"), &RING).unwrap()),
        SelectionUpdate::set(&keys, SelectionValue::tuples([
            [(pid("class"), ValueTest::equal(6_i64)), (pid("name"), ValueTest::one_of(["Building", "Bâtiment"]))],
            [(pid("class"), ValueTest::equal(f64::NAN)), (pid("name"), ValueTest::equal(ScalarValue::Utf8(None)))],
        ]).with_gesture(Gesture::new("legend", [[1.5, -2.25]]).with_param("soft", 0.1))),
        SelectionUpdate::toggle(&keys, SelectionValue::tuple([(pid("class"), ValueTest::equal(2_i64)), (pid("name"), ValueTest::equal("Ground"))])),
        SelectionUpdate::set(&brush, SelectionValue::tuple([
            (pid("x"), ValueTest::Range { lower: Bound::Excluded(ScalarValue::Float64(Some(0.1 + 0.2))), upper: Bound::Unbounded }),
            (pid("y"), ValueTest::range(20.0..40.0)),
        ])),
        SelectionUpdate::set(&when, SelectionValue::tuple([(pid("t"), ValueTest::range(ts(1_700_000_000_123)..ts(1_700_000_060_000)))])),
        SelectionUpdate::clear(&brush),
        SelectionUpdate::clear_all(&other),
        SelectionUpdate::set(&when, SelectionValue::tuple([(pid("t"), ValueTest::range(ts(-5)..=ts(5)))])),
    ];
    let start = SelectionSet::new([(id(), Resolution::Intersect), (other.clone(), Resolution::Union)]).unwrap();
    let lines: Vec<String> = updates.iter().map(|u| serde_json::to_string(&u.to_json().unwrap()).unwrap()).collect();
    let producers = Producers::new(defs);
    let replayed: Vec<SelectionUpdate> = lines
        .iter()
        .map(|l| SelectionUpdate::from_json(&serde_json::from_str(l).unwrap(), &producers).unwrap())
        .collect();
    let (mut a, mut b) = (start.clone(), start);
    for (u, r) in updates.into_iter().zip(replayed) {
        a = a.apply(u).unwrap();
        b = b.apply(r).unwrap();
        for sel in [id(), other.clone()] {
            let f = filter(view("points"), SelectionFilter::membership(&sel, EmptySelection::MatchNone));
            assert_eq!(format!("{}", f.predicate(&a).unwrap()), format!("{}", f.predicate(&b).unwrap()));
            let (ca, cb): (Vec<_>, Vec<_>) = (a.contributions(&sel).unwrap().collect(), b.contributions(&sel).unwrap().collect());
            assert_eq!(ca, cb, "contributions, gestures included, after {}", lines.len());
        }
    }
    // A timestamp keeps its unit and zone; a replayed log names what it misses.
    assert!(lines.iter().any(|l| l.contains("Timestamp") && l.contains("UTC")));
    let _ = TimeUnit::Millisecond;
    let err = SelectionUpdate::from_json(&serde_json::from_str(&lines[0]).unwrap(), &Producers::default()).unwrap_err();
    assert!(err.to_string().contains("producer lasso of view map"), "{err}");
}

#[tokio::test]
async fn cells_select_exactly_their_rows() {
    let (gx, gy) = (grid(4.0), grid(4.0));
    let p = producer("voxels", view("map"), &["x", "y"]).with_pixel_grids([(pid("x"), gx.clone()), (pid("y"), gy.clone())]).unwrap();
    let cells = vec![vec![1, 2], vec![3, 4], vec![3, 5]];
    let v = SelectionValue::cells(&p, &[&pid("x"), &pid("y")], cells.clone()).unwrap();
    let s = state(Resolution::Intersect).set(&p, v).unwrap();
    let pts: Vec<(f64, f64)> = (0..25).flat_map(|i| (0..25).map(move |j| (i as f64 * 4.0 + 1.3, j as f64 * 4.0 + 2.1))).collect();
    let rows = batch(vec![
        ("id", Arc::new(Int64Array::from_iter_values(0..pts.len() as i64))),
        ("x", numbers(&pts.iter().map(|p| p.0).collect::<Vec<_>>())),
        ("y", numbers(&pts.iter().map(|p| p.1).collect::<Vec<_>>())),
    ]);
    let want: Vec<i64> = pts
        .iter()
        .enumerate()
        .filter(|(_, p)| cells.contains(&vec![(p.0 / 4.0).floor() as i64, (p.1 / 4.0).floor() as i64]))
        .map(|(i, _)| i as i64)
        .collect();
    assert_eq!(want.len(), 3);
    assert_eq!(selected(rows, membership().predicate(&s).unwrap()).await, want);
}
