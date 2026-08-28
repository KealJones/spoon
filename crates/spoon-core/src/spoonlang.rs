//! Spoonlang: a small infix surface language that compiles to `pure_expr_v2` JSON.

use serde_json::{json, Value as JsonValue};
use thiserror::Error;

pub const MAX_SPOONLANG_BYTES: usize = 64 * 1024;

pub const SPOONLANG_GRAMMAR: &str = r#"Spoonlang. Put the entire proposal in JSON field "source". interpretations is [] unless a known graph concept applies. Do not copy examples. Author THIS situation. Do not put tagged IR JSON in source.

kind reusable_lesson | answer_only | external_observation | abstain
kind names: reusable_lesson teaches an executable procedure; answer_only is a one-shot fact or phrase; external_observation is an unverified world report; abstain is "I cannot teach this".

reusable_lesson:
  kind reusable_lesson
  concept <key>: definitional|defeasible_general|procedural
    "<description>"
  proc <key>(<param>: any|null|bool|number|text|list|map, ...)
    name "<display name>"
    <expr>
  example <proc_key>(<literal>, ...) => <literal>
  rel <src_key> <kind> <tgt_key> <strength>

answer_only / external_observation:
  kind answer_only
  answer <literal>

abstain:
  kind abstain
  reason "<text>"

Format example (do not reuse unless the user asked about doubling):
kind reusable_lesson
concept double: procedural
  "Twice a number"
proc double(x: number)
  x * 2
example double(7) => 14

Format example for a stable fact (do not reuse unless the situation is that fact):
kind answer_only
answer 2

expr:
  1, 1.5, "text", true, false, null, result
  params are bare identifiers
  + - * / %   == != < <= > >=   && || !
  if <c> then <t> else <e>
  let <x> = <v> in <body>
  [a, b]    { k: v }    obj.field    arr[i]
  path_get(data, path) and other snake_case intrinsics
  cap("<contentId>", "<procedureId>", <input>)
  dep("<alias>", args...)
  map <xs> <x> => <body>
  filter <xs> <x> => <pred>
  reduce <xs> <acc> = <init>, <x> => <body>
  comments: ; or // to end of line
