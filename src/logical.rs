//! The name query: several names OR'ed together, or full logical expressions.
//!
//! Without `--logical` every positional argument is one literal pattern and the
//! patterns are OR'ed. With `--logical` every argument is an expression over the
//! operators `AND`, `OR` and `NOT`, and the expressions are OR'ed as well.
//!
//! Operators are uppercase words; lowercase `and` and friends are ordinary
//! terms. Adjacent operands are an implicit AND, so `apple NOT .txt` means
//! `apple AND NOT .txt`. Parentheses nest, and a term can be quoted to keep
//! spaces, parentheses or quotes out of the parser: `"foo bar"`.

use crate::error::{Error, Result};
use crate::i18n;
use crate::matcher::Matcher;

pub struct Query {
    alternatives: Vec<Expr>,
}

enum Expr {
    Term(Matcher),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

impl Query {
    /// No `--logical`: every name is a literal pattern and they are OR'ed.
    pub fn simple(names: &[String], regex: bool) -> Result<Self> {
        let mut alternatives = Vec::with_capacity(names.len());
        for name in names {
            alternatives.push(Expr::Term(term(name, regex)?));
        }
        Ok(Query { alternatives })
    }

    /// `--logical`: every argument is an expression, and the expressions are
    /// OR'ed.
    pub fn logical(expressions: &[String], regex: bool) -> Result<Self> {
        let mut alternatives = Vec::with_capacity(expressions.len());
        for text in expressions {
            alternatives.push(parse(text, regex)?);
        }
        Ok(Query { alternatives })
    }

    pub fn is_match(&self, name: &[u8]) -> bool {
        self.alternatives.iter().any(|expr| expr.is_match(name))
    }

    /// The terms that should be highlighted in a matching name: every leaf not
    /// under an odd number of `NOT`s.
    pub fn positive_terms(&self) -> Vec<&Matcher> {
        let mut terms = Vec::new();
        for expr in &self.alternatives {
            expr.collect_terms(false, &mut terms);
        }
        terms
    }
}

fn term(text: &str, regex: bool) -> Result<Matcher> {
    if regex {
        Matcher::regex(text)
    } else {
        Ok(Matcher::substring(text))
    }
}

impl Expr {
    fn is_match(&self, name: &[u8]) -> bool {
        match self {
            Expr::Term(matcher) => matcher.is_match(name),
            Expr::Not(inner) => !inner.is_match(name),
            Expr::And(left, right) => left.is_match(name) && right.is_match(name),
            Expr::Or(left, right) => left.is_match(name) || right.is_match(name),
        }
    }

    fn collect_terms<'a>(&'a self, negated: bool, out: &mut Vec<&'a Matcher>) {
        match self {
            Expr::Term(matcher) => {
                if !negated {
                    out.push(matcher);
                }
            }
            Expr::Not(inner) => inner.collect_terms(!negated, out),
            Expr::And(left, right) | Expr::Or(left, right) => {
                left.collect_terms(negated, out);
                right.collect_terms(negated, out);
            }
        }
    }
}

enum Token {
    Term(String),
    And,
    Or,
    Not,
    Open,
    Close,
}

fn parse(text: &str, regex: bool) -> Result<Expr> {
    let tokens = tokenize(text)?;
    if tokens.is_empty() {
        return Err(Error::usage(i18n::t!(err_logical_empty)));
    }
    let mut parser = Parser {
        text,
        tokens,
        pos: 0,
        regex,
    };
    let expr = parser.parse_or()?;
    if parser.pos != parser.tokens.len() {
        return Err(parser.paren_error());
    }
    Ok(expr)
}

fn tokenize(text: &str) -> Result<Vec<Token>> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
        } else if bytes[i] == b'(' {
            tokens.push(Token::Open);
            i += 1;
        } else if bytes[i] == b')' {
            tokens.push(Token::Close);
            i += 1;
        } else if bytes[i] == b'"' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                end += 1;
            }
            if end == bytes.len() {
                return Err(Error::usage(i18n::t!(err_logical_quote, expr = text)));
            }
            tokens.push(Token::Term(text[start..end].to_owned()));
            i = end + 1;
        } else {
            let start = i;
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && !matches!(bytes[i], b'(' | b')' | b'"')
            {
                i += 1;
            }
            let word = &text[start..i];
            tokens.push(match word {
                "AND" => Token::And,
                "OR" => Token::Or,
                "NOT" => Token::Not,
                _ => Token::Term(word.to_owned()),
            });
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    regex: bool,
}

