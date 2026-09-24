//! Phase B: the observation a decider sees. Schema and summary statistics
//! of the data, the chart's `describe` before and after, the difference, the
//! policy and, for typed input, the instruction. Never raw rows.

use datafusion::error::Result;
use lidar_pipeline::pipeline::Pipeline;
use serde_json::{json, Value};

/// How a column is used: decides which options make sense for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Numeric position in a projected or geographic system (x, y, cx, …).
    Coordinate,
    /// A geometry, as WKT text.
    Geometry,
    /// Few distinct values: a category.
    Category,
    /// A number to measure.
    Measure,
    Text,
}

#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub dtype: String,
    pub role: Role,
    pub distinct: u64,
    pub nulls: u64,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub p90: Option<f64>,
    pub p999: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Stats {
    pub rows: u64,
    pub columns: Vec<Column>,
}

impl Stats {
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }
    pub fn with_role(&self, role: Role) -> impl Iterator<Item = &Column> {
        self.columns.iter().filter(move |c| c.role == role)
    }
}

const COORDINATE_NAMES: [&str; 10] = [
    "x", "y", "cx", "cy", "lon", "lat", "easting", "northing", "gx", "gy",
];

fn f64_at(b: &arrow::array::RecordBatch, i: usize) -> Option<f64> {
    let a = arrow::compute::cast(b.column(i), &arrow::datatypes::DataType::Float64).ok()?;
    let a = arrow::array::AsArray::as_primitive::<arrow::datatypes::Float64Type>(&a);
    (!arrow::array::Array::is_null(a, 0)).then(|| a.value(0))
}

/// Statistics of the pipeline's current data. One query, executed.
pub async fn stats(p: &Pipeline) -> Result<Stats> {
    let schema = p.plan()?.schema().clone();
    let fields: Vec<(String, arrow::datatypes::DataType)> = schema
        .fields()
        .iter()
        .map(|f| (f.name().clone(), f.data_type().clone()))
        .collect();
    let mut select = vec!["count(*) AS rows".to_string()];
    for (name, t) in &fields {
        let c = format!("\"{name}\"");
        select.push(format!("count(DISTINCT {c})"));
        select.push(format!("count(*) - count({c})"));
        if t.is_numeric() {
            select.push(format!("min({c})"));
            select.push(format!("max({c})"));
            // Exact percentiles: an approximate one varies with batch order,
            // which makes the observation, and its cache key, irreproducible.
            select.push(format!("percentile_cont(0.9) WITHIN GROUP (ORDER BY {c})"));
            select.push(format!(
                "percentile_cont(0.999) WITHIN GROUP (ORDER BY {c})"
            ));
        } else {
            // A text sample, only to recognise WKT geometry; never sent.
            select.push(format!("min(CAST({c} AS VARCHAR))"));
        }
    }
    p.expose_input()?;
    let b = p
        .ctx
        .sql(&format!("SELECT {} FROM input", select.join(", ")))
        .await?
        .collect()
        .await?;
    let b = &b[0];
    let rows = f64_at(b, 0).unwrap_or(0.0) as u64;
    let mut i = 1;
    let mut columns = vec![];
    for (name, t) in fields {
        let distinct = f64_at(b, i).unwrap_or(0.0) as u64;
        let nulls = f64_at(b, i + 1).unwrap_or(0.0) as u64;
        i += 2;
        let (mut min, mut max, mut p90, mut p999, mut sample) = (None, None, None, None, None);
        if t.is_numeric() {
            (min, max, p90, p999) = (
                f64_at(b, i),
                f64_at(b, i + 1),
                f64_at(b, i + 2),
                f64_at(b, i + 3),
            );
            i += 4;
        } else {
            let a = arrow::compute::cast(b.column(i), &arrow::datatypes::DataType::Utf8)?;
            let a = arrow::array::AsArray::as_string::<i32>(&a);
            if !arrow::array::Array::is_null(a, 0) {
                sample = Some(a.value(0).to_string());
            }
            i += 1;
        }
        let lower = name.to_lowercase();
        let role = if t.is_numeric() && COORDINATE_NAMES.contains(&lower.as_str()) {
            Role::Coordinate
        } else if sample.as_deref().is_some_and(|s| {
            s.starts_with("POINT") || s.starts_with("POLYGON") || s.starts_with("LINESTRING")
        }) {
            Role::Geometry
        } else if !t.is_numeric() && distinct <= 50 {
            Role::Category
        } else if t.is_numeric() {
            Role::Measure
        } else {
            Role::Text
        };
        columns.push(Column {
            name,
            dtype: t.to_string(),
            role,
            distinct,
            nulls,
            min,
            max,
            p90,
            p999,
        });
    }
    Ok(Stats { rows, columns })
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::Coordinate => "coordinate",
        Role::Geometry => "geometry (WKT)",
        Role::Category => "category",
        Role::Measure => "measure",
        Role::Text => "text",
    }
}

