//! A parser for the Vega expression language: the JavaScript expression
//! subset described at <https://vega.github.io/vega/docs/expressions/>.
//! Literals, identifiers, member access, calls, unary, binary, logical and
//! conditional operators, and array and object literals. No statements,
//! assignments, `new`, or function definitions, as in Vega.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Ast {
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Ident(String),
    /// `object.name` or `object[expr]`.
    Member {
        object: Box<Ast>,
        property: Box<Ast>,
        computed: bool,
    },
    Call {
        callee: String,
        args: Vec<Ast>,
    },
    Unary {
        op: &'static str,
        arg: Box<Ast>,
    },
    Binary {
        op: &'static str,
        left: Box<Ast>,
        right: Box<Ast>,
    },
    /// `&&`, `||`: JavaScript returns an operand, not a Boolean.
    Logical {
        op: &'static str,
        left: Box<Ast>,
        right: Box<Ast>,
    },
    Conditional {
        test: Box<Ast>,
        then: Box<Ast>,
        otherwise: Box<Ast>,
    },
    Array(Vec<Ast>),
    Object(Vec<(String, Ast)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub offset: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at offset {}", self.message, self.offset)
    }
}

impl std::error::Error for ParseError {}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Punct(&'static str),
    Regex,
    End,
}

const PUNCT: [&str; 44] = [
    ">>>=", "===", "!==", ">>>", "<<=", ">>=", "**=", "&&", "||", "??", "==", "!=", "<=", ">=",
    "<<", ">>", "**", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "?.", "+", "-", "*", "/",
    "%", "<", ">", "!", "~", "&", "|", "^", "?", ":", "(", ")", "[", "]",
];
const PUNCT_TAIL: [&str; 4] = [",", ".", "{", "}"];