"#;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpoonlangError {
    #[error("spoonlang parse error at byte {offset}: {message}")]
    Parse { offset: usize, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpoonlangKind {
    ReusableLesson,
    ExternalObservation,
    AnswerOnly,
    Abstain,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedProposal {
    pub kind: SpoonlangKind,
    pub lesson: Option<JsonValue>,
    pub answer: Option<JsonValue>,
    pub abstain_reason: Option<String>,
}

pub fn parse_expr(source: &str) -> Result<JsonValue, SpoonlangError> {
    let mut parser = Parser::new(source)?;
    let expr = parser.parse_expression(0)?;
    parser.expect_eof()?;
    Ok(expr)
}

pub fn parse_proposal(source: &str) -> Result<ParsedProposal, SpoonlangError> {
    let mut parser = Parser::new(source)?;
    let parsed = parser.parse_proposal()?;
    parser.expect_eof()?;
    Ok(parsed)
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    String(String),
    Int(i64),
    Float(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Bang,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Dot,
    Eq,
    Arrow,
    Eof,
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<(Token, usize)>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Result<Self, SpoonlangError> {
        if source.len() > MAX_SPOONLANG_BYTES {
            return Err(SpoonlangError::Parse {
                offset: 0,
                message: format!("source exceeds {MAX_SPOONLANG_BYTES} bytes"),
            });
        }
        let tokens = tokenize(source)?;
        Ok(Self {
            source,
            tokens,
            pos: 0,
        })
    }

    fn offset(&self) -> usize {
        self.tokens
            .get(self.pos)
            .map(|(_, offset)| *offset)
            .unwrap_or(self.source.len())
    }

    fn error(&self, message: impl Into<String>) -> SpoonlangError {
        SpoonlangError::Parse {
            offset: self.offset(),
            message: message.into(),
        }
    }

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .map(|(token, _)| token)
            .unwrap_or(&Token::Eof)
    }

    fn peek_nth(&self, n: usize) -> &Token {
        self.tokens
            .get(self.pos + n)
            .map(|(token, _)| token)
            .unwrap_or(&Token::Eof)
    }

    fn bump(&mut self) -> Token {
        if self.pos >= self.tokens.len() {
            return Token::Eof;
        }
        let token = self.tokens[self.pos].0.clone();
        self.pos += 1;
        token
    }

    fn ident_named(&self, name: &str) -> bool {
        matches!(self.peek(), Token::Ident(value) if value == name)
    }

    fn expect_ident(&mut self) -> Result<String, SpoonlangError> {
        match self.bump() {
            Token::Ident(name) => Ok(name),
            other => Err(self.error(format!("expected identifier, got {other:?}"))),
        }
    }

    fn expect_keyword(&mut self, keyword: &str) -> Result<(), SpoonlangError> {
        match self.bump() {
            Token::Ident(name) if name == keyword => Ok(()),
            other => Err(self.error(format!("expected {keyword}, got {other:?}"))),
        }
    }

    fn expect_string(&mut self) -> Result<String, SpoonlangError> {
        match self.bump() {
            Token::String(value) => Ok(value),
            other => Err(self.error(format!("expected string, got {other:?}"))),
        }
    }

    fn expect_token(&mut self, expected: Token) -> Result<(), SpoonlangError> {
        let got = self.bump();
        if got == expected {
            Ok(())
        } else {
            Err(self.error(format!("expected {expected:?}, got {got:?}")))
        }
    }

    fn expect_eof(&self) -> Result<(), SpoonlangError> {
        match self.peek() {
            Token::Eof => Ok(()),
            other => Err(self.error(format!("unexpected token {other:?}"))),
        }
    }

    fn parse_proposal(&mut self) -> Result<ParsedProposal, SpoonlangError> {
        let kind = if self.ident_named("kind") {
            self.bump();
            match self.expect_ident()?.as_str() {
                "reusable_lesson" => SpoonlangKind::ReusableLesson,
                "external_observation" => SpoonlangKind::ExternalObservation,
                "answer_only" => SpoonlangKind::AnswerOnly,
                "abstain" => SpoonlangKind::Abstain,
                other => {
                    return Err(self.error(format!("unknown proposal kind {other}")));
                }
            }
        } else {
            SpoonlangKind::ReusableLesson
        };

        match kind {
            SpoonlangKind::AnswerOnly | SpoonlangKind::ExternalObservation => {
                self.expect_keyword("answer")?;
                let answer = self.parse_literal_value()?;
                Ok(ParsedProposal {
                    kind,
                    lesson: None,
                    answer: Some(answer),
                    abstain_reason: None,
                })
            }
            SpoonlangKind::Abstain => {
                self.expect_keyword("reason")?;
                let reason = self.expect_string()?;
                Ok(ParsedProposal {
                    kind,
                    lesson: None,
                    answer: None,
                    abstain_reason: Some(reason),
                })
            }
            SpoonlangKind::ReusableLesson => {
                let mut parsed = self.parse_reusable_lesson()?;
                finish_invocation_names(&mut parsed);
                Ok(parsed)
            }
        }
    }

    fn parse_reusable_lesson(&mut self) -> Result<ParsedProposal, SpoonlangError> {
        let mut concepts = Vec::new();
        let mut relationships = Vec::new();
        let mut procedures = Vec::new();
        let mut invocation = None;
        let mut answer = None;

        while !matches!(self.peek(), Token::Eof) {
            if self.ident_named("concept") {
                concepts.push(self.parse_concept()?);
            } else if self.ident_named("proc") {
                procedures.push(self.parse_procedure(&concepts)?);
            } else if self.ident_named("example") {
                let (inv, value) = self.parse_example()?;
                invocation = Some(inv);
                answer = Some(value);
            } else if self.ident_named("rel") {
                relationships.push(self.parse_relationship()?);
            } else if self.ident_named("answer") {
                self.bump();
                answer = Some(self.parse_literal_value()?);
            } else if self.ident_named("kind") {
                return Err(self.error("unexpected second kind in spoonlang proposal"));
            } else {
                return Err(self.error(format!("unexpected token {:?}", self.peek())));
            }
        }

        if concepts.is_empty() {
            if let Some(procedure) = procedures.first() {
                let key = procedure["key"].as_str().unwrap_or("concept").to_string();
                concepts.push(json!({
                    "key": key,
                    "name": key,
                    "description": key,
                    "mutability": "procedural",
                }));
            } else {
                return Err(self.error("reusable_lesson needs a concept or procedure"));
            }
        }
        if procedures.is_empty() {
            return Err(self.error("reusable_lesson needs a procedure"));
        }
        let Some(invocation) = invocation else {
            return Err(self.error("reusable_lesson needs an example invocation"));
        };

        Ok(ParsedProposal {
            kind: SpoonlangKind::ReusableLesson,
            lesson: Some(json!({
                "primitiveSet": "pure_expr_v2",
                "concepts": concepts,
                "relationships": relationships,
                "procedures": procedures,
                "invocation": invocation,
            })),
            answer,
            abstain_reason: None,
        })
    }

    fn parse_concept(&mut self) -> Result<JsonValue, SpoonlangError> {
        self.expect_keyword("concept")?;
        let key = self.expect_ident()?;
        self.expect_token(Token::Colon)?;
        let mutability = self.expect_ident()?;
        if !matches!(
            mutability.as_str(),
            "definitional" | "defeasible_general" | "procedural"
        ) {
            return Err(self.error(format!("unknown mutability {mutability}")));
        }
        let description = self.expect_string()?;
        Ok(json!({
            "key": key,
            "name": key,
            "description": description,
            "mutability": mutability,
        }))
    }

    fn parse_procedure(&mut self, concepts: &[JsonValue]) -> Result<JsonValue, SpoonlangError> {
        self.expect_keyword("proc")?;
        let (key, mut name) = match self.bump() {
            Token::Ident(key) => (key.clone(), key),
            Token::String(display) => (slugify(&display), display),
            other => return Err(self.error(format!("expected procedure name, got {other:?}"))),
        };
        self.expect_token(Token::LParen)?;
        let mut parameters = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                let param_name = self.expect_ident()?;
                self.expect_token(Token::Colon)?;
                let value_type = self.expect_ident()?;
                if !matches!(
                    value_type.as_str(),
                    "any" | "null" | "bool" | "number" | "text" | "list" | "map"
                ) {
                    return Err(self.error(format!("unknown parameter type {value_type}")));
                }
                parameters.push(json!({
                    "name": param_name,
                    "description": param_name,
                    "valueType": value_type,
                }));
                if matches!(self.peek(), Token::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect_token(Token::RParen)?;
        if self.ident_named("name") {
            self.bump();
            name = self.expect_string()?;
        }
        let concept_key = concepts
            .first()
            .and_then(|concept| concept["key"].as_str())
            .unwrap_or(&key)
            .to_string();
        let body = self.parse_expression(0)?;
        Ok(json!({
            "key": key,
            "name": name,
            "concept": { "kind": "new_concept", "key": concept_key },
            "parameters": parameters,
            "body": body,
            "contract": {
                "requires": [],
                "promises": [],
                "failsWhen": [],
            },
        }))
    }

    fn parse_example(&mut self) -> Result<(JsonValue, JsonValue), SpoonlangError> {
        self.expect_keyword("example")?;
        let procedure_key = self.expect_ident()?;
        self.expect_token(Token::LParen)?;
        let mut inputs = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                let value = self.parse_literal_value()?;
                inputs.push(value);
                if matches!(self.peek(), Token::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect_token(Token::RParen)?;
        self.expect_token(Token::Arrow)?;
        let answer = self.parse_literal_value()?;
        Ok((
            json!({
                "procedureKey": procedure_key,
                "inputs": named_inputs_for(&procedure_key, &inputs),
            }),
            answer,
        ))
    }

    fn parse_relationship(&mut self) -> Result<JsonValue, SpoonlangError> {
        self.expect_keyword("rel")?;
        let source = self.expect_ident()?;
        let kind = self.expect_ident()?;
        let target = self.expect_ident()?;
        let strength = match self.peek() {
            Token::Int(_) | Token::Float(_) => number_token(self.bump()),
            _ => json!(0.5),
        };
        Ok(json!({
            "source": { "kind": "new_concept", "key": source },
            "target": { "kind": "new_concept", "key": target },
            "kind": kind,
            "strength": strength,
        }))
    }

    fn parse_literal_value(&mut self) -> Result<JsonValue, SpoonlangError> {
        expr_to_literal(&self.parse_expression(0)?)
            .map_err(|message| self.error(message))
    }

    fn parse_expression(&mut self, min_bp: u8) -> Result<JsonValue, SpoonlangError> {
        let mut lhs = self.parse_prefix()?;
        loop {
            if let Some(postfix_bp) = postfix_binding_power(self.peek()) {
                if postfix_bp < min_bp {
                    break;
                }
                lhs = self.parse_postfix(lhs)?;
                continue;
            }
            let Some((l_bp, r_bp, op)) = infix_binding_power(self.peek()) else {
                break;
            };
            if l_bp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.parse_expression(r_bp)?;
            lhs = json!({
                "kind": "binary",
                "op": op,
                "left": lhs,
                "right": rhs,
            });
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<JsonValue, SpoonlangError> {
        match self.bump() {
            Token::Int(value) => Ok(literal(json!(value))),
            Token::Float(value) => Ok(literal(json!(value))),
            Token::String(value) => Ok(literal(json!(value))),
            Token::Minus => {
                let operand = self.parse_expression(prefix_binding_power())?;
                Ok(json!({
                    "kind": "unary",
                    "op": "negate",
                    "operand": operand,
                }))
            }
            Token::Bang => {
                let operand = self.parse_expression(prefix_binding_power())?;
                Ok(json!({
                    "kind": "unary",
                    "op": "not",
                    "operand": operand,
                }))
            }
            Token::LParen => {
                let expr = self.parse_expression(0)?;
                self.expect_token(Token::RParen)?;
                Ok(expr)
            }
            Token::LBracket => self.parse_list(),
            Token::LBrace => self.parse_brace(),
            Token::Ident(name) => self.parse_ident_prefix(name),
            other => Err(self.error(format!("unexpected token {other:?}"))),
        }
    }

    fn parse_ident_prefix(&mut self, name: String) -> Result<JsonValue, SpoonlangError> {
        match name.as_str() {
            "true" => Ok(literal(json!(true))),
            "false" => Ok(literal(json!(false))),
            "null" => Ok(literal(json!(null))),
            "result" => Ok(json!({ "kind": "result" })),
            "if" => self.parse_if(),
            "let" => self.parse_let(),
            "map" => self.parse_map_binder(),
            "filter" => self.parse_filter_binder(),
            "reduce" => self.parse_reduce_binder(),
            "cap" => self.parse_cap(),
            "dep" => self.parse_dep(),
            _ if matches!(self.peek(), Token::LParen) => self.parse_call(name),
            _ => Ok(json!({ "kind": "parameter", "name": name })),
        }
    }

    fn parse_if(&mut self) -> Result<JsonValue, SpoonlangError> {
        let condition = self.parse_expression(0)?;
        self.expect_keyword("then")?;
        let then_expr = self.parse_expression(0)?;
        self.expect_keyword("else")?;
        let else_expr = self.parse_expression(0)?;
        Ok(json!({
            "kind": "if",
            "condition": condition,
            "then": then_expr,
            "else": else_expr,
        }))
    }

    fn parse_let(&mut self) -> Result<JsonValue, SpoonlangError> {
        let name = self.expect_ident()?;
        self.expect_token(Token::Eq)?;
        let value = self.parse_expression(0)?;
        self.expect_keyword("in")?;
        let body = self.parse_expression(0)?;
        Ok(json!({
            "kind": "let",
            "name": name,
            "value": value,
            "body": body,
        }))
    }

    fn parse_map_binder(&mut self) -> Result<JsonValue, SpoonlangError> {
        let collection = self.parse_expression(prefix_binding_power())?;
        let var = self.expect_ident()?;
        self.expect_token(Token::Arrow)?;
        let body = self.parse_expression(0)?;
        Ok(json!({
            "kind": "map",
            "collection": collection,
            "var": var,
            "body": body,
        }))
    }

    fn parse_filter_binder(&mut self) -> Result<JsonValue, SpoonlangError> {
        let collection = self.parse_expression(prefix_binding_power())?;
        let var = self.expect_ident()?;
        self.expect_token(Token::Arrow)?;
        let predicate = self.parse_expression(0)?;
        Ok(json!({
            "kind": "filter",
            "collection": collection,
            "var": var,
            "predicate": predicate,
        }))
    }

    fn parse_reduce_binder(&mut self) -> Result<JsonValue, SpoonlangError> {
        let collection = self.parse_expression(prefix_binding_power())?;
        let acc = self.expect_ident()?;
        self.expect_token(Token::Eq)?;
        let init = self.parse_expression(0)?;
        self.expect_token(Token::Comma)?;
        let var = self.expect_ident()?;
        self.expect_token(Token::Arrow)?;
        let body = self.parse_expression(0)?;
        Ok(json!({
            "kind": "reduce",
            "collection": collection,
            "init": init,
            "acc": acc,
            "var": var,
            "body": body,
        }))
    }

    fn parse_cap(&mut self) -> Result<JsonValue, SpoonlangError> {
        self.expect_token(Token::LParen)?;
        let content_id = self.expect_string()?;
        self.expect_token(Token::Comma)?;
        let procedure_id = self.expect_string()?;
        self.expect_token(Token::Comma)?;
        let input = self.parse_expression(0)?;
        self.expect_token(Token::RParen)?;
        Ok(json!({
            "kind": "capability_call",
            "contentId": content_id,
            "procedureId": procedure_id,
            "input": input,
        }))
    }

    fn parse_dep(&mut self) -> Result<JsonValue, SpoonlangError> {
        self.expect_token(Token::LParen)?;
        let alias = self.expect_string()?;
        let mut args = Vec::new();
        while matches!(self.peek(), Token::Comma) {
            self.bump();
            args.push(self.parse_expression(0)?);
        }
        self.expect_token(Token::RParen)?;
        Ok(json!({
            "kind": "dependency",
            "alias": alias,
            "args": args,
        }))
    }

    fn parse_call(&mut self, op: String) -> Result<JsonValue, SpoonlangError> {
        self.expect_token(Token::LParen)?;
        let mut args = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                args.push(self.parse_expression(0)?);
                if matches!(self.peek(), Token::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect_token(Token::RParen)?;
        Ok(json!({
            "kind": "intrinsic",
            "version": 1,
            "op": op,
            "args": args,
        }))
    }

    fn parse_list(&mut self) -> Result<JsonValue, SpoonlangError> {
        let mut items = Vec::new();
        if !matches!(self.peek(), Token::RBracket) {
            loop {
                items.push(self.parse_expression(0)?);
                if matches!(self.peek(), Token::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect_token(Token::RBracket)?;
        Ok(json!({ "kind": "list", "items": items }))
    }

    fn parse_brace(&mut self) -> Result<JsonValue, SpoonlangError> {
        if matches!(self.peek(), Token::RBrace) {
            self.bump();
            return Ok(literal(json!({})));
        }
        if self.looks_like_map() {
            return self.parse_map_literal();
        }
        let expr = self.parse_expression(0)?;
        self.expect_token(Token::RBrace)?;
        Ok(expr)
    }

    fn looks_like_map(&self) -> bool {
        match self.peek() {
            Token::String(_) => matches!(self.peek_nth(1), Token::Colon),
            Token::Ident(_) => matches!(self.peek_nth(1), Token::Colon),
            _ => false,
        }
    }

    fn parse_map_literal(&mut self) -> Result<JsonValue, SpoonlangError> {
        let mut entries = Vec::new();
        loop {
            let key = match self.bump() {
                Token::Ident(key) | Token::String(key) => key,
                other => return Err(self.error(format!("expected map key, got {other:?}"))),
            };
            self.expect_token(Token::Colon)?;
            let value = self.parse_expression(0)?;
            entries.push((key, value));
            if matches!(self.peek(), Token::Comma) {
                self.bump();
                if matches!(self.peek(), Token::RBrace) {
                    break;
                }
                continue;
            }
            break;
        }
        self.expect_token(Token::RBrace)?;
        let mut expr = literal(json!({}));
        for (key, value) in entries {
            expr = json!({
                "kind": "intrinsic",
                "version": 1,
                "op": "map_set",
                "args": [expr, literal(json!(key)), value],
            });
        }
        Ok(expr)
    }

    fn parse_postfix(&mut self, lhs: JsonValue) -> Result<JsonValue, SpoonlangError> {
        match self.bump() {
            Token::Dot => {
                let field = self.expect_ident()?;
                Ok(json!({
                    "kind": "field",
                    "object": lhs,
                    "field": field,
                }))
            }
            Token::LBracket => {
                let index = self.parse_expression(0)?;
                self.expect_token(Token::RBracket)?;
                Ok(json!({
                    "kind": "index",
                    "collection": lhs,
                    "index": index,
                }))
            }
            other => Err(self.error(format!("unexpected postfix {other:?}"))),
        }
    }
}

fn named_inputs_for(procedure_key: &str, values: &[JsonValue]) -> JsonValue {
    let _ = procedure_key;
    JsonValue::Array(
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                json!({
                    "name": placeholder_input_name(index),
                    "value": value,
                })
            })
            .collect(),
    )
}

fn placeholder_input_name(index: usize) -> String {
    // Filled with real parameter names after the procedure is parsed.
    format!("${index}")
}

fn finish_invocation_names(proposal: &mut ParsedProposal) {
    let Some(lesson) = proposal.lesson.as_mut() else {
        return;
    };
    let Some(procedures) = lesson.get("procedures").and_then(JsonValue::as_array) else {
        return;
    };
    let Some(invocation) = lesson.get("invocation") else {
        return;
    };
    let Some(procedure_key) = invocation.get("procedureKey").and_then(JsonValue::as_str) else {
        return;
    };
    let Some(procedure) = procedures.iter().find(|proc| proc["key"] == procedure_key) else {
        return;
    };
    let Some(parameters) = procedure.get("parameters").and_then(JsonValue::as_array) else {
        return;
    };
    let Some(inputs) = invocation.get("inputs").and_then(JsonValue::as_array) else {
        return;
    };
    let named: Vec<JsonValue> = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| {
            let name = parameters
                .get(index)
                .and_then(|parameter| parameter["name"].as_str())
                .unwrap_or(input["name"].as_str().unwrap_or("value"));
            json!({ "name": name, "value": input["value"] })
        })
        .collect();
    lesson["invocation"]["inputs"] = JsonValue::Array(named);
}

fn literal(value: JsonValue) -> JsonValue {
    json!({ "kind": "literal", "value": value })
}

fn number_token(token: Token) -> JsonValue {
    match token {
        Token::Int(value) => json!(value),
        Token::Float(value) => json!(value),
        _ => json!(null),
    }
}

fn expr_to_literal(expr: &JsonValue) -> Result<JsonValue, String> {
    match expr.get("kind").and_then(JsonValue::as_str) {
        Some("literal") => Ok(expr.get("value").cloned().unwrap_or(JsonValue::Null)),
        Some("list") => {
            let items = expr
                .get("items")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| "list literal is malformed".to_string())?;
            let values = items
                .iter()
                .map(expr_to_literal)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(JsonValue::Array(values))
        }
        _ => Err("example/answer must be a literal value".into()),
    }
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut prev_underscore = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_underscore = false;
        } else if !prev_underscore && !slug.is_empty() {
            slug.push('_');
            prev_underscore = true;
        }
    }
    slug.trim_matches('_').to_string()
}

fn prefix_binding_power() -> u8 {
    15
}

fn postfix_binding_power(token: &Token) -> Option<u8> {
    match token {
        Token::Dot | Token::LBracket => Some(18),
        _ => None,
    }
}

fn infix_binding_power(token: &Token) -> Option<(u8, u8, &'static str)> {
    match token {
        Token::OrOr => Some((1, 2, "or")),
        Token::AndAnd => Some((3, 4, "and")),
        Token::EqEq => Some((5, 6, "equal")),
        Token::NotEq => Some((5, 6, "not_equal")),
        Token::Lt => Some((7, 8, "less_than")),
        Token::Le => Some((7, 8, "less_or_equal")),
        Token::Gt => Some((7, 8, "greater_than")),
        Token::Ge => Some((7, 8, "greater_or_equal")),
        Token::Plus => Some((9, 10, "add")),
        Token::Minus => Some((9, 10, "subtract")),
        Token::Star => Some((11, 12, "multiply")),
        Token::Slash => Some((11, 12, "divide")),
        Token::Percent => Some((11, 12, "modulo")),
        _ => None,
    }
}

fn tokenize(source: &str) -> Result<Vec<(Token, usize)>, SpoonlangError> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let ch = bytes[i] as char;
        if ch.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if ch == ';' || (ch == '/' && bytes.get(i + 1) == Some(&b'/')) {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let token = match ch {
            '+' => {
                i += 1;
                Token::Plus
            }
            '-' => {
                i += 1;
                Token::Minus
            }
            '*' => {
                i += 1;
                Token::Star
            }
            '/' => {
                i += 1;
                Token::Slash
            }
            '%' => {
                i += 1;
                Token::Percent
            }
            '(' => {
                i += 1;
                Token::LParen
            }
            ')' => {
                i += 1;
                Token::RParen
            }
            '[' => {
                i += 1;
                Token::LBracket
            }
            ']' => {
                i += 1;
                Token::RBracket
            }
            '{' => {
                i += 1;
                Token::LBrace
            }
            '}' => {
                i += 1;
                Token::RBrace
            }
            ',' => {
                i += 1;
                Token::Comma
            }
            ':' => {
                i += 1;
                Token::Colon
            }
            '.' => {
                i += 1;
                Token::Dot
            }
            '!' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                Token::NotEq
            }
            '!' => {
                i += 1;
                Token::Bang
            }
            '=' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                Token::EqEq
            }
            '=' if bytes.get(i + 1) == Some(&b'>') => {
                i += 2;
                Token::Arrow
            }
            '=' => {
                i += 1;
                Token::Eq
            }
            '<' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                Token::Le
            }
            '<' => {
                i += 1;
                Token::Lt
            }
            '>' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                Token::Ge
            }
            '>' => {
                i += 1;
                Token::Gt
            }
            '&' if bytes.get(i + 1) == Some(&b'&') => {
                i += 2;
                Token::AndAnd
            }
            '|' if bytes.get(i + 1) == Some(&b'|') => {
                i += 2;
                Token::OrOr
            }
            '"' => {
                let (value, next) = lex_string(source, i)?;
                i = next;
                Token::String(value)
            }
            '0'..='9' => {
                let (token, next) = lex_number(source, i)?;
                i = next;
                token
            }
            'A'..='Z' | 'a'..='z' | '_' => {
                let (ident, next) = lex_ident(source, i);
                i = next;
                Token::Ident(ident)
            }
            _ => {
                return Err(SpoonlangError::Parse {
                    offset: start,
                    message: format!("unexpected character {ch:?}"),
                });
            }
        };
        tokens.push((token, start));
    }
    tokens.push((Token::Eof, source.len()));
    Ok(tokens)
}

