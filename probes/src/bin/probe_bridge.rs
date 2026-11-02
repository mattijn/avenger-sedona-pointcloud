//! Hands Arrow 58 batches (SedonaDB, DataFusion 54, Avenger #130's DataFusion
//! crates) to Avenger `main`'s scales, which are on Arrow 59, through the Arrow
//! C Data Interface. Checks that no buffer is copied and that the scaled values
//! equal those of the same data built natively in Arrow 59, and times the
//! handover against the scaling itself. Needs no tile.
//!
//! Usage: cargo run --release -p lidar-probes --bin probe_bridge
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{Array as _, ArrayRef, Float64Array, RecordBatch, StructArray, UInt8Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow59::array::{Array as _, ArrayRef as ArrayRef59, AsArray as _};

/// One Arrow 58 array as an Arrow 59 array, sharing its buffers. The two
/// crates' FFI structs are the same C structs, so the exported pair is moved
/// across by pointer; Arrow 59 takes ownership and calls the release callback
/// that Arrow 58 put in it when the last reference goes.
pub fn to_arrow59(array: &ArrayRef) -> ArrayRef59 {
    let (mut ffi_array, mut ffi_schema) = arrow::ffi::to_ffi(&array.to_data()).expect("export");
    // SAFETY: both are #[repr(C)] structs of the Arrow C Data Interface;
    // `from_raw` moves the value out and leaves an empty (released) one behind.
    let (ffi_array, ffi_schema) = unsafe {
        (
            arrow59::ffi::FFI_ArrowArray::from_raw(&mut ffi_array as *mut _ as *mut _),
            arrow59::ffi::FFI_ArrowSchema::from_raw(&mut ffi_schema as *mut _ as *mut _),
        )
    };
    let data = unsafe { arrow59::ffi::from_ffi(ffi_array, &ffi_schema) }.expect("import");
    arrow59::array::make_array(data)
}

/// A whole batch, as one struct array, then split back into its columns.
pub fn batch_to_arrow59(batch: &RecordBatch) -> arrow59::array::RecordBatch {
    let as_struct: ArrayRef = Arc::new(StructArray::from(batch.clone()));
    let imported = to_arrow59(&as_struct);
    arrow59::array::RecordBatch::from(imported.as_struct().clone())
}

fn points(n: usize) -> RecordBatch {
    let x: Float64Array = (0..n).map(|i| 657_000.0 + (i % 1000) as f64).collect();
    let y: Float64Array = (0..n).map(|i| 6_867_000.0 + (i / 1000 % 1000) as f64).collect();
    let z: Float64Array = (0..n).map(|i| 35.0 + (i % 65) as f64 * 0.9).collect();
    let class: UInt8Array = (0..n).map(|i| (i % 7) as u8).collect();
    let schema = Schema::new(vec![
        Field::new("x", DataType::Float64, false),
        Field::new("y", DataType::Float64, false),
        Field::new("z", DataType::Float64, false),
        Field::new("classification", DataType::UInt8, false),
    ]);
    RecordBatch::try_new(Arc::new(schema), vec![Arc::new(x), Arc::new(y), Arc::new(z), Arc::new(class)]).unwrap()
}

fn best<T>(runs: usize, mut f: impl FnMut() -> T) -> (f64, T) {
    let mut best = f64::MAX;
    let mut out = None;
    for _ in 0..runs {
        let t = Instant::now();
        let v = f();
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
        out = Some(v);
    }
    (best, out.unwrap())
}

fn main() {
    use avenger_scales_main::scales::linear::LinearScale;
    let scale = LinearScale::configured((657_000.0, 658_000.0), (0.0, 900.0));

    println!("Arrow {} batches into Avenger main's scales (Arrow {}), best of 5", "58", "59");
    println!("{:>11}  {:>13}  {:>13}  {:>11}  {:>12}  {}", "points", "column handover", "batch handover", "scale x", "same buffers", "same values");
    for n in [100_000, 1_000_000, 4_000_000, 17_300_000] {
        let batch = points(n);
        let x58 = batch.column(0).clone();

        let (t_col, x59) = best(5, || to_arrow59(&x58));
        let (t_batch, b59) = best(5, || batch_to_arrow59(&batch));

        // Zero copy: the imported values point at the exported buffer.
        let p58 = x58.to_data().buffers()[0].as_ptr();
        let p59 = x59.to_data().buffers()[0].as_ptr();
        let pb = b59.column(0).to_data().buffers()[0].as_ptr();
        let same_buffers = p58 == p59 && p58 == pb;

        // The scale on the imported column, against the same data built in Arrow 59.
        let (t_scale, scaled) = best(5, || scale.scale_to_numeric(&x59).unwrap().as_vec(n, None));
        let native: ArrayRef59 = Arc::new(arrow59::array::Float64Array::from(
            x58.as_any().downcast_ref::<Float64Array>().unwrap().values().to_vec(),
        ));
        let expected = scale.scale_to_numeric(&native).unwrap().as_vec(n, None);
        let same_values = scaled == expected && b59.num_rows() == n && b59.num_columns() == 4;

        println!(
            "{:>11}  {:>12.4} ms  {:>11.4} ms  {:>8.2} ms  {:>12}  {}",
            n, t_col, t_batch, t_scale, same_buffers, same_values
        );
    }
}
