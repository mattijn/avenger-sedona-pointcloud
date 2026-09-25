//! What data there is, before any chart: the LiDAR tile and the four tables
//! derived from it, registered by name in the session, so DataFusion's
//! `information_schema` lists them (`SHOW TABLES`, `information_schema.columns`)
//! and SQL can name them (`SELECT * FROM cells LIMIT 5`).
//!
//! `overview` puts that in one table: per table its columns, types, row count
//! and, per column, the minimum and maximum (distinct values for text).

use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::error::Result;
use datafusion::prelude::SessionContext;

use super::data;
use super::editor::Table;
use super::model::Dataset;

/// `(name, path, what it holds)`, in the order the overview shows them.
pub fn tables() -> Vec<(&'static str, String, &'static str)> {
    let mut v: Vec<(&'static str, String, &'static str)> = Dataset::ALL.iter().map(|(ds, id, about)| (*id, data::source(*ds), *about)).collect();
    v.push(("tile", crate::TILE.to_string(), "the LiDAR tile itself: every point, 17.3M rows"));
    v
}

/// Register the tables by name and turn `information_schema` on. Tables
/// whose file is missing are left out.
pub async fn register(ctx: &SessionContext) -> Result<()> {
    ctx.sql("SET datafusion.catalog.information_schema = true").await?;
    for (name, path, _) in tables() {
        if std::path::Path::new(&path).exists() {
            let df = ctx.sql(&format!("SELECT * FROM '{path}'")).await?;
            ctx.register_table(name, df.into_view())?;
        }
    }
    Ok(())
}

fn text(b: &[datafusion::arrow::record_batch::RecordBatch]) -> Vec<Vec<String>> {
    let opts = FormatOptions::default().with_null("");
    let mut out = vec![];
    for batch in b {
        let f: Vec<ArrayFormatter> = batch.columns().iter().map(|c| ArrayFormatter::try_new(c.as_ref(), &opts).unwrap()).collect();
        for i in 0..batch.num_rows() {
            out.push(f.iter().map(|x| x.value(i).to_string()).collect());
        }
    }
    out
}

/// A number, shortened: 6867530.600000001 → 6867530.6.
fn short(s: &str) -> String {
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() => format!("{}", (v * 100.0).round() / 100.0 + 0.0),
        _ => s.to_string(),
    }
}

/// One row per column of every table: its type, and its range or its
/// number of distinct values. `in_chart` names the table the chart draws.
pub async fn overview(ctx: &SessionContext, in_chart: Option<&str>) -> std::result::Result<Table, String> {
    let t = std::time::Instant::now();
    let e = |x: datafusion::error::DataFusionError| x.to_string();
    let mut rows = vec![];
    let mut notes = vec![];
    for (name, path, about) in tables() {
        let cols = ctx
            .sql(&format!("SELECT column_name, data_type FROM information_schema.columns WHERE table_name = '{name}' ORDER BY ordinal_position"))
            .await
            .map_err(e)?
            .collect()
            .await
            .map_err(e)?;
        let cols = text(&cols);
        if cols.is_empty() {
            continue;
        }
        // One pass per table: the count, and per column its range, or for
        // text its distinct values.
        let numeric = |t: &str| t.contains("Int") || t.contains("Float") || t.contains("Decimal");
        let mut agg = vec!["count(*)".to_string()];
        for (c, ty) in cols.iter().map(|r| (&r[0], &r[1])) {
            if numeric(ty) {
                agg.push(format!("CAST(min(\"{c}\") AS VARCHAR)"));
                agg.push(format!("CAST(max(\"{c}\") AS VARCHAR)"));
            } else if ty.contains("Utf8") && name != "tile" {
                agg.push(format!("CAST(count(DISTINCT \"{c}\") AS VARCHAR)"));
            }
        }
        let r = ctx.sql(&format!("SELECT {} FROM {name}", agg.join(", "))).await.map_err(e)?.collect().await.map_err(e)?;
        let r = text(&r).into_iter().next().unwrap_or_default();
        let mut k = 1;
        let marker = if in_chart == Some(name) { " ← chart" } else { "" };
        for (i, (c, ty)) in cols.iter().map(|r| (&r[0], &r[1])).enumerate() {
            let (lo, hi) = if numeric(ty) {
                k += 2;
                (short(&r[k - 2]), short(&r[k - 1]))
            } else if ty.contains("Utf8") && name != "tile" {
                k += 1;
                (format!("{} distinct", r[k - 1]), String::new())
            } else {
                (String::new(), String::new())
            };
            let (table, n) = if i == 0 { (format!("{name}{marker}"), r[0].clone()) } else { (String::new(), String::new()) };
            rows.push(vec![table, n, c.clone(), ty.replace("Timestamp(Nanosecond, None)", "Timestamp"), lo, hi]);
        }
        notes.push(format!("{name}: {about} · {path}"));
    }
    notes.push("SQL can name these tables, in the editor: SHOW TABLES · SELECT * FROM cells LIMIT 5 · SELECT * FROM information_schema.columns".into());
    Ok(Table {
        columns: ["table", "rows", "column", "type", "min", "max"].iter().map(|s| s.to_string()).collect(),
        note: format!("{} tables, {} columns", notes.len() - 1, rows.len()),
        rows,
        text: notes,
        ms: t.elapsed().as_secs_f64() * 1e3,
    })
}

/// The overview with the table the chart draws marked `← chart`; the rest
/// of it does not depend on the chart, so it is computed once.
pub fn mark(t: &Table, in_chart: &str) -> Table {
    let mut t = t.clone();
    for r in &mut t.rows {
        if r[0] == in_chart {
            r[0] = format!("{in_chart} ← chart");
        }
    }
    t
}