fn lex_ident(source: &str, start: usize) -> (String, usize) {
    let mut end = start + 1;
    while let Some(ch) = source[end..].chars().next() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            end += ch.len_utf8();
        } else {
            break;
        }
    }
    (source[start..end].to_string(), end)
}

fn lex_number(source: &str, start: usize) -> Result<(Token, usize), SpoonlangError> {
    let mut end = start;
    let mut seen_dot = false;
    while let Some(ch) = source.as_bytes().get(end) {
        match *ch {
            b'0'..=b'9' => end += 1,
            b'.' if !seen_dot => {
                seen_dot = true;
                end += 1;
            }
            _ => break,
        }
    }
    let text = &source[start..end];
    if seen_dot {
        let value: f64 = text.parse().map_err(|_| SpoonlangError::Parse {
            offset: start,
            message: format!("invalid float {text}"),
        })?;
        Ok((Token::Float(value), end))
    } else {
        let value: i64 = text.parse().map_err(|_| SpoonlangError::Parse {
            offset: start,
            message: format!("invalid integer {text}"),
        })?;
        Ok((Token::Int(value), end))
    }
}

fn lex_string(source: &str, start: usize) -> Result<(String, usize), SpoonlangError> {
    let mut i = start + 1;
    let mut value = String::new();
    while i < source.len() {
        let ch = source[i..].chars().next().ok_or_else(|| SpoonlangError::Parse {
            offset: start,
            message: "unterminated string".into(),
        })?;
        match ch {
            '"' => return Ok((value, i + ch.len_utf8())),
            '\\' => {
                i += ch.len_utf8();
                let escaped = source[i..].chars().next().ok_or_else(|| SpoonlangError::Parse {
                    offset: start,
                    message: "unterminated string escape".into(),
                })?;
                match escaped {
                    'n' => value.push('\n'),
                    't' => value.push('\t'),
                    'r' => value.push('\r'),
                    '"' => value.push('"'),
                    '\\' => value.push('\\'),
                    other => {
                        return Err(SpoonlangError::Parse {
                            offset: i,
                            message: format!("unknown string escape \\{other}"),
                        });
                    }
                }
                i += escaped.len_utf8();
            }
            other => {
                value.push(other);
                i += other.len_utf8();
            }
        }
    }
    Err(SpoonlangError::Parse {
        offset: start,
        message: "unterminated string".into(),
    })
}
