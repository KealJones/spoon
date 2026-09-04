//! Parse Python-style function calls into concepts.
//!
//! The model outputs expressions like `do(text_reverse("banana"))` and this
//! module turns them into `Concept::call("text-reverse", [Concept::text("banana")])`.
//!
//! Only handles the subset we ask the model for: nested function calls with
//! string/number/bool literals and `lambda x:` for holes. No variables, no
//! imports, no control flow.

use std::sync::Arc;

use spoon_concept::{Concept, Ground, SymbolTable};

#[derive(Debug)]
pub enum PyParseError {
    Empty,
    Unexpected(String),
    UnterminatedString,
}

impl std::fmt::Display for PyParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PyParseError::Empty => write!(f, "empty expression"),
            PyParseError::Unexpected(s) => write!(f, "unexpected: {s}"),
            PyParseError::UnterminatedString => write!(f, "unterminated string"),
        }
    }
}

pub fn parse_pycall(input: &str, table: &SymbolTable) -> Result<Concept, PyParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(PyParseError::Empty);
    }
    let mut parser = PyParser {
        input: input.as_bytes(),
        pos: 0,
        table,
    };
    let result = parser.expression()?;
    parser.skip_ws();
    if parser.pos < parser.input.len() {
        return Err(PyParseError::Unexpected(format!(
            "trailing: {}",
            &input[parser.pos..]
        )));
    }
    Ok(result)
}

struct PyParser<'a> {
    input: &'a [u8],
    pos: usize,
    table: &'a SymbolTable,
}

impl<'a> PyParser<'a> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let b = self.input.get(self.pos).copied()?;
        self.pos += 1;
        Some(b)
    }

    fn skip_ws(&mut self) {
        while self.pos < self.input.len() && self.input[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn expression(&mut self) -> Result<Concept, PyParseError> {
        self.skip_ws();
        match self.peek() {
            Some(b'"') | Some(b'\'') => self.string_literal(),
            Some(b'-') | Some(b'0'..=b'9') => self.number(),
            Some(b'?') => self.hole(),
            Some(b'[') => self.list_literal(),
            Some(b) if b.is_ascii_alphabetic() || b == b'_' => self.name_or_call(),
            Some(b) => Err(PyParseError::Unexpected(format!("'{}'", b as char))),
            None => Err(PyParseError::Empty),
        }
    }

    fn string_literal(&mut self) -> Result<Concept, PyParseError> {
        let quote = self.advance().unwrap();
        let start = self.pos;
        let mut escaped = String::new();
        let mut has_escape = false;
        while let Some(b) = self.peek() {
            if b == b'\\' {
                if !has_escape {
                    escaped = String::from_utf8_lossy(&self.input[start..self.pos]).into_owned();
                    has_escape = true;
                }
                self.advance();
                match self.advance() {
                    Some(b'n') => escaped.push('\n'),
                    Some(b't') => escaped.push('\t'),
                    Some(b'\\') => escaped.push('\\'),
                    Some(c) => {
                        escaped.push('\\');
                        escaped.push(c as char);
                    }
                    None => return Err(PyParseError::UnterminatedString),
                }
                continue;
            }
            if b == quote {
                self.advance();
                let text = if has_escape {
                    escaped
                } else {
                    String::from_utf8_lossy(&self.input[start..self.pos - 1]).into_owned()
                };
                return Ok(Concept::text(text));
            }
            if has_escape {
                escaped.push(b as char);
            }
            self.advance();
        }
        Err(PyParseError::UnterminatedString)
    }

    fn number(&mut self) -> Result<Concept, PyParseError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.advance();
        }
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.advance();
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.advance();
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.advance();
            }
        }
        let text = std::str::from_utf8(&self.input[start..self.pos]).unwrap();
        if is_float {
            let f: f64 = text
                .parse()
                .map_err(|_| PyParseError::Unexpected(format!("bad float: {text}")))?;
            Ok(Concept::float(f))
        } else {
            let i: i64 = text
                .parse()
                .map_err(|_| PyParseError::Unexpected(format!("bad int: {text}")))?;
            Ok(Concept::int(i))
        }
    }

    fn hole(&mut self) -> Result<Concept, PyParseError> {
        self.advance(); // skip ?
        let start = self.pos;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.advance();
        }
        let text = std::str::from_utf8(&self.input[start..self.pos]).unwrap();
        let index: u32 = text.parse().unwrap_or(0);
        Ok(Concept::hole(index))
    }

    fn list_literal(&mut self) -> Result<Concept, PyParseError> {
        self.advance(); // skip [
        let items = self.arg_list(b']')?;
        Ok(Concept::call("list-list", items))
    }

    fn name_or_call(&mut self) -> Result<Concept, PyParseError> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            self.advance();
        }
        let raw_name = std::str::from_utf8(&self.input[start..self.pos]).unwrap();

        // Python uses underscores, concepts use hyphens
        let name = raw_name.replace('_', "-");

        // Check for keywords
        match name.as_str() {
            "true" | "True" => return Ok(Concept::bool(true)),
            "false" | "False" => return Ok(Concept::bool(false)),
            "None" | "none" => return Ok(Concept::text("")),
            "lambda" => return self.lambda_expr(),
            _ => {}
        }

        self.skip_ws();
        if self.peek() == Some(b'(') {
            // Function call
            self.advance(); // skip (
            let args = self.arg_list(b')')?;
            self.table.intern(&name);
            Ok(Concept::call(&name, args))
        } else {
            // Bare name - a concept reference
            self.table.intern(&name);
            Ok(Concept::named(&name))
        }
    }

    /// Parse `x: expr` after `lambda` keyword -> Concept::hole(0) in the body
    fn lambda_expr(&mut self) -> Result<Concept, PyParseError> {
        self.skip_ws();
        // Skip the parameter name(s) - we map them to ?0, ?1, etc.
        // Simple version: lambda x: body -> replace x with ?0
        let mut params = Vec::new();
        loop {
            self.skip_ws();
            let start = self.pos;
            while self
                .peek()
                .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                self.advance();
            }
            if self.pos > start {
                let param = std::str::from_utf8(&self.input[start..self.pos]).unwrap();
                params.push(param.to_string());
            }
            self.skip_ws();
            if self.peek() == Some(b',') {
                self.advance();
            } else {
                break;
            }
        }
        if self.peek() != Some(b':') {
            return Err(PyParseError::Unexpected("expected ':' in lambda".into()));
        }
        self.advance(); // skip :
        self.skip_ws();

        // Parse the body, but we need to intercept parameter names
        // and replace them with holes. For now, we'll parse the body
        // and then substitute.
        let body = self.expression()?;
        // Replace parameter names with holes
        let mut result = body;
        for (i, param) in params.iter().enumerate() {
            result = replace_name(&result, param, Concept::hole(i as u32));
        }
        Ok(result)
    }

    fn arg_list(&mut self, close: u8) -> Result<Vec<Concept>, PyParseError> {
        let mut args = Vec::new();
        self.skip_ws();
        if self.peek() == Some(close) {
            self.advance();
            return Ok(args);
        }
        loop {
            self.skip_ws();
            // Skip # comments in arg lists
            if self.peek() == Some(b'#') {
                while self.peek().is_some_and(|b| b != b'\n') {
                    self.advance();
                }
                continue;
            }
            args.push(self.expression()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.advance();
                }
                Some(b) if b == close => {
                    self.advance();
                    return Ok(args);
                }
                Some(b) => {
                    return Err(PyParseError::Unexpected(format!(
                        "expected ',' or '{}' but got '{}'",
                        close as char, b as char
                    )));
                }
                None => {
                    return Err(PyParseError::Unexpected(format!(
                        "unterminated argument list, expected '{}'",
                        close as char
                    )));
                }
            }
        }
    }
}

