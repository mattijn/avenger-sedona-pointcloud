//! SedonaDB's native spatial functions (`st_point`, `st_astext`, …) as a
//! function package. Only `sedona-functions`: `sedona-geo` cannot be linked
//! next to Avenger's `geo` 0.29 in this workspace (see the README).

use datafusion::logical_expr::ScalarUDF;

use crate::pipeline::Package;

pub fn package() -> Package {
    let set = sedona_functions::register::default_function_set();
    Package {
        name: "sedona",
        functions: set
            .scalar_udfs()
            .map(|u| ScalarUDF::new_from_impl(u.clone()))
            .collect(),
        steps: vec![],
    }
}
