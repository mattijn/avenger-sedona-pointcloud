//! Experiment 6: Vega expressions, SQL and chained pipelines.

pub mod vega {
    pub mod compile;
    pub mod parse;

    pub use compile::{analyze, Analysis, Blocker, Compiler};
    pub use parse::{parse, Ast, ParseError};
}

/// Function and step packages that can be registered on a session.
pub mod packages {
    pub mod chart;
    pub mod sedona;
    pub mod terrain;
    pub mod vega_compat;
    pub mod vega_format;

    use crate::pipeline::Package;

    pub fn vega_format() -> Package {
        Package {
            name: "vega-format",
            functions: vega_format::functions(),
            steps: vec![],
        }
    }

    pub fn vega_compat() -> Package {
        Package {
            name: "vega-compat",
            functions: vega_compat::functions(),
            steps: vec![],
        }
    }
}

pub mod pipeline;
pub mod steps;