impl Parser<'_> {
    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while self.eat_or() {
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_unary()?;
        loop {
            if self.eat_and() || self.starts_operand() {
                let right = self.parse_unary()?;
                left = Expr::And(Box::new(left), Box::new(right));
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if self.eat_not() {
            return Ok(Expr::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.tokens.get(self.pos) {
            Some(Token::Term(text)) => {
                let text = text.clone();
                self.pos += 1;
                Ok(Expr::Term(term(&text, self.regex)?))
            }
            Some(Token::Open) => {
                self.pos += 1;
                let expr = self.parse_or()?;
                if !self.eat_close() {
                    return Err(self.paren_error());
                }
                Ok(expr)
            }
            Some(Token::Close) => Err(self.paren_error()),
            _ => Err(self.operand_error()),
        }
    }

    fn starts_operand(&self) -> bool {
        matches!(
            self.tokens.get(self.pos),
            Some(Token::Term(_) | Token::Open | Token::Not)
        )
    }

    fn eat_and(&mut self) -> bool {
        self.eat(|token| matches!(token, Token::And))
    }

    fn eat_or(&mut self) -> bool {
        self.eat(|token| matches!(token, Token::Or))
    }

    fn eat_not(&mut self) -> bool {
        self.eat(|token| matches!(token, Token::Not))
    }

    fn eat_close(&mut self) -> bool {
        self.eat(|token| matches!(token, Token::Close))
    }

    fn eat(&mut self, wanted: impl Fn(&Token) -> bool) -> bool {
        match self.tokens.get(self.pos) {
            Some(token) if wanted(token) => {
                self.pos += 1;
                true
            }
            _ => false,
        }
    }

    fn operand_error(&self) -> Error {
        Error::usage(i18n::t!(err_logical_operand, expr = self.text))
    }

    fn paren_error(&self) -> Error {
        Error::usage(i18n::t!(err_logical_paren, expr = self.text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(query: &Query, names: &[&str]) -> Vec<String> {
        names
            .iter()
            .filter(|name| query.is_match(name.as_bytes()))
            .map(|name| (*name).to_owned())
            .collect()
    }

    fn simple(patterns: &[&str]) -> Query {
        let patterns: Vec<String> = patterns.iter().map(|p| (*p).to_owned()).collect();
        Query::simple(&patterns, false).unwrap()
    }

    fn logical(expressions: &[&str]) -> Query {
        let expressions: Vec<String> = expressions.iter().map(|e| (*e).to_owned()).collect();
        Query::logical(&expressions, false).unwrap()
    }

    #[test]
    fn names_are_ored_by_default() {
        let query = simple(&["foo", "bar"]);
        assert_eq!(
            names(&query, &["foo", "xbar", "baz", "FOO"]),
            ["foo", "xbar", "FOO"]
        );
    }

    #[test]
    fn the_example_expressions_work() {
        let query = logical(&["example AND ( .mp4 OR .mp3 )", "apple NOT .txt"]);
        assert_eq!(
            names(
                &query,
                &[
                    "example.mp4",
                    "example.mp3",
                    "example.txt",
                    "apple.mp4",
                    "apple.txt",
                    "banana.mp4",
                ]
            ),
            ["example.mp4", "example.mp3", "apple.mp4"]
        );
    }

    #[test]
    fn adjacent_operands_are_an_implicit_and() {
        let query = logical(&["foo bar"]);
        assert_eq!(
            names(&query, &["foo bar", "foo", "bar", "foobar"]),
            ["foo bar", "foobar"]
        );
    }

    #[test]
    fn and_binds_tighter_than_or() {
        let query = logical(&["a OR b AND c"]);
        assert_eq!(
            names(&query, &["a", "b", "c", "bc", "bXc"]),
            ["a", "bc", "bXc"]
        );
    }

    #[test]
    fn parens_nest_and_not_is_unary() {
        let query = logical(&["( a OR ( b AND NOT c ) )"]);
        assert_eq!(names(&query, &["a", "b", "bc", "c"]), ["a", "b"]);
    }

    #[test]
    fn quoted_terms_keep_spaces_and_parens() {
        let query = logical(&["\"foo bar\" OR \"(x)\""]);
        assert_eq!(
            names(&query, &["foo bar", "(x)", "foo", "bar"]),
            ["foo bar", "(x)"]
        );
    }

    #[test]
    fn regex_terms_are_compiled_per_term() {
        let expressions = vec!["\"^(mp4|mp3)$\"".to_owned()];
        let query = Query::logical(&expressions, true).unwrap();
        assert_eq!(
            names(&query, &["mp4", "mp3", "MP4", "mp4x"]),
            ["mp4", "mp3", "MP4"]
        );
    }

    #[test]
    fn bad_expressions_are_reported() {
        for bad in ["", "a AND", "( a OR b", "a )", "NOT", "\"unterminated"] {
            let expressions = vec![bad.to_owned()];
            assert!(Query::logical(&expressions, false).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn only_positive_terms_are_highlighted() {
        let query = logical(&["apple NOT .txt", "NOT NOT banana"]);
        let terms = query.positive_terms();
        assert!(terms.iter().any(|term| term.is_match(b"apple")));
        assert!(terms.iter().any(|term| term.is_match(b"banana")));
        assert!(!terms.iter().any(|term| term.is_match(b".txt")));
    }
}
