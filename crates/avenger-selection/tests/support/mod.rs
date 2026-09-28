// Not upstream: helpers the tests of the additions share (VENDORED.md).
#![allow(dead_code)]
use avenger_scales_datafusion::BuiltinScale;
use avenger_selection::{PixelGrid, ProjectionId};
use datafusion::arrow::array::{ArrayRef, Float64Array};
use std::{collections::HashMap, sync::Arc};

pub fn numbers(values: &[f64]) -> ArrayRef {
    Arc::new(Float64Array::from(values.to_vec()))
}
pub fn pid(name: &str) -> ProjectionId {
    ProjectionId::new(name).unwrap()
}
pub fn linear(domain: [f64; 2], range: [f64; 2], size: f64) -> PixelGrid {
    PixelGrid::new(
        BuiltinScale::Linear,
        numbers(&domain),
        numbers(&range),
        HashMap::new(),
        0.0,
        size,
    )
    .unwrap()
}
/// Data 0..100 on 0..100 px: a data unit is a pixel.
pub fn grid(size: f64) -> PixelGrid {
    linear([0.0, 100.0], [0.0, 100.0], size)
}
/// A seeded generator of numbers in [0, 1), the same on every run.
pub fn uniform(seed: u64) -> impl FnMut() -> f64 {
    let mut s = seed;
    move || {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (s >> 11) as f64 / (1u64 << 53) as f64
    }
}