fn round(v: f64) -> f64 {
    if v.abs() >= 100.0 {
        v.round()
    } else {
        (v * 100.0).round() / 100.0
    }
}

/// The data as the decider sees it: one line per column.
pub fn describe_data(s: &Stats) -> String {
    let mut out = format!("{} rows. Columns:\n", s.rows);
    for c in &s.columns {
        out += &format!(
            "- {} ({}, {}): {} distinct",
            c.name,
            c.dtype,
            role_name(c.role),
            c.distinct
        );
        if let (Some(lo), Some(hi)) = (c.min, c.max) {
            out += &format!(", range {}..{}", round(lo), round(hi));
        }
        if c.nulls > 0 {
            out += &format!(", {} missing", c.nulls);
        }
        out += "\n";
    }
    out
}

/// What changed between two data states, in words.
pub fn diff_data(before: &Stats, after: &Stats) -> String {
    let mut out = vec![];
    if before.rows != after.rows {
        let pct = 100.0 * (after.rows as f64 - before.rows as f64) / before.rows.max(1) as f64;
        out.push(format!(
            "rows {} -> {} ({pct:+.0} %)",
            before.rows, after.rows
        ));
    }
    for c in &after.columns {
        match before.column(&c.name) {
            None => out.push(format!("new column {} ({})", c.name, role_name(c.role))),
            Some(b) => {
                if c.role == Role::Category && c.distinct > b.distinct {
                    out.push(format!(
                        "{}: {} new categories",
                        c.name,
                        c.distinct - b.distinct
                    ));
                }
                // An outlier: a measure whose maximum lies far beyond its own
                // 99.9th percentile, and did not before. Growth of the data
                // (more rows, a wider extent) is not an outlier.
                let far = |c: &crate::observe::Column| match (c.max, c.p999, c.min) {
                    (Some(max), Some(p), Some(min)) => max - p > 0.5 * (p - min).abs().max(1e-9),
                    _ => false,
                };
                if c.role == Role::Measure && far(c) && !far(b) {
                    out.push(format!(
                        "{}: an extreme value {} appeared, far above the 99.9th percentile {}",
                        c.name,
                        round(c.max.unwrap()),
                        round(c.p999.unwrap())
                    ));
                }
            }
        }
    }
    for c in &before.columns {
        if after.column(&c.name).is_none() {
            out.push(format!("column {} removed", c.name));
        }
    }
    if out.is_empty() {
        "no change in schema or statistics".into()
    } else {
        out.join("; ")
    }
}

/// The observation document handed to a decider (strings only, as Jev's
/// `state` expects).
pub fn observation(
    policy: &str,
    chart: &Value,
    data: &Stats,
    change: Option<&str>,
    instruction: Option<&str>,
) -> Value {
    let mut o = json!({
        "policy": policy,
        "chart": chart.to_string(),
        "data": describe_data(data),
    });
    if let Some(c) = change {
        o["data_change"] = json!(c);
    }
    if let Some(i) = instruction {
        o["instruction"] = json!(i);
    }
    o
}
