//! Print the JSON Schema of the supported subset:
//! `cargo run -p avenger-vegalite-spec --features schema --example json_schema`
fn main() {
    let schema = avenger_vegalite_spec::schema::unit_spec_schema();
    println!("{}", serde_json::to_string_pretty(&schema).unwrap());
}
