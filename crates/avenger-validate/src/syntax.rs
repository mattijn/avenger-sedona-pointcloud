//! The pipeline text into steps: `read x.laz ! sql "…" ! chart bar --x a:N`.
//! Words split at whitespace, quotes group and are removed, a backslash in
//! quotes escapes, and an unquoted `!` separates steps. Every step and value
//! keeps its byte range in the text, so an error can point at it.

use std::collections::BTreeMap;

use serde::Serialize;

/// Byte offsets `[start, end)` in the pipeline text.
pub type Span = (usize, usize);

/// One step as written: `name positional… --flag value`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Call {
    pub name: String,
    pub args: Vec<String>,
    pub flags: BTreeMap<String, String>,
    /// The whole step, from its name to its last word.
    pub span: Span,
    pub arg_spans: Vec<Span>,
    pub flag_spans: BTreeMap<String, Span>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SyntaxError {
    pub message: String,
    pub offset: usize,
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SyntaxError {}

struct Word {
    text: String,
    quoted: bool,
    span: Span,
}

/// Split a pipeline into steps at unquoted `!`, honouring quotes.
pub fn parse(src: &str) -> Result<Vec<Call>, SyntaxError> {
    let mut words: Vec<Word> = vec![];
    let mut cur = String::new();
    let (mut quoted, mut in_quote, mut start, mut quote_at) = (false, None::<char>, None::<usize>, 0);
    let mut chars = src.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match (in_quote, c) {
            (Some(q), c) if c == q => in_quote = None,
            (Some(_), '\\') => {
                if let Some((_, n)) = chars.next() {
                    cur.push(n)
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                in_quote = Some(c);
                quoted = true;
                quote_at = i;
                start.get_or_insert(i);
            }
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() || quoted {
                    words.push(Word { text: std::mem::take(&mut cur), quoted, span: (start.unwrap_or(i), i) });
                }
                quoted = false;
                start = None;
            }
            (None, c) => {
                cur.push(c);
                start.get_or_insert(i);
            }
        }
    }
    if in_quote.is_some() {
        return Err(SyntaxError { message: "unterminated quote in pipeline".into(), offset: quote_at });
    }
    if !cur.is_empty() || quoted {
        words.push(Word { text: cur, quoted, span: (start.unwrap_or(src.len()), src.len()) });
    }
    let mut calls = vec![];
    for group in words.split(|w| w.text == "!" && !w.quoted) {
        let mut it = group.iter().peekable();
        let Some(name) = it.next() else { continue };
        let mut call = Call { name: name.text.clone(), span: name.span, ..Default::default() };
        while let Some(w) = it.next() {
            call.span.1 = w.span.1;
            if let (Some(k), false) = (w.text.strip_prefix("--"), w.quoted) {
                let (v, span) = match it.peek() {
                    Some(v) if v.quoted || !v.text.starts_with("--") => {
                        let v = it.next().unwrap();
                        call.span.1 = v.span.1;
                        (v.text.clone(), v.span)
                    }
                    _ => ("true".into(), w.span),
                };
                call.flags.insert(k.to_string(), v);
                call.flag_spans.insert(k.to_string(), span);
            } else {
                call.args.push(w.text.clone());
                call.arg_spans.push(w.span);
            }
        }
        calls.push(call);
    }
    Ok(calls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_flags_and_spans() {
        let src = r#"read a.laz --statistics ! sql "SELECT \"x\" FROM input" ! chart bar --x label:N"#;
        let c = parse(src).unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].flags["statistics"], "true");
        assert_eq!(c[1].args[0], r#"SELECT "x" FROM input"#);
        assert_eq!(&src[c[2].flag_spans["x"].0..c[2].flag_spans["x"].1], "label:N");
        assert_eq!(&src[c[2].span.0..c[2].span.1], "chart bar --x label:N");
    }

    #[test]
    fn a_quoted_bang_is_a_word() {
        let c = parse(r#"title "hi ! there""#).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].args[0], "hi ! there");
    }

    #[test]
    fn unterminated_quote() {
        let e = parse(r#"title "oops"#).unwrap_err();
        assert_eq!((e.message.as_str(), e.offset), ("unterminated quote in pipeline", 6));
    }
}
