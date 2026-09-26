//! Layer 1: one value against its type. Each type is a regular expression
//! (or a list of choices), checked natively here and exported as the same CEL
//! `matches`, so Rust, Python and JavaScript agree on every value.

use regex::Regex;
use serde_json::Value;

/// A number in 0..1, written plainly: `0`, `0.25`, `.5`, `1`, `1.0`.
const UNIT: &str = r"(?:0(?:\.[0-9]+)?|1(?:\.0+)?|\.[0-9]+)";
/// A decimal number, optionally signed, optionally with an exponent.
const NUM: &str = r"-?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?";
const HEX: &str = r"#[0-9a-fA-F]{6}";

#[derive(Clone, Debug, PartialEq)]
pub enum ArgType {
    /// Any string; checked by later layers if at all (`text`, `path`).
    Any(String),
    /// A string matching a pattern, anchored.
    Pattern { name: String, pattern: String },
    Choice(Vec<String>),
    /// A Vega expression: parsed in layer 3.
    Vega,
    /// A SQL query over `input`: planned in layer 4.
    Sql,
}

impl ArgType {
    pub fn from_json(t: &Value) -> Result<ArgType, String> {
        if let Some(a) = t.as_array() {
            return Ok(ArgType::Choice(a.iter().map(|c| c.as_str().unwrap_or("").to_string()).collect()));
        }
        let name = t.as_str().ok_or("a type is a name or a list of choices")?;
        let p = |pattern: String| Ok(ArgType::Pattern { name: name.to_string(), pattern: format!("^(?:{pattern})$") });
        match name {
            "text" | "path" => Ok(ArgType::Any(name.to_string())),
            "vega" => Ok(ArgType::Vega),
            "sql" => Ok(ArgType::Sql),
            "hex" => p(HEX.into()),
            "unit" => p(UNIT.into()),
            "unit_pair" => p(format!("{UNIT},{UNIT}")),
            "number" => p(NUM.into()),
            "range_pair" => p(format!(r"{NUM}\.\.{NUM}\s+{NUM}\.\.{NUM}")),
            "flag" => Ok(ArgType::Choice(vec!["true".into(), "false".into()])),
            // A field, with an optional Vega-Lite type; a colour where one is allowed.
            "channel" => p(format!("{HEX}|[^:]+(?::[NOQT])?")),
            other => Err(format!("unknown type `{other}`")),
        }
    }

    /// The name used in messages and in the export.
    pub fn name(&self) -> String {
        match self {
            ArgType::Any(n) | ArgType::Pattern { name: n, .. } => n.clone(),
            ArgType::Choice(c) => format!("one of {}", c.join(", ")),
            ArgType::Vega => "vega".into(),
            ArgType::Sql => "sql".into(),
        }
    }

    /// The same check as CEL over a string `v`.
    pub fn cel(&self) -> String {
        match self {
            ArgType::Any(_) | ArgType::Vega | ArgType::Sql => "true".into(),
            ArgType::Pattern { pattern, .. } => format!("v.matches({})", cel_string(pattern)),
            ArgType::Choice(c) => format!("v in [{}]", c.iter().map(|s| cel_string(s)).collect::<Vec<_>>().join(", ")),
        }
    }
}

/// A CEL string literal (raw strings are not in every CEL implementation).
pub fn cel_string(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Types with their patterns compiled, for checking.
pub struct Checker {
    compiled: Vec<(String, Regex)>,
}

impl Checker {
    pub fn new<'a>(types: impl Iterator<Item = &'a ArgType>) -> Checker {
        let mut compiled: Vec<(String, Regex)> = vec![];
        for t in types {
            if let ArgType::Pattern { pattern, .. } = t {
                if !compiled.iter().any(|(p, _)| p == pattern) {
                    compiled.push((pattern.clone(), Regex::new(pattern).expect("a type's pattern compiles")));
                }
            }
        }
        Checker { compiled }
    }

    pub fn check(&self, ty: &ArgType, v: &str) -> Result<(), String> {
        let ok = match ty {
            ArgType::Any(_) | ArgType::Vega | ArgType::Sql => true,
            ArgType::Choice(c) => c.iter().any(|x| x == v),
            ArgType::Pattern { pattern, .. } => self.compiled.iter().find(|(p, _)| p == pattern).is_some_and(|(_, r)| r.is_match(v)),
        };
        if ok {
            Ok(())
        } else {
            Err(match ty {
                ArgType::Choice(c) => format!("`{v}` is not one of {}", c.join(", ")),
                ArgType::Pattern { name, .. } => format!("`{v}` is not a {}", describe(name)),
                _ => unreachable!(),
            })
        }
    }
}

fn describe(name: &str) -> &str {
    match name {
        "hex" => "#rrggbb colour",
        "unit" => "number in 0..1",
        "unit_pair" => "pair of numbers in 0..1, `a,b`",
        "number" => "number",
        "range_pair" => "pair of ranges, `a..b c..d`",
        "channel" => "field, `field:N|O|Q|T`, or a #rrggbb colour",
        n => n,
    }
}
