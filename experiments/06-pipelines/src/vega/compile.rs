//! Vega expression AST → DataFusion `Expr`.
//!
//! Every function is resolved by name through the session's function
//! registry, so a Vega built-in is only a mapping onto an SQL function, and
//! any function a package registers (geodatafusion, Sedona) is callable from
//! Vega expressions as well. Truthiness comes from `avenger-transform`, so
//! the semantics match Jon's `filter` and `formula`.
//!
//! Expressions that read the chart (`scale`, `domain`, `bandwidth`, `data`)
//! or belong to interaction (`event`, `vlSelectionTest`, `modify`) are not
//! row expressions. The analysis reports them as such instead of compiling
//! them: they are queries against the chart object, or commands.

use std::collections::BTreeSet;
use std::sync::Arc;

use arrow::datatypes::{DataType, Field};
use datafusion::common::{Column, DFSchema, ScalarValue};
use datafusion::execution::FunctionRegistry;
use datafusion::logical_expr::expr::Placeholder;
use datafusion::logical_expr::{cast, lit, try_cast, when, Expr, ExprSchemable, Operator};

use avenger_transform::expr_fn::truthy;

use super::parse::Ast;

/// Functions that read chart state: queries against the chart object.
pub const CHART_QUERIES: [&str; 17] = [
    "scale",
    "invert",
    "domain",
    "range",
    "bandwidth",
    "bandspace",
    "copy",
    "data",
    "indata",
    "geoScale",
    "geoArea",
    "geoBounds",
    "geoCentroid",
    "geoShape",
    "containerSize",
    "screen",
    "windowSize",
];

/// Functions and names that belong to interaction: event handling and
/// selection state, the command side.
pub const INTERACTION: [&str; 26] = [
    "event",
    "item",
    "group",
    "parent",
    "x",
    "y",
    "xy",
    "vlSelectionTest",
    "vlSelectionResolve",
    "vlSelectionIdTest",
    "vlSelectionTuples",
    "vlSetIdTest",
    "modify",
    "panLinear",
    "panLog",
    "panPow",
    "panSymlog",
    "zoomLinear",
    "zoomLog",
    "zoomPow",
    "zoomSymlog",
    "isTuple",
    "encode",
    "treePath",
    "treeAncestors",
    "pinchDistance",
];

const CONSTANTS: [(&str, f64); 11] = [
    ("PI", std::f64::consts::PI),
    ("E", std::f64::consts::E),
    ("LN2", std::f64::consts::LN_2),
    ("LN10", std::f64::consts::LN_10),
    ("LOG2E", std::f64::consts::LOG2_E),
    ("LOG10E", std::f64::consts::LOG10_E),
    ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
    ("SQRT2", std::f64::consts::SQRT_2),
    ("MIN_VALUE", 5e-324),
    ("MAX_VALUE", f64::MAX),
    ("NaN", f64::NAN),
];

/// Why an expression is not a row expression, or could not be compiled.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Blocker {
    ChartQuery(String),
    Interaction(String),
    Unsupported(String),
}

impl std::fmt::Display for Blocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blocker::ChartQuery(s) => write!(f, "reads the chart: {s}"),
            Blocker::Interaction(s) => write!(f, "interaction: {s}"),
            Blocker::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

/// What an expression refers to, found before compiling it.
#[derive(Default, Debug)]
pub struct Analysis {
    /// `datum` fields it reads.
    pub fields: BTreeSet<String>,
    /// Signals: bare names that become query parameters.
    pub signals: BTreeSet<String>,
    pub blockers: BTreeSet<Blocker>,
}

pub fn analyze(ast: &Ast) -> Analysis {
    let mut a = Analysis::default();
    walk(ast, &mut a);
    a
}