fn tokenize(src: &str) -> Result<Vec<(Tok, usize)>, ParseError> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = vec![];
    let err = |m: &str, o: usize| ParseError {
        message: m.into(),
        offset: o,
    };
    while i < b.len() {
        let c = b[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if c.is_ascii_digit()
            || (c == '.' && i + 1 < b.len() && (b[i + 1] as char).is_ascii_digit())
        {
            if c == '0' && i + 1 < b.len() && matches!(b[i + 1], b'x' | b'X') {
                i += 2;
                while i < b.len() && (b[i] as char).is_ascii_hexdigit() {
                    i += 1;
                }
                let v = i64::from_str_radix(&src[start + 2..i], 16)
                    .map_err(|_| err("invalid hex literal", start))?;
                out.push((Tok::Num(v as f64), start));
                continue;
            }
            while i < b.len() && ((b[i] as char).is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && matches!(b[i], b'e' | b'E') {
                i += 1;
                if i < b.len() && matches!(b[i], b'+' | b'-') {
                    i += 1;
                }
                while i < b.len() && (b[i] as char).is_ascii_digit() {
                    i += 1;
                }
            }
            let v: f64 = src[start..i]
                .parse()
                .map_err(|_| err("invalid number", start))?;
            out.push((Tok::Num(v), start));
            continue;
        }
        if c == '"' || c == '\'' {
            i += 1;
            let mut s = String::new();
            loop {
                if i >= b.len() {
                    return Err(err("unterminated string", start));
                }
                let ch = src[i..].chars().next().unwrap();
                if ch == c {
                    i += 1;
                    break;
                }
                if ch == '\\' {
                    i += 1;
                    let e = src[i..]
                        .chars()
                        .next()
                        .ok_or_else(|| err("bad escape", i))?;
                    s.push(match e {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '0' => '\0',
                        other => other,
                    });
                    i += e.len_utf8();
                    continue;
                }
                s.push(ch);
                i += ch.len_utf8();
            }
            out.push((Tok::Str(s), start));
            continue;
        }
        if c.is_alphabetic() || c == '_' || c == '$' {
            while i < b.len() {
                let ch = src[i..].chars().next().unwrap();
                if ch.is_alphanumeric() || ch == '_' || ch == '$' {
                    i += ch.len_utf8();
                } else {
                    break;
                }
            }
            out.push((Tok::Ident(src[start..i].to_string()), start));
            continue;
        }
        // A regex literal can only start where an operand is expected.
        if c == '/' && operand_expected(&out) {
            i += 1;
            while i < b.len() && b[i] != b'/' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            while i < b.len() && (b[i] as char).is_ascii_alphabetic() {
                i += 1;
            }
            out.push((Tok::Regex, start));
            continue;
        }
        let rest = &src[i..];
        let p = PUNCT
            .iter()
            .chain(PUNCT_TAIL.iter())
            .find(|p| rest.starts_with(**p))
            .ok_or_else(|| err(&format!("unexpected character `{c}`"), i))?;
        i += p.len();
        out.push((Tok::Punct(p), start));
    }
    out.push((Tok::End, src.len()));
    Ok(out)
}

fn operand_expected(toks: &[(Tok, usize)]) -> bool {
    match toks.last() {
        None => true,
        Some((Tok::Punct(p), _)) => !matches!(*p, ")" | "]" | "}"),
        _ => false,
    }
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
}

/// Binary operators by precedence, lowest first (JavaScript's table).
fn binary_precedence(op: &str) -> Option<u8> {
    Some(match op {
        "??" => 1,
        "||" => 2,
        "&&" => 3,
        "|" => 4,
        "^" => 5,
        "&" => 6,
        "==" | "!=" | "===" | "!==" => 7,
        "<" | ">" | "<=" | ">=" | "in" => 8,
        "<<" | ">>" | ">>>" => 9,
        "+" | "-" => 10,
        "*" | "/" | "%" => 11,
        "**" => 12,
        _ => return None,
    })
}

fn intern(op: &str) -> &'static str {
    const OPS: [&str; 22] = [
        "??", "||", "&&", "|", "^", "&", "==", "!=", "===", "!==", "<", ">", "<=", ">=", "in",
        "<<", ">>", ">>>", "+", "-", "*", "/",
    ];
    OPS.iter()
        .chain(["%", "**"].iter())
        .find(|o| **o == op)
        .copied()
        .unwrap_or("?")
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }
    fn offset(&self) -> usize {
        self.toks[self.pos].1
    }
    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        self.pos += 1;
        t
    }
    fn err<T>(&self, m: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            message: m.into(),
            offset: self.offset(),
        })
    }
    fn eat(&mut self, p: &str) -> bool {
        if self.peek() == &Tok::Punct(intern_punct(p)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, p: &str) -> Result<(), ParseError> {
        if self.eat(p) {
            Ok(())
        } else {
            self.err(format!("expected `{p}`, found {:?}", self.peek()))
        }
    }

    fn expression(&mut self) -> Result<Ast, ParseError> {
        let test = self.binary(0)?;
        if self.eat("?") {
            let then = self.expression()?;
            self.expect(":")?;
            let otherwise = self.expression()?;
            return Ok(Ast::Conditional {
                test: Box::new(test),
                then: Box::new(then),
                otherwise: Box::new(otherwise),
            });
        }
        Ok(test)
    }

    fn binary(&mut self, min: u8) -> Result<Ast, ParseError> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Punct(p) => *p,
                Tok::Ident(s) if s == "in" => "in",
                _ => break,
            };
            let Some(prec) = binary_precedence(op) else {
                break;
            };
            if prec <= min && !(op == "**" && prec == min) {
                break;
            }
            self.pos += 1;
            // `**` is right-associative.
            let right = self.binary(if op == "**" { prec - 1 } else { prec })?;
            let op = intern(op);
            left = if matches!(op, "&&" | "||" | "??") {
                Ast::Logical {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                }
            } else {
                Ast::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                }
            };
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Ast, ParseError> {
        for op in ["!", "-", "+", "~"] {
            if self.eat(op) {
                let arg = self.unary()?;
                return Ok(Ast::Unary {
                    op: intern_punct(op),
                    arg: Box::new(arg),
                });
            }
        }
        if let Tok::Ident(s) = self.peek() {
            if s == "typeof" || s == "void" {
                return self.err(format!("`{s}` is not part of Vega expressions"));
            }
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Ast, ParseError> {
        let mut e = self.primary()?;
        loop {
            if self.eat(".") {
                match self.next() {
                    Tok::Ident(name) => {
                        e = Ast::Member {
                            object: Box::new(e),
                            property: Box::new(Ast::String(name)),
                            computed: false,
                        }
                    }
                    t => return self.err(format!("expected a property name, found {t:?}")),
                }
            } else if self.eat("[") {
                let p = self.expression()?;
                self.expect("]")?;
                e = Ast::Member {
                    object: Box::new(e),
                    property: Box::new(p),
                    computed: true,
                };
            } else if self.peek() == &Tok::Punct("(") {
                let Ast::Ident(callee) = e else {
                    return self.err("only named functions can be called");
                };
                self.pos += 1;
                let mut args = vec![];
                if !self.eat(")") {
                    loop {
                        args.push(self.expression()?);
                        if self.eat(")") {
                            break;
                        }
                        self.expect(",")?;
                    }
                }
                e = Ast::Call { callee, args };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Ast, ParseError> {
        let offset = self.offset();
        match self.next() {
            Tok::Num(v) => Ok(Ast::Number(v)),
            Tok::Str(s) => Ok(Ast::String(s)),
            Tok::Regex => Err(ParseError {
                message: "regular expression literals are not supported".into(),
                offset,
            }),
            Tok::Ident(s) => Ok(match s.as_str() {
                "true" => Ast::Bool(true),
                "false" => Ast::Bool(false),
                "null" => Ast::Null,
                _ => Ast::Ident(s),
            }),
            Tok::Punct("(") => {
                let e = self.expression()?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Punct("[") => {
                let mut items = vec![];
                if !self.eat("]") {
                    loop {
                        items.push(self.expression()?);
                        if self.eat("]") {
                            break;
                        }
                        self.expect(",")?;
                    }
                }
                Ok(Ast::Array(items))
            }
            Tok::Punct("{") => {
                let mut fields = vec![];
                if !self.eat("}") {
                    loop {
                        let key = match self.next() {
                            Tok::Ident(s) | Tok::Str(s) => s,
                            Tok::Num(v) => v.to_string(),
                            t => return self.err(format!("expected an object key, found {t:?}")),
                        };
                        self.expect(":")?;
                        fields.push((key, self.expression()?));
                        if self.eat("}") {
                            break;
                        }
                        self.expect(",")?;
                    }
                }
                Ok(Ast::Object(fields))
            }
            t => Err(ParseError {
                message: format!("unexpected {t:?}"),
                offset,
            }),
        }
    }
}

fn intern_punct(p: &str) -> &'static str {
    PUNCT
        .iter()
        .chain(PUNCT_TAIL.iter())
        .find(|q| **q == p)
        .copied()
        .unwrap_or("?")
}

pub fn parse(src: &str) -> Result<Ast, ParseError> {
    let mut p = Parser {
        toks: tokenize(src)?,
        pos: 0,
    };
    let e = p.expression()?;
    if p.peek() != &Tok::End {
        return p.err(format!("unexpected {:?} after the expression", p.peek()));
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_members() {
        let e = parse("isValid(datum[\"a\"]) && datum.b * 2 + 1 > 3 ? 'x' : null").unwrap();
        let Ast::Conditional { test, .. } = e else {
            panic!()
        };
        assert!(matches!(*test, Ast::Logical { op: "&&", .. }));
        assert!(parse("2 ** 3 ** 2").is_ok());
        assert!(parse("{\"a\": [1, 2], b: 'c'}").is_ok());
        assert!(parse("datum.x /").is_err());
        assert!(parse("a / b / c").is_ok());
    }
}
