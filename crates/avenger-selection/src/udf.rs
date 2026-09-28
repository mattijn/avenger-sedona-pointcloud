//! Not upstream (VENDORED.md): what the added DataFusion functions share.

use datafusion::{
    arrow::{
        array::{AsArray, PrimitiveArray},
        compute::cast,
        datatypes::ArrowPrimitiveType,
    },
    common::Result,
    logical_expr::ScalarFunctionArgs,
};

/// A function's arguments cast to one primitive type, one array per argument.
pub(crate) fn columns<T: ArrowPrimitiveType>(
    args: &ScalarFunctionArgs,
) -> Result<Vec<PrimitiveArray<T>>> {
    args.args
        .iter()
        .map(|a| {
            Ok(
                cast(&a.clone().into_array(args.number_rows)?, &T::DATA_TYPE)?
                    .as_primitive::<T>()
                    .clone(),
            )
        })
        .collect()
}