fn walk(ast: &Ast, a: &mut Analysis) {
    match ast {
        Ast::Ident(name) => {
            if name == "datum" {
                a.blockers
                    .insert(Blocker::Unsupported("`datum` as a whole object".into()));
            } else if INTERACTION.contains(&name.as_str()) {
                a.blockers.insert(Blocker::Interaction(name.clone()));
            } else if !CONSTANTS.iter().any(|c| c.0 == name) {
                a.signals.insert(name.clone());
            }
        }
        Ast::Member {
            object,
            property,
            computed,
        } => match (object.as_ref(), property.as_ref()) {
            (Ast::Ident(d), Ast::String(f)) if d == "datum" => {
                a.fields.insert(f.clone());
            }
            (Ast::Ident(d), p) if d == "datum" && *computed => {
                a.blockers
                    .insert(Blocker::Unsupported("computed `datum[...]` field".into()));
                walk(p, a);
            }
            (o, p) => {
                walk(o, a);
                if *computed {
                    walk(p, a);
                }
            }
        },
        Ast::Call { callee, args } => {
            if CHART_QUERIES.contains(&callee.as_str()) {
                a.blockers
                    .insert(Blocker::ChartQuery(format!("{callee}()")));
            } else if INTERACTION.contains(&callee.as_str()) {
                a.blockers
                    .insert(Blocker::Interaction(format!("{callee}()")));
            }
            args.iter().for_each(|x| walk(x, a));
        }
        Ast::Unary { arg, .. } => walk(arg, a),
        Ast::Binary { left, right, .. } | Ast::Logical { left, right, .. } => {
            walk(left, a);
            walk(right, a);
        }
        Ast::Conditional {
            test,
            then,
            otherwise,
        } => {
            walk(test, a);
            walk(then, a);
            walk(otherwise, a);
        }
        Ast::Array(xs) => xs.iter().for_each(|x| walk(x, a)),
        Ast::Object(fs) => fs.iter().for_each(|(_, x)| walk(x, a)),
        _ => {}
    }
}

pub struct Compiler<'a> {
    pub schema: &'a DFSchema,
    pub registry: &'a dyn FunctionRegistry,
    /// JavaScript semantics (null is 0 in arithmetic, NaN compares false,
    /// numbers print as JavaScript does) instead of SQL's. Needs the
    /// `vega-compat` package registered.
    pub vega: bool,
}

type R = Result<Expr, Blocker>;

fn unsupported(s: impl Into<String>) -> Blocker {
    Blocker::Unsupported(s.into())
}

fn is_string(t: &DataType) -> bool {
    matches!(t, DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View)
}

fn is_numeric(t: &DataType) -> bool {
    t.is_numeric()
}

/// One or more function arguments.
struct Args(Vec<Expr>);

impl From<Expr> for Args {
    fn from(e: Expr) -> Self {
        Args(vec![e])
    }
}

impl From<Vec<Expr>> for Args {
    fn from(v: Vec<Expr>) -> Self {
        Args(v)
    }
}