fn replace_name(concept: &Concept, name: &str, replacement: Concept) -> Concept {
    match concept {
        Concept::Atomic(id) => {
            if let Some(sym) = id.as_symbol() {
                let sym_name = name.replace('_', "-");
                if spoon_concept::SymbolId::of(&sym_name) == sym {
                    return replacement;
                }
            }
            concept.clone()
        }
        Concept::Compound { head, args } => {
            let new_head = replace_name(head, name, replacement.clone());
            let new_args: Vec<Concept> = args
                .iter()
                .map(|a| replace_name(a, name, replacement.clone()))
                .collect();
            Concept::apply(new_head, new_args)
        }
        Concept::Hole(_) => concept.clone(),
    }
}

/// Extract metadata lines (# comments) from the model's output.
/// Returns (metadata_lines, expression_lines).
pub fn split_metadata(output: &str) -> (Vec<String>, Vec<String>) {
    let mut meta = Vec::new();
    let mut exprs = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("```") {
            continue;
        }
        if trimmed.starts_with('#') {
            meta.push(trimmed.trim_start_matches('#').trim().to_string());
        } else {
            exprs.push(trimmed.to_string());
        }
    }
    (meta, exprs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(input: &str) -> Concept {
        let table = SymbolTable::new();
        parse_pycall(input, &table).unwrap()
    }

    #[test]
    fn simple_call() {
        let c = p("text_reverse(\"banana\")");
        assert_eq!(c.head_symbol(), Some(spoon_concept::SymbolId::of("text-reverse")));
    }

    #[test]
    fn nested_call() {
        let c = p("do(text_reverse(\"banana\"))");
        assert_eq!(c.head_symbol(), Some(spoon_concept::SymbolId::of("do")));
    }

    #[test]
    fn number_args() {
        let c = p("math_add(2, 3)");
        assert_eq!(c.head_symbol(), Some(spoon_concept::SymbolId::of("math-add")));
        assert_eq!(c.args().len(), 2);
    }

    #[test]
    fn list_literal() {
        let c = p("[1, 2, 3]");
        assert_eq!(c.head_symbol(), Some(spoon_concept::SymbolId::of("list-list")));
        assert_eq!(c.args().len(), 3);
    }

    #[test]
    fn hole_syntax() {
        let c = p("math_mul(?0, 2)");
        assert_eq!(c.args()[0], Concept::hole(0));
    }

    #[test]
    fn lambda_to_hole() {
        let c = p("list_filter([1, 2, 3], lambda x: math_gt(x, 1))");
        let pred = &c.args()[1];
        assert!(!spoon_concept::holes(pred).is_empty());
    }

    #[test]
    fn underscore_to_hyphen() {
        let c = p("text_starts_with(\"hello\", \"he\")");
        assert_eq!(
            c.head_symbol(),
            Some(spoon_concept::SymbolId::of("text-starts-with"))
        );
    }

    #[test]
    fn bare_name() {
        let c = p("do(math_add(banana, 3))");
        let inner = &c.args()[0];
        assert_eq!(inner.args()[0], Concept::named("banana"));
    }

    #[test]
    fn bool_literals() {
        let c = p("logic_if(true, 1, 2)");
        assert_eq!(c.args()[0], Concept::bool(true));
    }
}
