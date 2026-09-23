//! The `core` step package.

use std::sync::Arc;

use arrow::util::pretty::pretty_format_batches;
use async_trait::async_trait;
use datafusion::common::Column;
use datafusion::error::Result;
use datafusion::logical_expr::{scalar_subquery, Expr};

use crate::pipeline::{err, Call, Kind, Pipeline, Step};

macro_rules! step {
    ($ty:ident, $kind:expr, $help:expr, |$p:ident, $c:ident| $body:block) => {
        pub struct $ty;
        #[async_trait]
        impl Step for $ty {
            fn kind(&self) -> Kind {
                $kind
            }
            fn help(&self) -> &'static str {
                $help
            }
            async fn run(&self, $p: &mut Pipeline, $c: &Call) -> Result<Option<String>> {
                $body
            }
        }
    };
}

fn col(name: &str) -> Expr {
    Expr::Column(Column::from_name(name))
}

step!(
    Read,
    Kind::Source,
    "read <path> [--statistics]: a LAZ, Parquet or CSV file",
    |p, c| {
        let path = c.arg(0)?.to_string();
        if c.flag("statistics").is_some() {
            for s in [
                "SET las.collect_statistics = 'true'",
                "SET las.parallel_statistics_extraction = 'true'",
                "SET las.persist_statistics = 'true'",
            ] {
                p.ctx.sql(s).await?;
            }
        }
        let df = p.ctx.sql(&format!("SELECT * FROM '{path}'")).await?;
        p.plan = Some(df.into_unoptimized_plan());
        Ok(None)
    }
);

step!(
    Filter,
    Kind::Transform,
    "filter --vega <expr> | --sql <predicate>",
    |p, c| {
        let plan = p.plan()?.clone();
        let predicate = match (c.flag("vega"), c.flag("sql")) {
            (Some(v), _) => p.vega(v)?,
            (_, Some(s)) => p.dataframe()?.parse_sql_expr(s)?,
            _ => return Err(err("filter needs --vega or --sql")),
        };
        // avenger-transform's filter applies Vega truthiness to non-Boolean values.
        p.plan = Some(avenger_transform::filter(plan, predicate)?);
        Ok(None)
    }
);

step!(
    Calc,
    Kind::Transform,
    "calc <name> --vega <expr> | --sql <expr>: add or replace a field",
    |p, c| {
        let name = c.arg(0)?.to_string();
        let plan = p.plan()?.clone();
        let value = match (c.flag("vega"), c.flag("sql")) {
            (Some(v), _) => p.vega(v)?,
            (_, Some(s)) => p.dataframe()?.parse_sql_expr(s)?,
            _ => return Err(err("calc needs --vega or --sql")),
        };
        p.plan = Some(avenger_transform::formula(plan, value, &name)?);
        Ok(None)
    }
);

step!(
    Sql,
    Kind::Transform,
    "sql <query>: the data so far is the table `input`; first in a pipeline, a source (e.g. VALUES)",
    |p, c| {
        if p.plan.is_some() {
            p.expose_input()?;
        }
        let df = p.ctx.sql(c.arg(0)?).await?;
        p.plan = Some(df.into_unoptimized_plan());
        Ok(None)
    }
);

step!(
    Bin,
    Kind::Transform,
    "bin <field> [--maxbins n] [--as start,end]: Vega binning",
    |p, c| {
        let field = c.arg(0)?;
        let plan = p.plan()?.clone();
        let names: Vec<String> = c
            .flag("as")
            .map(|s| s.split(',').map(str::to_string).collect())
            .unwrap_or_else(|| vec![format!("{field}_start"), format!("{field}_end")]);
        let extent = avenger_transform::extent(plan.clone(), col(field))?;
        let options = avenger_transform::BinOptions {
            maxbins: c
                .flag("maxbins")
                .map(|v| datafusion::logical_expr::lit(v.parse::<f64>().unwrap_or(20.0))),
            ..Default::default()
        };
        let parameters =
            avenger_transform::bin_parameters(scalar_subquery(Arc::new(extent)), options)?;
        p.plan = Some(avenger_transform::bin(
            plan,
            col(field),
            parameters,
            [names[0].as_str(), names[1].as_str()],
        )?);
        Ok(None)
    }
);

step!(
    Materialize,
    Kind::Transform,
    "materialize: execute now and continue from memory",
    |p, _c| {
        let rows = p.materialize().await?;
        Ok(Some(format!("materialized {rows} rows")))
    }
);

step!(Count, Kind::Query, "count: number of rows", |p, _c| {
    Ok(Some(format!("{} rows", p.dataframe()?.count().await?)))
});

step!(Head, Kind::Query, "head [n]: the first rows", |p, c| {
    let n = c.args.first().and_then(|v| v.parse().ok()).unwrap_or(5);
    let batches = p.dataframe()?.limit(0, Some(n))?.collect().await?;
    Ok(Some(pretty_format_batches(&batches)?.to_string()))
});

step!(Schema, Kind::Query, "schema: fields and types", |p, _c| {
    let s = p.plan()?.schema();
    Ok(Some(
        s.fields()
            .iter()
            .map(|f| format!("{}: {}", f.name(), f.data_type()))
            .collect::<Vec<_>>()
            .join("\n"),
    ))
});

step!(
    Explain,
    Kind::Query,
    "explain: the optimised and physical plans",
    |p, _c| {
        let plans = p.explain().await?;
        Ok(Some(plans))
    }
);

step!(
    Query,
    Kind::Query,
    "query --sql <query over input>: read without changing the pipeline",
    |p, c| {
        p.expose_input()?;
        let q = c.flag("sql").ok_or_else(|| err("query needs --sql"))?;
        let batches = p.ctx.sql(q).await?.collect().await?;
        Ok(Some(pretty_format_batches(&batches)?.to_string()))
    }
);

step!(Write, Kind::Sink, "write <path.parquet>", |p, c| {
    let path = c.arg(0)?;
    let n = p
        .dataframe()?
        .write_parquet(path, Default::default(), None)
        .await?
        .iter()
        .map(|b| b.num_rows())
        .sum::<usize>();
    Ok(Some(format!("wrote {path} ({n} batches)")))
});

step!(
    Save,
    Kind::Sink,
    "save <path.json>: the pipeline as data steps plus the command log",
    |p, c| {
        let path = c.arg(0)?;
        let json = p.to_json();
        std::fs::write(path, serde_json::to_string_pretty(&json).unwrap())?;
        Ok(Some(format!("saved {path}")))
    }
);

pub fn register_core(p: &mut Pipeline) {
    let core: [(&str, Arc<dyn Step>); 13] = [
        ("save", Arc::new(Save)),
        ("read", Arc::new(Read)),
        ("filter", Arc::new(Filter)),
        ("calc", Arc::new(Calc)),
        ("sql", Arc::new(Sql)),
        ("bin", Arc::new(Bin)),
        ("materialize", Arc::new(Materialize)),
        ("count", Arc::new(Count)),
        ("head", Arc::new(Head)),
        ("schema", Arc::new(Schema)),
        ("explain", Arc::new(Explain)),
        ("query", Arc::new(Query)),
        ("write", Arc::new(Write)),
    ];
    for (name, step) in core {
        p.register_step("core", name, step);
    }
}