impl Compiler<'_> {
    fn typ(&self, e: &Expr) -> Option<DataType> {
        e.get_type(self.schema).ok()
    }

    /// Call a registered SQL function by name.
    fn call(&self, name: &str, args: Vec<Expr>) -> R {
        let udf = self
            .registry
            .udf(name)
            .map_err(|_| unsupported(format!("SQL function `{name}` is not registered")))?;
        Ok(udf.call(args))
    }

    /// A value as a double, keeping null. JavaScript numbers are doubles.
    fn num_raw(&self, e: Expr) -> Expr {
        match self.typ(&e) {
            Some(DataType::Float64) => e,
            Some(t) if is_string(&t) && self.vega => self.js("js_to_number", e),
            Some(t) if is_string(&t) => try_cast(e, DataType::Float64),
            _ => cast(e, DataType::Float64),
        }
    }

    /// A value in numeric context: under Vega semantics null is 0, as
    /// `Number(null)` is in JavaScript.
    fn num(&self, e: Expr) -> Expr {
        let x = self.num_raw(e);
        if self.vega {
            self.js("coalesce", vec![x, lit(0.0)])
        } else {
            x
        }
    }

    fn string(&self, e: Expr) -> Expr {
        if self.vega {
            return self.js("js_string", e);
        }
        match self.typ(&e) {
            Some(t) if is_string(&t) => e,
            _ => cast(e, DataType::Utf8),
        }
    }

    /// Call a function that must exist (built in, or from vega-compat).
    fn js(&self, name: &str, args: impl Into<Args>) -> Expr {
        self.call(name, args.into().0)
            .expect("vega-compat functions are registered")
    }

    /// NaN never compares true in JavaScript; SQL orders it above every number.
    fn nan_guard(&self, cmp: Expr, l: &Expr, r: &Expr, not_eq: bool) -> Expr {
        let nan = self.js("isnan", l.clone()).or(self.js("isnan", r.clone()));
        if not_eq {
            cmp.or(nan)
        } else {
            cmp.and(Expr::Not(Box::new(nan)))
        }
    }

    /// `coalesce(x, 0) < c` hides `x < c` from statistics pruning. With a
    /// literal on one side the same JavaScript semantics can be written as
    /// `(x < c OR x IS NULL [if 0 < c]) AND NOT isnan(x)`, which pruning
    /// understands. Returns None when the rewrite does not apply.
    fn pruning_friendly(&self, l: &Expr, o: Operator, r: &Expr) -> Option<Expr> {
        let lit_value = |e: &Expr| match e {
            Expr::Literal(v, _) => match v {
                ScalarValue::Float64(Some(x)) => Some(*x),
                ScalarValue::Int64(Some(x)) => Some(*x as f64),
                _ => None,
            },
            _ => None,
        };
        let (x, c, o) = match (lit_value(l), lit_value(r)) {
            (None, Some(c)) => (l, c, o),
            (Some(c), None) => (r, c, o.swap()?),
            _ => return None,
        };
        if o == Operator::NotEq || c.is_nan() || !self.typ(x).is_some_and(|t| t.is_numeric()) {
            return None;
        }
        // null is 0 in JavaScript: does 0 satisfy the comparison?
        let zero_passes = match o {
            Operator::Lt => 0.0 < c,
            Operator::LtEq => 0.0 <= c,
            Operator::Gt => 0.0 > c,
            Operator::GtEq => 0.0 >= c,
            Operator::Eq => c == 0.0,
            _ => return None,
        };
        let x = self.num_raw(x.clone());
        let cmp = Expr::BinaryExpr(datafusion::logical_expr::BinaryExpr::new(
            Box::new(x.clone()),
            o,
            Box::new(lit(c)),
        ));
        let cmp = if zero_passes {
            cmp.or(x.clone().is_null())
        } else {
            cmp
        };
        let not_nan = Expr::Not(Box::new(self.js("isnan", x.clone()))).or(x.is_null());
        Some(cmp.and(not_nan))
    }

    /// max/min: NaN wins in JavaScript's Math.max and Math.min.
    fn extreme(&self, sql: &str, args: Vec<Expr>) -> R {
        let e = self.call(sql, args.clone())?;
        if !self.vega {
            return Ok(e);
        }
        let any_nan = args
            .iter()
            .map(|a| self.js("isnan", a.clone()))
            .reduce(|a, b| a.or(b))
            .unwrap_or(lit(false));
        when(any_nan, lit(f64::NAN))
            .otherwise(e)
            .map_err(|e| unsupported(e.to_string()))
    }

    pub fn compile(&self, ast: &Ast) -> R {
        match ast {
            Ast::Number(v) => Ok(lit(*v)),
            Ast::String(s) => Ok(lit(s.clone())),
            Ast::Bool(b) => Ok(lit(*b)),
            Ast::Null => Ok(lit(ScalarValue::Null)),
            Ast::Ident(name) => {
                if let Some((_, v)) = CONSTANTS.iter().find(|c| c.0 == name) {
                    return Ok(lit(*v));
                }
                // A signal becomes a typed query parameter.
                Ok(Expr::Placeholder(Placeholder::new_with_field(
                    format!("${name}"),
                    Some(Arc::new(Field::new(name, DataType::Float64, true))),
                )))
            }
            Ast::Member {
                object,
                property,
                computed,
            } => self.member(object, property, *computed),
            Ast::Unary { op, arg } => {
                let x = self.compile(arg)?;
                match *op {
                    "-" => Ok(Expr::Negative(Box::new(self.num(x)))),
                    "+" => Ok(self.num(x)),
                    "!" => Ok(Expr::Not(Box::new(truthy(x)))),
                    o => Err(unsupported(format!("unary `{o}`"))),
                }
            }
            Ast::Binary { op, left, right } => self.binary(op, left, right),
            Ast::Logical { op, left, right } => {
                let (l, r) = (self.compile(left)?, self.compile(right)?);
                let both_bool = self.typ(&l) == Some(DataType::Boolean)
                    && self.typ(&r) == Some(DataType::Boolean);
                match *op {
                    "&&" if both_bool => Ok(l.and(r)),
                    "||" if both_bool => Ok(l.or(r)),
                    // JavaScript returns an operand: a && b is b if a is truthy.
                    "&&" => when(truthy(l.clone()), r)
                        .otherwise(l)
                        .map_err(|e| unsupported(e.to_string())),
                    "||" => when(truthy(l.clone()), l)
                        .otherwise(r)
                        .map_err(|e| unsupported(e.to_string())),
                    "??" => self.call("coalesce", vec![l, r]),
                    o => Err(unsupported(format!("logical `{o}`"))),
                }
            }
            Ast::Conditional {
                test,
                then,
                otherwise,
            } => {
                let (t, a, b) = (
                    self.compile(test)?,
                    self.compile(then)?,
                    self.compile(otherwise)?,
                );
                when(truthy(t), a)
                    .otherwise(b)
                    .map_err(|e| unsupported(e.to_string()))
            }
            Ast::Call { callee, args } => self.function(callee, args),
            Ast::Array(items) => {
                let xs = items
                    .iter()
                    .map(|x| self.compile(x))
                    .collect::<Result<Vec<_>, _>>()?;
                self.call("make_array", xs)
            }
            Ast::Object(_) => Err(unsupported("object literal")),
        }
    }

    /// Numbers are epoch milliseconds, as JavaScript dates are.
    fn instant(&self, e: Expr) -> R {
        match self.typ(&e) {
            // new Date(null) is the epoch in JavaScript.
            Some(t) if t.is_numeric() && self.vega => self.call(
                "to_timestamp_millis",
                vec![cast(self.num(e), DataType::Int64)],
            ),
            Some(t) if t.is_numeric() => {
                self.call("to_timestamp_millis", vec![cast(e, DataType::Int64)])
            }
            _ => Ok(e),
        }
    }

    fn member(&self, object: &Ast, property: &Ast, computed: bool) -> R {
        if let (Ast::Ident(d), Ast::String(f)) = (object, property) {
            if d == "datum" {
                return Ok(Expr::Column(Column::from_name(f.clone())));
            }
        }
        if let (false, Ast::String(p)) = (computed, property) {
            if p == "length" {
                let o = self.compile(object)?;
                return match self.typ(&o) {
                    Some(DataType::List(_)) => self.call("array_length", vec![o]),
                    _ => self.call("character_length", vec![self.string(o)]),
                };
            }
            // Nested field of a struct column: datum.a.b.
            let o = self.compile(object)?;
            return self.call("get_field", vec![o, lit(p.clone())]);
        }
        let (o, i) = (self.compile(object)?, self.compile(property)?);
        // Array index: JavaScript is 0-based, SQL arrays 1-based.
        self.call(
            "array_element",
            vec![o, cast(i, DataType::Int64) + lit(1i64)],
        )
    }

    fn binary(&self, op: &str, left: &Ast, right: &Ast) -> R {
        let (l, r) = (self.compile(left)?, self.compile(right)?);
        let (tl, tr) = (self.typ(&l), self.typ(&r));
        let s = |t: &Option<DataType>| t.as_ref().is_some_and(is_string);
        let op_of = |o: &str| {
            Some(match o {
                "-" => Operator::Minus,
                "*" => Operator::Multiply,
                "/" => Operator::Divide,
                "%" => Operator::Modulo,
                "<" => Operator::Lt,
                ">" => Operator::Gt,
                "<=" => Operator::LtEq,
                ">=" => Operator::GtEq,
                "==" | "===" => Operator::Eq,
                "!=" | "!==" => Operator::NotEq,
                _ => return None,
            })
        };
        let bin = |a: Expr, o: Operator, b: Expr| {
            Expr::BinaryExpr(datafusion::logical_expr::BinaryExpr::new(
                Box::new(a),
                o,
                Box::new(b),
            ))
        };
        match op {
            // `+` concatenates as soon as one side is a string.
            "+" if s(&tl) || s(&tr) => self.call("concat", vec![self.string(l), self.string(r)]),
            "+" => Ok(bin(self.num(l), Operator::Plus, self.num(r))),
            "-" | "*" | "/" | "%" => Ok(bin(self.num(l), op_of(op).unwrap(), self.num(r))),
            "**" => self.call("power", vec![self.num(l), self.num(r)]),
            "===" | "!=="
                if s(&tl) != s(&tr)
                    && tl.is_some()
                    && tr.is_some()
                    && tl != Some(DataType::Null)
                    && tr != Some(DataType::Null) =>
            {
                // Strict equality across types is always false.
                Ok(lit(op == "!=="))
            }
            "==" | "===" | "!=" | "!==" | "<" | ">" | "<=" | ">=" => {
                let o = op_of(op).unwrap();
                let same_kind = (s(&tl) && s(&tr))
                    || (tl == Some(DataType::Boolean) && tr == Some(DataType::Boolean));
                let e = if same_kind {
                    bin(l, o, r)
                } else if self.vega {
                    if let Some(e) = self.pruning_friendly(&l, o, &r) {
                        return Ok(e);
                    }
                    let (l, r) = (self.num(l), self.num(r));
                    self.nan_guard(bin(l.clone(), o, r.clone()), &l, &r, o == Operator::NotEq)
                } else {
                    bin(self.num(l), o, self.num(r))
                };
                // A comparison is never null in JavaScript.
                Ok(if self.vega {
                    self.js("coalesce", vec![e, lit(false)])
                } else {
                    e
                })
            }
            "&" | "|" | "^" | "<<" | ">>" => {
                let o = match op {
                    "&" => Operator::BitwiseAnd,
                    "|" => Operator::BitwiseOr,
                    "^" => Operator::BitwiseXor,
                    "<<" => Operator::BitwiseShiftLeft,
                    _ => Operator::BitwiseShiftRight,
                };
                let i = |e: Expr| cast(self.num(e), DataType::Int32);
                Ok(cast(bin(i(l), o, i(r)), DataType::Float64))
            }
            o => Err(unsupported(format!("operator `{o}`"))),
        }
    }

    fn function(&self, name: &str, args: &[Ast]) -> R {
        if CHART_QUERIES.contains(&name) {
            return Err(Blocker::ChartQuery(format!("{name}()")));
        }
        if INTERACTION.contains(&name) {
            return Err(Blocker::Interaction(format!("{name}()")));
        }
        let xs = args
            .iter()
            .map(|x| self.compile(x))
            .collect::<Result<Vec<_>, _>>()?;
        let n = |i: usize| {
            xs.get(i)
                .cloned()
                .ok_or_else(|| unsupported(format!("{name}: missing argument {i}")))
        };
        let num = |i: usize| n(i).map(|e| self.num(e));
        let unary_math = [
            ("abs", "abs"),
            ("acos", "acos"),
            ("asin", "asin"),
            ("atan", "atan"),
            ("ceil", "ceil"),
            ("cos", "cos"),
            ("exp", "exp"),
            ("floor", "floor"),
            ("log", "ln"),
            ("sin", "sin"),
            ("sqrt", "sqrt"),
            ("tan", "tan"),
            ("round", "round"),
            ("sign", "signum"),
            ("trunc", "trunc"),
        ];
        if name == "round" && self.vega {
            // Math.round rounds half up: round(-2.5) is -2.
            return self.call("floor", vec![num(0)? + lit(0.5)]);
        }
        if let Some((_, sql)) = unary_math.iter().find(|m| m.0 == name) {
            return self.call(sql, vec![num(0)?]);
        }
        match name {
            "atan2" => self.call("atan2", vec![num(0)?, num(1)?]),
            "pow" => self.call("power", vec![num(0)?, num(1)?]),
            "hypot" => {
                let (a, b) = (num(0)?, num(1)?);
                self.call("sqrt", vec![a.clone() * a + b.clone() * b])
            }
            "max" | "min" => {
                let args: Vec<Expr> = xs.into_iter().map(|e| self.num(e)).collect();
                self.extreme(if name == "max" { "greatest" } else { "least" }, args)
            }
            "clamp" => {
                let x = self.extreme("least", vec![num(0)?, num(2)?])?;
                self.extreme("greatest", vec![x, num(1)?])
            }
            "random" => self.call("random", vec![]),
            "if" => when(truthy(n(0)?), n(1)?)
                .otherwise(n(2)?)
                .map_err(|e| unsupported(e.to_string())),
            // Only `undefined` is undefined in JavaScript; SQL has no such value.
            "isDefined" => Ok(lit(true)),
            "isNaN" => self.call("isnan", vec![num(0)?]),
            "isFinite" => {
                // Number.isFinite: no coercion, null is not finite.
                let x = self.num_raw(n(0)?);
                let finite = Expr::Not(Box::new(self.call("isnan", vec![x.clone()])?))
                    .and(self.call("abs", vec![x.clone()])?.lt(lit(f64::INFINITY)));
                Ok(x.is_not_null().and(finite))
            }
            "isValid" => {
                let x = n(0)?;
                match self.typ(&x) {
                    Some(t) if t.is_floating() => Ok(x
                        .clone()
                        .is_not_null()
                        .and(Expr::Not(Box::new(self.call("isnan", vec![x])?)))),
                    _ => Ok(x.is_not_null()),
                }
            }
            "isNumber" | "isString" | "isBoolean" | "isDate" | "isArray" | "isRegExp"
            | "isObject" => {
                let t = self.typ(&n(0)?);
                Ok(lit(match (name, t) {
                    ("isNumber", Some(t)) => is_numeric(&t),
                    ("isString", Some(t)) => is_string(&t),
                    ("isBoolean", Some(t)) => t == DataType::Boolean,
                    ("isDate", Some(t)) => matches!(
                        t,
                        DataType::Timestamp(..) | DataType::Date32 | DataType::Date64
                    ),
                    ("isArray", Some(t)) => matches!(t, DataType::List(_) | DataType::LargeList(_)),
                    _ => false,
                }))
            }
            "toNumber" if self.vega => {
                // Vega's toNumber: null and '' are null, otherwise Number(x).
                let x = n(0)?;
                let empty = match self.typ(&x) {
                    Some(t) if is_string(&t) => x.clone().is_null().or(x.clone().eq(lit(""))),
                    _ => x.clone().is_null(),
                };
                when(empty, lit(ScalarValue::Float64(None)))
                    .otherwise(self.num_raw(x))
                    .map_err(|e| unsupported(e.to_string()))
            }
            "toNumber" => Ok(self.num_raw(n(0)?)),
            "toString" if self.vega => {
                let x = n(0)?;
                when(x.clone().is_null(), lit(ScalarValue::Utf8(None)))
                    .otherwise(self.string(x))
                    .map_err(|e| unsupported(e.to_string()))
            }
            "toString" => Ok(self.string(n(0)?)),
            "toBoolean" if self.vega => {
                // Vega's toBoolean: null and '' are null, 'false' and '0' false.
                let x = n(0)?;
                match self.typ(&x) {
                    Some(t) if is_string(&t) => when(
                        x.clone().is_null().or(x.clone().eq(lit(""))),
                        lit(ScalarValue::Boolean(None)),
                    )
                    .when(
                        x.clone().eq(lit("false")).or(x.clone().eq(lit("0"))),
                        lit(false),
                    )
                    .otherwise(truthy(x))
                    .map_err(|e| unsupported(e.to_string())),
                    _ => when(x.clone().is_null(), lit(ScalarValue::Boolean(None)))
                        .otherwise(truthy(x))
                        .map_err(|e| unsupported(e.to_string())),
                }
            }
            "toBoolean" => Ok(truthy(n(0)?)),
            "toDate" => Ok(cast(
                n(0)?,
                DataType::Timestamp(arrow::datatypes::TimeUnit::Millisecond, None),
            )),
            "parseFloat" => num(0),
            "parseInt" => Ok(self.call("trunc", vec![num(0)?])?),
            "length" => {
                let x = n(0)?;
                match self.typ(&x) {
                    Some(DataType::List(_)) => self.call("array_length", vec![x]),
                    _ => self.call("character_length", vec![self.string(x)]),
                }
            }
            "upper" => self.call("upper", vec![self.string(n(0)?)]),
            "lower" => self.call("lower", vec![self.string(n(0)?)]),
            "trim" => self.call("btrim", vec![self.string(n(0)?)]),
            "substring" | "slice" if args.len() >= 2 => {
                let s = self.string(n(0)?);
                let start = cast(num(1)?, DataType::Int64);
                let from = start.clone() + lit(1i64);
                match xs.get(2) {
                    Some(end) => {
                        let len = cast(self.num(end.clone()), DataType::Int64) - start;
                        self.call("substr", vec![s, from, len])
                    }
                    None => self.call("substr", vec![s, from]),
                }
            }
            "indexof" => {
                let x = n(0)?;
                if matches!(self.typ(&x), Some(DataType::List(_))) {
                    let p = self.call("array_position", vec![x, n(1)?])?;
                    Ok(self.call(
                        "coalesce",
                        vec![cast(p, DataType::Float64) - lit(1.0), lit(-1.0)],
                    )?)
                } else {
                    let p = self.call("strpos", vec![self.string(x), self.string(n(1)?)])?;
                    Ok(cast(p, DataType::Float64) - lit(1.0))
                }
            }
            "replace" => {
                // JavaScript replaces the first match of a string pattern.
                let pattern = self.call(
                    "regexp_replace",
                    vec![
                        self.string(n(1)?),
                        lit(r"([.*+?^${}()|\[\]\\])"),
                        lit(r"\$1"),
                        lit("g"),
                    ],
                )?;
                self.call(
                    "regexp_replace",
                    vec![self.string(n(0)?), pattern, self.string(n(2)?)],
                )
            }
            "inrange" => {
                let (v, range) = (num(0)?, n(1)?);
                let a = self.call("array_element", vec![range.clone(), lit(1i64)])?;
                let b = self.call("array_element", vec![range, lit(2i64)])?;
                let lo = self.call("least", vec![self.num(a.clone()), self.num(b.clone())])?;
                let hi = self.call("greatest", vec![self.num(a), self.num(b)])?;
                Ok(v.clone().gt_eq(lo).and(v.lt_eq(hi)))
            }
            "now" => self.call("now", vec![]),
            "year" | "quarter" | "month" | "date" | "day" | "hours" | "minutes" | "seconds"
            | "milliseconds" | "dayofyear" => {
                let part = match name {
                    "date" => "day",
                    "day" => "dow",
                    "hours" => "hour",
                    "minutes" => "minute",
                    "seconds" => "second",
                    "milliseconds" => "millisecond",
                    "dayofyear" => "doy",
                    other => other,
                };
                let v = self.call("date_part", vec![lit(part), self.instant(n(0)?)?])?;
                // JavaScript months are 0-based.
                Ok(if name == "month" {
                    cast(v, DataType::Float64) - lit(1.0)
                } else {
                    cast(v, DataType::Float64)
                })
            }
            "time" => {
                let v = self.call("date_part", vec![lit("epoch"), self.instant(n(0)?)?])?;
                Ok(cast(v, DataType::Float64) * lit(1000.0))
            }
            // Anything else: a function registered by a package, called by
            // its own name. This is how SQL function packages reach Vega.
            // SQL function names are lower case (Sedona's st_point); Vega
            // names are camelCase. Try both.
            other => match self
                .registry
                .udf(other)
                .or_else(|_| self.registry.udf(&other.to_lowercase()))
            {
                Ok(udf) if self.vega && matches!(other, "format" | "timeFormat" | "utcFormat") => {
                    self.call("coalesce", vec![udf.call(xs), lit("null")])
                }
                Ok(udf) => Ok(udf.call(xs)),
                Err(_) => Err(unsupported(format!("{other}()"))),
            },
        }
    }
}
