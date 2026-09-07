//! Arithmetic expression parser for SCE.
//!
//! Parses the arithmetic sub-language used in `calculate`/`compute`/`evaluate`:
//!   ArithExpr := Term (('+' | '-') Term)*
//!   Term      := Factor (('*' | '/' | '%') Factor)*
//!   Factor    := Unary ('^' Unary)?
//!   Unary     := '-' Unary | Atom
//!   Atom      := Number | Var | '(' ArithExpr ')'

use spoon_core::types::clause::{ArithExpr, ArithOp};

use crate::sce::tokenizer::Tok;

pub struct ArithParser<'a> {
    toks: &'a [Tok],
    pub pos: usize,
}

impl<'a> ArithParser<'a> {
    pub fn new(toks: &'a [Tok], start: usize) -> Self {
        ArithParser { toks, pos: start }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    #[allow(dead_code)]
    fn advance(&mut self) -> Option<&Tok> {
        let t = self.toks.get(self.pos)?;
        self.pos += 1;
        Some(t)
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.peek() == Some(tok) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub fn parse_expr(&mut self) -> Result<ArithExpr, String> {
        let mut lhs = self.parse_term()?;
        loop {
            let op = if self.eat(&Tok::Plus) {
                ArithOp::Add
            } else if self.eat(&Tok::Minus) {
                ArithOp::Sub
            } else {
                break;
            };
            let rhs = self.parse_term()?;
            lhs = ArithExpr::Bin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_term(&mut self) -> Result<ArithExpr, String> {
        let mut lhs = self.parse_factor()?;
        loop {
            let op = if self.eat(&Tok::Star) {
                ArithOp::Mul
            } else if self.eat(&Tok::Slash) {
                ArithOp::Div
            } else if self.eat(&Tok::Percent) {
                ArithOp::Mod
            } else {
                break;
            };
            let rhs = self.parse_factor()?;
            lhs = ArithExpr::Bin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_factor(&mut self) -> Result<ArithExpr, String> {
        let base = self.parse_unary()?;
        if self.eat(&Tok::Caret) {
            let exp = self.parse_unary()?;
            Ok(ArithExpr::Bin {
                op: ArithOp::Pow,
                lhs: Box::new(base),
                rhs: Box::new(exp),
            })
        } else {
            Ok(base)
        }
    }

    fn parse_unary(&mut self) -> Result<ArithExpr, String> {
        if self.eat(&Tok::Minus) {
            let inner = self.parse_unary()?;
            Ok(ArithExpr::Neg { of: Box::new(inner) })
        } else {
            self.parse_atom()
        }
    }

    fn parse_atom(&mut self) -> Result<ArithExpr, String> {
        match self.peek() {
            Some(Tok::Number(n)) => {
                let n = *n;
                self.pos += 1;
                Ok(ArithExpr::Num { value: n })
            }
            Some(Tok::LParen) => {
                self.pos += 1; // consume (
                let expr = self.parse_expr()?;
                if self.eat(&Tok::RParen) {
                    Ok(expr)
                } else {
                    Err("Expected closing parenthesis".to_string())
                }
            }
            Some(Tok::Word(w)) => {
                let w = w.clone();
                // Single capital letter or capital+digits: variable reference
                if is_arith_var(&w) {
                    self.pos += 1;
                    Ok(ArithExpr::Ref { var: w })
                } else {
                    Err(format!("Expected number, variable, or '(' in arithmetic expression, got '{w}'"))
                }
            }
            other => Err(format!("Expected arithmetic atom, got {:?}", other)),
        }
    }
}

fn is_arith_var(w: &str) -> bool {
    let mut chars = w.chars();
    match chars.next() {
        Some(c) if c.is_uppercase() => chars.all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

/// Render an ArithExpr back to a fully-parenthesized SCE string.
pub fn render_arith(expr: &ArithExpr) -> String {
    match expr {
        ArithExpr::Num { value } => {
            if value.fract() == 0.0 {
                format!("{}", *value as i64)
            } else {
                value.to_string()
            }
        }
        ArithExpr::Ref { var } => var.clone(),
        ArithExpr::Neg { of } => format!("-{}", render_arith(of)),
        ArithExpr::Bin { op, lhs, rhs } => {
            let op_str = match op {
                ArithOp::Add => "+",
                ArithOp::Sub => "-",
                ArithOp::Mul => "*",
                ArithOp::Div => "/",
                ArithOp::Mod => "%",
                ArithOp::Pow => "^",
            };
            format!("({} {} {})", render_arith(lhs), op_str, render_arith(rhs))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sce::tokenizer::tokenize;

    fn parse(s: &str) -> ArithExpr {
        let toks = tokenize(s);
        let mut p = ArithParser::new(&toks, 0);
        p.parse_expr().expect("parse failed")
    }

    #[test]
    fn simple_add() {
        let e = parse("3 + 5");
        assert!(matches!(e, ArithExpr::Bin { op: ArithOp::Add, .. }));
    }

    #[test]
    fn mul_div() {
        let e = parse("(3 / 500) * 3600");
        assert!(matches!(e, ArithExpr::Bin { op: ArithOp::Mul, .. }));
    }

    #[test]
    fn nested() {
        let e = parse("2 * (((599 + 32) / 0) * 6)");
        // outer should be Mul
        assert!(matches!(e, ArithExpr::Bin { op: ArithOp::Mul, .. }));
    }

    #[test]
    fn render_roundtrip() {
        let e = parse("(3 / 500) * 3600");
        let s = render_arith(&e);
        let toks2 = tokenize(&s);
        let mut p2 = ArithParser::new(&toks2, 0);
        let e2 = p2.parse_expr().unwrap();
        assert_eq!(e, e2);
    }
}
